//! The quick search: Prime Files' own Listary. The Ctrl+T box in a window of
//! its own, summoned from any program by the shortcut in `quick_keys`.
//!
//! What a choice does depends on where it was summoned from:
//!
//! - over a browser's Open dialog that Prime Files is answering
//!   (`file_dialog`): a file is sent, a folder opens in Prime Files to pick
//!   from;
//! - over any program's own Open/Save dialog: the file (or folder) goes into
//!   it, as Listary's quick switch does;
//! - anywhere else: a file opens in its program, a folder in Prime Files, and
//!   Ctrl+Enter shows a file in its folder.
//!
//! It is the same box as Ctrl+T (`explorer::palette`), drawn as the whole
//! window, which grows with the list.

use gpui::prelude::*;
use gpui::*;
use nohrs_core::t;

use crate::explorer::palette::{
    Chosen, FILL_MARGIN, FILL_MAX, Palette, PaletteBox, PaletteHost, PaletteMedia,
};

actions!(
    quick_search,
    [
        /// Opens the quick search, or closes it when it is open.
        ToggleQuickSearch
    ]
);

/// The box's width, without the room for its shadow.
const WIDTH: f32 = 760.0;

// Where it was summoned from.
#[derive(Clone, Copy)]
enum Target {
    Anywhere,
    /// Another program's Open/Save dialog, in front when summoned.
    Dialog(isize),
}

/// The quick search window's view.
pub struct QuickSearch {
    palette: Option<Palette>,
    media: PaletteMedia,
    target: Target,
    closing: bool,
    _subscriptions: Vec<Subscription>,
}

// The open window, so the shortcut pressed again closes it.
#[derive(Default)]
struct OpenQuick(Option<WindowHandle<gpui_component::Root>>);

impl Global for OpenQuick {}

/// Whether sounds show their waveform in the box, as `[ui]
/// audio_waveforms` says; set by the main window.
static WAVEFORMS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Starts listening for the shortcut (`quick_keys`) and binds Ctrl+K inside
/// Prime Files. Once per run.
pub fn install(cx: &mut App) {
    crate::quick_keys::install(cx, toggle, |cx| {
        close_open(cx);
    });
    cx.bind_keys([KeyBinding::new("ctrl-k", ToggleQuickSearch, None)]);
    cx.on_action(|_: &ToggleQuickSearch, cx: &mut App| toggle(cx));
}

/// Takes the settings the quick search follows.
pub fn apply_config(config: &nohrs_core::config::Config) {
    crate::quick_keys::set(&config.launcher.hotkey);
    WAVEFORMS.store(
        config.ui.audio_waveforms,
        std::sync::atomic::Ordering::Relaxed,
    );
}

// Closes the quick search when it is open.
fn close_open(cx: &mut App) -> bool {
    let Some(handle) = cx.try_global::<OpenQuick>().and_then(|open| open.0) else {
        return false;
    };
    cx.set_global(OpenQuick(None));
    crate::quick_keys::set_open(false);
    // Not from inside another window's update, which gpui refuses.
    cx.defer(move |cx| {
        handle
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    });
    true
}

/// Opens the quick search over whatever is in front, or closes it.
pub fn toggle(cx: &mut App) {
    if close_open(cx) {
        return;
    }
    let target =
        crate::file_dialog::foreground_file_dialog().map_or(Target::Anywhere, Target::Dialog);
    let width = WIDTH + 2.0 * FILL_MARGIN;
    let height = 56.0 + 34.0 + 2.0 + 2.0 * FILL_MARGIN;
    let bounds = match cx.primary_display() {
        Some(display) => {
            let area = display.bounds();
            let x = area.origin.x + (area.size.width - px(width)) / 2.0;
            let y = area.origin.y + area.size.height * 0.2;
            Bounds::new(point(x, y), size(px(width), px(height)))
        }
        None => Bounds::new(point(px(200.0), px(160.0)), size(px(width), px(height))),
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: true,
        show: true,
        // Over the program it was summoned on.
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: None,
        // Opaque: gpui_component's `Root`, which the text field cannot do
        // without (it crashed drawing the field, 07/11/2026), paints the
        // whole window. Windows rounds its corners (`round_corners`).
        window_background: WindowBackgroundAppearance::Opaque,
        app_id: None,
        window_min_size: None,
        window_decorations: None,
        tabbing_identifier: None,
    };
    let opened = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| QuickSearch::new(target, window, cx));
        cx.new(|cx| gpui_component::Root::new(view, window, cx))
    });
    match opened {
        Ok(handle) => {
            cx.set_global(OpenQuick(Some(handle)));
            crate::quick_keys::set_open(true);
            // Summoned by a key pressed in another program, it is not given
            // the keyboard on its own: Windows keeps the foreground with the
            // program the person was typing in.
            handle
                .update(cx, |_, window, _| {
                    if let Some(hwnd) = crate::ipc::window_hwnd(window) {
                        platform::round_corners(hwnd);
                        platform::take_foreground(hwnd);
                    }
                })
                .ok();
        }
        Err(error) => tracing::warn!("could not open the quick search: {error}"),
    }
}

