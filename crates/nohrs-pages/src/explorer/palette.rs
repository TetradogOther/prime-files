//! Ctrl+T: a search box over the window, as Zen Browser's new tab is. It
//! lists the open tabs that match (to go to one instead of opening it twice)
//! and every file and folder of the computer whose name matches, from the
//! whole-computer index. Enter or a click opens the hit in a new tab — a
//! folder shows itself, a file its folder with it selected — and a hit can be
//! dragged out like a file of the listing. With nothing typed, Enter opens a
//! plain new tab, which is what Ctrl+T and the "+" did before.
//!
//! The hits are drawn as the explorer draws files: Windows' icons, the
//! pictures of images and videos, and the waveforms of sounds. Ctrl+wheel,
//! Ctrl+= / Ctrl+- / Ctrl+0 or the − / + of the box change their size, from
//! a list of small icons to a grid of large pictures; the size is kept for
//! the next time.
//!
//! A click on a video or a sound does not open it: it plays in the box
//! first, with buttons to open it, show it in its folder or copy it or its
//! path ([`HitPreview`]). The right button on any hit opens a menu with the
//! same actions over the Windows menu of the file.
//!
//! The same box is the quick search (`crate::quick_search`), a window of its
//! own summoned from anywhere, as Listary's bar is. Each host says what the
//! box lists besides the index and what a choice does ([`PaletteHost`]); the
//! box itself, its search and its look, are one.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::prelude::*;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Icon, IconName};
use nohrs_core::telemetry::LogErr;
use nohrs_core::{t, tf};
use nohrs_ui::theme::theme;

use super::media::{self as player, MediaPreview};
use super::page::ExplorerPage;
use super::thumbnails::Thumbnails;
use super::waveforms::Waveforms;
use crate::shell_menu::{self, Choice, Entry, Request};

// How many hits of the index the box shows.
const HITS: usize = 60;
// How far the pointer moves with the button down before a press is a drag.
const DRAG_THRESHOLD: f32 = 6.0;

// The size of the box's pictures, as one number from 0 (a list of small
// icons) to 1 (a grid of large pictures). See `icon_size`.
const ZOOM_DEFAULT: f32 = 0.1;
const ZOOM_STEP: f32 = 0.08;
// The icon size, in pixels, from which the hits are a grid of tiles.
const GRID_FROM: f32 = 64.0;
// The smallest icon size the list shows pictures at, rather than icons:
// below it a photo is a smudge and the type's icon reads better.
const PICTURES_FROM: f32 = 32.0;
// A row of the list holds its icon and this much around it.
const ROW_PADDING: f32 = 14.0;
const ROW_MIN: f32 = 40.0;
// A tile holds its picture, two lines of name, and this around them.
const TILE_PADDING: f32 = 10.0;
const TILE_NAME: f32 = 42.0;
// The box as it was before it had sizes: its width, and a row's height.
const PLAIN_WIDTH: f32 = 680.0;
const PLAIN_ROW: f32 = 46.0;
// Heights of the card's parts, for a window that is sized to the box: the
// field, a group's heading, the "searching" line and the footer.
const FIELD: f32 = 56.0;
const HEADING: f32 = 28.0;
const SEARCHING: f32 = 36.0;
const FOOTER: f32 = 34.0;
/// Room around the box in a window that is only the box: none. Such a
/// window is opaque (its root is gpui_component's `Root`, which the text
/// field needs and which paints the whole window), so the box fills it and
/// Windows rounds the window's corners instead of the card's.
pub(crate) const FILL_MARGIN: f32 = 0.0;
/// The tallest such a window grows.
pub(crate) const FILL_MAX: f32 = 640.0;
// A hit's preview: the line with its name, the bar under the picture, and
// the row of buttons; the picture takes the rest of `PREVIEW_HEIGHT`.
const PREVIEW_HEIGHT: f32 = 460.0;
const PREVIEW_TITLE: f32 = 44.0;
const PREVIEW_CONTROLS: f32 = 40.0;
const PREVIEW_ACTIONS: f32 = 52.0;
// How long "copied" stays in the footer.
const NOTICE_MS: u64 = 1600;

/// The box while it is open.
pub(crate) struct Palette {
    input: Entity<InputState>,
    _subscription: Subscription,
    query: String,
    rows: Vec<Row>,
    selected: usize,
    cancel: Arc<AtomicBool>,
    _search: Option<Task<()>>,
    // The row pressed and where, until it is released or becomes a drag.
    press: Option<(usize, Point<Pixels>)>,
    scroll: ScrollHandle,
    // The index generation the hits came from.
    generation: u64,
    // Whether the arrows chose a row since the text last changed. With
    // nothing typed and no row chosen, Enter makes a plain new tab rather
    // than going to the first tab listed.
    chose: bool,
    // Tiles per line in the last frame drawn as a grid; 0 as a list. The
    // arrows move by a line of tiles.
    columns: usize,
    // For each row, the child of the scrolled list that holds it, as last
    // drawn: what `scroll_to_item` is given.
    line_of: Vec<usize>,
    /// The card's height as last drawn: a window that is only the box
    /// (`PaletteHost::palette_fills_window`) is sized to it.
    pub(crate) height: f32,
    // The video or sound clicked, shown in place of the list.
    preview: Option<HitPreview>,
    // What the footer says for a moment after a copy, and the timer that
    // takes it away.
    notice: Option<String>,
    _notice: Option<Task<()>>,
}

/// A video or sound shown in the box from a click on its hit, before it is
/// opened: it plays there, and the buttons under it act on it.
pub(crate) struct HitPreview {
    path: String,
    name: String,
    folder: String,
    media: Option<MediaPreview>,
    // The timeline's bounds as last painted, for a click on it to seek.
    track: Rc<Cell<Bounds<Pixels>>>,
}

impl HitPreview {
    fn close(self, cx: &mut App) {
        if let Some(media) = self.media {
            media.close(cx);
        }
    }
}

// The player of the box's preview, wherever its host keeps the box.
fn preview_player<H: PaletteHost>(host: &mut H) -> Option<&mut MediaPreview> {
    host.palette_slot()
        .as_mut()?
        .preview
        .as_mut()?
        .media
        .as_mut()
}

/// What can be done to a hit, from its preview or its right-click menu. The
/// numbers are the menu's ids, below the Shell's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HitAction {
    Preview = 1,
    Open,
    ShowInFolder,
    Copy,
    CopyPath,
}

impl HitAction {
    const ALL: [HitAction; 5] = [
        HitAction::Preview,
        HitAction::Open,
        HitAction::ShowInFolder,
        HitAction::Copy,
        HitAction::CopyPath,
    ];

    fn from_id(id: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|action| *action as u32 == id)
    }
}

// Whether a click on a hit plays it in the box rather than opening it.
fn plays_in_box(path: &str, dir: bool) -> bool {
    !dir && player::is_media(path)
}

#[derive(Clone)]
pub(crate) enum Row {
    /// An open tab: its entity id, title and folder.
    Tab {
        id: u64,
        title: String,
        folder: String,
    },
    /// A path typed in full that exists.
    Typed { path: String, dir: bool },
    /// A hit of the index, with its size and modification time (seconds),
    /// which a waveform is kept by.
    Hit {
        name: String,
        path: String,
        dir: bool,
        size: u64,
        modified: u64,
    },
}

impl Row {
    fn path(&self) -> Option<(&str, bool)> {
        match self {
            Row::Tab { .. } => None,
            Row::Typed { path, dir } | Row::Hit { path, dir, .. } => Some((path, *dir)),
        }
    }

    fn is_hit(&self) -> bool {
        matches!(self, Row::Hit { .. })
    }
}

/// The pictures of the box, kept by the page between openings so a search
/// made again does not ask Windows for them again.
#[derive(Default)]
pub(crate) struct PaletteMedia {
    thumbnails: Thumbnails,
    waveforms: Waveforms,
    zoom: Option<f32>,
}

/// What was chosen in the box.
pub(crate) enum Chosen {
    /// An open tab, by entity id.
    Tab(u64),
    /// A file or folder; `in_folder` when asked to show it in its folder
    /// (Ctrl+Enter) rather than open it.
    Path {
        path: String,
        dir: bool,
        in_folder: bool,
    },
    /// A file to open in its program (or send to the dialog waiting for
    /// one): the preview's Open.
    Open(String),
    /// Enter with nothing typed and nothing chosen.
    Empty,
}

