use crate::explorer::ExplorerPane;
use crate::explorer::places::{Place, SidebarInputPurpose, THIS_PC};
use crate::explorer::tags::Tags;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{Icon, IconName, Sizable as _};
use nohrs_core::t;
use nohrs_ui::theme::theme;

const ROW_HEIGHT: f32 = 28.0;
const ICON_SIZE: f32 = 16.0;
/// Width of the expand chevron's column; each tree level indents by it.
const CHEVRON_WIDTH: f32 = 16.0;

/// Renders the sidebar the way Explorer's navigation pane is laid out: Quick
/// access, then This PC as a tree of drives and folders, then Network and
/// Linux. Names and icons come from the Shell, so they match Explorer's.
pub fn render(
    page: &mut ExplorerPane,
    window: &mut Window,
    cx: &mut Context<ExplorerPane>,
) -> AnyElement {
    page.ensure_places(cx);
    let icon_px = ((ICON_SIZE * window.scale_factor()).round() as u32).max(16);
    let places = page.sidebar.places.clone().unwrap_or_default();
    // Docked above or below the listing the sidebar is a wide, short band:
    // a tree would show three rows of it. Each group becomes a line of
    // chips that wraps instead, its name at the start.
    if !super::panels::side(super::panels::Panel::Sidebar).is_column() {
        return render_band(page, &places, icon_px, cx).into_any_element();
    }

    let mut list = div().flex().flex_col().px(px(6.0)).gap(px(1.0));
    for place in &places.quick_access {
        list = list.child(row(page, place, 0, None, icon_px, cx));
    }
    // Favorite files under a heading of their own, apart from the folders
    // (issue #24).
    if !places.favorites.is_empty() {
        list = list.child(
            div()
                .mt(px(10.0))
                .mb(px(2.0))
                .px(px(8.0))
                .text_xs()
                .text_color(rgb(theme::muted()))
                .child(t!("Favoritos")),
        );
        for place in &places.favorites {
            list = list.child(row(page, place, 0, None, icon_px, cx));
        }
    }
    list = list.child(
        div()
            .my(px(8.0))
            .mx(px(6.0))
            .h(px(1.0))
            .bg(rgb(theme::border())),
    );

    let this_pc = places.this_pc.clone().unwrap_or_else(|| Place {
        name: t!("Este Computador").to_string(),
        path: None,
        parsing_name: THIS_PC.to_string(),
        pinned: false,
    });
    let pc_open = page.sidebar.expanded.contains(THIS_PC);
    list = list.child(row(page, &this_pc, 0, Some(pc_open), icon_px, cx));
    if pc_open {
        for drive in &places.drives {
            list = tree(page, list, drive, 1, icon_px, cx);
        }
    }
    for place in &places.other {
        list = list.child(row(page, place, 0, None, icon_px, cx));
        // The network places added by hand go under Network, where a share
        // found by discovery would be.
        if place
            .parsing_name
            .eq_ignore_ascii_case(crate::explorer::network::NETWORK)
        {
            list = network_rows(page, list, cx);
        }
    }
    if !places.other.iter().any(|place| {
        place
            .parsing_name
            .eq_ignore_ascii_case(crate::explorer::network::NETWORK)
    }) {
        list = network_rows(page, list, cx);
    }
    if crate::explorer::tags::in_sidebar() {
        list = tag_rows(page, list, cx);
    }

    // A bar to grab, shown while the pointer is over the sidebar: it only
    // scrolled by the wheel, and Paullo's wheel skips (07/11/2026). Drawn
    // over the rows' right edge, only while the pointer is there.
    div()
        .relative()
        .size_full()
        .when(!page.framed.sidebar, |this| this.bg(rgb(theme::bg())))
        .child(
            div()
                .id("sidebar")
                .size_full()
                .py(px(8.0))
                .overflow_y_scroll()
                .track_scroll(&page.sidebar_scroll)
                .child(list),
        )
        .child(
            gpui_component::scroll::Scrollbar::vertical(&page.sidebar_scroll)
                .id("sidebar-scrollbar")
                .scrollbar_show(gpui_component::scroll::ScrollbarShow::Hover),
        )
        .into_any_element()
}

// What a cell of the band opens.
#[derive(Clone)]
enum BandTarget {
    Place(Place),
    Folder(String),
}

