//! The sidebar's places, read from the Shell the way Explorer's navigation
//! pane shows them: Quick access (pinned, then frequent), the drives under
//! This PC with their Explorer names, Network and Linux.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Context, WeakEntity, Window};
use nohrs_core::telemetry::LogErr;

use super::ExplorerPane;
use super::thumbnails::Thumbnails;

/// One sidebar entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// The name Explorer shows ("Área de Trabalho", "Disco Local (C:)").
    pub name: String,
    /// The folder on disk, when there is one. Virtual places (Recycle Bin,
    /// Network) have none and open in Explorer.
    pub path: Option<String>,
    /// The Shell's parsing name: the path, or `::{CLSID}` for a virtual
    /// place. What the icon and the context menu are asked for.
    pub parsing_name: String,
    /// Pinned to Quick access, rather than there because it is frequent.
    pub pinned: bool,
}

/// Everything the sidebar lists.
#[derive(Clone, Debug, Default)]
pub struct Places {
    pub quick_access: Vec<Place>,
    /// Files added with Windows' "Add to favorites". The Shell lists them
    /// among the Quick access folders; they get a group of their own
    /// (issue #24).
    pub favorites: Vec<Place>,
    /// This PC itself, for its localized name.
    pub this_pc: Option<Place>,
    pub drives: Vec<Place>,
    /// Network and Linux, when present.
    pub other: Vec<Place>,
}

/// Reads the places. Slow (the Shell may touch network drives), so it runs
/// on a background thread.
pub fn load() -> Places {
    let places = platform::load();
    if let Ok(mut known) = IN_QUICK_ACCESS.lock() {
        *known = Some(
            places
                .quick_access
                .iter()
                .chain(&places.favorites)
                .filter(|place| place.pinned)
                .filter_map(|place| place.path.as_deref())
                .map(str::to_lowercase)
                .collect(),
        );
    }
    places
}

// The paths pinned to Quick access or added to favorites, as of the last
// read of the places, in lowercase.
static IN_QUICK_ACCESS: std::sync::Mutex<Option<HashSet<String>>> = std::sync::Mutex::new(None);

/// Whether `path` is pinned to Quick access or a favorite, as far as the
/// sidebar knows. The Shell's menu for an item says "Unpin" or "Remove from
/// favorites" only when it is built from the item's Quick access entry, not
/// from its folder (`shell_menu`, issues #11 and #26).
pub(crate) fn in_quick_access(path: &str) -> bool {
    IN_QUICK_ACCESS
        .lock()
        .is_ok_and(|known| known.as_ref().is_some_and(|known| known.contains(&path.to_lowercase())))
}

/// The subfolders of `path`, sorted by name, for expanding a tree node.
#[allow(clippy::disallowed_methods)]
pub fn subfolders(path: &str, show_hidden: bool) -> Vec<Place> {
    let Ok(read) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut folders: Vec<Place> = read
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| show_hidden || !platform::is_hidden(&entry.path()))
        .map(|entry| {
            let path = entry.path().to_string_lossy().into_owned();
            Place {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: Some(path.clone()),
                parsing_name: path,
                pinned: false,
            }
        })
        .collect();
    folders.sort_by_key(|place| place.name.to_lowercase());
    folders
}

/// The computers network discovery finds, as the Network folder lists them.
/// Slow (discovery may take seconds), so it runs off the UI thread.
pub fn network_children() -> Vec<Place> {
    platform::network_children()
}

/// Opens a place that has no folder on disk (Recycle Bin, Network) in the
/// program the desktop says handles folders.
pub fn open_in_explorer(parsing_name: &str) {
    crate::reveal::leave_next_explorer_window();
    let target = if parsing_name.starts_with("::") {
        format!("shell:{parsing_name}")
    } else {
        parsing_name.to_string()
    };
    if let Err(error) = std::process::Command::new("explorer.exe").arg(target).spawn() {
        tracing::warn!("could not open {parsing_name} in Explorer: {error}");
    }
}

/// The tree node that holds the drives, and the address a pane shows it at.
#[cfg(windows)]
pub const THIS_PC: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";

