use nohrs_core::{t, tf};
use std::sync::Mutex;

use gpui::{AppContext as _, AsyncWindowContext, Context, Pixels, Point, WeakEntity, Window};
use nohrs_core::config::{self, ThemeMode};
use nohrs_core::telemetry::LogErr;

use super::ExplorerPane;
use super::grouping::GroupBy;
use super::types::{SortKey, StatusLevel, ViewMode};
use crate::explorer::commands::{OpenIn, interned, keys_of};
use crate::shell_menu::{self, Choice, Entry, MenuItem, Request, Session, Target};
use nohrs_core::shortcuts::Action;

// Nohrs's own menu entries. The values are the menu ids, so they stay below
// the range the Shell's entries are numbered from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
enum Command {
    ViewList = 1,
    ViewGrid,
    ViewTree,
    ViewCompact,
    ToggleHidden,
    ThemeDark,
    ThemeLight,
    ThemeSystem,
    SortName,
    SortType,
    SortSize,
    SortModified,
    SortAscending,
    SortDescending,
    Refresh,
    Paste,
    NewFolder,
    GroupNone,
    GroupKind,
    GroupModified,
    GroupSize,
    FitColumn,
    FitAllColumns,
    ColumnName,
    ColumnType,
    ColumnSize,
    ColumnModified,
    Cut,
    Copy,
    Rename,
    Delete,
    MoreOptions,
    OpenCommandPrompt,
    OpenPowerShell,
    OpenWindowsTerminal,
    OpenFileExplorer,
    NewTag,
    RenameTag,
    DeleteTag,
    RemoveNetworkPlace,
    CopyPath,
    OpenInNewTab,
    Properties,
    Customize,
    UndoTrash,
    RestoreFromBin,
    PurgeFromBin,
    EmptyBin,
    RemoveFromTag,
    Redo,
}

impl Command {
    const ALL: [Command; 50] = [
        Command::ViewList,
        Command::ViewGrid,
        Command::ViewTree,
        Command::ViewCompact,
        Command::ToggleHidden,
        Command::ThemeDark,
        Command::ThemeLight,
        Command::ThemeSystem,
        Command::SortName,
        Command::SortType,
        Command::SortSize,
        Command::SortModified,
        Command::SortAscending,
        Command::SortDescending,
        Command::Refresh,
        Command::Paste,
        Command::NewFolder,
        Command::GroupNone,
        Command::GroupKind,
        Command::GroupModified,
        Command::GroupSize,
        Command::FitColumn,
        Command::FitAllColumns,
        Command::ColumnName,
        Command::ColumnType,
        Command::ColumnSize,
        Command::ColumnModified,
        Command::Cut,
        Command::Copy,
        Command::Rename,
        Command::Delete,
        Command::MoreOptions,
        Command::OpenCommandPrompt,
        Command::OpenPowerShell,
        Command::OpenWindowsTerminal,
        Command::OpenFileExplorer,
        Command::NewTag,
        Command::RenameTag,
        Command::DeleteTag,
        Command::RemoveNetworkPlace,
        Command::CopyPath,
        Command::OpenInNewTab,
        Command::Properties,
        Command::Customize,
        Command::UndoTrash,
        Command::Redo,
        Command::RestoreFromBin,
        Command::PurgeFromBin,
        Command::EmptyBin,
        Command::RemoveFromTag,
    ];

    fn from_id(id: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|command| *command as u32 == id)
    }
}

// The theme mode the config asks for, as last applied by the root view. The
// pane only needs it to tick the right entry, and `system` cannot be told
// apart from the palette it resolved to.
static THEME_MODE: Mutex<ThemeMode> = Mutex::new(ThemeMode::Dark);

/// Records the configured theme mode, for the menu's Theme entries.
pub(crate) fn note_theme_mode(mode: ThemeMode) {
    if let Ok(mut current) = THEME_MODE.lock() {
        *current = mode;
    }
}

fn theme_mode() -> ThemeMode {
    THEME_MODE.lock().map_or(ThemeMode::Dark, |mode| *mode)
}

// The file menu as `[menu]` arranges it, as last applied by the root view.
static MENU: Mutex<Option<config::Menu>> = Mutex::new(None);

/// Records how the file menu is arranged.
pub(crate) fn note_menu(menu: &config::Menu) {
    if let Ok(mut current) = MENU.lock() {
        *current = Some(menu.clone());
    }
}

fn menu_config() -> config::Menu {
    MENU.lock()
        .ok()
        .and_then(|menu| menu.clone())
        .unwrap_or_default()
}

/// The label and icon of an id `[menu]` may hold, for the menu and for the
/// settings page that arranges it. `None` for an id this build does not know.
pub(crate) fn menu_entry_info(id: &str) -> Option<(&'static str, &'static str)> {
    Some(match id {
        "cut" => (t!("Recortar"), "icons/scissors.svg"),
        "copy" => (t!("Copiar"), "icons/copy.svg"),
        "copy_path" => (t!("Copiar como caminho"), "icons/link.svg"),
        "rename" => (t!("Renomear"), "icons/pencil.svg"),
        "delete" => (t!("Excluir"), "icons/trash-2.svg"),
        "new_tab" => (t!("Abrir em nova aba"), "icons/plus.svg"),
        "properties" => (t!("Propriedades"), "icons/info.svg"),
        "tags" => (t!("Tags"), "icons/tag.svg"),
        "windows" => (t!("Itens do Windows"), "icons/app-window.svg"),
        "more_options" => (t!("Mostrar mais opções"), "icons/ellipsis.svg"),
        "customize" => (t!("Personalizar este menu…"), "icons/settings.svg"),
        _ => return None,
    })
}