/// A view that shows the box: the explorer page (Ctrl+T) or the quick search
/// window. The box's own state and pictures live in the host.
pub(crate) trait PaletteHost: Sized + 'static {
    fn palette_slot(&mut self) -> &mut Option<Palette>;
    fn palette_ref(&self) -> Option<&Palette>;
    fn media(&mut self) -> &mut PaletteMedia;
    /// The open tabs listed above the hits: entity id, title, folder.
    fn palette_tabs(&self, cx: &App) -> Vec<(u64, String, String)>;
    /// The folder hits are ranked near.
    fn palette_folder(&self, cx: &App) -> String;
    /// Whether sounds show their waveform.
    fn palette_waveforms(&self) -> bool;
    /// Acts on a choice. The box is already closed.
    fn palette_chose(&mut self, chosen: Chosen, window: &mut Window, cx: &mut Context<Self>);
    /// The box closed: Esc, a click outside it, or a choice.
    fn palette_closed(&mut self, window: &mut Window, cx: &mut Context<Self>);
    /// The box is the whole window (the quick search) rather than a card
    /// over a dimmed one.
    fn palette_fills_window(&self) -> bool;
    /// What Enter does, for the footer.
    fn palette_enter_hint(&self) -> &'static str;
    /// What Ctrl+Enter does, when it does something else.
    fn palette_ctrl_enter_hint(&self) -> Option<&'static str> {
        None
    }
}

// Icon size in pixels for a zoom: 20 px at 0, 220 at 1.
fn icon_size(zoom: f32) -> f32 {
    20.0 + zoom.clamp(0.0, 1.0) * 200.0
}

// The zoom shown as a percentage of the default size.
fn zoom_percent(zoom: f32) -> u32 {
    (icon_size(zoom) / icon_size(ZOOM_DEFAULT) * 100.0).round() as u32
}

fn zoom_file() -> PathBuf {
    nohrs_core::config::paths::data_dir().join("palette-zoom")
}

#[allow(clippy::disallowed_methods)]
fn load_zoom() -> f32 {
    std::fs::read_to_string(zoom_file())
        .ok()
        .and_then(|text| text.trim().parse::<f32>().ok())
        .filter(|zoom| (0.0..=1.0).contains(zoom))
        .unwrap_or(ZOOM_DEFAULT)
}

fn save_zoom(zoom: f32, cx: &mut App) {
    cx.background_spawn(async move {
        let path = zoom_file();
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                #[allow(clippy::disallowed_methods)]
                std::fs::write(&path, format!("{zoom:.3}"))
            });
        if let Err(error) = written {
            tracing::warn!("could not keep the search box's size: {error}");
        }
    })
    .detach();
}

/// The box's behaviour, the same for every host.
pub(crate) trait PaletteBox: PaletteHost {
    fn palette_zoom(&mut self) -> f32 {
        *self.media().zoom.get_or_insert_with(load_zoom)
    }

    // Changes the pictures' size by `step` (or back to the default with
    // `None`), keeping the selected row in view.
    fn zoom_palette(&mut self, step: Option<f32>, cx: &mut Context<Self>) {
        let now = self.palette_zoom();
        let next = step.map_or(ZOOM_DEFAULT, |step| (now + step).clamp(0.0, 1.0));
        if (next - now).abs() < f32::EPSILON {
            return;
        }
        self.media().zoom = Some(next);
        save_zoom(next, cx);
        if let Some(palette) = self.palette_slot().as_mut() {
            // Lines change with the size; the selected one is brought back
            // once the next frame has laid them out.
            palette.line_of.clear();
        }
        cx.notify();
    }