// One cell of the band, already decided: what it shows and what it opens.
struct BandItem {
    id: SharedString,
    icon: AnyElement,
    name: String,
    active: bool,
    target: BandTarget,
    // How full a drive is, drawn under its name.
    space: Option<crate::explorer::drives::Space>,
    // Right click: the place's Windows menu, or a tag's menu.
    menu: Option<BandMenu>,
    // Shown after the name, muted: a tag's item count.
    note: Option<String>,
    // The cell is a field instead: the tag being named.
    editing: Option<Entity<gpui_component::input::InputState>>,
}

#[derive(Clone)]
enum BandMenu {
    Place(Place),
    Tag(u32),
    NetworkPlace(String),
}

const BAND_CELL_WIDTH: f32 = 176.0;
const BAND_ROW: f32 = 30.0;
const BAND_HEADER: f32 = 24.0;
const BAND_PADDING: f32 = 8.0;

// The sidebar docked above or below the listing. A wide, short space: the
// places run in columns, top to bottom and then to the next column, as
// Explorer's List view lays out files, each group under its name and apart
// from the next by a line. The wheel slides the band sideways. A taller band
// simply fits more rows in each column, so nothing wraps into a heap of
// chips.
fn render_band(
    page: &mut ExplorerPane,
    places: &crate::explorer::places::Places,
    icon_px: u32,
    cx: &mut Context<ExplorerPane>,
) -> impl IntoElement + use<> {
    let height = super::panels::sidebar_band_height();
    let rows = (((height - BAND_PADDING * 2.0 - BAND_HEADER) / BAND_ROW).floor() as usize).max(1);

    let place_items = |page: &mut ExplorerPane,
                       members: &[Place],
                       group: &str,
                       cx: &mut Context<ExplorerPane>| {
        members
            .iter()
            .map(|place| band_place(page, place, group, icon_px, cx))
            .collect::<Vec<_>>()
    };

    let pc_name = places
        .this_pc
        .as_ref()
        .map(|place| place.name.clone())
        .unwrap_or_else(|| t!("Este Computador").to_string());
    let mut sections: Vec<(String, &'static str, Vec<BandItem>, Option<AnyElement>)> = Vec::new();
    let quick = place_items(page, &places.quick_access, "quick", cx);
    sections.push((
        t!("Acesso rápido").to_string(),
        "icons/star.svg",
        quick,
        None,
    ));
    let favorites = place_items(page, &places.favorites, "favorite", cx);
    sections.push((
        t!("Favoritos").to_string(),
        "icons/star.svg",
        favorites,
        None,
    ));
    let mut drives = Vec::new();
    if let Some(this_pc) = &places.this_pc {
        drives.push(band_place(page, this_pc, "pc", icon_px, cx));
    }
    drives.extend(place_items(page, &places.drives, "drive", cx));
    sections.push((pc_name, "icons/app-window.svg", drives, None));

    let mut other = place_items(page, &places.other, "other", cx);
    for address in crate::explorer::network::NetworkPlaces::all(cx) {
        let active = crate::explorer::same_path(&address, &page.cwd);
        other.push(BandItem {
            id: SharedString::from(format!("band-network-{address}")),
            icon: Icon::new(Icon::empty())
                .path("icons/server.svg")
                .size_4()
                .text_color(rgb(theme::muted()))
                .into_any_element(),
            name: crate::explorer::network::display_name(&address),
            active,
            target: BandTarget::Folder(address.clone()),
            space: None,
            menu: Some(BandMenu::NetworkPlace(address)),
            note: None,
            editing: None,
        });
    }
    sections.push((t!("Locais").to_string(), "icons/folder.svg", other, None));

    let counts = Tags::counts(cx);
    let tags: Vec<BandItem> = Tags::all(cx)
        .into_iter()
        .map(|tag| {
            let location = crate::explorer::tags::location(tag.id);
            BandItem {
                id: SharedString::from(format!("band-tag-{}", tag.id)),
                icon: div()
                    .size(px(10.0))
                    .mx(px(3.0))
                    .rounded_full()
                    .bg(rgb(tag.color))
                    .into_any_element(),
                name: tag.name,
                active: page.cwd == location,
                target: BandTarget::Folder(location),
                space: None,
                menu: Some(BandMenu::Tag(tag.id)),
                note: counts
                    .get(&tag.id)
                    .filter(|count| **count > 0)
                    .map(|count| count.to_string()),
                editing: page
                    .sidebar
                    .input
                    .as_ref()
                    .filter(|field| field.purpose == SidebarInputPurpose::RenameTag(tag.id))
                    .map(|field| field.input.clone()),
            }
        })
        .collect();
    let new_tag = div()
        .id("band-tag-new")
        .size(px(18.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .text_color(rgb(theme::muted()))
        .hover(|style| {
            style
                .bg(rgb(theme::bg_hover()))
                .text_color(rgb(theme::fg()))
        })
        .tooltip(|window, cx| {
            gpui_component::tooltip::Tooltip::new(t!("Nova tag")).build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            let name = t!("Nova tag").to_string();
            let id = Tags::create(name.clone(), &[], cx);
            this.begin_sidebar_input(SidebarInputPurpose::RenameTag(id), name, window, cx);
        }))
        .drag_over::<ExternalPaths>(|style, _, _, _| {
            style
                .bg(rgb(theme::accent_subtle()))
                .text_color(rgb(theme::accent()))
        })
        .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
            cx.stop_propagation();
            this.drop_on_new_tag(paths.paths(), window, cx);
        }))
        .child(Icon::new(IconName::Plus).size_3())
        .into_any_element();
    if crate::explorer::tags::in_sidebar() {
        sections.push((t!("Tags").to_string(), "icons/tag.svg", tags, Some(new_tag)));
    }

    // Not shrunk to the band: as wide as its columns, so the band has
    // something to scroll (in a block parent it took the band's width and
    // the wheel moved nothing).
    let mut band = div()
        .h_full()
        .flex_shrink_0()
        .flex()
        .items_start()
        .px(px(6.0));
    let mut first = true;
    for (name, icon, items, extra) in sections {
        if items.is_empty() && extra.is_none() {
            continue;
        }
        if !first {
            band = band.child(
                div()
                    .w(px(1.0))
                    .h_full()
                    .mx(px(6.0))
                    .flex_shrink_0()
                    .bg(rgb(theme::border())),
            );
        }
        first = false;
        let mut columns = div().flex().items_start();
        let mut items = items.into_iter().peekable();
        while items.peek().is_some() {
            let column: Vec<AnyElement> = items
                .by_ref()
                .take(rows)
                .map(|item| band_cell(item, cx))
                .collect();
            columns = columns.child(
                div()
                    .w(px(BAND_CELL_WIDTH))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .children(column),
            );
        }
        band = band.child(
            div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(BAND_HEADER))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(theme::muted()))
                        .child(Icon::new(Icon::empty()).path(icon).size_3())
                        .child(div().whitespace_nowrap().child(name))
                        .children(extra),
                )
                .child(columns),
        );
    }
    div()
        .id("sidebar")
        .size_full()
        .when(!page.framed.sidebar, |this| this.bg(rgb(theme::bg())))
        .py(px(BAND_PADDING))
        .flex()
        .overflow_x_scroll()
        .child(band)
}