/// The tree node that holds the drives, and the address a pane shows it at.
///
/// On Linux it is not a folder: the root of the machine is a folder like any
/// other, and the places that are not folders on disk are named apart from
/// it, the way the desktop's own file manager names them.
#[cfg(not(windows))]
pub const THIS_PC: &str = "::this-pc";

/// What This PC is called when the Shell's own name has not been read yet.
pub const THIS_PC_NAME: &str = nohrs_core::k!("Este Computador");

/// This PC's contents for the listing: the folders and drives it holds that
/// are folders on disk, each a row that opens here. Portable devices and
/// other virtual items have no folder and are left out. Slow like `load`
/// (the Shell may touch network drives); called from the listing thread.
pub fn this_pc_entries() -> Vec<nohrs_services::fs::listing::FileEntryDto> {
    platform::this_pc()
        .into_iter()
        .filter_map(|place| {
            let path = place.path?;
            Some(nohrs_services::fs::listing::FileEntryDto {
                name: place.name,
                path,
                kind: "dir".to_string(),
                size: 0,
                modified: 0,
                attributes: 0,
            })
        })
        .collect()
}

// How long a read of Quick access is shown before the next render reads it
// again: pinning in Explorer, or a folder becoming frequent, shows up without
// a restart, and the Shell is not asked on every frame.
const REFRESH_AFTER: Duration = Duration::from_secs(15);

/// What the sidebar shows and which tree nodes are open.
pub struct SidebarState {
    pub places: Option<Places>,
    loading: bool,
    loaded_at: Option<Instant>,
    /// Open tree nodes, by parsing name. This PC starts open, as in Explorer.
    pub expanded: HashSet<String>,
    /// Subfolders read for open nodes, by path.
    pub children: HashMap<String, Vec<Place>>,
    /// The sidebar's icons. Its own cache: the listing's is emptied on every
    /// change of folder, and these stay the same.
    pub icons: Thumbnails,
    /// A field open in the sidebar: a network address being added, or a
    /// tag being named.
    pub input: Option<SidebarInput>,
}

/// What a field open in the sidebar is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarInputPurpose {
    /// A network address to add to the places (`\\nas\edicao`, `smb://…`).
    NetworkPlace,
    /// A new name for tag `id`.
    RenameTag(u32),
}

/// A field open in the sidebar.
pub struct SidebarInput {
    pub purpose: SidebarInputPurpose,
    pub input: gpui::Entity<gpui_component::input::InputState>,
    // Drives Enter and Blur while the field is open; dropped with it.
    _subscription: gpui::Subscription,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            places: None,
            loading: false,
            loaded_at: None,
            expanded: HashSet::from([THIS_PC.to_string()]),
            children: HashMap::new(),
            icons: Thumbnails::default(),
            input: None,
        }
    }
}