    /// Opens the box, or closes it when it is already open.
    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette_ref().is_some() {
            self.close_palette(window, cx);
            return;
        }
        // The index loads (or is brought up to date) while the first letters
        // are typed.
        super::everything::ensure(cx);
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!(
                "Buscar arquivos e pastas no computador ou digitar um caminho"
            ))
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |page, _input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => page.palette_changed(cx),
                // Ctrl+Enter reaches the field as Enter with the key held.
                InputEvent::PressEnter { .. } => {
                    let in_folder = window.modifiers().control;
                    page.activate_palette_row(None, in_folder, window, cx);
                }
                _ => {}
            },
        );
        *self.palette_slot() = Some(Palette {
            input,
            _subscription: subscription,
            query: String::new(),
            rows: Vec::new(),
            selected: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            _search: None,
            press: None,
            scroll: ScrollHandle::new(),
            generation: cx
                .try_global::<super::everything::Everything>()
                .map_or(0, |state| state.generation),
            chose: false,
            columns: 0,
            line_of: Vec::new(),
            height: 0.0,
            preview: None,
            notice: None,
            _notice: None,
        });
        self.palette_changed(cx);
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(mut palette) = self.palette_slot().take() {
            palette.cancel.store(true, Ordering::Relaxed);
            if let Some(preview) = palette.preview.take() {
                preview.close(cx);
            }
            self.palette_closed(window, cx);
            cx.notify();
        }
    }

    // The text changed: the tabs are matched here, the index off the UI
    // thread, and its hits replace the old ones when they come.
    fn palette_changed(&mut self, cx: &mut Context<Self>) {
        let titles = self.palette_tabs(cx);
        let cwd = self.palette_folder(cx);
        let Some(palette) = self.palette_slot().as_mut() else {
            return;
        };
        let query = palette.input.read(cx).value().to_string();
        // Typing goes back to the list: the words are for a new search.
        if query != palette.query
            && let Some(preview) = palette.preview.take()
        {
            preview.close(cx);
            palette.line_of.clear();
        }
        palette.query = query.clone();
        let needle = query.trim().to_lowercase();

        let mut rows: Vec<Row> = titles
            .into_iter()
            .filter(|(_, title, folder)| {
                needle.is_empty()
                    || title.to_lowercase().contains(&needle)
                    || folder.to_lowercase().contains(&needle)
            })
            .map(|(id, title, folder)| Row::Tab { id, title, folder })
            .collect();
        let typed = query.trim().trim_matches('"');
        if Path::new(typed).is_absolute() && Path::new(typed).exists() {
            rows.insert(
                0,
                Row::Typed {
                    path: typed.to_string(),
                    dir: Path::new(typed).is_dir(),
                },
            );
        }
        // The hits of the last search stay until the new ones come, so the
        // list does not blink at each letter.
        rows.extend(palette.rows.iter().filter(|row| row.is_hit()).cloned());
        palette.rows = rows;
        palette.selected = 0;
        palette.chose = false;

        palette.cancel.store(true, Ordering::Relaxed);
        if needle.is_empty() {
            palette.rows.retain(|row| !row.is_hit());
            cx.notify();
            return;
        }
        let Some(finder) = super::everything::finder(cx, &query, &cwd) else {
            cx.notify();
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        palette.cancel = cancel.clone();
        let search = cx.background_spawn(async move {
            let found = finder.run(&cancel);
            // Names gone since the index was made are left out; the size and
            // date of the others come with the same look at the disk.
            found
                .into_iter()
                .filter(|_| !cancel.load(Ordering::Relaxed))
                .filter_map(|(name, path, dir)| {
                    #[allow(clippy::disallowed_methods)]
                    let metadata = std::fs::metadata(&path).ok()?;
                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |since| since.as_secs());
                    Some(Row::Hit {
                        name,
                        path,
                        dir,
                        size: metadata.len(),
                        modified,
                    })
                })
                .take(HITS)
                .collect::<Vec<_>>()
        });
        palette._search = Some(cx.spawn(async move |page: WeakEntity<Self>, cx| {
            let found = search.await;
            page.update(cx, |page, cx| {
                let Some(palette) = page.palette_slot().as_mut() else {
                    return;
                };
                if palette.query != query {
                    return;
                }
                palette.rows.retain(|row| !row.is_hit());
                palette.rows.extend(found);
                palette.selected = palette.selected.min(palette.rows.len().saturating_sub(1));
                cx.notify();
            })
            .log_err();
        }));
        cx.notify();
    }

    /// The index was loaded, rebuilt or grew: the words typed are searched
    /// again, at most once per index generation.
    fn palette_index_changed(&mut self, cx: &mut Context<Self>) {
        let generation = cx
            .try_global::<super::everything::Everything>()
            .map_or(0, |state| state.generation);
        let Some(palette) = self.palette_slot().as_mut() else {
            return;
        };
        if palette.generation == generation {
            cx.notify();
            return;
        }
        palette.generation = generation;
        self.palette_changed(cx);
    }

    // Up/down (`vertical`) move by a line: one row of the list, or one line
    // of tiles among the hits of the grid. Left/right move by one, once the
    // arrows are choosing (before that they move the text cursor).
    fn move_palette_selection(&mut self, step: isize, vertical: bool, cx: &mut Context<Self>) {
        let Some(palette) = self.palette_slot().as_mut() else {
            return;
        };
        if palette.rows.is_empty() {
            return;
        }
        let last = palette.rows.len() as isize - 1;
        let first_hit = palette.rows.iter().position(Row::is_hit);
        let current = palette.selected as isize;
        let target = if !palette.chose && palette.query.trim().is_empty() {
            // The first press lands on the first row, which showed unlit.
            0
        } else if !palette.chose {
            current
        } else if vertical
            && palette.columns > 1
            && let Some(first_hit) = first_hit
        {
            let first_hit = first_hit as isize;
            let columns = palette.columns as isize;
            if current >= first_hit {
                let moved = current + step * columns;
                if moved < first_hit {
                    // Up from the first line of tiles: the row above them.
                    if first_hit > 0 {
                        first_hit - 1
                    } else {
                        current
                    }
                } else if moved > last {
                    // Down from a short last line keeps to the last tile.
                    if (current - first_hit) / columns < (last - first_hit) / columns {
                        last
                    } else {
                        current
                    }
                } else {
                    moved
                }
            } else {
                current + step.signum()
            }
        } else {
            current + step
        };
        palette.selected = target.clamp(0, last) as usize;
        palette.chose = true;
        if let Some(&line) = palette.line_of.get(palette.selected) {
            palette.scroll.scroll_to_item(line);
        }
        cx.notify();
    }

    // Enter (`None`: the selected row) or a click on `row`; `in_folder` for
    // Ctrl+Enter.
    fn activate_palette_row(
        &mut self,
        row: Option<usize>,
        in_folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(palette) = self.palette_ref() else {
            return;
        };
        let empty = palette.query.trim().is_empty();
        let chosen = match row {
            Some(row) => palette.rows.get(row).cloned(),
            None if empty && !palette.chose => None,
            None => palette.rows.get(palette.selected).cloned(),
        };
        self.close_palette(window, cx);
        let chosen = match chosen {
            Some(Row::Tab { id, .. }) => Chosen::Tab(id),
            Some(Row::Typed { path, dir }) | Some(Row::Hit { path, dir, .. }) => Chosen::Path {
                path,
                dir,
                in_folder,
            },
            None if empty => Chosen::Empty,
            None => return,
        };
        self.palette_chose(chosen, window, cx);
    }

    // A click on a hit: a video or a sound plays in the box first, anything
    // else opens as Enter would.
    fn click_palette_row(
        &mut self,
        index: usize,
        in_folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let plays = self
            .palette_ref()
            .and_then(|palette| palette.rows.get(index))
            .and_then(Row::path)
            .is_some_and(|(path, dir)| plays_in_box(path, dir));
        if plays && !in_folder {
            self.open_hit_preview(index, cx);
        } else {
            self.activate_palette_row(Some(index), in_folder, window, cx);
        }
    }

    // Shows the hit `index` (a video or sound) in place of the list, playing.
    fn open_hit_preview(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((path, dir)) = self
            .palette_ref()
            .and_then(|palette| palette.rows.get(index))
            .and_then(Row::path)
            .map(|(path, dir)| (path.to_string(), dir))
        else {
            return;
        };
        if !plays_in_box(&path, dir) {
            return;
        }
        let media = MediaPreview::start(path.clone(), true, preview_player::<Self>, cx);
        let file = Path::new(&path);
        let preview = HitPreview {
            name: file
                .file_name()
                .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned()),
            folder: file
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path,
            media,
            track: Rc::default(),
        };
        let Some(palette) = self.palette_slot().as_mut() else {
            return;
        };
        palette.selected = index;
        palette.chose = true;
        palette.press = None;
        if let Some(old) = palette.preview.replace(preview) {
            old.close(cx);
        }
        cx.notify();
    }

    // Back from the preview to the list. Whether there was one.
    fn close_hit_preview(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(palette) = self.palette_slot().as_mut() else {
            return false;
        };
        let Some(preview) = palette.preview.take() else {
            return false;
        };
        preview.close(cx);
        // The list is laid out again; the row shown is brought back in view.
        palette.line_of.clear();
        cx.notify();
        true
    }

    fn toggle_hit_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(media) = preview_player(self) {
            media.toggle();
            cx.notify();
        }
    }

    // Moves the preview's media by `seconds`.
    fn nudge_hit_preview(&mut self, seconds: f64, cx: &mut Context<Self>) {
        if let Some(media) = preview_player(self)
            && media.duration > 0.0
        {
            let at = (media.position + seconds) / media.duration;
            media.seek(at);
            cx.notify();
        }
    }

    // A click on the timeline (or the wave) at `x`.
    fn seek_hit_preview(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(track) = self
            .palette_ref()
            .and_then(|palette| palette.preview.as_ref())
            .map(|preview| preview.track.get())
        else {
            return;
        };
        let width = f32::from(track.size.width);
        if width <= 0.0 {
            return;
        }
        let fraction = f64::from((f32::from(x - track.origin.x) / width).clamp(0.0, 1.0));
        if let Some(media) = preview_player(self) {
            media.seek(fraction);
            cx.notify();
        }
    }

    // Says `text` in the footer for a moment.
    fn palette_notice(&mut self, text: String, cx: &mut Context<Self>) {
        let timer = cx.spawn(async move |host: WeakEntity<Self>, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(NOTICE_MS))
                .await;
            host.update(cx, |host, cx| {
                if let Some(palette) = host.palette_slot().as_mut() {
                    palette.notice = None;
                    cx.notify();
                }
            })
            .ok();
        });
        if let Some(palette) = self.palette_slot().as_mut() {
            palette.notice = Some(text);
            palette._notice = Some(timer);
        }
        cx.notify();
    }

    // Does `action` to the file or folder at `path`; `index` is its row,
    // for the preview.
    fn hit_action(
        &mut self,
        action: HitAction,
        path: String,
        dir: bool,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            HitAction::Preview => {
                if let Some(index) = index {
                    self.open_hit_preview(index, cx);
                }
            }
            HitAction::Open => {
                self.close_palette(window, cx);
                let chosen = if dir {
                    Chosen::Path {
                        path,
                        dir,
                        in_folder: false,
                    }
                } else {
                    Chosen::Open(path)
                };
                self.palette_chose(chosen, window, cx);
            }
            HitAction::ShowInFolder => {
                self.close_palette(window, cx);
                // A folder too is shown in the folder it is in, selected.
                self.palette_chose(
                    Chosen::Path {
                        path,
                        dir: false,
                        in_folder: true,
                    },
                    window,
                    cx,
                );
            }
            HitAction::Copy => {
                super::file_ops::copy_files(&[PathBuf::from(path)], cx);
                let said = nohrs_core::i18n::plural(
                    1,
                    nohrs_core::k!("{n} item copiado"),
                    nohrs_core::k!("{n} itens copiados"),
                );
                self.palette_notice(said, cx);
            }
            HitAction::CopyPath => {
                // In quotes, as the Explorer's "Copy as path" puts it.
                cx.write_to_clipboard(ClipboardItem::new_string(format!("\"{path}\"")));
                self.palette_notice(t!("Caminho copiado").to_string(), cx);
            }
        }
    }

    // The right button on the hit `index`: it is selected and its menu opens.
    fn open_hit_menu(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = self.palette_slot().as_mut() else {
            return;
        };
        let Some((path, dir)) = palette
            .rows
            .get(index)
            .and_then(Row::path)
            .map(|(path, dir)| (path.to_string(), dir))
        else {
            return;
        };
        palette.selected = index;
        palette.chose = true;
        palette.press = None;
        cx.notify();
        self.show_hit_menu(path, dir, Some(index), window, cx);
    }

    // The menu of a hit at the pointer: the box's own actions first, then
    // the Windows menu of the file (Open with, Properties, what programs
    // add), less the entries the box's own replace. `index` is `None` from
    // the preview, which needs no Preview entry.
    fn show_hit_menu(
        &mut self,
        path: String,
        dir: bool,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(hwnd) = crate::ipc::window_hwnd(window) else {
            return;
        };
        let mut own = Vec::new();
        if index.is_some() && plays_in_box(&path, dir) {
            own.push(Entry::command(
                HitAction::Preview as u32,
                t!("Visualização rápida"),
            ));
        }
        own.push(Entry::command(HitAction::Open as u32, t!("Abrir")));
        own.push(Entry::command(
            HitAction::ShowInFolder as u32,
            t!("Mostrar na pasta"),
        ));
        own.push(Entry::Separator);
        own.push(Entry::command(HitAction::Copy as u32, t!("Copiar")));
        own.push(Entry::command(
            HitAction::CopyPath as u32,
            t!("Copiar como caminho"),
        ));
        let dark = theme::is_dark();
        // The menu runs its own message loop until it closes: from a task,
        // so the app is not borrowed meanwhile (as the listing's menu).
        cx.spawn_in(
            window,
            async move |host: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
                let target = shell_menu::Target::Items(vec![path.clone()]);
                let choice = shell_menu::show(&Request {
                    hwnd: hwnd as isize,
                    target: &target,
                    own: &own,
                    before: &[],
                    own_first: true,
                    extended: false,
                    dark,
                    intercept: &[],
                    hide: &["open", "copy", "copyaspath"],
                });
                host.update_in(cx, |host, window, cx| match choice {
                    Choice::Own(id) => {
                        if let Some(action) = HitAction::from_id(id) {
                            host.hit_action(action, path, dir, index, window, cx);
                        }
                    }
                    // A Windows command ran (delete, rename, …): the hits are
                    // looked at again, so a file gone leaves the list.
                    Choice::Ran if index.is_some() => host.palette_changed(cx),
                    _ => {}
                })
                .log_err();
            },
        )
        .detach();
    }

    // A press that moved far enough with the button down: the hit leaves the
    // box as a file dragged from the listing would.
    fn palette_drag_moved(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(palette) = self.palette_slot().as_mut() else {
            return;
        };
        let Some((row, start)) = palette.press else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            palette.press = None;
            return;
        }
        let moved = event.position - start;
        if f32::from(moved.x).hypot(f32::from(moved.y)) < DRAG_THRESHOLD {
            return;
        }
        palette.press = None;
        let Some((path, _)) = palette.rows.get(row).and_then(Row::path) else {
            return;
        };
        let path = PathBuf::from(path);
        let Some(hwnd) = crate::ipc::window_hwnd(window) else {
            return;
        };
        // Out of the way first, so a drop on this window lands on the listing.
        self.close_palette(window, cx);
        // The drag runs its own message loop until the drop: from a task, so
        // the app is not borrowed meanwhile (as in `drag.rs`).
        cx.spawn_in(window, async move |_page, _cx| {
            if let Err(error) = super::drag::drag_out(hwnd, &[path]) {
                tracing::warn!("could not drag from the search box: {error}");
            }
        })
        .detach();
    }

    // What the hit at `path` shows in a square of `icon` pixels: its
    // waveform (a sound, with waveforms on), its picture (big enough for one
    // to read) or Windows' icon. The caches are the page's own: the page
    // draws the box, and what arrives redraws the page.
    fn palette_media(&mut self, row: &Row, icon: f32, scale: f32, cx: &mut Context<Self>) -> Media {
        // A tab shows the icon of the folder it is on, as the folder would
        // show in the listing.
        let (path, dir) = match row {
            Row::Tab { folder, .. } => (folder.as_str(), true),
            _ => match row.path() {
                Some(found) => found,
                None => return Media::None,
            },
        };
        let pixels = ((icon * scale).round() as u32).max(16);
        if let Row::Hit { size, modified, .. } = row
            && !dir
            && icon >= PICTURES_FROM
            && self.palette_waveforms()
            && super::waveforms::is_audio(path)
            && let Some(bars) = self.media().waveforms.get_for(
                path,
                *size,
                *modified,
                |page: &mut Self| &mut page.media().waveforms,
                cx,
            )
        {
            return Media::Waveform(bars);
        }
        let image = if dir || icon < PICTURES_FROM {
            self.media().thumbnails.icon_for(
                path,
                dir,
                pixels,
                |page: &mut Self| &mut page.media().thumbnails,
                cx,
            )
        } else {
            let bucket = super::zoom::thumbnail_bucket(icon, scale);
            self.media().thumbnails.get_for(
                path,
                bucket,
                |page: &mut Self| &mut page.media().thumbnails,
                cx,
            )
        };
        image.map_or(Media::None, Media::Image)
    }

    // The preview in place of the list, `width` by `height`.
    fn render_hit_preview(
        &mut self,
        width: f32,
        height: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let preview = self.palette_ref()?.preview.as_ref()?;
        let (path, name, folder, track) = (
            preview.path.clone(),
            preview.name.clone(),
            preview.folder.clone(),
            preview.track.clone(),
        );
        let screen_height = (height - PREVIEW_TITLE - PREVIEW_CONTROLS - PREVIEW_ACTIONS).max(60.0);
        let inner = width - 24.0;
        let media = preview.media.as_ref();
        let (position, duration, playing) = media.map_or((0.0, 0.0, false), |media| {
            (media.position, media.duration, media.playing)
        });
        let fraction = if duration > 0.0 {
            (position / duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };

        // The picture: the video's frame, the sound's wave, or what went wrong.
        let screen = div()
            .id("palette-preview-screen")
            .w(px(inner))
            .h(px(screen_height))
            .mx(px(12.0))
            .rounded(px(10.0))
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(theme::preview_backdrop()))
            .cursor_pointer();
        let screen = match media {
            None => screen.child(
                div()
                    .text_sm()
                    .text_color(rgb(theme::muted()))
                    .child(t!("(Sem visualização para este arquivo)")),
            ),
            Some(media) => match (&media.error, &media.frame) {
                (Some(error), _) => screen.child(
                    div()
                        .px(px(16.0))
                        .text_sm()
                        .text_color(rgb(theme::muted()))
                        .child(error.clone()),
                ),
                (None, Some(frame)) => screen
                    .on_click(
                        cx.listener(|page, _: &ClickEvent, _, cx| page.toggle_hit_preview(cx)),
                    )
                    .child(
                        img(ImageSource::Render(frame.clone()))
                            .w(px(inner))
                            .h(px(screen_height))
                            .object_fit(ObjectFit::Contain),
                    ),
                (None, None) if player::is_audio(&path) => {
                    let bars =
                        player::waveform_bars(&media.peaks, media.peak_max, media.duration, 400);
                    let track = track.clone();
                    // A click on the wave seeks there, as on the timeline.
                    screen
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|page, event: &MouseDownEvent, _, cx| {
                                page.seek_hit_preview(event.position.x, cx)
                            }),
                        )
                        .child(wave(
                            bars,
                            fraction,
                            inner - 32.0,
                            screen_height * 0.7,
                            track,
                        ))
                }
                (None, None) => screen.child(match media.preparing {
                    Some(Some(done)) => div()
                        .text_sm()
                        .text_color(rgb(theme::muted()))
                        .child(format!("{}%", (done * 100.0).round() as u32)),
                    _ => div().child(
                        Icon::new(Icon::empty())
                            .path("icons/loader.svg")
                            .size_5()
                            .text_color(rgb(theme::muted())),
                    ),
                }),
            },
        };

        // Play, the time, and the timeline.
        let is_audio = player::is_audio(&path);
        let controls = div()
            .h(px(PREVIEW_CONTROLS))
            .px(px(14.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .id("palette-preview-play")
                    .size(px(28.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(14.0))
                    .cursor_pointer()
                    .bg(rgb(theme::bg()))
                    .hover(|style| style.bg(rgb(theme::bg_hover())))
                    .on_click(
                        cx.listener(|page, _: &ClickEvent, _, cx| page.toggle_hit_preview(cx)),
                    )
                    .child(
                        div()
                            .font_family(theme::icon_font())
                            .text_size(px(12.0))
                            .text_color(rgb(theme::fg()))
                            .child(if playing { "\u{e769}" } else { "\u{e768}" }),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(rgb(theme::muted()))
                    .child(format!(
                        "{} / {}",
                        player::clock(position),
                        player::clock(duration)
                    )),
            )
            // The wave is the timeline of a sound; a video gets a bar.
            .when(!is_audio, |this| {
                let bar_width = (inner - 150.0).max(40.0);
                let track = track.clone();
                this.child(
                    div()
                        .id("palette-preview-track")
                        .w(px(bar_width))
                        .h(px(16.0))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|page, event: &MouseDownEvent, _, cx| {
                                page.seek_hit_preview(event.position.x, cx)
                            }),
                        )
                        .child(timeline(fraction, bar_width, track)),
                )
            });

        // What can be done with it.
        let open_path = path.clone();
        let folder_path = path.clone();
        let copy_path = path.clone();
        let copy_as_path = path.clone();
        let more_path = path.clone();
        let actions = div()
            .h(px(PREVIEW_ACTIONS))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                action_button(
                    "palette-preview-open",
                    "icons/external-link.svg",
                    t!("Abrir"),
                    true,
                )
                .on_click(cx.listener(move |page, _: &ClickEvent, window, cx| {
                    page.hit_action(HitAction::Open, open_path.clone(), false, None, window, cx)
                })),
            )
            .child(
                action_button(
                    "palette-preview-folder",
                    "icons/folder-open.svg",
                    t!("Mostrar na pasta"),
                    false,
                )
                .on_click(cx.listener(move |page, _: &ClickEvent, window, cx| {
                    page.hit_action(
                        HitAction::ShowInFolder,
                        folder_path.clone(),
                        false,
                        None,
                        window,
                        cx,
                    )
                })),
            )
            .child(
                action_button(
                    "palette-preview-copy",
                    "icons/copy.svg",
                    t!("Copiar"),
                    false,
                )
                .on_click(cx.listener(move |page, _: &ClickEvent, window, cx| {
                    page.hit_action(HitAction::Copy, copy_path.clone(), false, None, window, cx)
                })),
            )
            .child(
                action_button(
                    "palette-preview-copy-path",
                    "icons/link.svg",
                    t!("Copiar como caminho"),
                    false,
                )
                .on_click(cx.listener(move |page, _: &ClickEvent, window, cx| {
                    page.hit_action(
                        HitAction::CopyPath,
                        copy_as_path.clone(),
                        false,
                        None,
                        window,
                        cx,
                    )
                })),
            )
            .child(div().flex_1())
            .child(
                action_button(
                    "palette-preview-more",
                    "icons/ellipsis.svg",
                    t!("Mostrar mais opções"),
                    false,
                )
                .on_click(cx.listener(move |page, _: &ClickEvent, window, cx| {
                    page.show_hit_menu(more_path.clone(), false, None, window, cx)
                })),
            );

        // Back, the name and the folder.
        let title = div()
            .h(px(PREVIEW_TITLE))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .id("palette-preview-back")
                    .flex_shrink_0()
                    .h(px(28.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(theme::fg_secondary()))
                    .hover(|style| style.bg(rgb(theme::bg_hover())))
                    .on_click(cx.listener(|page, _: &ClickEvent, _, cx| {
                        page.close_hit_preview(cx);
                    }))
                    .child(Icon::new(IconName::ArrowLeft).size_3())
                    .child(t!("Voltar")),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .max_w(px((width * 0.45).max(120.0)))
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(theme::fg()))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(name),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::muted()))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(super::view::listing::truncate_middle(
                        &folder,
                        ((width * 0.4 / 6.5) as usize).max(12),
                    )),
            );

        Some(
            div()
                .id("palette-preview")
                .w(px(width))
                .h(px(height))
                .flex_shrink_0()
                .flex()
                .flex_col()
                // The file's own menu, as on its hit in the list.
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |page, _: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        page.show_hit_menu(path.clone(), false, None, window, cx);
                    }),
                )
                .child(title)
                .child(screen)
                .child(controls)
                .child(actions)
                .into_any_element(),
        )
    }

    /// The box over the window, when it is open.
    fn render_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.palette_ref()?;
        let fills = self.palette_fills_window();
        let zoom = self.palette_zoom();
        let icon = icon_size(zoom);
        let grid = icon >= GRID_FROM;
        let scale = window.scale_factor();
        // What the last frame did not ask for scrolled away or is gone.
        self.media().thumbnails.begin_frame(cx);
        self.media().waveforms.begin_frame();

        let viewport = window.viewport_size();
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        // Without files found (nothing typed: the open tabs) the box is the
        // one Ctrl+T always was, whatever the size chosen for files: the
        // size is about pictures, and tabs have none.
        let plain = self.palette_ref()?.rows.iter().all(|row| !row.is_hit());
        // One width for the list and the grid: changing the size must not
        // move the − / + out from under the pointer.
        // A window that is only the box gives it all its width, less the
        // room its shadow needs, and grows to the list up to `FILL_MAX`.
        let (width, left, top, list_max) = if fills {
            let width = (vw - 2.0 * FILL_MARGIN).max(280.0);
            (
                width,
                FILL_MARGIN,
                FILL_MARGIN,
                (FILL_MAX - 150.0).max(120.0),
            )
        } else {
            let width = (vw - 48.0).clamp(280.0, if plain { PLAIN_WIDTH } else { 960.0 });
            let top = (vh * 0.10).clamp(16.0, 96.0);
            // Room for the list under the field and above the footer.
            (
                (width),
                (vw - width) / 2.0,
                top,
                (vh - top - 150.0).max(120.0),
            )
        };
        // The scrolled area's inner width: the card's, less its padding and
        // the scrollbar's lane.
        let inner = width - 2.0 * 6.0 - 10.0;
        let columns = if grid {
            ((inner / (icon + 2.0 * TILE_PADDING)).floor() as usize).max(1)
        } else {
            0
        };
        // The tiles share the line's width, so the grid has no gap at its end.
        let tile_width = if grid {
            (inner / columns as f32).floor()
        } else {
            0.0
        };
        let row_height = (icon + ROW_PADDING).max(ROW_MIN);

        let (rows, selected, lit_any, query_empty, previewing) = {
            let palette = self.palette_ref()?;
            (
                palette.rows.clone(),
                palette.selected,
                palette.chose || !palette.query.trim().is_empty(),
                palette.query.trim().is_empty(),
                palette.preview.is_some(),
            )
        };
        let enter_hint = self.palette_enter_hint();
        let tab_count = rows
            .iter()
            .filter(|row| matches!(row, Row::Tab { .. }))
            .count();
        let hit_count = rows.iter().filter(|row| row.is_hit()).count();

        // The scrolled list, as lines: headings, rows, and lines of tiles.
        let mut lines: Vec<AnyElement> = Vec::new();
        let mut line_of = vec![0; rows.len()];
        // Headings only beside files: the plain box is the tabs alone.
        if tab_count > 0 && !plain {
            lines.push(heading(t!("Abas abertas"), tab_count));
        }
        let mut index = 0;
        // Tabs and a typed path are always rows.
        while index < rows.len() && !rows[index].is_hit() {
            if index == tab_count && matches!(rows[index], Row::Typed { .. }) && tab_count > 0 {
                lines.push(div().h(px(6.0)).into_any_element());
            }
            let lit = lit_any && index == selected;
            line_of[index] = lines.len();
            let media = self.palette_media(&rows[index], 16.0, scale, cx);
            lines.push(render_row(
                &rows[index],
                index,
                lit,
                media,
                PLAIN_ROW,
                16.0,
                inner,
                enter_hint,
                cx,
            ));
            index += 1;
        }
        if hit_count > 0 {
            lines.push(heading(t!("No computador"), hit_count));
        }
        let hits: Vec<usize> = (index..rows.len()).collect();
        if grid {
            for chunk in hits.chunks(columns) {
                let line = lines.len();
                let mut tiles = Vec::with_capacity(chunk.len());
                for &at in chunk {
                    line_of[at] = line;
                    let media = self.palette_media(&rows[at], icon, scale, cx);
                    tiles.push(render_tile(
                        &rows[at],
                        at,
                        lit_any && at == selected,
                        media,
                        icon,
                        tile_width,
                        cx,
                    ));
                }
                lines.push(
                    div()
                        .flex()
                        .items_start()
                        .children(tiles)
                        .into_any_element(),
                );
            }
        } else {
            for at in hits {
                line_of[at] = lines.len();
                let media = self.palette_media(&rows[at], icon, scale, cx);
                lines.push(render_row(
                    &rows[at],
                    at,
                    lit_any && at == selected,
                    media,
                    row_height,
                    icon,
                    inner,
                    enter_hint,
                    cx,
                ));
            }
        }
        let empty = rows.is_empty() && !previewing;
        let preview_height = PREVIEW_HEIGHT
            .min(list_max)
            .max(PREVIEW_TITLE + PREVIEW_CONTROLS + PREVIEW_ACTIONS + 80.0);
        // The list's height, as the lines above lay it out: what a window
        // that is only the box is sized to.
        let list_height: f32 = {
            let mut height = 0.0;
            let mut index = 0;
            if tab_count > 0 && !plain {
                height += HEADING;
            }
            while index < rows.len() && !rows[index].is_hit() {
                height += PLAIN_ROW;
                index += 1;
            }
            if hit_count > 0 {
                height += HEADING;
                height += if grid {
                    hit_count.div_ceil(columns.max(1)) as f32
                        * (icon + 2.0 * TILE_PADDING + TILE_NAME)
                } else {
                    hit_count as f32 * row_height
                };
            }
            if previewing {
                preview_height
            } else {
                (height + 12.0).min(list_max)
            }
        };
        let preview_element = if previewing {
            self.render_hit_preview(width - 2.0, preview_height, cx)
        } else {
            None
        };

        let palette = self.palette_slot().as_mut()?;
        let relaid = palette.line_of.is_empty() && !line_of.is_empty();
        palette.line_of = line_of;
        palette.columns = columns;
        if relaid
            && palette.chose
            && let Some(&line) = palette.line_of.get(palette.selected)
        {
            palette.scroll.scroll_to_item(line);
        }
        let searching = !query_empty && hit_count == 0 && !previewing;
        palette.height = FIELD
            + if empty { 0.0 } else { 1.0 + list_height }
            + if searching { SEARCHING } else { 0.0 }
            + FOOTER
            + 2.0;
        let ctrl_enter_hint = self.palette_ctrl_enter_hint();
        let palette = self.palette_ref()?;
        let notice = palette.notice.clone();
        // A first indexing still under way says how far it got instead.
        let indexing = cx
            .try_global::<super::everything::Everything>()
            .and_then(|state| state.building.as_ref())
            .map(|found| found.load(Ordering::Relaxed));

        let card =
            div()
                .id("palette")
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(width))
                // A window that is only the box: the card reaches its bottom
                // edge, so a window a few pixels taller than the estimate below
                // shows card, not a strip of the window under the footer.
                .when(fills, |card| card.bottom(px(0.0)))
                .flex()
                .flex_col()
                .rounded(px(if fills { 0.0 } else { 14.0 }))
                .border_1()
                .border_color(rgb(theme::border()))
                .bg(rgb(theme::bg_secondary()))
                .when(!fills, |card| card.shadow_2xl())
                .overflow_hidden()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .capture_key_down(cx.listener(|page, event: &KeyDownEvent, window, cx| {
                    let keys = &event.keystroke;
                    let ctrl = keys.modifiers.control || keys.modifiers.platform;
                    let choosing = page
                        .palette_ref()
                        .is_some_and(|palette| palette.chose && palette.columns > 1);
                    match keys.key.as_str() {
                        "=" | "+" if ctrl => page.zoom_palette(Some(ZOOM_STEP), cx),
                        "-" if ctrl => page.zoom_palette(Some(-ZOOM_STEP), cx),
                        "0" if ctrl => page.zoom_palette(None, cx),
                        "down" => page.move_palette_selection(1, true, cx),
                        "up" => page.move_palette_selection(-1, true, cx),
                        "right" if choosing => page.move_palette_selection(1, false, cx),
                        "left" if choosing => page.move_palette_selection(-1, false, cx),
                        "pagedown" => page.move_palette_selection(6, true, cx),
                        "pageup" => page.move_palette_selection(-6, true, cx),
                        "escape" => page.close_palette(window, cx),
                        "enter" if ctrl => page.activate_palette_row(None, true, window, cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                }))
                // With a preview open its keys come first: Esc goes back to the
                // list, Enter opens the file, Space plays or pauses, the arrows
                // move through it.
                .capture_key_down(cx.listener(|page, event: &KeyDownEvent, window, cx| {
                    let Some(path) = page
                        .palette_ref()
                        .and_then(|palette| palette.preview.as_ref())
                        .map(|preview| preview.path.clone())
                    else {
                        return;
                    };
                    let keys = &event.keystroke;
                    if keys.modifiers.control || keys.modifiers.alt || keys.modifiers.platform {
                        return;
                    }
                    match keys.key.as_str() {
                        "escape" => {
                            page.close_hit_preview(cx);
                        }
                        "enter" => page.hit_action(HitAction::Open, path, false, None, window, cx),
                        "space" | " " => page.toggle_hit_preview(cx),
                        "left" => page.nudge_hit_preview(-5.0, cx),
                        "right" => page.nudge_hit_preview(5.0, cx),
                        "up" | "down" | "pageup" | "pagedown" => {}
                        _ => return,
                    }
                    cx.stop_propagation();
                }))
                // Ctrl+wheel changes the size, as in the listing; the wheel alone
                // scrolls.
                .on_scroll_wheel(cx.listener(|page, event: &ScrollWheelEvent, window, cx| {
                    if !(event.modifiers.control || event.modifiers.platform) {
                        return;
                    }
                    let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
                    if delta.abs() < 0.5 {
                        return;
                    }
                    page.zoom_palette(
                        Some(if delta > 0.0 {
                            ZOOM_STEP / 2.0
                        } else {
                            -ZOOM_STEP / 2.0
                        }),
                        cx,
                    );
                    cx.stop_propagation();
                }))
                .on_mouse_move(cx.listener(|page, event: &MouseMoveEvent, window, cx| {
                    page.palette_drag_moved(event, window, cx);
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|page, _: &MouseUpEvent, _, _| {
                        if let Some(palette) = page.palette_slot().as_mut() {
                            palette.press = None;
                        }
                    }),
                )
                .child(
                    div()
                        .h(px(56.0))
                        .px(px(16.0))
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .child(Icon::new(IconName::Search).size_5().text_color(rgb(
                            if query_empty {
                                theme::muted()
                            } else {
                                theme::accent()
                            },
                        )))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_base()
                                .child(Input::new(&palette.input).appearance(false)),
                        )
                        .when(!plain, |this| this.child(zoom_control(zoom, cx))),
                )
                .when_some(preview_element, |card, preview| {
                    card.child(div().h(px(1.0)).bg(rgb(theme::border())))
                        .child(preview)
                })
                .when(!empty && !previewing, |card| {
                    card.child(div().h(px(1.0)).bg(rgb(theme::border()))).child(
                        div()
                            .id("palette-rows")
                            .max_h(px(list_max))
                            .px(px(6.0))
                            .py(px(6.0))
                            .overflow_y_scroll()
                            .track_scroll(&palette.scroll)
                            .children(lines),
                    )
                })
                .when(searching, |card| {
                    card.child(
                        div()
                            .px(px(18.0))
                            .py(px(10.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_xs()
                            .text_color(rgb(theme::muted()))
                            .child(Icon::new(Icon::empty()).path("icons/loader.svg").size_3())
                            .child(match indexing {
                                Some(found) => tf!("Indexando o computador… {n} itens", n = found),
                                None => t!("Buscando no computador…").to_string(),
                            }),
                    )
                })
                .child(
                    div()
                        .px(px(14.0))
                        .py(px(8.0))
                        .flex()
                        .items_center()
                        .gap(px(14.0))
                        // Kept on the window's bottom edge, whatever is left over.
                        .when(fills, |footer| footer.mt_auto())
                        .border_t_1()
                        .border_color(rgb(theme::border()))
                        .bg(rgb(theme::bg()))
                        .text_xs()
                        .text_color(rgb(theme::muted()))
                        .when(previewing, |this| {
                            this.child(key_hint("Enter", t!("Abrir")))
                                .child(key_hint("Esc", t!("Voltar")))
                        })
                        .when(!previewing, |this| {
                            this.child(key_hint("↑↓", t!("Escolher")))
                                .child(key_hint("Enter", enter_hint))
                                .when_some(ctrl_enter_hint, |this, hint| {
                                    this.child(key_hint("Ctrl+Enter", hint))
                                })
                                .child(key_hint("Ctrl+roda", t!("Tamanho")))
                                .child(key_hint("Esc", t!("Fechar")))
                        })
                        .child(div().flex_1())
                        .child(match notice {
                            Some(notice) => div().text_color(rgb(theme::accent())).child(notice),
                            None if previewing => div(),
                            None => div()
                                .text_color(rgb(theme::muted()))
                                .child(t!("Arraste para levar o arquivo")),
                        }),
                );

        if fills {
            return Some(card.into_any_element());
        }
        Some(
            deferred(
                anchored()
                    .position_mode(AnchoredPositionMode::Window)
                    .position(point(px(0.0), px(0.0)))
                    .child(
                        div()
                            .id("palette-backdrop")
                            .w(px(vw))
                            .h(px(vh))
                            .bg(rgba(0x0000_0059))
                            .occlude()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|page, _, window, cx| page.close_palette(window, cx)),
                            )
                            .child(card),
                    ),
            )
            .with_priority(20)
            .into_any_element(),
        )
    }
}