// A place of the Shell as a cell of the band.
fn band_place(
    page: &mut ExplorerPane,
    place: &Place,
    group: &str,
    icon_px: u32,
    cx: &mut Context<ExplorerPane>,
) -> BandItem {
    let active = place
        .path
        .as_deref()
        .is_some_and(|path| crate::explorer::same_path(path, &page.cwd));
    let icon = match page.sidebar.icons.icon_in(
        &place.parsing_name,
        true,
        icon_px,
        |pane| &mut pane.sidebar.icons,
        cx,
    ) {
        Some(image) => img(ImageSource::Render(image))
            .size(px(ICON_SIZE))
            .flex_shrink_0()
            .into_any_element(),
        None => Icon::new(IconName::Folder)
            .size_4()
            .text_color(rgb(theme::gray_500()))
            .into_any_element(),
    };
    BandItem {
        id: SharedString::from(format!("band-{group}-{}", place.parsing_name)),
        icon,
        name: place.name.clone(),
        active,
        target: BandTarget::Place(place.clone()),
        space: place
            .path
            .as_deref()
            .and_then(|path| crate::explorer::drives::space(path, cx)),
        menu: Some(BandMenu::Place(place.clone())),
        note: None,
        editing: None,
    }
}

fn band_cell(item: BandItem, cx: &mut Context<ExplorerPane>) -> AnyElement {
    if let Some(input) = item.editing {
        return div()
            .h(px(BAND_ROW))
            .mx(px(2.0))
            .flex()
            .items_center()
            .child(gpui_component::input::Input::new(&input).small())
            .into_any_element();
    }
    let opened = item.target.clone();
    let in_tab = item.target.clone();
    let menu = item.menu.clone();
    // A tag takes the files dropped on it.
    let dropped_tag = match &item.menu {
        Some(BandMenu::Tag(tag)) => Some(*tag),
        _ => None,
    };
    let tip = item.name.clone();
    let name = div()
        .min_w(px(0.0))
        .text_sm()
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .text_color(rgb(theme::fg()))
        .child(item.name);
    div()
        .id(item.id)
        .h(px(BAND_ROW))
        .mx(px(2.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .when(item.active, |this| this.bg(rgb(theme::accent_subtle())))
        .when(!item.active, |this| {
            this.hover(|this| this.bg(rgb(theme::bg_hover())))
        })
        .tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
        })
        .on_click(cx.listener(move |this, _, window, cx| match &opened {
            BandTarget::Place(place) => this.open_place(place, window, cx),
            BandTarget::Folder(folder) => this.change_dir(folder.clone(), window, cx),
        }))
        .on_mouse_down(
            MouseButton::Middle,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                match &in_tab {
                    BandTarget::Place(place) => {
                        if let Some(address) = ExplorerPane::place_address(place) {
                            this.open_path_in_new_tab(&address, true, true, cx);
                        }
                    }
                    BandTarget::Folder(folder) => this.open_path_in_new_tab(folder, true, true, cx),
                }
            }),
        )
        .when_some(dropped_tag, |this, tag| {
            this.drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(theme::accent_subtle())))
                .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                    cx.stop_propagation();
                    this.drop_on_tag(tag, paths.paths(), cx);
                }))
        })
        .when_some(menu, |this, menu| {
            this.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    match &menu {
                        BandMenu::Place(place) => this.open_place_menu(place.clone(), window, cx),
                        BandMenu::Tag(tag) => this.open_tag_menu(*tag, window, cx),
                        BandMenu::NetworkPlace(address) => {
                            this.open_network_place_menu(address.clone(), window, cx)
                        }
                    }
                }),
            )
        })
        .child(div().flex_shrink_0().child(item.icon))
        .child(div().flex_1().min_w(px(0.0)).flex().child(name))
        // A drive: how full it is, under its name, as This PC's cards. Placed
        // on the cell rather than in a column with the name: text in a flex
        // column is measured at its narrowest and came out as "…".
        .when_some(item.space, |this, space| {
            this.relative().pb(px(6.0)).child(
                div()
                    .absolute()
                    .left(px(32.0))
                    .right(px(10.0))
                    .bottom(px(3.0))
                    .child(crate::explorer::view::listing::usage_bar(Some(space), 3.0)),
            )
        })
        .when_some(item.note, |this, note| {
            this.child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(rgb(theme::muted()))
                    .child(note),
            )
        })
        .into_any_element()
}