impl QuickSearch {
    fn new(target: Target, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe_global::<crate::explorer::everything::Everything>(|this, cx| {
                this.palette_index_changed(cx)
            }),
            // A click on another program sends it away, as Listary's bar.
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.close(window, cx);
                }
            }),
        ];
        let mut this = Self {
            palette: None,
            media: PaletteMedia::default(),
            target,
            closing: false,
            _subscriptions: subscriptions,
        };
        this.toggle_palette(window, cx);
        this.close_when_left(window, cx);
        this
    }

    // Closes the window once another window is in front, as Listary's bar
    // goes away on a click elsewhere. The activation observer above did not
    // see it (the owner, 07/11/2026): asked of Windows instead, a few times a
    // second. Only after the box has been in front, so a summon that Windows
    // kept behind does not vanish at once.
    fn close_when_left(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let mut was_in_front = false;
            let mut away = 0;
            let mut looks = 0;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(120))
                    .await;
                let keep = this
                    .update_in(cx, |this, window, cx| {
                        if this.closing {
                            return false;
                        }
                        let hwnd = crate::ipc::window_hwnd(window);
                        let in_front = hwnd.is_some_and(platform::is_in_front);
                        looks += 1;
                        if !in_front && !was_in_front && looks <= 6 {
                            // Windows kept the foreground where the keys
                            // were pressed: asked again, for a moment.
                            if let Some(hwnd) = hwnd {
                                platform::take_foreground(hwnd);
                            }
                        }
                        if in_front {
                            was_in_front = true;
                            away = 0;
                        } else if was_in_front {
                            // Two looks in a row: a menu or tooltip of its
                            // own taking the front for a moment is not a
                            // click elsewhere.
                            away += 1;
                            if away >= 2 {
                                this.close(window, cx);
                                return false;
                            }
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.closing, true) {
            return;
        }
        if let Some(palette) = self.palette.take() {
            palette.cancel();
        }
        cx.set_global(OpenQuick(None));
        crate::quick_keys::set_open(false);
        window.remove_window();
    }
}

impl PaletteHost for QuickSearch {
    fn palette_slot(&mut self) -> &mut Option<Palette> {
        &mut self.palette
    }

    fn palette_ref(&self) -> Option<&Palette> {
        self.palette.as_ref()
    }

    fn media(&mut self) -> &mut PaletteMedia {
        &mut self.media
    }

    fn palette_tabs(&self, _cx: &App) -> Vec<(u64, String, String)> {
        Vec::new()
    }

    fn palette_folder(&self, _cx: &App) -> String {
        dirs::home_dir()
            .map(|home| home.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn palette_waveforms(&self) -> bool {
        WAVEFORMS.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn palette_chose(&mut self, chosen: Chosen, _window: &mut Window, cx: &mut Context<Self>) {
        // The preview's Open is Enter on a file.
        let (path, dir, in_folder) = match chosen {
            Chosen::Path {
                path,
                dir,
                in_folder,
            } => (path, dir, in_folder),
            Chosen::Open(path) => (path, false, false),
            Chosen::Tab(_) | Chosen::Empty => return,
        };
        let target = self.target;
        // The main window is updated once this one's update is over: gpui
        // does not let one window update another from inside its own.
        cx.defer(move |cx| {
            if crate::file_dialog::picking().is_some() {
                if dir {
                    show_in_prime_files(path, None, cx);
                } else if crate::file_dialog::deliver(std::slice::from_ref(&path)) {
                    crate::windows::with_target_now(cx, |_, window, _| window.minimize_window());
                } else {
                    show_in_prime_files(parent_of(&path), Some(path), cx);
                }
                return;
            }
            match target {
                Target::Dialog(hwnd) if crate::file_dialog::fill_dialog(hwnd, &path) => {}
                _ if dir => show_in_prime_files(path, None, cx),
                _ if in_folder => show_in_prime_files(parent_of(&path), Some(path), cx),
                _ => crate::explorer::open_with_default(&path),
            }
        });
    }

    fn palette_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
    }

    fn palette_fills_window(&self) -> bool {
        true
    }

    fn palette_enter_hint(&self) -> &'static str {
        if crate::file_dialog::picking().is_some() {
            t!("Enviar")
        } else if matches!(self.target, Target::Dialog(_)) {
            t!("Usar na janela")
        } else {
            t!("Abrir")
        }
    }

    fn palette_ctrl_enter_hint(&self) -> Option<&'static str> {
        (crate::file_dialog::picking().is_none() && matches!(self.target, Target::Anywhere))
            .then(|| t!("Abrir a pasta"))
    }
}