impl<H: PaletteHost> PaletteBox for H {}

impl Palette {
    /// Stops the search under way, when the box goes away by another way
    /// than `close_palette` (its window closing).
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

// The explorer page: Ctrl+T over its window.
impl PaletteHost for ExplorerPage {
    fn palette_slot(&mut self) -> &mut Option<Palette> {
        &mut self.palette
    }

    fn palette_ref(&self) -> Option<&Palette> {
        self.palette.as_ref()
    }

    fn media(&mut self) -> &mut PaletteMedia {
        &mut self.palette_media
    }

    fn palette_tabs(&self, cx: &App) -> Vec<(u64, String, String)> {
        self.tabs(cx)
            .into_iter()
            .filter_map(|(id, folder, _)| {
                let tab = self
                    .group
                    .all_tabs()
                    .into_iter()
                    .find(|tab| tab.entity_id().as_u64() == id)?;
                let title = crate::pane_group::PaneItem::tab_title(tab.read(cx), cx);
                Some((id, title, folder))
            })
            .collect()
    }

    fn palette_folder(&self, cx: &App) -> String {
        self.group.active_pane().read(cx).cwd.clone()
    }

    fn palette_waveforms(&self) -> bool {
        self.ui.audio_waveforms
    }

    fn palette_chose(&mut self, chosen: Chosen, window: &mut Window, cx: &mut Context<Self>) {
        match chosen {
            Chosen::Tab(id) => {
                if let Some(tab) = self
                    .group
                    .all_tabs()
                    .into_iter()
                    .find(|tab| tab.entity_id().as_u64() == id)
                    && let Some((pane, index)) = self.group.locate_tab(tab.entity_id())
                {
                    self.group.set_active_tab(pane, index, window, cx);
                    self.group.set_active(pane, window, cx);
                    self.group.focus_active(window, cx);
                    cx.notify();
                }
            }
            // While a browser's Open dialog waits, the box is how a file is
            // found anywhere and sent at once, as Listary's bar under the
            // dialog does (the owner, 07/11/2026); a folder opens in the tab
            // on screen, as it would in the dialog.
            Chosen::Path { path, dir, .. } if crate::file_dialog::picking().is_some() => {
                let pane = self.group.active_pane().clone();
                if dir {
                    pane.update(cx, |pane, cx| pane.change_dir(path, window, cx));
                } else {
                    pane.update(cx, |pane, cx| pane.send_to_dialog(vec![path], window, cx));
                }
            }
            Chosen::Path { path, dir, .. } => {
                let (folder, select) = folder_and_selection(&path, dir);
                self.open_in_new_tab(folder, select, false, window, cx);
            }
            Chosen::Open(path) if crate::file_dialog::picking().is_some() => {
                let pane = self.group.active_pane().clone();
                pane.update(cx, |pane, cx| pane.send_to_dialog(vec![path], window, cx));
            }
            Chosen::Open(path) => super::open_with_default(&path),
            Chosen::Empty => {
                let pane = self.group.active();
                self.new_tab_in_pane(pane, window, cx);
            }
        }
    }