// The places added by hand, then the row (or field) to add another.
fn network_rows(page: &mut ExplorerPane, mut list: Div, cx: &mut Context<ExplorerPane>) -> Div {
    for address in crate::explorer::network::NetworkPlaces::all(cx) {
        let active = crate::explorer::same_path(&address, &page.cwd);
        let opened = address.clone();
        let removed = address.clone();
        list = list.child(
            simple_row(
                SharedString::from(format!("network-place-{address}")),
                Icon::new(Icon::empty())
                    .path("icons/server.svg")
                    .size_4()
                    .text_color(rgb(theme::muted()))
                    .into_any_element(),
                crate::explorer::network::display_name(&address),
                active,
                1,
            )
            .on_click(
                cx.listener(move |this, _, window, cx| this.change_dir(opened.clone(), window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_network_place_menu(removed.clone(), window, cx);
                }),
            ),
        );
    }
    let adding = page
        .sidebar
        .input
        .as_ref()
        .filter(|field| field.purpose == SidebarInputPurpose::NetworkPlace)
        .map(|field| field.input.clone());
    match adding {
        Some(input) => list.child(field_row(input, 1)),
        None => list.child(
            simple_row(
                "network-add",
                Icon::new(IconName::Plus)
                    .size_4()
                    .text_color(rgb(theme::muted()))
                    .into_any_element(),
                t!("Adicionar local de rede").to_string(),
                false,
                1,
            )
            .text_color(rgb(theme::muted()))
            .on_click(cx.listener(|this, _, window, cx| {
                this.begin_sidebar_input(
                    SidebarInputPurpose::NetworkPlace,
                    String::new(),
                    window,
                    cx,
                )
            })),
        ),
    }
}