impl Render for QuickSearch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let card = self.render_palette(window, cx);
        // The window is the card and the room for its shadow, so it grows
        // and shrinks with the list.
        if let Some(height) = self
            .palette
            .as_ref()
            .map(|palette| palette.height)
            .filter(|height| *height > 0.0)
        {
            let wanted = (height.min(FILL_MAX) + 2.0 * FILL_MARGIN).ceil();
            let current = window.viewport_size();
            if (f32::from(current.height) - wanted).abs() > 1.0 {
                window.resize(size(current.width, px(wanted)));
            }
        }
        div().size_full().relative().children(card)
    }
}

fn parent_of(path: &str) -> String {
    std::path::Path::new(path)
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

// Shows `folder` in the Prime Files window used last, in a new tab, with
// `select` selected; opens a window when there is none.
fn show_in_prime_files(folder: String, select: Option<String>, cx: &mut App) {
    let shown = crate::windows::with_target_now(cx, |root, window, cx| {
        root.show_explorer(cx);
        root.explorer_entity().update(cx, |page, cx| {
            page.open_path(folder.clone(), select.clone(), window, cx)
        });
        window.activate_window();
    });
    if shown.is_none() {
        let request = crate::windows::WindowRequest {
            path: Some(std::path::PathBuf::from(select.unwrap_or(folder))),
            bounds: None,
            background: false,
        };
        if let Err(error) = crate::windows::open_window(request, cx) {
            tracing::warn!("could not open a window for the quick search: {error}");
        }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
    };

    /// Whether the window is the one in front, or owns the one in front
    /// (a tooltip of its own).
    pub fn is_in_front(hwnd: i64) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::{GA_ROOTOWNER, GetAncestor};
        unsafe {
            let front = GetForegroundWindow();
            front.0 as i64 == hwnd || GetAncestor(front, GA_ROOTOWNER).0 as i64 == hwnd
        }
    }

    /// Windows 11's round corners, as its own pop-ups have; a pop-up without
    /// a frame gets square ones. Earlier Windows ignores the request.
    pub fn round_corners(hwnd: i64) {
        let preference = DWMWCP_ROUND;
        unsafe {
            let _ = DwmSetWindowAttribute(
                HWND(hwnd as *mut _),
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &preference as *const _ as *const std::ffi::c_void,
                std::mem::size_of_val(&preference) as u32,
            );
        }
    }

    /// Brings the window to the front with the keyboard. Windows lets the
    /// program in front give the foreground away, not a program take it;
    /// joined for a moment to the input of the one in front, Prime Files is
    /// that program.
    pub fn take_foreground(hwnd: i64) {
        unsafe {
            let window = HWND(hwnd as *mut _);
            // A mouse "move" by nothing: Windows lets the process that sent
            // the last input take the foreground, and the shortcut's keys
            // were the program in front's. The hook listens to keys only.
            let nudge = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_MOVE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            SendInput(&[nudge], std::mem::size_of::<INPUT>() as i32);
            let front = GetForegroundWindow();
            let front_thread = GetWindowThreadProcessId(front, None);
            let own_thread = GetCurrentThreadId();
            let joined = front_thread != 0
                && front_thread != own_thread
                && AttachThreadInput(own_thread, front_thread, true).as_bool();
            let _ = BringWindowToTop(window);
            let _ = SetForegroundWindow(window);
            if joined {
                let _ = AttachThreadInput(own_thread, front_thread, false);
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn is_in_front(_hwnd: i64) -> bool {
        true
    }
    pub fn round_corners(_hwnd: i64) {}
    pub fn take_foreground(_hwnd: i64) {}
}