    fn palette_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.group.focus_active(window, cx);
    }

    fn palette_fills_window(&self) -> bool {
        false
    }

    fn palette_enter_hint(&self) -> &'static str {
        if crate::file_dialog::picking().is_some() {
            t!("Enviar")
        } else {
            t!("Abrir em nova aba")
        }
    }
}

// The names the rest of the explorer calls the box by.
impl ExplorerPage {
    /// Opens the box, or closes it when it is already open.
    pub(crate) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        PaletteBox::toggle_palette(self, window, cx);
    }

    /// The index was loaded, rebuilt or grew.
    pub(crate) fn palette_index_changed(&mut self, cx: &mut Context<Self>) {
        PaletteBox::palette_index_changed(self, cx);
    }

    /// The box over the window, when it is open.
    pub(crate) fn render_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        PaletteBox::render_palette(self, window, cx)
    }
}

// What a hit shows in its square.
pub(crate) enum Media {
    None,
    Image(Arc<RenderImage>),
    Waveform(Arc<[u8]>),
}

// The folder a new tab opens on, and what it selects there.
fn folder_and_selection(path: &str, dir: bool) -> (String, Option<String>) {
    if dir {
        return (path.to_string(), None);
    }
    match Path::new(path).parent() {
        Some(parent) => (
            parent.to_string_lossy().into_owned(),
            Some(path.to_string()),
        ),
        None => (path.to_string(), None),
    }
}