// The tags, each with its colour and how many items carry it; a click lists
// them. A tag being renamed is a field in its place.
fn tag_rows(page: &mut ExplorerPane, mut list: Div, cx: &mut Context<ExplorerPane>) -> Div {
    let tags = Tags::all(cx);
    let counts = Tags::counts(cx);
    list = list
        .child(
            div()
                .my(px(8.0))
                .mx(px(6.0))
                .h(px(1.0))
                .bg(rgb(theme::border())),
        )
        .child(
            div()
                .id("tags-heading")
                .flex()
                .items_center()
                .justify_between()
                .rounded(px(4.0))
                // Files dropped on the heading (or its +) get a new tag of their
                // own, named at once.
                .drag_over::<ExternalPaths>(|style, _, _, _| {
                    style
                        .bg(rgb(theme::accent_subtle()))
                        .text_color(rgb(theme::accent()))
                })
                .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                    cx.stop_propagation();
                    this.drop_on_new_tag(paths.paths(), window, cx);
                }))
                .h(px(24.0))
                .pl(px(8.0))
                .pr(px(4.0))
                .text_xs()
                .text_color(rgb(theme::muted()))
                .child(t!("Tags"))
                .child(
                    div()
                        .id("tag-new")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(20.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(theme::bg_hover())))
                        .on_click(cx.listener(|this, _, window, cx| {
                            let name = t!("Nova tag").to_string();
                            let id = Tags::create(name.clone(), &[], cx);
                            this.begin_sidebar_input(
                                SidebarInputPurpose::RenameTag(id),
                                name,
                                window,
                                cx,
                            );
                        }))
                        .child(Icon::new(IconName::Plus).size_3()),
                ),
        );
    if tags.is_empty() {
        list = list.child(
            div()
                .px(px(8.0))
                .py(px(4.0))
                .text_xs()
                .text_color(rgb(theme::muted()))
                .child(t!("Arraste arquivos ou pastas para cá para criar uma tag.")),
        );
    }
    for tag in tags {
        let renaming = page
            .sidebar
            .input
            .as_ref()
            .filter(|field| field.purpose == SidebarInputPurpose::RenameTag(tag.id))
            .map(|field| field.input.clone());
        if let Some(input) = renaming {
            list = list.child(field_row(input, 0));
            continue;
        }
        let location = crate::explorer::tags::location(tag.id);
        let active = page.cwd == location;
        let was_active = active;
        let held_name = tag.name.clone();
        let count = counts.get(&tag.id).copied().unwrap_or(0);
        let id = tag.id;
        let tag_color = tag.color;
        let tag_name = tag.name.clone();
        list = list.child(
            simple_row(
                SharedString::from(format!("tag-{}", tag.id)),
                div()
                    .size(px(10.0))
                    .mx(px(3.0))
                    .rounded_full()
                    .bg(rgb(tag.color))
                    .into_any_element(),
                tag.name.clone(),
                active,
                0,
            )
            .when(count > 0, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(rgb(theme::muted()))
                        .child(count.to_string()),
                )
            })
            // Three ways to name it, as users asked (Senkozinho and the owner,
            // 07/11/2026; the four tags that come with the app were only
            // renamed from their menu): holding the button down on it, three
            // clicks, or a second, slow click on the tag already open, as on
            // a file in the listing.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.arm_tag_hold(id, held_name.clone(), window, cx);
                }),
            )
            // Let go off the tag before the hold was long enough: no rename.
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _window, _cx| this.slow_rename = None),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() >= 3 {
                    this.slow_rename = None;
                    this.begin_sidebar_input(
                        SidebarInputPurpose::RenameTag(id),
                        tag_name.clone(),
                        window,
                        cx,
                    );
                    return;
                }
                this.arm_tag_rename(id, tag_name.clone(), was_active, event, window, cx);
                this.change_dir(location.clone(), window, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_tag_menu(id, window, cx);
                }),
            )
            // Files and folders dropped on a tag get it, as on Finder's and
            // the Files app's sidebar.
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style
                    .bg(rgb(theme::accent_subtle()))
                    .border_1()
                    .border_color(rgb(tag_color))
            })
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                cx.stop_propagation();
                this.drop_on_tag(id, paths.paths(), cx);
            })),
        );
    }
    list
}

