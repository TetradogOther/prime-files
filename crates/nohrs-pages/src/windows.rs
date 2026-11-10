//! Several explorer windows, and which one the outside world talks to.
//!
//! A tab dragged out of a window opens in a window of its own. Folders that
//! other programs send (Premiere's "Reveal in Explorer", a browser's "Show
//! in folder", Listary, a second `nohrs.exe`) open as a new tab in the window
//! used last, as browsers do with links from other programs.
//!
//! What exists once per run — the local channel, the Explorer hook, the
//! update check, the session kept in `state.redb` — belongs to the first
//! window; every other window is a view onto the same folders, opened on the
//! folder it was given and not restored at the next start.
//!
//! The first window's size, position and maximized state are kept, so the
//! next start opens where the last one closed.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, Entity, Global, Pixels, Task,
    WeakEntity, Window, WindowBounds, point, px, size,
};
use gpui_component::Root;
use gpui_component::resizable::ResizableState;
use nohrs_core::config::{self, ConfigOverride};
use nohrs_core::telemetry::LogErr as _;
use nohrs_services::fs::trash::Ledger as TrashLedger;
use nohrs_services::search::SearchService;
use nohrs_store::{KvKey, KvStore, kv_key};
use nohrs_ui::components::layout::unified_toolbar::UNIFIED_TOOLBAR_HEIGHT;
use nohrs_ui::window::traffic_lights::TrafficLightsHook;
use serde::{Deserialize, Serialize};

use crate::root::RootView;

const WINDOW_KEY: KvKey = kv_key!("window.bounds");

/// What every explorer window is built from, made once by the binary.
#[derive(Clone)]
pub struct WindowSetup {
    /// The full-text search service, shared by every window.
    pub search_service: Option<Arc<SearchService>>,
    /// `state.redb`: the first window's tab session and its size.
    pub store: Option<Arc<dyn KvStore>>,
    /// The trash ledger (macOS only; nothing on Windows).
    pub trash_ledger: TrashLedger,
    /// The config file and the layers above it (environment, command line).
    pub config_path: PathBuf,
    /// See `config_path`.
    pub config_overrides: Vec<ConfigOverride>,
}

impl Global for WindowSetup {}

/// Where a window opens, and on what.
pub struct WindowRequest {
    /// The folder (or the file, selected in its folder) to show.
    pub path: Option<PathBuf>,
    /// The window's place on screen; `None` for the kept or a centred one.
    pub bounds: Option<WindowBounds>,
    /// Opened minimized, at sign-in.
    pub background: bool,
}