// A group's name over its rows, with how many there are.
fn heading(label: &str, count: usize) -> AnyElement {
    div()
        .px(px(10.0))
        .pt(px(8.0))
        .pb(px(4.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(theme::muted()))
        .child(label.to_string())
        .child(
            div()
                .font_weight(FontWeight::NORMAL)
                .child(count.to_string()),
        )
        .into_any_element()
}

// A key and what it does, for the footer.
fn key_hint(key: &'static str, label: &str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .child(
            div()
                .px(px(5.0))
                .py(px(1.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(rgb(theme::border()))
                .bg(rgb(theme::bg_secondary()))
                .text_color(rgb(theme::fg_secondary()))
                .child(key),
        )
        .child(label.to_string())
}

// The − 100% + at the end of the field.
fn zoom_control<H: PaletteBox>(zoom: f32, cx: &mut Context<H>) -> impl IntoElement {
    let button = |id: &'static str, icon: IconName, enabled: bool| {
        div()
            .id(id)
            .size(px(24.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.0))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::bg_hover())))
            })
            .when(!enabled, |this| this.opacity(0.35))
            .child(
                Icon::new(icon)
                    .size_3()
                    .text_color(rgb(theme::fg_secondary())),
            )
    };
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(2.0))
        .px(px(4.0))
        .py(px(2.0))
        .rounded(px(8.0))
        .bg(rgb(theme::bg()))
        .child(
            button("palette-zoom-out", IconName::Minus, zoom > 0.0)
                .on_click(cx.listener(|page, _, _, cx| page.zoom_palette(Some(-ZOOM_STEP), cx))),
        )
        .child(
            div()
                .id("palette-zoom-reset")
                .w(px(40.0))
                .text_xs()
                .text_center()
                .cursor_pointer()
                .text_color(rgb(theme::fg_secondary()))
                .on_click(cx.listener(|page, _, _, cx| page.zoom_palette(None, cx)))
                .child(format!("{}%", zoom_percent(zoom))),
        )
        .child(
            button("palette-zoom-in", IconName::Plus, zoom < 1.0)
                .on_click(cx.listener(|page, _, _, cx| page.zoom_palette(Some(ZOOM_STEP), cx))),
        )
}