impl ExplorerPane {
    // The slow click on a tag: one plain click on the tag already open, not
    // followed by a second, opens its name field once the double-click time
    // has passed. Any other click, or leaving the tag, calls it off.
    fn arm_tag_rename(
        &mut self,
        id: u32,
        name: String,
        was_active: bool,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.slow_rename = None;
        let plain =
            !(event.modifiers().shift || event.modifiers().control || event.modifiers().platform);
        if !was_active || event.click_count() != 1 || !plain || self.sidebar.input.is_some() {
            return;
        }
        let location = crate::explorer::tags::location(id);
        let delay = crate::explorer::file_ops::double_click_time();
        self.slow_rename = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update_in(cx, |pane, window, cx| {
                pane.slow_rename = None;
                if pane.cwd == location && pane.sidebar.input.is_none() {
                    pane.begin_sidebar_input(SidebarInputPurpose::RenameTag(id), name, window, cx);
                }
            })
            .ok();
        }));
    }

    // Holding the button down on a tag: its name field opens once the press
    // has lasted, unless the release (the click, which replaces this task)
    // comes first.
    fn arm_tag_hold(&mut self, id: u32, name: String, window: &mut Window, cx: &mut Context<Self>) {
        const HOLD: std::time::Duration = std::time::Duration::from_millis(600);
        self.slow_rename = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(HOLD).await;
            this.update_in(cx, |pane, window, cx| {
                pane.slow_rename = None;
                if pane.sidebar.input.is_none() {
                    pane.begin_sidebar_input(SidebarInputPurpose::RenameTag(id), name, window, cx);
                }
            })
            .ok();
        }));
    }

    /// Puts tag `id` on what was dropped on it, saying so in the footer.
    fn drop_on_tag(&mut self, id: u32, paths: &[std::path::PathBuf], cx: &mut Context<Self>) {
        let paths: Vec<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        if paths.is_empty() {
            return;
        }
        let added = Tags::add(&paths, id, cx);
        let name = Tags::get(id, cx).map(|tag| tag.name).unwrap_or_default();
        let text = if added == 0 {
            nohrs_core::tf!("Já tinham a tag {tag}", tag = name)
        } else {
            nohrs_core::i18n::plural(
                added,
                nohrs_core::k!("{n} item marcado com {tag}"),
                nohrs_core::k!("{n} itens marcados com {tag}"),
            )
            .replace("{tag}", &name)
        };
        self.set_status(crate::explorer::types::StatusLevel::Info, text);
        cx.notify();
    }

    /// A new tag on what was dropped, its name field open.
    fn drop_on_new_tag(
        &mut self,
        paths: &[std::path::PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths: Vec<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        if paths.is_empty() {
            return;
        }
        let name = t!("Nova tag").to_string();
        let id = Tags::create(name.clone(), &paths, cx);
        self.begin_sidebar_input(SidebarInputPurpose::RenameTag(id), name, window, cx);
    }
}

// A sidebar row with no tree around it: an icon and a name.
fn simple_row(
    id: impl Into<ElementId>,
    icon: AnyElement,
    name: String,
    active: bool,
    depth: usize,
) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .gap(px(6.0))
        .pl(px(2.0 + CHEVRON_WIDTH + depth as f32 * CHEVRON_WIDTH))
        .pr(px(8.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .when(active, |this| this.bg(rgb(theme::accent_subtle())))
        .when(!active, |this| {
            this.hover(|this| this.bg(rgb(theme::bg_hover())))
        })
        .child(div().flex_shrink_0().child(icon))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_sm()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_color(rgb(theme::fg()))
                .child(name),
        )
}