/// Opens an explorer window. The first one opened is the main window.
pub fn open_window(request: WindowRequest, cx: &mut App) -> anyhow::Result<AnyWindowHandle> {
    let Some(setup) = cx.try_global::<WindowSetup>().cloned() else {
        anyhow::bail!("the window setup was not installed");
    };
    let primary = cx
        .try_global::<OpenWindows>()
        .is_none_or(|open| open.windows.is_empty());
    let bounds = request
        .bounds
        .or_else(|| primary.then(|| kept_bounds(&setup, cx)).flatten())
        .unwrap_or_else(|| {
            WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(config::WINDOW_WIDTH), px(config::WINDOW_HEIGHT)),
                cx,
            ))
        });
    let traffic_lights = TrafficLightsHook::new().center_vertically(UNIFIED_TOOLBAR_HEIGHT);
    let mut options =
        nohrs_ui::window::unified_window_options(bounds.get_bounds(), &traffic_lights);
    options.window_bounds = Some(bounds);
    // gpui places a window on the primary screen unless told which: bounds
    // on a second monitor were thrown away for the default ones.
    let origin = bounds.get_bounds().origin;
    options.display_id = cx
        .displays()
        .iter()
        .find(|screen| screen.bounds().contains(&origin))
        .map(|screen| screen.id());

    // The file is read again for every window after the first: by then it may
    // have changed, and the windows' own watchers keep each one current.
    let (mut loaded, diagnostics) = config::load_from_path(&setup.config_path);
    for layer in &setup.config_overrides {
        loaded.apply_override(layer);
    }
    let config_error = config::report_diagnostics(&diagnostics);

    let handle = cx.open_window(options, move |window, cx| {
        let resizable = cx.new(|_| ResizableState::default());
        let view = cx.new(|cx| {
            RootView::new(
                resizable,
                setup.search_service.clone(),
                // Only the main window keeps a session: two windows writing the
                // same key would each undo the other's.
                if primary { setup.store.clone() } else { None },
                setup.trash_ledger.clone(),
                loaded,
                setup.config_path.clone(),
                setup.config_overrides.clone(),
                config_error,
                primary,
                window,
                cx,
            )
        });
        register(window.window_handle(), view.downgrade(), cx);
        if let Some(path) = &request.path {
            view.update(cx, |root, cx| {
                if primary {
                    root.open_initial_path(path, window, cx)
                } else {
                    root.open_path_in_new_window(path, window, cx)
                }
            });
        }
        if primary {
            view.update(cx, |root, cx| {
                root.keep_window_bounds(setup.store.clone(), window, cx)
            });
        }
        if request.background {
            window.minimize_window();
        }
        cx.new(|cx| Root::new(view, window, cx))
    })?;
    Ok(handle.into())
}

/// Opens `folder` in a window of its own near `origin` (screen pixels): a tab
/// dragged out of its window and let go there.
pub fn open_window_at(
    folder: String,
    origin: gpui::Point<Pixels>,
    window_size: gpui::Size<Pixels>,
    cx: &mut App,
) {
    let width = (window_size.width * 0.85).max(px(640.0));
    let height = (window_size.height * 0.85).max(px(420.0));
    // The pointer lands on the new window's tab row, as when a browser tab is
    // torn off.
    let mut bounds = Bounds::new(
        point(origin.x - px(120.0), origin.y - px(18.0)),
        size(width, height),
    );
    // Kept inside the monitor it was let go on: gpui drops bounds that do not
    // fit their screen for its default ones, far from the pointer.
    if let Some(screen) = cx
        .displays()
        .into_iter()
        .find(|screen| screen.bounds().contains(&origin))
    {
        let area = screen.bounds();
        bounds.size.width = bounds.size.width.min(area.size.width);
        bounds.size.height = bounds.size.height.min(area.size.height);
        bounds.origin.x = bounds
            .origin
            .x
            .clamp(area.origin.x, area.right() - bounds.size.width);
        bounds.origin.y = bounds
            .origin
            .y
            .clamp(area.origin.y, area.bottom() - bounds.size.height);
    }
    let request = WindowRequest {
        path: Some(PathBuf::from(folder)),
        bounds: Some(WindowBounds::Windowed(bounds)),
        background: false,
    };
    open_window(request, cx).log_err();
}

/// The explorer windows open, and which was used last.
#[derive(Default)]
struct OpenWindows {
    windows: Vec<(AnyWindowHandle, WeakEntity<RootView>)>,
    last: Option<AnyWindowHandle>,
}

impl Global for OpenWindows {}

fn register(handle: AnyWindowHandle, root: WeakEntity<RootView>, cx: &mut App) {
    let open = cx.default_global::<OpenWindows>();
    open.windows.push((handle, root));
    open.last = Some(handle);
}

/// Records `window` as the one used last: the next folder from another
/// program opens there.
pub(crate) fn touch(window: &Window, cx: &mut App) {
    let handle = window.window_handle();
    let open = cx.default_global::<OpenWindows>();
    if open.windows.iter().any(|(known, _)| *known == handle) {
        open.last = Some(handle);
    }
}