impl ExplorerPane {
    /// Reads the places when they were never read or are stale.
    pub(crate) fn ensure_places(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.sidebar;
        let fresh = state.loaded_at.is_some_and(|at| at.elapsed() < REFRESH_AFTER);
        if state.loading || fresh {
            return;
        }
        state.loading = true;
        let read = cx.background_spawn(async move { load() });
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let places = read.await;
            this.update(cx, |pane, cx| {
                pane.sidebar.places = Some(places);
                pane.sidebar.loading = false;
                pane.sidebar.loaded_at = Some(Instant::now());
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    /// Reads the places again on the next render, after something that may
    /// have changed them (unpinning from the context menu).
    pub(crate) fn refresh_places(&mut self) {
        self.sidebar.loaded_at = None;
    }

    /// [`refresh_places`](Self::refresh_places) after a Shell command, and
    /// again a moment later. Pinning and "Add to favorites" return before
    /// the Shell has updated Quick access (about 100 ms here): the read made
    /// at once found the old list, and the item showed up only on the next
    /// timed read, 15 s later (issue #25).
    pub(crate) fn refresh_places_after_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_places();
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
            for wait in [400, 1500] {
                cx.background_executor().timer(Duration::from_millis(wait)).await;
                if this
                    .update(cx, |pane, cx| {
                        pane.refresh_places();
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
    }

    /// Opens or closes a tree node, reading its subfolders the first time.
    pub(crate) fn toggle_place(&mut self, place: &Place, cx: &mut Context<Self>) {
        let key = place.parsing_name.clone();
        if !self.sidebar.expanded.insert(key.clone()) {
            self.sidebar.expanded.remove(&key);
            cx.notify();
            return;
        }
        cx.notify();
        let Some(path) = place.path.clone() else {
            return;
        };
        let show_hidden = self.show_hidden;
        let read = cx.background_spawn({
            let path = path.clone();
            async move { subfolders(&path, show_hidden) }
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let folders = read.await;
            this.update(cx, |pane, cx| {
                pane.sidebar.children.insert(path, folders);
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    /// Goes to a place: its folder here, This PC here as a list of its
    /// drives and folders, or Explorer for another virtual one.
    pub(crate) fn open_place(&mut self, place: &Place, window: &mut Window, cx: &mut Context<Self>) {
        match &place.path {
            // A favorite is a file: it opens in its program, as a click on it
            // in the Explorer's Home does. Entering it as a folder left an
            // empty listing with an error.
            Some(path) if std::path::Path::new(path).is_file() => self.open_with_default_app(path),
            Some(path) => self.change_dir(path.clone(), window, cx),
            None if place.parsing_name.eq_ignore_ascii_case(THIS_PC) => {
                self.change_dir(THIS_PC.to_string(), window, cx)
            }
            // Network opens here too: its computers, mapped drives and the
            // places added by hand (`network.rs`). Until 05/11/2026 it opened
            // Explorer, and shares were out of reach.
            None if place.parsing_name.eq_ignore_ascii_case(super::network::NETWORK) => {
                self.change_dir(super::network::NETWORK.to_string(), window, cx)
            }
            // The Recycle Bin opens here too (issue #4).
            None if super::recycle_bin::is_recycle_bin(&place.parsing_name) => {
                self.change_dir(super::recycle_bin::RECYCLE_BIN.to_string(), window, cx)
            }
            // A server or a share found on the network.
            None if super::network::is_unc(&place.parsing_name) => {
                self.change_dir(place.parsing_name.clone(), window, cx)
            }
            None => open_in_explorer(&place.parsing_name),
        }
    }

    /// The address a place opens at in Prime Files (its folder, This PC,
    /// Network, a share, the Recycle Bin), for the middle click that opens it
    /// in a new tab (issue #14). `None` for one only the Explorer opens.
    pub(crate) fn place_address(place: &Place) -> Option<String> {
        if let Some(path) = &place.path {
            // A favorite file has no tab to open.
            return (!std::path::Path::new(path).is_file()).then(|| path.clone());
        }
        let name = place.parsing_name.as_str();
        (name.eq_ignore_ascii_case(THIS_PC)
            || name.eq_ignore_ascii_case(super::network::NETWORK)
            || super::network::is_unc(name)
            || super::recycle_bin::is_recycle_bin(name))
        .then(|| if super::recycle_bin::is_recycle_bin(name) { super::recycle_bin::RECYCLE_BIN.to_string() } else { name.to_string() })
    }

    /// Opens a field in the sidebar for `purpose`, holding `text` selected.
    pub(crate) fn begin_sidebar_input(
        &mut self,
        purpose: SidebarInputPurpose,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui::EntityInputHandler as _;
        use gpui_component::input::{InputEvent, InputState};
        let input = cx.new(|cx| {
            let state = InputState::new(window, cx);
            match purpose {
                SidebarInputPurpose::NetworkPlace => state.placeholder(nohrs_core::t!("\\\\servidor\\pasta ou smb://servidor/pasta")),
                SidebarInputPurpose::RenameTag(_) => state,
            }
        });
        input.update(cx, |state, cx| {
            let length = text.encode_utf16().count();
            state.replace_and_mark_text_in_range(Some(0..0), &text, Some(0..length), window, cx);
            state.unmark_text(window, cx);
            state.focus(window, cx);
        });
        let subscription = cx.subscribe_in(&input, window, |this, _input, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.commit_sidebar_input(window, cx),
            InputEvent::Blur => this.cancel_sidebar_input(window, cx),
            _ => {}
        });
        self.sidebar.input = Some(SidebarInput { purpose, input, _subscription: subscription });
        cx.notify();
    }

    pub(crate) fn cancel_sidebar_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.input.take().is_some() {
            cx.focus_self(window);
            cx.notify();
        }
    }

    // Enter in the sidebar's field: adds the network place and opens it, or
    // renames the tag.
    fn commit_sidebar_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.sidebar.input.take() else {
            return;
        };
        let text = field.input.read(cx).value().to_string();
        cx.focus_self(window);
        match field.purpose {
            SidebarInputPurpose::NetworkPlace => {
                if text.trim().is_empty() {
                    cx.notify();
                    return;
                }
                match super::network::NetworkPlaces::add(&text, cx) {
                    Some(address) => self.change_dir(address, window, cx),
                    None => {
                        self.set_status(
                            super::types::StatusLevel::Error,
                            nohrs_core::tf!("Não é um endereço de rede: {address}", address = text.trim()),
                        );
                    }
                }
            }
            SidebarInputPurpose::RenameTag(id) => super::tags::Tags::rename(id, text, cx),
        }
        cx.notify();
    }

    /// The name of where the pane is, when the address is not a folder's
    /// path: This PC's localized name, Network's, a server's or a tag's.
    pub(crate) fn location_name(&self, cx: &gpui::App) -> Option<String> {
        if self.cwd.eq_ignore_ascii_case(super::network::NETWORK) {
            let shell_name = self
                .sidebar
                .places
                .as_ref()
                .and_then(|places| places.other.iter().find(|place| place.parsing_name.eq_ignore_ascii_case(super::network::NETWORK)))
                .map(|place| place.name.clone());
            return Some(shell_name.unwrap_or_else(|| nohrs_core::i18n::tr(super::network::NETWORK_NAME).to_string()));
        }
        if super::network::is_server(&self.cwd) {
            return super::network::server_of(&self.cwd).map(str::to_string);
        }
        if let Some(id) = super::tags::tag_of_location(&self.cwd) {
            return Some(super::tags::Tags::get(id, cx).map(|tag| tag.name).unwrap_or_else(|| self.cwd.clone()));
        }
        if super::recycle_bin::is_recycle_bin(&self.cwd) {
            let shell_name = self
                .sidebar
                .places
                .as_ref()
                .and_then(|places| places.other.iter().find(|place| super::recycle_bin::is_recycle_bin(&place.parsing_name)))
                .map(|place| place.name.clone());
            return Some(shell_name.unwrap_or_else(|| nohrs_core::i18n::tr(super::recycle_bin::RECYCLE_BIN_NAME).to_string()));
        }
        if !self.cwd.eq_ignore_ascii_case(THIS_PC) {
            return None;
        }
        let shell_name = self.sidebar.places.as_ref().and_then(|places| places.this_pc.as_ref()).map(|place| place.name.clone());
        Some(shell_name.unwrap_or_else(|| nohrs_core::i18n::tr(THIS_PC_NAME).to_string()))
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    //! Shell enumeration has no safe binding.

    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, GetFileAttributesW};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree};
    use windows::Win32::UI::Shell::PropertiesSystem::PSGetPropertyKeyFromName;
    use windows::Win32::UI::Shell::{
        BHID_EnumItems, IEnumShellItems, IShellItem, IShellItem2, SHCreateItemFromParsingName,
        SIGDN, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY,
    };
    use windows::core::{HSTRING, Interface, w};

    use super::{Place, Places};

    const QUICK_ACCESS: &str = "shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}";
    use super::THIS_PC;
    const NETWORK: &str = "::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}";
    const LINUX: &str = "::{B2B4A4D1-2754-4140-A2EB-9A76D9D7CDC6}";

    pub fn load() -> Places {
        if let Err(error) = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok() {
            tracing::debug!("CoInitializeEx: {error}");
        }
        // A pinned file is a favorite: Quick access itself pins only folders.
        let (favorites, quick_access): (Vec<Place>, Vec<Place>) = children(QUICK_ACCESS)
            .into_iter()
            .filter_map(|item| place(&item))
            .partition(|place| {
                place.pinned && place.path.as_deref().is_some_and(|path| std::path::Path::new(path).is_file())
            });
        // This PC also lists the user folders and portable devices; the
        // drives are the ones whose path is a root.
        let drives = children(THIS_PC)
            .into_iter()
            .filter_map(|item| place(&item))
            .filter(|place| place.path.as_deref().is_some_and(|path| path.ends_with(":\\")))
            .collect();
        let mut other = Vec::new();
        if let Some(network) = item(NETWORK).and_then(|item| place(&item)) {
            other.push(network);
        }
        // The navigation pane's Linux node, present only with WSL installed.
        if let Some(linux) = item(LINUX).and_then(|item| place(&item)) {
            other.push(linux);
        }
        Places {
            quick_access,
            favorites,
            this_pc: item(THIS_PC).and_then(|item| place(&item)),
            drives,
            other,
        }
    }

    pub fn this_pc() -> Vec<Place> {
        if let Err(error) = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok() {
            tracing::debug!("CoInitializeEx: {error}");
        }
        children(THIS_PC).into_iter().filter_map(|item| place(&item)).collect()
    }

    pub fn network_children() -> Vec<Place> {
        if let Err(error) = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok() {
            tracing::debug!("CoInitializeEx: {error}");
        }
        children(NETWORK).into_iter().filter_map(|item| place(&item)).collect()
    }

    fn item(parsing_name: &str) -> Option<IShellItem> {
        unsafe { SHCreateItemFromParsingName(&HSTRING::from(parsing_name), None) }.ok()
    }

    fn children(parsing_name: &str) -> Vec<IShellItem> {
        let Some(folder) = item(parsing_name) else {
            return Vec::new();
        };
        let Ok(enumerator) =
            (unsafe { folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) })
        else {
            return Vec::new();
        };
        let mut items = Vec::new();
        loop {
            let mut batch = [None];
            let mut fetched = 0;
            if unsafe { enumerator.Next(&mut batch, Some(&mut fetched)) }.is_err() || fetched == 0 {
                break;
            }
            items.extend(batch.into_iter().flatten());
        }
        items
    }

    fn place(item: &IShellItem) -> Option<Place> {
        Some(Place {
            name: display_name(item, SIGDN_NORMALDISPLAY)?,
            path: display_name(item, SIGDN_FILESYSPATH),
            parsing_name: display_name(item, SIGDN_DESKTOPABSOLUTEPARSING)?,
            pinned: is_pinned(item),
        })
    }

    fn display_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
        let name = unsafe { item.GetDisplayName(kind) }.ok()?;
        let text = unsafe { name.to_string() }.ok();
        unsafe { CoTaskMemFree(Some(name.0 as *const _)) };
        text.filter(|text| !text.is_empty())
    }

    // Quick access marks pinned folders with this property; frequent ones
    // lack it or have it false.
    fn is_pinned(item: &IShellItem) -> bool {
        let Ok(item) = item.cast::<IShellItem2>() else {
            return false;
        };
        let mut key = Default::default();
        if unsafe { PSGetPropertyKeyFromName(w!("System.Home.IsPinned"), &mut key) }.is_err() {
            return false;
        }
        unsafe { item.GetBool(&key) }.is_ok_and(|pinned| pinned.as_bool())
    }

    pub fn is_hidden(path: &std::path::Path) -> bool {
        let attributes = unsafe { GetFileAttributesW(&HSTRING::from(path.as_os_str())) };
        attributes != u32::MAX && attributes & FILE_ATTRIBUTE_HIDDEN.0 != 0
    }
}

#[cfg(not(windows))]
mod platform {
    //! The places of a Linux desktop: the folders the user keeps their things
    //! in, the filesystems mounted on the machine, and the places that are
    //! not folders on disk (the network, the trash).

    use super::{Place, Places};
    use crate::explorer::drives;
    use crate::linux;

    /// The places that are not folders on disk.
    const NETWORK: &str = "::network";
    const TRASH: &str = "::trash";

    pub fn load() -> Places {
        let mut quick_access = Vec::new();
        if let Some(home) = dirs::home_dir() {
            quick_access.push(place(&home.to_string_lossy(), &home));
        }
        // The folders the user named for themselves: Desktop, Documents,
        // Downloads and the rest, as `~/.config/user-dirs.dirs` lists them.
        for (_name, path) in linux::user_dirs() {
            if quick_access.iter().any(|place: &Place| place.path.as_deref() == Some(path.to_string_lossy().as_ref())) {
                continue;
            }
            quick_access.push(place(&path.to_string_lossy(), &path));
        }

        let mut other = Vec::new();
        other.push(Place {
            name: nohrs_core::i18n::tr(crate::explorer::network::NETWORK_NAME).to_string(),
            path: None,
            parsing_name: NETWORK.to_string(),
            pinned: false,
        });
        other.push(Place {
            name: nohrs_core::i18n::tr(crate::explorer::recycle_bin::RECYCLE_BIN_NAME).to_string(),
            path: None,
            parsing_name: TRASH.to_string(),
            pinned: false,
        });

        Places {
            quick_access,
            favorites: bookmarks(),
            this_pc: Some(Place {
                name: nohrs_core::i18n::tr(super::THIS_PC_NAME).to_string(),
                path: None,
                parsing_name: super::THIS_PC.to_string(),
                pinned: false,
            }),
            drives: drive_places(),
            other,
        }
    }

    /// The filesystems mounted on the machine, as the units of the sidebar.
    pub fn drive_places() -> Vec<Place> {
        drives::mounts()
            .into_iter()
            .map(|(mount, device)| {
                // The root of the machine is named after the disk it is on,
                // as the other mounts are named after their own folder.
                let name = mount
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| device.clone());
                Place {
                    name,
                    path: Some(mount.to_string_lossy().into_owned()),
                    parsing_name: mount.to_string_lossy().into_owned(),
                    pinned: false,
                }
            })
            .collect()
    }

    /// The filesystems and the folders of the root, as "This PC" lists them.
    pub fn this_pc() -> Vec<Place> {
        drive_places()
    }

    /// The places of the network: the desktop's own file manager shows them.
    pub fn network_children() -> Vec<Place> {
        Vec::new()
    }

    pub fn is_hidden(path: &std::path::Path) -> bool {
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'))
    }

    /// A place for a folder, named after the folder itself.
    fn place(name: &str, path: &std::path::Path) -> Place {
        Place {
            name: name.to_string(),
            path: Some(path.to_string_lossy().into_owned()),
            parsing_name: path.to_string_lossy().into_owned(),
            pinned: false,
        }
    }

    /// The folders the user bookmarked, as the GTK file manager lists them:
    /// `~/.config/gtk-3.0/bookmarks`, one folder per line, the name after
    /// the path when the folder was given another name. The paths are
    /// written as URIs, with the escapes of the characters they hold.
    fn bookmarks() -> Vec<Place> {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        let path = home.join(".config/gtk-3.0/bookmarks");
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| {
                let (uri, name) = match line.split_once(char::is_whitespace) {
                    Some((uri, name)) => (uri, Some(name.trim())),
                    None => (line.trim(), None),
                };
                // "file:///home/gabriel/M%C3%BAsica" is the folder of the
                // user's music, written the way a URI writes it.
                let folder = uri.strip_prefix("file://").unwrap_or(uri);
                let folder = linux::percent_decoded(folder);
                if folder.is_empty() {
                    return None;
                }
                let name = name
                    .filter(|name| !name.is_empty())
                    .map(linux::percent_decoded)
                    .unwrap_or_else(|| {
                        std::path::Path::new(&folder)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| folder.clone())
                    });
                Some(Place {
                    name,
                    path: Some(folder.clone()),
                    parsing_name: folder,
                    pinned: true,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    // Reads the real machine's places; run by hand with `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_places() {
        let places = super::load();
        for place in places.quick_access.iter().chain(&places.drives).chain(&places.other) {
            println!("{place:?}");
        }
    }
}