/// Which Shell verbs the explorer runs itself. Copy and Cut go through Nohrs's
/// clipboard, so its own Paste and the conflict dialog see them; Delete goes to
/// the recycle bin with Nohrs's undo; Rename edits inline. Open on a folder
/// navigates here instead of opening another window.
fn intercepted(target_is_one_folder: bool) -> &'static [&'static str] {
    if target_is_one_folder {
        &[
            "copy",
            "cut",
            "delete",
            "rename",
            "open",
            "explore",
            "opencontaining",
        ]
    } else {
        &["copy", "cut", "delete", "rename", "opencontaining"]
    }
}

/// One of the icons in the row above a file's menu, as in Windows 11.
pub(crate) struct QuickAction {
    pub id: u32,
    pub icon: &'static str,
    pub label: &'static str,
}

/// What an open menu was opened for, which decides what its entries do.
#[derive(Clone)]
pub(crate) enum MenuPurpose {
    /// The listing: selected entries, or the folder's empty space.
    Listing {
        target: Target,
        one_folder: bool,
        extended: bool,
    },
    /// A place in the sidebar.
    Place(super::places::Place),
    /// The column headings; the column right-clicked, if any.
    Columns(Option<usize>),
    /// The funnel's kinds.
    Filter,
    /// A tag in the sidebar.
    Tag(u32),
    /// A network place added by hand.
    NetworkPlace(String),
}

/// A menu on screen: what it shows, where, and what runs its entries.
pub(crate) struct OpenMenu {
    pub items: Vec<MenuItem>,
    pub quick: Vec<QuickAction>,
    pub position: Point<Pixels>,
    /// The open submenu at each level, by index.
    pub open_path: Vec<usize>,
    session: Option<Session>,
    purpose: MenuPurpose,
}

// The filter menu's ids, past the commands' range.
const FIRST_KIND: u32 = 0x800;
// A tag's tick in a file's Tags submenu: this plus the tag's id, for the
// first TAG_IDS tags (the Shell's own entries start at 0x1000).
const FIRST_TAG: u32 = 0x900;
const TAG_IDS: u32 = 0x400;
// A colour in a tag's menu: this plus its place in `tags::COLORS`.
const FIRST_TAG_COLOR: u32 = 0x8F0;

impl ExplorerPane {
    /// Opens the right-click menu: for the row `row` (selecting it first if
    /// it is not part of the selection), or for the folder's empty space.
    pub(crate) fn open_context_menu(
        &mut self,
        row: Option<usize>,
        extended: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = row
            && !self.is_selected(row)
        {
            self.select_single(row);
            cx.notify();
        }
        let Some(hwnd) = crate::ipc::window_hwnd(window) else {
            return;
        };
        let paths = if row.is_some() {
            self.selected_paths()
        } else {
            Vec::new()
        };
        let one_folder = match paths.as_slice() {
            [path] => self
                .filtered_entries
                .iter()
                .any(|entry| entry.kind == "dir" && &entry.path == path),
            _ => false,
        };
        let purpose = MenuPurpose::Listing {
            target: if paths.is_empty() {
                Target::Background(self.cwd.clone())
            } else {
                Target::Items(paths)
            },
            one_folder,
            extended,
        };
        // In the Recycle Bin a file's Windows entries would act on its copy
        // inside `$Recycle.Bin`; the Bin's own are what make sense there.
        if super::recycle_bin::is_recycle_bin(&self.cwd) {
            let own = if row.is_some() {
                vec![
                    Entry::command(Command::RestoreFromBin as u32, t!("Restaurar"))
                        .with_icon("icons/undo-2.svg"),
                    Entry::Separator,
                    Entry::command(Command::PurgeFromBin as u32, t!("Excluir para sempre"))
                        .with_icon("icons/trash-2.svg"),
                ]
            } else {
                vec![
                    Entry::command(Command::EmptyBin as u32, t!("Esvaziar a Lixeira"))
                        .with_icon("icons/trash-2.svg"),
                    Entry::command(Command::Refresh as u32, t!("Atualizar"))
                        .with_icon("icons/refresh-cw.svg"),
                ]
            };
            self.show_menu_with(purpose, Vec::new(), own, false, hwnd, window, cx);
            return;
        }
        if menu_config().classic {
            self.show_classic_menu(purpose, window, cx);
            return;
        }
        let (before, own, windows) = self.listing_entries(&purpose, cx);
        self.show_menu_with(purpose, before, own, windows, hwnd, window, cx);
    }