// A button of the preview's row: an icon and its label; `primary` in the
// accent colour.
fn action_button(
    id: &'static str,
    icon: &'static str,
    label: &str,
    primary: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .h(px(32.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .gap(px(7.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .text_xs()
        .when(primary, |this| {
            this.bg(rgb(theme::accent()))
                .text_color(rgb(theme::accent_fg()))
                .hover(|style| style.opacity(0.9))
        })
        .when(!primary, |this| {
            this.border_1()
                .border_color(rgb(theme::border()))
                .bg(rgb(theme::bg()))
                .text_color(rgb(theme::fg()))
                .hover(|style| style.bg(rgb(theme::bg_hover())))
        })
        .child(Icon::new(Icon::empty()).path(icon).size_3())
        .child(label.to_string())
}

// The video's timeline, `width` wide: what was played in the accent colour.
// Its bounds go to `track` as it paints, for a click to seek.
fn timeline(fraction: f32, width: f32, track: Rc<Cell<Bounds<Pixels>>>) -> AnyElement {
    let (done, rest) = (rgb(theme::accent()), rgb(theme::border()));
    canvas(
        move |bounds, _, _| track.set(bounds),
        move |bounds, (), window, _| {
            let middle = bounds.origin.y + bounds.size.height / 2.0;
            let full = Bounds::new(
                point(bounds.origin.x, middle - px(2.0)),
                size(bounds.size.width, px(4.0)),
            );
            window.paint_quad(fill(full, rest).corner_radii(px(2.0)));
            let played = Bounds::new(full.origin, size(bounds.size.width * fraction, px(4.0)));
            window.paint_quad(fill(played, done).corner_radii(px(2.0)));
            let knob = point(
                bounds.origin.x + bounds.size.width * fraction - px(6.0),
                middle - px(6.0),
            );
            window.paint_quad(
                fill(Bounds::new(knob, size(px(12.0), px(12.0))), done).corner_radii(px(6.0)),
            );
        },
    )
    .w(px(width))
    .h(px(16.0))
    .into_any_element()
}

// A sound's wave, `width` by `height`, from the player's bars (`None` for a
// stretch not read yet); what was played in the accent colour. Its bounds go
// to `track` as it paints, for a click to seek.
fn wave(
    bars: Vec<Option<f32>>,
    fraction: f32,
    width: f32,
    height: f32,
    track: Rc<Cell<Bounds<Pixels>>>,
) -> AnyElement {
    let (done, rest) = (rgb(theme::accent()), rgb(theme::fg_secondary()));
    canvas(
        move |bounds, _, _| track.set(bounds),
        move |bounds, (), window, _| {
            const BAR: f32 = 2.0;
            const GAP: f32 = 1.0;
            let width = f32::from(bounds.size.width);
            let count = ((width + GAP) / (BAR + GAP)).floor().max(1.0) as usize;
            let middle = f32::from(bounds.origin.y) + f32::from(bounds.size.height) / 2.0;
            let tall = f32::from(bounds.size.height);
            for index in 0..count {
                let at = index * bars.len() / count.max(1);
                let level = bars.get(at).copied().flatten().unwrap_or(0.0);
                // Silence still draws a line, so the wave never looks empty.
                let bar = (level * tall).max(1.0);
                let x = f32::from(bounds.origin.x) + index as f32 * (BAR + GAP);
                let played = (index as f32 + 0.5) / count as f32 <= fraction;
                window.paint_quad(fill(
                    Bounds::new(point(px(x), px(middle - bar / 2.0)), size(px(BAR), px(bar))),
                    if played { done } else { rest },
                ));
            }
        },
    )
    .w(px(width))
    .h(px(height))
    .into_any_element()
}

// The square a row or tile draws its picture in.
fn media_square(row: &Row, media: Media, icon: f32, selected: bool) -> AnyElement {
    let square = div()
        .size(px(icon))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center();
    match media {
        Media::Image(image) => square
            .child(
                img(ImageSource::Render(image))
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            )
            .into_any_element(),
        Media::Waveform(bars) => square
            .rounded(px(6.0))
            .bg(rgb(theme::bg()))
            .child(
                div()
                    .size_full()
                    .child(super::waveforms::canvas(bars, icon, selected, None)),
            )
            .into_any_element(),
        Media::None => {
            let (path, color) = match row {
                Row::Tab { .. } | Row::Typed { dir: true, .. } | Row::Hit { dir: true, .. } => {
                    ("icons/folder.svg", theme::accent())
                }
                _ => ("icons/file.svg", theme::fg_secondary()),
            };
            square
                .child(
                    Icon::new(Icon::empty())
                        .path(path)
                        .size(px((icon * 0.8).clamp(16.0, 96.0)))
                        .text_color(rgb(color)),
                )
                .into_any_element()
        }
    }
}

// Selects the row on a press, readying a drag; a click opens it.
fn pressable<E: InteractiveElement + StatefulInteractiveElement, H: PaletteBox>(
    element: E,
    index: usize,
    draggable: bool,
    cx: &mut Context<H>,
) -> E {
    element
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |page, event: &MouseDownEvent, _, cx| {
                if let Some(palette) = page.palette_slot().as_mut() {
                    palette.selected = index;
                    palette.press = draggable.then_some((index, event.position));
                    cx.notify();
                }
            }),
        )
        // The right button: the hit's menu, as a file's in the listing.
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |page, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                page.open_hit_menu(index, window, cx);
            }),
        )
        .on_click(cx.listener(move |page, event: &ClickEvent, window, cx| {
            let in_folder = event.modifiers().control;
            page.click_palette_row(index, in_folder, window, cx)
        }))
}