// The window used last that is still open, else any open one.
fn target(cx: &mut App) -> Option<(AnyWindowHandle, WeakEntity<RootView>)> {
    let open = cx.default_global::<OpenWindows>();
    open.windows.retain(|(_, root)| root.upgrade().is_some());
    let last = open.last;
    open.windows
        .iter()
        .find(|(handle, _)| Some(*handle) == last)
        .or_else(|| open.windows.last())
        .cloned()
}

/// Runs `f` on the root of the window used last. `None` when every window is
/// closed.
pub(crate) fn with_target<R>(
    cx: &mut AsyncApp,
    f: impl FnOnce(&mut RootView, &mut Window, &mut Context<RootView>) -> R,
) -> Option<R> {
    let (handle, root) = cx.update(target).ok()??;
    let root: Entity<RootView> = root.upgrade()?;
    handle
        .update(cx, |_, window, cx| {
            root.update(cx, |root, cx| f(root, window, cx))
        })
        .ok()
}

/// [`with_target`] from the app itself, between window updates (a deferred
/// call): the quick search acts on the main window this way.
pub(crate) fn with_target_now<R>(
    cx: &mut App,
    f: impl FnOnce(&mut RootView, &mut Window, &mut Context<RootView>) -> R,
) -> Option<R> {
    let (handle, root) = target(cx)?;
    let root: Entity<RootView> = root.upgrade()?;
    handle
        .update(cx, |_, window, cx| {
            root.update(cx, |root, cx| f(root, window, cx))
        })
        .ok()
}

// ---- The main window's place on screen ----

#[derive(Serialize, Deserialize)]
struct KeptBounds {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    maximized: bool,
}

// The size and place the main window had when it last changed, if it still
// lands on a screen: a monitor unplugged since must not open it off view.
fn kept_bounds(setup: &WindowSetup, cx: &App) -> Option<WindowBounds> {
    let bytes = setup.store.as_ref()?.get(&WINDOW_KEY).log_err()??;
    let kept: KeptBounds = serde_json::from_slice(&bytes).log_err()?;
    if kept.width < 320.0 || kept.height < 240.0 {
        return None;
    }
    let bounds = Bounds::new(
        point(px(kept.x), px(kept.y)),
        size(px(kept.width), px(kept.height)),
    );
    let title_bar = Bounds::new(bounds.origin, size(bounds.size.width, px(40.0)));
    let visible = cx
        .displays()
        .iter()
        .any(|display| display.bounds().intersects(&title_bar));
    if !visible {
        return None;
    }
    Some(if kept.maximized {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    })
}

impl RootView {
    /// Writes the window's size and place whenever they settle after a
    /// change (moving, resizing, maximizing).
    pub(crate) fn keep_window_bounds(
        &mut self,
        store: Option<Arc<dyn KvStore>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = store else {
            return;
        };
        let pending: std::rc::Rc<std::cell::RefCell<Option<Task<()>>>> = Default::default();
        cx.observe_window_bounds(window, move |_root, window, cx| {
            let (bounds, maximized) = match window.window_bounds() {
                WindowBounds::Windowed(bounds) => (bounds, false),
                WindowBounds::Maximized(bounds) | WindowBounds::Fullscreen(bounds) => {
                    (bounds, true)
                }
            };
            // Windows puts a minimized window at -32000, -32000.
            if f32::from(bounds.origin.x) <= -30000.0 || f32::from(bounds.origin.y) <= -30000.0 {
                return;
            }
            let kept = KeptBounds {
                x: f32::from(bounds.origin.x),
                y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
                maximized,
            };
            let store = store.clone();
            // A drag reports every step; only where it ends is written.
            let task = cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
                let Some(bytes) = serde_json::to_vec(&kept).log_err() else {
                    return;
                };
                cx.background_spawn(async move { store.put(&WINDOW_KEY, &bytes).log_err() })
                    .await;
            });
            *pending.borrow_mut() = Some(task);
        })
        .detach();
    }
}