    // Nohrs's entries for a listing menu, as (above the Windows entries,
    // below them, whether the Windows entries are shown at all): the view and
    // sort submenus on the folder's empty space; on files, what `[menu]`
    // lists, in its order.
    fn listing_entries(
        &self,
        purpose: &MenuPurpose,
        cx: &mut Context<Self>,
    ) -> (Vec<Entry>, Vec<Entry>, bool) {
        let more = Entry::command(Command::MoreOptions as u32, t!("Mostrar mais opções"))
            .with_icon("icons/ellipsis.svg")
            .with_shortcut("Shift+F10");
        match purpose {
            MenuPurpose::Listing {
                target: Target::Background(_),
                ..
            } => {
                let mut entries = self.background_entries(cx);
                entries.push(Entry::Separator);
                entries.push(more);
                (Vec::new(), entries, true)
            }
            MenuPurpose::Listing {
                target: Target::Items(paths),
                ..
            } => {
                let menu = menu_config();
                let mut before = Vec::new();
                let mut after = Vec::new();
                let mut windows = false;
                for id in &menu.items {
                    let entry = match id.as_str() {
                        "windows" => {
                            windows = true;
                            continue;
                        }
                        "new_tab" => {
                            Entry::command(Command::OpenInNewTab as u32, t!("Abrir em nova aba"))
                                .with_icon("icons/plus.svg")
                                .with_shortcut(t!("Botão do meio"))
                        }
                        "copy_path" => {
                            Entry::command(Command::CopyPath as u32, t!("Copiar como caminho"))
                                .with_icon("icons/link.svg")
                                .with_shortcut(keys_of(Action::CopyPath))
                        }
                        "properties" => {
                            Entry::command(Command::Properties as u32, t!("Propriedades"))
                                .with_icon("icons/info.svg")
                        }
                        "tags" => {
                            // Ticked when every selected item has it; a click
                            // puts it on all of them, or takes it off all.
                            let mut tags: Vec<Entry> = super::tags::Tags::all(cx)
                                .into_iter()
                                .filter(|tag| tag.id < TAG_IDS)
                                .map(|tag| {
                                    Entry::check(
                                        FIRST_TAG + tag.id,
                                        &tag.name,
                                        super::tags::Tags::all_have(paths, tag.id, cx),
                                    )
                                })
                                .collect();
                            tags.push(Entry::Separator);
                            tags.push(
                                Entry::command(Command::NewTag as u32, t!("Nova tag…"))
                                    .with_icon("icons/plus.svg"),
                            );
                            Entry::submenu(t!("Tags"), tags).with_icon("icons/tag.svg")
                        }
                        "more_options" => more.clone(),
                        "customize" => {
                            Entry::command(Command::Customize as u32, t!("Personalizar este menu…"))
                                .with_icon("icons/settings.svg")
                        }
                        _ => continue,
                    };
                    if windows {
                        after.push(entry);
                    } else {
                        before.push(entry);
                    }
                }
                // In a tag's list, what is in it can leave it, first thing:
                // Delete there would send the file itself to the Recycle Bin.
                if super::tags::tag_of_location(&self.cwd).is_some() {
                    before.insert(
                        0,
                        Entry::command(Command::RemoveFromTag as u32, t!("Remover desta tag"))
                            .with_icon("icons/tag.svg"),
                    );
                }
                (before, after, windows)
            }
            _ => (Vec::new(), vec![more], true),
        }
    }

    fn show_menu(
        &mut self,
        purpose: MenuPurpose,
        own: Vec<Entry>,
        hwnd: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_menu_with(purpose, Vec::new(), own, true, hwnd, window, cx);
    }