#[allow(clippy::too_many_arguments)]
fn render_row<H: PaletteBox>(
    row: &Row,
    index: usize,
    selected: bool,
    media: Media,
    height: f32,
    icon: f32,
    width: f32,
    hint: &'static str,
    cx: &mut Context<H>,
) -> AnyElement {
    let (name, detail, hint): (String, String, &'static str) = match row {
        Row::Tab { title, folder, .. } => (title.clone(), folder.clone(), t!("Ir para a aba")),
        Row::Typed { path, .. } => (path.clone(), String::new(), hint),
        Row::Hit { name, path, .. } => (
            name.clone(),
            Path::new(path)
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_default(),
            hint,
        ),
    };
    // Two lines (name over folder) once the icon is tall enough for them.
    let stacked = icon >= 32.0;
    // The folder is cut in its middle to the room it has, keeping the drive
    // and the folder it is in: the text's own ellipsis measured that room
    // as nothing in the stacked layout and showed only "…". About 6.5 px a
    // character at the size it is drawn; the hint takes ~150 px when shown.
    let hinted = selected || matches!(row, Row::Tab { .. });
    let room =
        width - icon - 60.0 - if stacked { 0.0 } else { 180.0 } - if hinted { 150.0 } else { 0.0 };
    let detail = super::view::listing::truncate_middle(&detail, ((room / 6.5) as usize).max(12));
    let draggable = row.path().is_some();
    let element = div()
        .id(("palette-row", index))
        .relative()
        .h(px(height))
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(12.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .when(selected, |this| this.bg(rgb(theme::bg_hover())))
        .when(!selected, |this| {
            this.hover(|this| this.bg(rgb(theme::bg_hover())))
        })
        // A bar of the accent at the selected row's edge.
        .when(selected, |this| {
            this.child(
                div()
                    .absolute()
                    .left(px(2.0))
                    .top(px(height * 0.25))
                    .h(px(height * 0.5))
                    .w(px(3.0))
                    .rounded(px(2.0))
                    .bg(rgb(theme::accent())),
            )
        })
        .child(media_square(row, media, icon, selected))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .when(stacked, |this| this.flex_col().gap(px(2.0)))
                .when(!stacked, |this| this.items_baseline().gap(px(8.0)))
                .overflow_hidden()
                .child(
                    div()
                        .when(stacked, |this| this.w_full())
                        .when(!stacked, |this| this.flex_shrink_0().max_w(px(320.0)))
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(theme::fg()))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(name),
                )
                .when(!detail.is_empty(), |this| {
                    this.child(
                        div()
                            .min_w(px(0.0))
                            .when(stacked, |this| this.w_full())
                            .text_xs()
                            .text_color(rgb(theme::muted()))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(detail),
                    )
                }),
        )
        // Every tab says it goes to the tab, as it always did; a file only
        // when chosen.
        .when(selected || matches!(row, Row::Tab { .. }), |this| {
            this.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_xs()
                    .text_color(rgb(theme::muted()))
                    .child(hint)
                    .child(
                        div()
                            .size(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .bg(rgb(theme::bg()))
                            .child(Icon::new(IconName::ArrowRight).size_3()),
                    ),
            )
        });
    pressable(element, index, draggable, cx).into_any_element()
}

// A hit of the grid: its picture over its name, the folder in the tooltip.
fn render_tile<H: PaletteBox>(
    row: &Row,
    index: usize,
    selected: bool,
    media: Media,
    icon: f32,
    width: f32,
    cx: &mut Context<H>,
) -> AnyElement {
    let (name, path) = match row {
        Row::Hit { name, path, .. } => (name.clone(), path.clone()),
        _ => (String::new(), String::new()),
    };
    let tip: SharedString = path.into();
    let element = div()
        .id(("palette-tile", index))
        .w(px(width))
        .h(px(icon + 2.0 * TILE_PADDING + TILE_NAME))
        .p(px(TILE_PADDING))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(6.0))
        .rounded(px(10.0))
        .border_1()
        .cursor_pointer()
        .border_color(if selected {
            rgb(theme::accent())
        } else {
            rgba(0x0000_0000)
        })
        .when(selected, |this| this.bg(rgb(theme::bg_hover())))
        .when(!selected, |this| {
            this.hover(|this| this.bg(rgb(theme::bg_hover())))
        })
        .tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
        })
        .child(media_square(row, media, icon, selected))
        .child(
            div()
                .w_full()
                .h(px(TILE_NAME - 6.0))
                .text_xs()
                .line_height(px(17.0))
                .text_center()
                .line_clamp(2)
                .overflow_hidden()
                .text_color(rgb(theme::fg()))
                .child(name),
        );
    pressable(element, index, true, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: it brings gpui's `test` attribute over the std one.
    use super::{
        GRID_FROM, HitAction, PICTURES_FROM, ZOOM_DEFAULT, folder_and_selection, icon_size,
        plays_in_box, zoom_percent,
    };

    #[test]
    fn a_click_plays_videos_and_sounds_in_the_box_and_opens_the_rest() {
        assert!(plays_in_box(r"C:\a\clip.mp4", false));
        assert!(plays_in_box(r"C:\a\Bonk.WAV", false));
        assert!(!plays_in_box(r"C:\a\notes.txt", false));
        assert!(!plays_in_box(r"C:\a\photo.png", false));
        // A folder named like a video is still a folder.
        assert!(!plays_in_box(r"C:\a\takes.mp4", true));
    }

    #[test]
    fn every_menu_entry_comes_back_as_its_action() {
        for action in HitAction::ALL {
            assert_eq!(HitAction::from_id(action as u32), Some(action));
            assert!(action as u32 > 0, "0 is no entry");
        }
        assert_eq!(HitAction::from_id(0), None);
        assert_eq!(HitAction::from_id(0x1000), None);
    }

    #[test]
    fn the_box_turns_into_a_grid_past_the_list_sizes() {
        assert!(
            icon_size(0.0) < PICTURES_FROM,
            "the smallest size is plain icons"
        );
        assert!(
            icon_size(ZOOM_DEFAULT) >= PICTURES_FROM,
            "the default shows pictures"
        );
        assert!(icon_size(ZOOM_DEFAULT) < GRID_FROM, "the default is a list");
        assert!(icon_size(1.0) > GRID_FROM, "the largest size is a grid");
        assert_eq!(zoom_percent(ZOOM_DEFAULT), 100);
    }

    #[cfg(windows)]
    #[test]
    fn a_file_opens_its_folder_with_it_selected() {
        let (folder, select) = folder_and_selection(r"C:\a\b.txt", false);
        assert_eq!(folder, r"C:\a");
        assert_eq!(select.as_deref(), Some(r"C:\a\b.txt"));
        assert_eq!(
            folder_and_selection(r"C:\a", true),
            (r"C:\a".to_string(), None)
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn a_file_opens_its_folder_with_it_selected() {
        let (folder, select) = folder_and_selection("/a/b.txt", false);
        assert_eq!(folder, "/a");
        assert_eq!(select.as_deref(), Some("/a/b.txt"));
        assert_eq!(folder_and_selection("/a", true), ("/a".to_string(), None));
    }
}
