mod audio_session;
mod batch_rename;
pub(crate) mod commands;
pub(crate) mod context_menu;
mod creative;
mod documents;
mod drag;
pub(crate) mod drives;
mod entries;
pub(crate) mod everything;
pub(crate) mod file_ops;
mod filter;
mod folder_views;
pub(crate) mod folder_watch;
mod grouping;
pub(crate) mod hover_audio;
mod list_setup;
mod marquee;
/// The split-view container that owns one or more panes (`docs/explorer-essentials.md` §3).
mod media;
mod model3d;
mod navigation;
pub(crate) mod network;
mod page;
pub(crate) mod palette;
mod path_edit;
mod pdf;
mod picture;
pub(crate) mod places;
mod preview;
pub(crate) mod preview_listing;
mod quick_search;
pub(crate) mod recycle_bin;
mod search;
mod smooth_scroll;
mod state;
pub(crate) mod tags;
pub(crate) mod thumbnails;
mod tree;
mod types;
pub(crate) mod video_proxy;
/// Rendering of a single explorer pane: header, sidebar, listing, and preview.
pub mod view;
pub(crate) mod waveforms;
mod zoom;

#[cfg(test)]
mod tests;

pub(crate) use context_menu::note_theme_mode;
pub use page::{ExplorerPage, rebind_shortcuts};
pub use state::ExplorerPane;
// Part of `status_for_footer`'s signature, so it has to travel with it. The rest
// of `types` stays internal.
pub(crate) use navigation::open_with_default;
pub use types::StatusLevel;
/// The footer zoom menu's views.
pub use zoom::PRESETS as ZOOM_PRESETS;
/// The name the footer shows for a listing zoom value.
pub use zoom::label as zoom_label;

use gpui::{Context, IntoElement, Render, Window};

/// Whether two paths name the same place. Windows paths are compared without
/// case and with either separator, since a path handed over by another program
/// rarely matches the listing's spelling byte for byte.
pub(crate) fn same_path(left: &str, right: &str) -> bool {
    let normalize = |path: &str| {
        path.trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_lowercase()
    };
    normalize(left) == normalize(right)
}

impl Render for ExplorerPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        view::render(self, window, cx)
    }
}