    // Reads the menu for `purpose` and opens it at the pointer. The Shell is
    // asked between updates, as its extensions may pump messages. Without
    // `windows` it is not asked at all: the menu is only Nohrs's.
    #[allow(clippy::too_many_arguments)]
    fn show_menu_with(
        &mut self,
        purpose: MenuPurpose,
        before: Vec<Entry>,
        own: Vec<Entry>,
        windows: bool,
        hwnd: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let position = window.mouse_position();
        let menu = menu_config();
        let own_labels: Vec<String> = before
            .iter()
            .chain(own.iter())
            .filter_map(|entry| entry.label().map(str::to_string))
            .collect();
        // A Windows verb leaves the list only while the row above has it: take
        // Cut out of the row and Windows' Cut comes back.
        let shell_hidden: Vec<&'static str> = std::iter::once("paste")
            .chain(
                ["cut", "copy", "rename", "delete"]
                    .into_iter()
                    .filter(|verb| menu.quick.iter().any(|id| id == verb)),
            )
            // Windows 11 has its own "Copy as path"; ours stands for it.
            .chain(
                (menu
                    .quick
                    .iter()
                    .chain(menu.items.iter())
                    .any(|id| id == "copy_path"))
                .then_some("copyaspath"),
            )
            .collect();
        // The Recycle Bin's items take only its own entries: the row's Cut,
        // Copy, Rename and Delete would act on their copies in `$Recycle.Bin`.
        let in_recycle_bin = super::recycle_bin::is_recycle_bin(&self.cwd);
        cx.spawn_in(
            window,
            async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
                let (target, own_first, extended, intercept, hide, quick) = match &purpose {
                    MenuPurpose::Listing {
                        target,
                        one_folder,
                        extended,
                    } => {
                        let files = matches!(target, Target::Items(_));
                        (
                            target.clone(),
                            !files,
                            *extended,
                            intercepted(*one_folder).to_vec(),
                            // Paste is Nohrs's own; on files the ones in the
                            // row above replace the Shell's.
                            if files {
                                &shell_hidden[..]
                            } else {
                                &["paste"][..]
                            },
                            files && !in_recycle_bin,
                        )
                    }
                    MenuPurpose::Place(place) => (
                        Target::Items(vec![place.parsing_name.clone()]),
                        false,
                        false,
                        if place.path.is_some() {
                            vec!["open", "explore"]
                        } else {
                            Vec::new()
                        },
                        &[][..],
                        false,
                    ),
                    MenuPurpose::Columns(_)
                    | MenuPurpose::Filter
                    | MenuPurpose::Tag(_)
                    | MenuPurpose::NetworkPlace(_) => {
                        (Target::None, true, false, Vec::new(), &[][..], false)
                    }
                };
                let target = if windows { target } else { Target::None };
                let mut prepared = shell_menu::prepare(&Request {
                    hwnd: hwnd as isize,
                    target: &target,
                    own: &own,
                    before: &before,
                    own_first,
                    extended,
                    dark: true,
                    intercept: &intercept,
                    hide,
                });
                // Windows entries the person hid, by the label the menu shows.
                for item in &mut prepared.items {
                    item.from_windows = is_windows_entry(item, &own_labels);
                }
                prepared.items.retain(|item| {
                    !(item.from_windows && menu.hidden.iter().any(|hidden| *hidden == item.label))
                });
                let items = shell_menu::tidy(prepared.items);
                this.update(cx, |pane, cx| {
                    pane.context_menu = Some(OpenMenu {
                        items,
                        quick: if quick {
                            quick_actions(&menu.quick)
                        } else {
                            Vec::new()
                        },
                        position,
                        open_path: Vec::new(),
                        session: Some(prepared.session),
                        purpose,
                    });
                    cx.notify();
                })
                .log_err();
            },
        )
        .detach();
    }

    /// Closes the open menu without choosing.
    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    /// The pointer is over entry `index` of `level`: its submenu opens, and
    /// any deeper one closes.
    pub(crate) fn hover_menu_entry(
        &mut self,
        level: usize,
        index: usize,
        submenu: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.context_menu.as_mut() else {
            return;
        };
        let wanted: Vec<usize> = menu
            .open_path
            .iter()
            .take(level)
            .copied()
            .chain(submenu.then_some(index))
            .collect();
        if menu.open_path != wanted {
            menu.open_path = wanted;
            cx.notify();
        }
    }

    /// Hides a Windows entry of the open menu, now and in every menu after:
    /// its label goes to `[menu] hidden`, which Settings › Menu lists to
    /// bring it back.
    pub(crate) fn hide_menu_entry(&mut self, label: String, cx: &mut Context<Self>) {
        if let Some(menu) = self.context_menu.as_mut() {
            menu.items
                .retain(|item| !(item.from_windows && item.label == label));
            menu.items = shell_menu::tidy(std::mem::take(&mut menu.items));
            menu.open_path.clear();
        }
        let mut hidden = menu_config().hidden;
        if !hidden.contains(&label) {
            hidden.push(label.clone());
        }
        // Applied here at once; the watcher brings the file's copy back.
        if let Ok(mut current) = MENU.lock() {
            let mut menu = current.clone().unwrap_or_default();
            menu.hidden = hidden.clone();
            *current = Some(menu);
        }
        let quoted: Vec<String> = hidden
            .iter()
            .map(|label| format!("\"{}\"", label.replace('\\', "\\\\").replace('"', "\\\"")))
            .collect();
        self.write_config("menu", "hidden", format!("[{}]", quoted.join(", ")), cx);
        self.set_status(
            StatusLevel::Info,
            tf!(
                "\"{label}\" escondido do menu. Volte com ele em Configurações › Menu.",
                label = label
            ),
        );
        cx.notify();
    }

    /// Runs entry `id` of the open menu and closes it.
    pub(crate) fn choose_menu_item(
        &mut self,
        id: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };
        cx.notify();
        if id == Command::MoreOptions as u32 {
            self.show_classic_menu(menu.purpose, window, cx);
            return;
        }
        let purpose = menu.purpose;
        let Some(session) = menu.session else {
            self.apply_choice(purpose, Choice::Own(id), window, cx);
            return;
        };
        // A Shell command may open its own dialogs: it runs between updates.
        cx.spawn_in(
            window,
            async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
                let choice = shell_menu::run(session, id);
                this.update_in(cx, |pane, window, cx| {
                    pane.apply_choice(purpose, choice, window, cx)
                })
                .log_err();
            },
        )
        .detach();
    }

    // What an entry does, by what the menu was opened for.
    fn apply_choice(
        &mut self,
        purpose: MenuPurpose,
        choice: Choice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match purpose {
            MenuPurpose::Listing { .. } => self.apply_menu_choice(choice, window, cx),
            MenuPurpose::Place(place) => {
                match choice {
                    Choice::Verb(_) => self.open_place(&place, window, cx),
                    // Unpinning, renaming or deleting changes the list.
                    Choice::Ran => self.refresh_places_after_shell(window, cx),
                    Choice::Dismissed | Choice::Own(_) => {}
                }
                cx.notify();
            }
            MenuPurpose::Columns(column) => {
                if let Choice::Own(id) = choice {
                    match Command::from_id(id) {
                        Some(Command::FitColumn) => {
                            if let Some(column) = column {
                                self.fit_column(column);
                            }
                        }
                        Some(Command::FitAllColumns) => {
                            (0..4).for_each(|column| self.fit_column(column))
                        }
                        Some(Command::ColumnType) => self.toggle_column(1),
                        Some(Command::ColumnSize) => self.toggle_column(2),
                        Some(Command::ColumnModified) => self.toggle_column(3),
                        _ => {}
                    }
                }
                cx.notify();
            }
            MenuPurpose::Tag(tag) => {
                if let Choice::Own(id) = choice {
                    match Command::from_id(id) {
                        Some(Command::RenameTag) => {
                            let name = super::tags::Tags::get(tag, cx)
                                .map(|tag| tag.name)
                                .unwrap_or_default();
                            self.begin_sidebar_input(
                                super::places::SidebarInputPurpose::RenameTag(tag),
                                name,
                                window,
                                cx,
                            );
                        }
                        Some(Command::DeleteTag) => {
                            super::tags::Tags::delete(tag, cx);
                            if super::tags::tag_of_location(&self.cwd) == Some(tag) {
                                self.go_back(window, cx);
                            }
                        }
                        _ => {
                            if let Some((color, _)) = id
                                .checked_sub(FIRST_TAG_COLOR)
                                .and_then(|index| super::tags::COLORS.get(index as usize))
                            {
                                super::tags::Tags::recolor(tag, *color, cx);
                            }
                        }
                    }
                }
                cx.notify();
            }
            MenuPurpose::NetworkPlace(address) => {
                if let Choice::Own(id) = choice
                    && Command::from_id(id) == Some(Command::RemoveNetworkPlace)
                {
                    super::network::NetworkPlaces::remove(&address, cx);
                }
                cx.notify();
            }
            MenuPurpose::Filter => {
                use super::filter::KindFilter;
                if let Choice::Own(id) = choice
                    && let Some(kind) = id
                        .checked_sub(FIRST_KIND)
                        .and_then(|index| KindFilter::ALL.get(index as usize).copied())
                {
                    self.set_kind_filter(kind, cx);
                }
            }
        }
    }

    // "Show more options": the classic Windows menu, with every entry an
    // extension draws itself, as Windows 11 keeps it.
    fn show_classic_menu(
        &mut self,
        purpose: MenuPurpose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(hwnd) = crate::ipc::window_hwnd(window) else {
            return;
        };
        let MenuPurpose::Listing {
            target,
            one_folder,
            extended,
        } = purpose.clone()
        else {
            return;
        };
        let own = match &target {
            Target::Background(_) => self.background_entries(cx),
            _ => Vec::new(),
        };
        let dark = nohrs_ui::theme::theme::is_dark();
        cx.spawn_in(
            window,
            async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
                let choice = shell_menu::show(&Request {
                    hwnd: hwnd as isize,
                    target: &target,
                    own: &own,
                    before: &[],
                    own_first: matches!(target, Target::Background(_)),
                    extended,
                    dark,
                    intercept: intercepted(one_folder),
                    hide: &["paste"],
                });
                this.update_in(cx, |pane, window, cx| {
                    pane.apply_choice(purpose, choice, window, cx)
                })
                .log_err();
            },
        )
        .detach();
    }

    /// The Windows menu for a sidebar place: Unpin from Quick access, Open
    /// in new window, Properties and the rest, as in Explorer's pane.
    pub(crate) fn open_place_menu(
        &mut self,
        place: super::places::Place,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(hwnd) = crate::ipc::window_hwnd(window) else {
            return;
        };
        self.show_menu(MenuPurpose::Place(place), Vec::new(), hwnd, window, cx);
    }

    /// A tag's menu in the sidebar: rename, colour, delete.
    pub(crate) fn open_tag_menu(&mut self, tag: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hwnd) = crate::ipc::window_hwnd(window) else {
            return;
        };
        let current = super::tags::Tags::get(tag, cx).map(|tag| tag.color);
        let colors = super::tags::COLORS
            .iter()
            .enumerate()
            .map(|(index, (color, name))| {
                Entry::radio(
                    FIRST_TAG_COLOR + index as u32,
                    nohrs_core::i18n::tr(name),
                    current == Some(*color),
                )
            })
            .collect();
        let own = vec![
            Entry::command(Command::RenameTag as u32, t!("Renomear")).with_icon("icons/pencil.svg"),
            Entry::submenu(t!("Cor"), colors).with_icon("icons/palette-2.svg"),
            Entry::Separator,
            Entry::command(Command::DeleteTag as u32, t!("Excluir tag"))
                .with_icon("icons/trash-2.svg"),
        ];
        self.show_menu(MenuPurpose::Tag(tag), own, hwnd, window, cx);
    }

    /// A network place's menu in the sidebar.
    pub(crate) fn open_network_place_menu(
        &mut self,
        address: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(hwnd) = crate::ipc::window_hwnd(window) else {
            return;
        };
        let own = vec![
            Entry::command(
                Command::RemoveNetworkPlace as u32,
                t!("Remover dos locais de rede"),
            )
            .with_icon("icons/trash-2.svg"),
        ];
        self.show_menu(MenuPurpose::NetworkPlace(address), own, hwnd, window, cx);
    }

    /// The column header's menu, as Explorer's: fit one column or all, and
    /// which columns to show. `column` is the one right-clicked, if any.
    pub(crate) fn open_column_menu(
        &mut self,
        column: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(hwnd) = crate::ipc::window_hwnd(window) else {
            return;
        };
        let own = vec![
            Entry::command(Command::FitColumn as u32, t!("Ajustar coluna ao conteúdo"))
                .with_icon("icons/columns.svg")
                .enabled(column.is_some_and(|column| self.column_visible(column))),
            Entry::command(
                Command::FitAllColumns as u32,
                t!("Ajustar todas as colunas ao conteúdo"),
            )
            .with_icon("icons/table.svg"),
            Entry::Separator,
            Entry::check(Command::ColumnName as u32, t!("Nome"), true).enabled(false),
            Entry::check(
                Command::ColumnType as u32,
                t!("Tipo"),
                self.column_visible(1),
            ),
            Entry::check(
                Command::ColumnSize as u32,
                t!("Tamanho"),
                self.column_visible(2),
            ),
            Entry::check(
                Command::ColumnModified as u32,
                t!("Data de modificação"),
                self.column_visible(3),
            ),
        ];
        self.show_menu(MenuPurpose::Columns(column), own, hwnd, window, cx);
    }

    /// The bottom bar's filter menu: one entry per kind, the current one
    /// ticked; picking one shows only that kind.
    pub(crate) fn open_filter_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use super::filter::KindFilter;
        let Some(hwnd) = crate::ipc::window_hwnd(window) else {
            return;
        };
        let current = self.kind_filter;
        let mut own: Vec<Entry> = Vec::new();
        for (index, kind) in KindFilter::ALL.into_iter().enumerate() {
            let icon = match kind {
                KindFilter::All => "icons/layout-dashboard.svg",
                KindFilter::Folders => "icons/folder.svg",
                KindFilter::Files => "icons/file.svg",
                KindFilter::Images => "icons/image.svg",
                KindFilter::Videos => "icons/film.svg",
                KindFilter::Audio => "icons/music.svg",
                KindFilter::Documents => "icons/file-text.svg",
                KindFilter::Archives => "icons/archive.svg",
                KindFilter::Programs => "icons/app-window.svg",
            };
            own.push(
                Entry::check(
                    FIRST_KIND + index as u32,
                    kind.menu_label(),
                    kind == current,
                )
                .with_icon(icon),
            );
            // "Tudo", then folders and files, then the kinds of file.
            if matches!(kind, KindFilter::All | KindFilter::Files) {
                own.push(Entry::Separator);
            }
        }
        self.show_menu(MenuPurpose::Filter, own, hwnd, window, cx);
    }

    fn background_entries(&self, cx: &mut Context<Self>) -> Vec<Entry> {
        let theme = theme_mode();
        let sort = |command, label, key| Entry::radio(command as u32, label, self.sort_key == key);
        let group = |command, label, by| Entry::radio(command as u32, label, self.group_by == by);
        vec![
            Entry::submenu(
                t!("Exibir"),
                vec![
                    Entry::radio(
                        Command::ViewGrid as u32,
                        t!("Ícones"),
                        self.view_mode == ViewMode::Grid,
                    ),
                    Entry::radio(
                        Command::ViewCompact as u32,
                        t!("Lista"),
                        self.view_mode == ViewMode::Compact,
                    ),
                    Entry::radio(
                        Command::ViewTree as u32,
                        t!("Árvore"),
                        self.view_mode == ViewMode::Tree,
                    ),
                    Entry::radio(
                        Command::ViewList as u32,
                        t!("Detalhes"),
                        self.view_mode == ViewMode::List,
                    ),
                    Entry::Separator,
                    Entry::check(
                        Command::ToggleHidden as u32,
                        t!("Itens ocultos"),
                        self.show_hidden,
                    ),
                    Entry::Separator,
                    Entry::submenu(
                        t!("Tema"),
                        vec![
                            Entry::radio(
                                Command::ThemeDark as u32,
                                t!("Escuro"),
                                theme == ThemeMode::Dark,
                            ),
                            Entry::radio(
                                Command::ThemeLight as u32,
                                t!("Claro"),
                                theme == ThemeMode::Light,
                            ),
                            Entry::radio(
                                Command::ThemeSystem as u32,
                                t!("Igual ao Windows"),
                                theme == ThemeMode::System,
                            ),
                        ],
                    )
                    .with_icon("icons/palette-2.svg"),
                ],
            )
            .with_icon("icons/eye.svg"),
            Entry::submenu(
                t!("Classificar por"),
                vec![
                    sort(Command::SortName, t!("Nome"), SortKey::Name),
                    sort(Command::SortType, t!("Tipo"), SortKey::Type),
                    sort(Command::SortSize, t!("Tamanho"), SortKey::Size),
                    sort(
                        Command::SortModified,
                        t!("Data de modificação"),
                        SortKey::Modified,
                    ),
                    Entry::Separator,
                    Entry::radio(
                        Command::SortAscending as u32,
                        t!("Crescente"),
                        self.sort_asc,
                    ),
                    Entry::radio(
                        Command::SortDescending as u32,
                        t!("Decrescente"),
                        !self.sort_asc,
                    ),
                ],
            )
            .with_icon("icons/arrow-up-down.svg"),
            Entry::submenu(
                t!("Agrupar por"),
                vec![
                    group(Command::GroupNone, t!("Nenhum"), GroupBy::None),
                    Entry::Separator,
                    group(Command::GroupKind, t!("Tipo"), GroupBy::Kind),
                    group(
                        Command::GroupModified,
                        t!("Data de modificação"),
                        GroupBy::Modified,
                    ),
                    group(Command::GroupSize, t!("Tamanho"), GroupBy::Size),
                ],
            )
            .with_icon("icons/layers.svg"),
            Entry::command(Command::Refresh as u32, t!("Atualizar"))
                .with_icon("icons/refresh-cw.svg")
                .with_shortcut(keys_of(Action::Refresh)),
            Entry::command(
                Command::UndoTrash as u32,
                match self.undo_kind() {
                    Some(super::file_ops::UndoKind::Moved) => t!("Desfazer mover"),
                    Some(super::file_ops::UndoKind::Copied) => t!("Desfazer cópia"),
                    Some(super::file_ops::UndoKind::Renamed) => t!("Desfazer renomear"),
                    Some(super::file_ops::UndoKind::Created) => t!("Desfazer nova pasta"),
                    Some(super::file_ops::UndoKind::Trashed) | None => t!("Desfazer exclusão"),
                },
            )
            .with_icon("icons/undo-2.svg")
            .with_shortcut(keys_of(Action::Undo))
            .enabled(self.can_undo_trash()),
            Entry::command(Command::Redo as u32, t!("Refazer"))
                .with_icon("icons/redo-2.svg")
                .with_shortcut(keys_of(Action::Redo))
                .enabled(self.can_redo()),
            Entry::Separator,
            Entry::command(Command::Paste as u32, t!("Colar"))
                .with_icon("icons/clipboard.svg")
                .with_shortcut(keys_of(Action::Paste))
                .enabled(self.can_paste(cx)),
            Entry::command(Command::NewFolder as u32, t!("Nova pasta"))
                .with_icon("icons/folder-plus.svg")
                .with_shortcut(keys_of(Action::NewFolder)),
            Entry::Separator,
            Entry::submenu(t!("Abrir esta pasta no"), {
                let mut programs = vec![
                    Entry::command(Command::OpenCommandPrompt as u32, t!("Prompt de Comando"))
                        .with_icon("icons/square-terminal.svg")
                        .with_shortcut(keys_of(Action::OpenCommandPrompt)),
                    Entry::command(Command::OpenPowerShell as u32, "PowerShell")
                        .with_icon("icons/square-terminal.svg")
                        .with_shortcut(keys_of(Action::OpenPowerShell)),
                ];
                if crate::explorer::commands::has_windows_terminal() {
                    programs.push(
                        Entry::command(Command::OpenWindowsTerminal as u32, "Windows Terminal")
                            .with_icon("icons/app-terminal.svg"),
                    );
                }
                programs.push(
                    Entry::command(
                        Command::OpenFileExplorer as u32,
                        t!("Explorador do Windows"),
                    )
                    .with_icon("icons/folder-open.svg")
                    .with_shortcut(keys_of(Action::OpenFileExplorer)),
                );
                programs
            })
            .with_icon("icons/external-link.svg"),
        ]
    }

    fn apply_menu_choice(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        match choice {
            Choice::Dismissed => {}
            Choice::Own(id) => {
                if let Some(command) = Command::from_id(id) {
                    self.run_command(command, window, cx);
                } else if (FIRST_TAG..FIRST_TAG + TAG_IDS).contains(&id) {
                    let tag = id - FIRST_TAG;
                    let paths = self.selected_paths();
                    super::tags::Tags::toggle(&paths, tag, cx);
                }
            }
            Choice::Verb(verb) => match verb.as_str() {
                "copy" => self.copy_selection(cx),
                "cut" => self.cut_selection(cx),
                "delete" => self.trash_selection(cx),
                "rename" => {
                    if let Some(row) = self.active_row() {
                        self.rename_at(row, window, cx);
                    }
                }
                "open" | "explore" => {
                    let folder = self.selected_paths().into_iter().next().and_then(|path| {
                        self.filtered_entries
                            .iter()
                            .find(|entry| entry.path == path)
                            .cloned()
                    });
                    if let Some(folder) = folder {
                        self.activate_entry(folder, window, cx);
                    }
                }
                "opencontaining" => self.open_item_location(window, cx),
                _ => {}
            },
            // The Shell may have created, moved or deleted something ("New",
            // "Send to", an archiver); the listing is read again either way.
            // Pinning to Quick access changes the sidebar as well as the
            // folder's own menu.
            Choice::Ran => {
                self.refresh_places_after_shell(window, cx);
                self.reload(cx);
            }
        }
        cx.notify();
    }

    // "Open file location" run by the Shell starts the Explorer's own reveal,
    // which came back here with the folder but not the file (issue #35).
    // Done here, a shortcut's target is selected in this tab.
    fn open_item_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.selected_paths().into_iter().next() else {
            return;
        };
        let target = crate::shell_menu::shortcut_target(&item).unwrap_or(item);
        let target_path = std::path::Path::new(&target);
        let Some(folder) = target_path.parent().filter(|_| target_path.exists()) else {
            self.set_status(
                StatusLevel::Error,
                t!("O destino do atalho não existe mais").to_string(),
            );
            return;
        };
        let folder = folder.to_string_lossy().into_owned();
        if !super::same_path(&self.cwd, &folder) {
            self.change_dir(folder, window, cx);
        }
        self.ensure_loaded(cx);
        self.reveal(&target);
    }

    fn run_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            Command::ViewList => self.set_view_mode(ViewMode::List, cx),
            Command::ViewGrid => self.set_view_mode(ViewMode::Grid, cx),
            Command::ViewTree => self.set_view_mode(ViewMode::Tree, cx),
            Command::ViewCompact => self.set_view_mode(ViewMode::Compact, cx),
            // Written to the config like the theme, so it lasts; the reload
            // that follows is what applies it.
            Command::ToggleHidden => {
                let value = if self.show_hidden { "false" } else { "true" };
                self.write_config("ui", "show_hidden", value.to_string(), cx);
            }
            Command::ThemeDark => self.write_config("theme", "mode", "\"dark\"".into(), cx),
            Command::ThemeLight => self.write_config("theme", "mode", "\"light\"".into(), cx),
            Command::ThemeSystem => self.write_config("theme", "mode", "\"system\"".into(), cx),
            Command::SortName => self.set_sort(SortKey::Name, self.sort_asc, cx),
            Command::SortType => self.set_sort(SortKey::Type, self.sort_asc, cx),
            Command::SortSize => self.set_sort(SortKey::Size, self.sort_asc, cx),
            Command::SortModified => self.set_sort(SortKey::Modified, self.sort_asc, cx),
            Command::SortAscending => self.set_sort(self.sort_key, true, cx),
            Command::SortDescending => self.set_sort(self.sort_key, false, cx),
            Command::Refresh => self.reload(cx),
            Command::Paste => self.paste_into_cwd(window, cx),
            Command::NewFolder => self.create_new_folder(window, cx),
            Command::GroupNone => self.set_group_by(GroupBy::None),
            Command::GroupKind => self.set_group_by(GroupBy::Kind),
            Command::GroupModified => self.set_group_by(GroupBy::Modified),
            Command::GroupSize => self.set_group_by(GroupBy::Size),
            Command::Cut => self.cut_selection(cx),
            Command::Copy => self.copy_selection(cx),
            Command::Delete => self.trash_selection(cx),
            Command::Rename => {
                if let Some(row) = self.active_row() {
                    self.rename_at(row, window, cx);
                }
            }
            Command::NewTag => {
                // Made, put on the selection, and named in the sidebar.
                let paths = self.selected_paths();
                let name = t!("Nova tag").to_string();
                let id = super::tags::Tags::create(name.clone(), &paths, cx);
                // With the tags out of the sidebar there is no field to
                // name it in: it keeps the name, renamed from its menu later.
                if super::tags::in_sidebar() {
                    self.sidebar_visible = true;
                    self.begin_sidebar_input(
                        super::places::SidebarInputPurpose::RenameTag(id),
                        name,
                        window,
                        cx,
                    );
                } else {
                    self.set_status(
                        super::types::StatusLevel::Info,
                        nohrs_core::tf!("Tag {tag} criada", tag = name),
                    );
                }
            }
            Command::RemoveFromTag => {
                if let Some(tag) = super::tags::tag_of_location(&self.cwd) {
                    let paths = self.selected_paths();
                    super::tags::Tags::remove(&paths, tag, cx);
                    // The tag's list is the list of what has it.
                    self.reload(cx);
                }
            }
            Command::FitColumn
            | Command::FitAllColumns
            | Command::ColumnName
            | Command::ColumnType
            | Command::ColumnSize
            | Command::ColumnModified
            | Command::RenameTag
            | Command::DeleteTag
            | Command::RemoveNetworkPlace
            | Command::MoreOptions => {}
            Command::CopyPath => self.copy_paths_as_text(cx),
            Command::UndoTrash => self.undo_trash(cx),
            Command::Redo => self.redo(cx),
            Command::RestoreFromBin => self.restore_from_recycle_bin(cx),
            Command::PurgeFromBin => self.purge_from_recycle_bin(cx),
            Command::EmptyBin => self.empty_recycle_bin(cx),
            Command::OpenInNewTab => self.open_selection_in_new_tab(cx),
            Command::Properties => self.show_selection_properties(cx),
            Command::Customize => window.dispatch_action(
                nohrs_ui::components::layout::unified_toolbar::AccountMenuAction::boxed(
                    nohrs_ui::components::layout::unified_toolbar::AccountMenuCommand::Menu,
                ),
                cx,
            ),
            Command::OpenCommandPrompt => self.open_in(OpenIn::CommandPrompt, cx),
            Command::OpenPowerShell => self.open_in(OpenIn::PowerShell, cx),
            Command::OpenWindowsTerminal => self.open_in(OpenIn::WindowsTerminal, cx),
            Command::OpenFileExplorer => self.open_in(OpenIn::FileExplorer, cx),
        }
    }

    fn write_config(
        &mut self,
        section: &'static str,
        key: &'static str,
        value: String,
        cx: &mut Context<Self>,
    ) {
        let path = config::paths::config_file();
        let write =
            cx.background_spawn(async move { config::set_value(&path, section, key, &value) });
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            if let Err(error) = write.await {
                this.update(cx, |pane, cx| {
                    pane.set_status(
                        StatusLevel::Error,
                        tf!(
                            "Não foi possível salvar {section}.{key}: {error}",
                            section = section,
                            key = key,
                            error = error
                        ),
                    );
                    cx.notify();
                })
                .log_err();
            }
        })
        .detach();
    }
}