// A field in the sidebar, as tall as a row.
fn field_row(input: Entity<gpui_component::input::InputState>, depth: usize) -> impl IntoElement {
    div()
        .w_full()
        .pl(px(2.0 + depth as f32 * CHEVRON_WIDTH))
        .pr(px(4.0))
        .py(px(2.0))
        .child(gpui_component::input::Input::new(&input).small())
}

// A node and, when open, everything under it.
fn tree(
    page: &mut ExplorerPane,
    mut list: Div,
    place: &Place,
    depth: usize,
    icon_px: u32,
    cx: &mut Context<ExplorerPane>,
) -> Div {
    let open = page.sidebar.expanded.contains(&place.parsing_name);
    list = list.child(row(page, place, depth, Some(open), icon_px, cx));
    if open
        && let Some(path) = &place.path
        && let Some(children) = page.sidebar.children.get(path).cloned()
    {
        for child in &children {
            list = tree(page, list, child, depth + 1, icon_px, cx);
        }
    }
    list
}

// One row. `expandable` is `Some(open)` for a tree node, `None` for a leaf
// (Quick access entries are links, as in Explorer).
fn row(
    page: &mut ExplorerPane,
    place: &Place,
    depth: usize,
    expandable: Option<bool>,
    icon_px: u32,
    cx: &mut Context<ExplorerPane>,
) -> impl IntoElement + use<> {
    let active = place
        .path
        .as_deref()
        .is_some_and(|path| crate::explorer::same_path(path, &page.cwd));
    let icon = page.sidebar.icons.icon_in(
        &place.parsing_name,
        // Places are folders, drives and shell locations, each with its own.
        true,
        icon_px,
        |pane| &mut pane.sidebar.icons,
        cx,
    );
    let icon = match icon {
        Some(image) => img(ImageSource::Render(image))
            .size(px(ICON_SIZE))
            .flex_shrink_0()
            .into_any_element(),
        None => Icon::new(IconName::Folder)
            .size_4()
            .text_color(rgb(theme::gray_500()))
            .into_any_element(),
    };
    let chevron = match expandable {
        Some(open) => {
            let toggled = place.clone();
            div()
                .id(SharedString::from(format!("toggle-{}", place.parsing_name)))
                .w(px(CHEVRON_WIDTH))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_place(&toggled, cx);
                    }),
                )
                .child(
                    Icon::new(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size_3()
                    .text_color(rgb(theme::gray_500())),
                )
                .into_any_element()
        }
        None => div()
            .w(px(CHEVRON_WIDTH))
            .flex_shrink_0()
            .into_any_element(),
    };
    let opened = place.clone();
    let menu = place.clone();
    let drive_space = place
        .path
        .as_deref()
        .and_then(|path| crate::explorer::drives::space(path, cx));
    div()
        .id(SharedString::from(format!(
            "place-{depth}-{}",
            place.parsing_name
        )))
        .w_full()
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .gap(px(6.0))
        .pl(px(2.0 + depth as f32 * CHEVRON_WIDTH))
        .pr(px(8.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .when(active, |this| this.bg(rgb(theme::accent_subtle())))
        .when(!active, |this| {
            this.hover(|this| this.bg(rgb(theme::bg_hover())))
        })
        .on_click(cx.listener(move |this, _, window, cx| this.open_place(&opened, window, cx)))
        .when_some(ExplorerPane::place_address(&place), |this, path| {
            this.on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.open_path_in_new_tab(&path, true, true, cx);
                }),
            )
        })
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.open_place_menu(menu.clone(), window, cx);
            }),
        )
        .child(chevron)
        .child(icon)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_sm()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_color(rgb(theme::fg()))
                .child(place.name.clone()),
        )
        .when_some(drive_space, |this, space| {
            // How full the drive is, beside its name: a glance at C: and D:
            // without opening This PC.
            this.child(
                div()
                    .id(SharedString::from(format!(
                        "drive-space-{}",
                        place.parsing_name
                    )))
                    .w(px(34.0))
                    .flex_shrink_0()
                    .child(crate::explorer::view::listing::usage_bar(Some(space), 4.0))
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(space.describe()).build(window, cx)
                    }),
            )
        })
        .when(place.pinned, |this| {
            this.child(
                Icon::new(Icon::empty())
                    .path("icons/pin.svg")
                    .size_3()
                    .flex_shrink_0()
                    .text_color(rgb(theme::gray_500())),
            )
        })
}