// "Copiar (Ctrl+C)", with whatever keys the command has now.
fn labelled(label: &str, action: Action) -> &'static str {
    interned(nohrs_core::shortcuts::with_keys(label, action))
}

// The row on top of a file's menu, in the order `[menu] quick` lists it.
fn quick_actions(ids: &[String]) -> Vec<QuickAction> {
    ids.iter()
        .filter_map(|id| {
            let (_, icon) = menu_entry_info(id)?;
            let (command, label) = match id.as_str() {
                "cut" => (Command::Cut, labelled(t!("Recortar"), Action::Cut)),
                "copy" => (Command::Copy, labelled(t!("Copiar"), Action::Copy)),
                "copy_path" => (
                    Command::CopyPath,
                    labelled(t!("Copiar como caminho"), Action::CopyPath),
                ),
                "rename" => (Command::Rename, labelled(t!("Renomear"), Action::Rename)),
                "delete" => (Command::Delete, labelled(t!("Excluir"), Action::Trash)),
                "new_tab" => (Command::OpenInNewTab, t!("Abrir em nova aba")),
                "properties" => (Command::Properties, t!("Propriedades")),
                _ => return None,
            };
            Some(QuickAction {
                id: command as u32,
                icon,
                label,
            })
        })
        .collect()
}

// Whether a drawn entry came from Windows (or a program's Shell extension)
// rather than from Nohrs: those are the ones a person may hide. Submenus
// carry no id, so Nohrs's own are told apart by their labels.
fn is_windows_entry(item: &MenuItem, own_labels: &[String]) -> bool {
    match item.kind {
        shell_menu::ItemKind::Separator => false,
        shell_menu::ItemKind::Submenu(_) => !own_labels.iter().any(|label| *label == item.label),
        _ => item.id >= 0x1000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_ids_round_trip_and_stay_below_the_shell_range() {
        for command in Command::ALL {
            assert_eq!(Command::from_id(command as u32), Some(command));
            assert!((command as u32) < 0x1000);
        }
        assert_eq!(Command::from_id(0), None);
    }
}
