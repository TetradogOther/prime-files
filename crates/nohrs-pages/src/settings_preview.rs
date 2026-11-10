//! Live previews on the settings page: a small Prime Files that follows the
//! layout and compact-mode choices as they are made, a file to right-click
//! that opens the menu as it is arranged now, and a few sounds to hover to
//! feel the wait before one plays.
//!
//! Everything here is drawn from the config the page shows, so a choice made
//! below a preview shows in it on the next frame. Nothing is real: the
//! preview's panels hold grey lines, the menu's Windows entries are a fixed
//! sample, and the sounds are made up here. The welcome screen shows the same
//! layout, compact and sound previews, so those take the view that owns
//! their state instead of being tied to the settings page.

use std::time::Instant;

use gpui::prelude::*;
use gpui::{
    AnchoredPositionMode, AnyElement, Bounds, Context, Div, ElementId, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, Point, SharedString, Stateful, Window, anchored, canvas, deferred, div,
    point, px, rgb,
};
use gpui_component::Icon;
use nohrs_core::config::{Compact, DockSide};
use nohrs_core::{t, tf};
use nohrs_ui::theme::theme;

use super::SettingsPage;

// The preview window's parts, in its own small scale.
const TABS_H: f32 = 22.0;
const HEADER_H: f32 = 20.0;
const BOTTOM_H: f32 = 16.0;
const SIDEBAR_W: f32 = 100.0;
const PREVIEW_W: f32 = 110.0;
const TERMINAL_W: f32 = 120.0;
const DOCK_H: f32 = 54.0;
const VTABS_W: f32 = 26.0;
pub(crate) const FOLDER: u32 = 0xF5B83D;
const TINY: f32 = 9.0;

/// One edge of the compact preview, with the bars that hide behind it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edge {
    Top,
    Left,
    Right,
    Bottom,
}

const EDGES: [Edge; 4] = [Edge::Top, Edge::Left, Edge::Right, Edge::Bottom];

#[derive(Default, Clone, Copy)]
struct EdgeState {
    // The pointer is at the edge, or over its bars while they are out.
    hovered: bool,
    // When `hovered` last changed: the waits count from it.
    since: Option<Instant>,
    // Whether the bars are coming out (or out), once the wait has passed.
    out: bool,
    // How far out they are, 0..=1.
    progress: f32,
}

/// The compact preview's bars, moved by the pointer the way the real ones are.
/// Owned by the page that shows it (Settings, the welcome screen).
#[derive(Default)]
pub(crate) struct CompactDemo {
    edges: [EdgeState; 4],
    // Where the preview window was drawn, to tell which edge the pointer is at.
    bounds: Bounds<Pixels>,
    last_tick: Option<Instant>,
}

/// The sounds to hover: which one the pointer is on and since when, and
/// which one was sent to the player.
#[derive(Default)]
pub(crate) struct HoverDemo {
    hovered: Option<(usize, Instant)>,
    played: Option<usize>,
}

/// The menu test: where the menu is open, and what the last choice did.
#[derive(Default)]
pub(super) struct MenuTest {
    pub(super) open_at: Option<Point<Pixels>>,
    note: Option<SharedString>,
}

impl SettingsPage {
    // ---------------------------------------------------------------------
    // Layout

    /// A small window laid out as `[layout]` says: where each panel docks,
    /// the tabs on top or down the side, the tags in the sidebar, the grips.
    pub(super) fn layout_preview(&self) -> AnyElement {
        layout_window(&self.config.layout, 360.0).into_any_element()
    }

    /// Moves the compact preview's bars one frame: waits, then slides.
    pub(super) fn tick_compact_demo(&mut self, window: &mut Window) {
        self.compact_demo.tick(&self.config.compact, window);
    }

    pub(super) fn compact_preview(&self, cx: &mut Context<Self>) -> AnyElement {
        self.compact_demo
            .render(
                &self.config.compact,
                280.0,
                |page: &mut Self| &mut page.compact_demo,
                |page| &page.config.compact,
                cx,
            )
            .into_any_element()
    }

    /// Plays the hovered sample once its wait has passed.
    pub(super) fn tick_hover_demo(&mut self) {
        self.hover_demo.tick(self.config.ui.hover_audio_delay_ms);
    }

    /// Sounds to pass the pointer over, as many as fit the card's width.
    pub(super) fn hover_audio_preview(&self, cx: &mut Context<Self>) -> AnyElement {
        // The row's control spans the card inside its 16 px sides.
        let count = HoverDemo::fitting(self.column_width - 34.0);
        self.hover_demo
            .render(
                self.config.ui.hover_audio_delay_ms,
                count,
                |page: &mut Self| &mut page.hover_demo,
                cx,
            )
            .into_any_element()
    }
}

/// A small window laid out as `[layout]` says: where each panel docks, the
/// tabs on top or down the side, the tags in the sidebar, the grips.
pub(crate) fn layout_window(layout: &nohrs_core::config::Layout, height: f32) -> Div {
    let panels = [
        (Panel::Sidebar, layout.sidebar),
        (Panel::Preview, layout.preview),
        (Panel::Terminal, layout.terminal),
    ];
    let at = |side: DockSide| {
        panels
            .iter()
            .filter(move |(_, s)| *s == side)
            .map(|(panel, _)| *panel)
    };
    let grips = layout.move_grips;
    let tags = layout.sidebar_tags;
    let docked = |panel: Panel, side: DockSide| docked_panel(panel, side, grips, tags);
    let center = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .children(at(DockSide::Top).map(|panel| docked(panel, DockSide::Top)))
        .child(listing())
        .children(at(DockSide::Bottom).map(|panel| docked(panel, DockSide::Bottom)));
    let body = div()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .when(layout.vertical_tabs, |this| this.child(vertical_tabs()))
        .children(at(DockSide::Left).map(|panel| docked(panel, DockSide::Left)))
        .child(center)
        .children(at(DockSide::Right).map(|panel| docked(panel, DockSide::Right)));
    mini_window(height)
        .when(!layout.vertical_tabs, |this| this.child(tab_strip()))
        .child(header())
        .child(body)
        .child(bottom_bar())
}

fn docked_panel(panel: Panel, side: DockSide, grips: bool, tags: bool) -> Div {
    let across = matches!(side, DockSide::Top | DockSide::Bottom);
    let width = match panel {
        Panel::Sidebar => SIDEBAR_W,
        Panel::Preview => PREVIEW_W,
        Panel::Terminal => TERMINAL_W,
    };
    let content = match panel {
        Panel::Sidebar => sidebar(tags),
        Panel::Preview => preview_panel(),
        Panel::Terminal => terminal(),
    };
    // The grip sits on the edge that faces the listing.
    let grip = grips.then(|| {
        let pill = div().absolute().rounded_full().bg(rgb(theme::accent()));
        match side {
            DockSide::Left => pill
                .right(px(-2.0))
                .top(relative_center())
                .w(px(3.0))
                .h(px(18.0)),
            DockSide::Right => pill
                .left(px(-2.0))
                .top(relative_center())
                .w(px(3.0))
                .h(px(18.0)),
            DockSide::Top => pill
                .bottom(px(-2.0))
                .left(relative_center())
                .h(px(3.0))
                .w(px(18.0)),
            DockSide::Bottom => pill
                .top(px(-2.0))
                .left(relative_center())
                .h(px(3.0))
                .w(px(18.0)),
        }
    });
    div()
        .relative()
        .flex_shrink_0()
        .map(|this| {
            if across {
                this.w_full().h(px(DOCK_H))
            } else {
                this.h_full().w(px(width))
            }
        })
        .map(|this| match side {
            DockSide::Left => this.border_r_1(),
            DockSide::Right => this.border_l_1(),
            DockSide::Top => this.border_b_1(),
            DockSide::Bottom => this.border_t_1(),
        })
        .border_color(rgb(theme::border()))
        .child(content.size_full().overflow_hidden())
        .children(grip)
}

impl CompactDemo {
    /// Moves the bars one frame: waits, then slides. Called on every render
    /// of the page that shows the preview.
    pub(crate) fn tick(&mut self, compact: &Compact, window: &mut Window) {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map_or(0.0, |last| (now - last).as_secs_f32().min(0.1));
        let mut busy = false;
        for (ix, edge) in EDGES.into_iter().enumerate() {
            let state = &mut self.edges[ix];
            if !hides(compact, edge) {
                *state = EdgeState::default();
                continue;
            }
            let waited = state
                .since
                .map_or(u128::MAX, |since| (now - since).as_millis());
            if state.hovered && !state.out {
                if waited >= compact.reveal_delay_ms as u128 {
                    state.out = true;
                } else {
                    busy = true;
                }
            }
            if !state.hovered && state.out {
                if waited >= compact.hide_delay_ms as u128 {
                    state.out = false;
                } else {
                    busy = true;
                }
            }
            let target = if state.out { 1.0 } else { 0.0 };
            if compact.animation_ms == 0 {
                state.progress = target;
            } else {
                let step = dt * 1000.0 / compact.animation_ms as f32;
                state.progress = if state.progress < target {
                    (state.progress + step).min(target)
                } else {
                    (state.progress - step).max(target)
                };
            }
            if state.progress != target {
                busy = true;
            }
        }
        if busy {
            self.last_tick = Some(now);
            window.request_animation_frame();
        } else {
            self.last_tick = None;
        }
    }

    // Which edges the pointer is at, from its place over the preview (`None`
    // once it has left it). True when one of them changed.
    fn pointer(&mut self, compact: &Compact, position: Option<Point<Pixels>>) -> bool {
        let bounds = self.bounds;
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let zone = (compact.edge_px as f32 * 0.75).max(5.0);
        let top_depth = top_hidden(compact) + top_fixed(compact);
        let local = position.map(|p| {
            (
                f32::from(p.x - bounds.origin.x),
                f32::from(p.y - bounds.origin.y),
            )
        });
        let now = Instant::now();
        let mut changed = false;
        for (ix, edge) in EDGES.into_iter().enumerate() {
            let state = &mut self.edges[ix];
            let out = state.progress > 0.01;
            let hovered = hides(compact, edge)
                && local.is_some_and(|(x, y)| {
                    (0.0..=w).contains(&x)
                        && (0.0..=h).contains(&y)
                        && match edge {
                            Edge::Top => y < zone || (out && y < top_depth),
                            Edge::Left => x < zone || (out && x < SIDEBAR_W),
                            Edge::Right => x > w - zone || (out && x > w - PREVIEW_W),
                            Edge::Bottom => y > h - zone || (out && y > h - BOTTOM_H),
                        }
                });
            if hovered != state.hovered {
                state.hovered = hovered;
                state.since = Some(now);
                changed = true;
            }
        }
        changed
    }

    /// A small window whose chosen bars hide, and come out when the pointer
    /// rests at their edge: with the wait, the slide, over or pushing, and
    /// the frame that `compact` says. `demo` and `config` find this demo and
    /// the compact settings again inside the view `V` that owns them.
    pub(crate) fn render<V: 'static>(
        &self,
        compact: &Compact,
        height: f32,
        demo: fn(&mut V) -> &mut CompactDemo,
        config: fn(&V) -> &Compact,
        cx: &mut Context<V>,
    ) -> Stateful<Div> {
        let progress = |edge: Edge| {
            if hides(compact, edge) {
                self.edges[EDGES.iter().position(|e| *e == edge).unwrap_or(0)].progress
            } else {
                1.0
            }
        };
        let overlay = compact.overlay;
        let (pt, pl, pr, pb) = (
            progress(Edge::Top),
            progress(Edge::Left),
            progress(Edge::Right),
            progress(Edge::Bottom),
        );
        let top_hidden_h = top_hidden(compact);
        let frame = compact.frame_px as f32 * 0.6;

        // Bars that stay: in the flow, always.
        let fixed_top = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .when(!compact.hide_tabs, |this| this.child(tab_strip()))
            .when(!compact.hide_header, |this| this.child(header()));
        let hidden_top = || {
            div()
                .flex()
                .flex_col()
                .when(compact.hide_tabs, |this| this.child(tab_strip()))
                .when(compact.hide_header, |this| this.child(header()))
        };

        // Pushing: each hidden bar takes its share of room as it comes out,
        // anchored to its edge so it slides in from there.
        let pushed = |inner: Div, edge: Edge, size: f32, p: f32| -> Option<Div> {
            if overlay {
                return None;
            }
            let room = size * p;
            Some(
                div()
                    .flex_shrink_0()
                    .overflow_hidden()
                    .flex()
                    .map(|this| match edge {
                        Edge::Top => this.w_full().h(px(room)).flex_col().justify_end(),
                        Edge::Bottom => this.w_full().h(px(room)).flex_col().justify_start(),
                        Edge::Left => this.h_full().w(px(room)).justify_end(),
                        Edge::Right => this.h_full().w(px(room)).justify_start(),
                    })
                    .child(inner.flex_shrink_0().map(|this| match edge {
                        Edge::Top | Edge::Bottom => this.w_full().h(px(size)),
                        Edge::Left | Edge::Right => this.h_full().w(px(size)),
                    })),
            )
        };
        let side = |inner: Div, width: f32, left: bool| {
            inner
                .bg(rgb(theme::toolbar_bg()))
                .map(|this| {
                    if left {
                        this.border_r_1()
                    } else {
                        this.border_l_1()
                    }
                })
                .border_color(rgb(theme::border()))
                .w(px(width))
        };

        let listing_card = div()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .when(frame > 0.0, |this| this.p(px(frame)))
            .child(listing().size_full().when(frame > 0.0, |this| {
                this.rounded(px(7.0))
                    .border_1()
                    .border_color(rgb(theme::border()))
            }));

        let mut body = div()
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .bg(rgb(theme::toolbar_bg()));
        if !compact.hide_sidebar {
            body = body.child(side(sidebar(false), SIDEBAR_W, true));
        } else if let Some(room) = pushed(
            side(sidebar(false), SIDEBAR_W, true),
            Edge::Left,
            SIDEBAR_W,
            pl,
        ) {
            body = body.child(room);
        }
        body = body.child(listing_card);
        if !compact.hide_preview {
            body = body.child(side(preview_panel(), PREVIEW_W, false));
        } else if let Some(room) = pushed(
            side(preview_panel(), PREVIEW_W, false),
            Edge::Right,
            PREVIEW_W,
            pr,
        ) {
            body = body.child(room);
        }
        // Over the listing: each hidden bar slides from its edge, clipped to
        // the strip it ends up in.
        if overlay {
            if compact.hide_sidebar {
                body = body.child(slide(
                    side(sidebar(false), SIDEBAR_W, true).shadow_lg(),
                    Edge::Left,
                    SIDEBAR_W,
                    pl,
                ));
            }
            if compact.hide_preview {
                body = body.child(slide(
                    side(preview_panel(), PREVIEW_W, false).shadow_lg(),
                    Edge::Right,
                    PREVIEW_W,
                    pr,
                ));
            }
        }

        let entity = cx.entity().downgrade();
        let mut window = mini_window(height)
            .id("compact-preview")
            .on_mouse_move(
                cx.listener(move |view: &mut V, event: &MouseMoveEvent, _window, cx| {
                    let compact = config(view).clone();
                    if demo(view).pointer(&compact, Some(event.position)) {
                        cx.notify();
                    }
                }),
            )
            .on_hover(
                cx.listener(move |view: &mut V, hovered: &bool, _window, cx| {
                    if !*hovered {
                        let compact = config(view).clone();
                        if demo(view).pointer(&compact, None) {
                            cx.notify();
                        }
                    }
                }),
            )
            .child(fixed_top);
        if top_hidden_h > 0.0 && !overlay {
            window = window.children(pushed(hidden_top(), Edge::Top, top_hidden_h, pt));
        }
        window = window.child(body);
        if !compact.hide_bottom_bar {
            window = window.child(bottom_bar());
        } else if !overlay {
            window = window.children(pushed(bottom_bar(), Edge::Bottom, BOTTOM_H, pb));
        }
        if overlay && top_hidden_h > 0.0 {
            window = window.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top_fixed(compact)))
                    .h(px(top_hidden_h))
                    .overflow_hidden()
                    .child(
                        hidden_top()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(-(1.0 - pt) * top_hidden_h))
                            .shadow_lg(),
                    ),
            );
        }
        if overlay && compact.hide_bottom_bar {
            window = window.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(BOTTOM_H))
                    .overflow_hidden()
                    .child(
                        bottom_bar()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px((1.0 - pb) * BOTTOM_H)),
                    ),
            );
        }
        // Where it was drawn, for `pointer`.
        window.child(
            canvas(
                move |bounds, _window, cx| {
                    entity
                        .update(cx, |view, _cx| demo(view).bounds = bounds)
                        .ok();
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
    }
}

impl SettingsPage {
    // ---------------------------------------------------------------------
    // Right-click menu

    /// A file to right-click, which opens the file menu as `[menu]` has it.
    pub(super) fn menu_tester(&self, cx: &mut Context<Self>) -> AnyElement {
        let file = div()
            .id("menu-test-file")
            // A tile as the Icons view draws a file, not a row.
            .size(px(104.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .rounded(px(10.0))
            .border_1()
            .border_color(rgb(if self.menu_test.open_at.is_some() {
                theme::accent()
            } else {
                theme::border()
            }))
            .bg(rgb(if self.menu_test.open_at.is_some() {
                theme::accent_subtle()
            } else {
                theme::bg()
            }))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|page, event: &MouseDownEvent, _window, cx| {
                    page.menu_test.open_at = Some(event.position);
                    page.menu_test.note = None;
                    cx.notify();
                }),
            )
            .child(
                Icon::new(Icon::empty())
                    .path("icons/file-text.svg")
                    .size_8()
                    .text_color(rgb(theme::fg_secondary())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::fg()))
                            .child("Relatório.pdf"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::muted()))
                            .child("2,4 MB"),
                    ),
            );
        // Beside the row's text, on the right; what the last choice did sits
        // to the tile's left.
        div()
            .flex()
            .items_center()
            .gap(px(14.0))
            .children(self.menu_test.note.clone().map(|note| {
                div()
                    .max_w(px(240.0))
                    .text_xs()
                    .text_right()
                    .text_color(rgb(theme::fg_secondary()))
                    .child(note)
            }))
            .child(file)
            .children(self.menu_test.open_at.map(|at| self.test_menu(at, cx)))
            .into_any_element()
    }

    // The menu itself, drawn over the window at `at` like the real one.
    fn test_menu(&self, at: Point<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        let menu = &self.config.menu;
        let mut rows: Vec<TestEntry> = Vec::new();
        let quick: Vec<(&'static str, &'static str)> = if menu.classic {
            Vec::new()
        } else {
            menu.quick
                .iter()
                .filter_map(|id| crate::explorer::context_menu::menu_entry_info(id))
                .collect()
        };
        if menu.classic {
            rows.push(TestEntry::Note(t!(
                "Com o menu clássico ligado, o botão direito abre o menu antigo do Windows no lugar deste."
            )));
        } else {
            let mut before = Vec::new();
            let mut after = Vec::new();
            let mut windows = false;
            for id in &menu.items {
                if id == "windows" {
                    windows = true;
                    continue;
                }
                let Some((label, icon)) = crate::explorer::context_menu::menu_entry_info(id) else {
                    continue;
                };
                let shortcut = match id.as_str() {
                    "new_tab" => Some(t!("Botão do meio")),
                    "more_options" => Some("Shift+F10"),
                    _ => None,
                };
                let entry = TestEntry::Item {
                    label,
                    icon: Some(icon),
                    submenu: id == "tags",
                    shortcut,
                    windows: false,
                };
                if windows {
                    after.push(entry)
                } else {
                    before.push(entry)
                }
            }
            let sample: Vec<TestEntry> = if windows {
                [
                    (t!("Abrir"), false),
                    (t!("Abrir com"), true),
                    (t!("Compartilhar"), false),
                    (t!("Enviar para"), true),
                    (t!("Compactar para arquivo ZIP"), false),
                ]
                .into_iter()
                .filter(|(label, _)| !menu.hidden.iter().any(|hidden| hidden == label))
                .map(|(label, submenu)| TestEntry::Item {
                    label,
                    icon: None,
                    submenu,
                    shortcut: None,
                    windows: true,
                })
                .collect()
            } else {
                Vec::new()
            };
            let parts = [before, sample, after];
            for part in parts.into_iter().filter(|part| !part.is_empty()) {
                if !rows.is_empty() {
                    rows.push(TestEntry::Separator);
                }
                rows.extend(part);
            }
        }

        let width = 270.0;
        let height = 8.0
            + if quick.is_empty() { 0.0 } else { 46.0 }
            + rows
                .iter()
                .map(|row| {
                    if matches!(row, TestEntry::Separator) {
                        9.0
                    } else {
                        30.0
                    }
                })
                .sum::<f32>();
        let (vw, vh) = (
            f32::from(self.viewport.width),
            f32::from(self.viewport.height),
        );
        let x = f32::from(at.x).min(vw - width - 8.0).max(8.0);
        let mut y = f32::from(at.y);
        if y + height > vh - 8.0 {
            y = (y - height).max(8.0);
        }

        let close = |page: &mut SettingsPage,
                     note: Option<SharedString>,
                     cx: &mut Context<SettingsPage>| {
            page.menu_test.open_at = None;
            page.menu_test.note = note;
            cx.notify();
        };
        let quick_row = (!quick.is_empty()).then(|| {
            div()
                .h(px(46.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(4.0))
                .px(px(2.0))
                .border_b_1()
                .border_color(rgb(theme::border()))
                .children(quick.iter().enumerate().map(|(ix, (label, icon))| {
                    let label = *label;
                    div()
                        .id(ElementId::NamedInteger("menu-test-quick".into(), ix as u64))
                        .size(px(36.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .text_color(rgb(theme::fg()))
                        .hover(|button| button.bg(rgb(theme::bg_hover())))
                        .tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(label).build(window, cx)
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(
                            cx.listener(move |page, _, _, cx| close(page, Some(chosen(label)), cx)),
                        )
                        .child(
                            Icon::new(Icon::empty())
                                .path(SharedString::from(*icon))
                                .size_4(),
                        )
                }))
        });
        let entries = rows.into_iter().enumerate().map(|(ix, row)| match row {
            TestEntry::Separator => div()
                .h(px(9.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .px(px(8.0))
                .child(div().w_full().h(px(1.0)).bg(rgb(theme::border())))
                .into_any_element(),
            TestEntry::Note(text) => div()
                .px(px(8.0))
                .py(px(6.0))
                .text_xs()
                .text_color(rgb(theme::fg_secondary()))
                .child(text)
                .into_any_element(),
            TestEntry::Item {
                label,
                icon,
                submenu,
                shortcut,
                windows,
            } => div()
                .id(ElementId::NamedInteger("menu-test-entry".into(), ix as u64))
                .h(px(30.0))
                .flex_shrink_0()
                .px(px(8.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .rounded(px(5.0))
                .text_sm()
                .text_color(rgb(theme::fg()))
                .cursor_pointer()
                .hover(|row| row.bg(rgb(theme::bg_hover())))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |page, _, _, cx| close(page, Some(chosen(label)), cx)))
                // As in the real menu: a Windows entry right-clicked is hidden.
                .when(windows, |row| {
                    row.on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |page, _, _, cx| {
                            cx.stop_propagation();
                            let mut hidden = page.config.menu.hidden.clone();
                            if !hidden.iter().any(|item| item == label) {
                                hidden.push(label.to_string());
                            }
                            page.write_config("menu", "hidden", &super::toml_list(&hidden), cx);
                            page.menu_test.note = Some(
                                t!("Escondido. Ele volta pela lista \"Escondidos\" mais abaixo.")
                                    .into(),
                            );
                            cx.notify();
                        }),
                    )
                })
                .child(
                    div()
                        .w(px(16.0))
                        .flex_shrink_0()
                        .flex()
                        .justify_center()
                        .children(icon.map(|icon| {
                            Icon::new(Icon::empty())
                                .path(SharedString::from(icon))
                                .size_4()
                                .text_color(rgb(theme::fg_secondary()))
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(label),
                )
                .children(shortcut.map(|keys| {
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(rgb(theme::muted()))
                        .child(keys)
                }))
                .when(submenu, |row| {
                    row.child(
                        Icon::new(Icon::empty())
                            .path("icons/chevron-right.svg")
                            .size_3()
                            .text_color(rgb(theme::muted())),
                    )
                })
                .into_any_element(),
        });
        let panel = div()
            .id("menu-test-panel")
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(width))
            .p(px(4.0))
            .flex()
            .flex_col()
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(theme::border()))
            .bg(rgb(theme::bg_secondary()))
            .shadow_xl()
            .occlude()
            .children(quick_row)
            .children(entries);
        deferred(
            anchored()
                .position_mode(AnchoredPositionMode::Window)
                .position(point(px(0.0), px(0.0)))
                .child(
                    div()
                        .w(px(vw))
                        .h(px(vh))
                        .child(
                            // Clicks outside close it, as the real one's do.
                            div()
                                .id("menu-test-backdrop")
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .occlude()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |page, _, _, cx| close(page, None, cx)),
                                )
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |page, _, _, cx| close(page, None, cx)),
                                ),
                        )
                        .child(panel),
                ),
        )
        .with_priority(10)
        .into_any_element()
    }
}

const HOVER_TILE: f32 = 104.0;
const HOVER_GAP: f32 = 10.0;

impl HoverDemo {
    /// How many sound tiles fit a row `width` wide.
    pub(crate) fn fitting(width: f32) -> usize {
        let room = width.max(HOVER_TILE);
        (((room + HOVER_GAP) / (HOVER_TILE + HOVER_GAP)).floor() as usize).clamp(1, SAMPLES.len())
    }

    /// Whether a tile is under the pointer: the page then ticks every frame.
    pub(crate) fn is_active(&self) -> bool {
        self.hovered.is_some()
    }

    /// Silences the tiles when they leave the screen under the pointer,
    /// which then never says it left them.
    pub(crate) fn stop(&mut self) {
        self.hovered = None;
        if self.played.take().is_some() {
            crate::explorer::hover_audio::stop();
        }
    }

    /// Plays the hovered sample once its wait has passed, on the listing's
    /// own player: called on every render while one is hovered.
    pub(crate) fn tick(&mut self, delay_ms: u32) {
        let Some((ix, since)) = self.hovered else {
            return;
        };
        if self.played == Some(ix) || since.elapsed().as_millis() < u128::from(delay_ms) {
            return;
        }
        self.played = Some(ix);
        match sample_file(ix) {
            Ok(path) => crate::explorer::hover_audio::play(path.to_string_lossy().into_owned()),
            Err(error) => tracing::warn!("could not write the sample sound {ix}: {error}"),
        }
    }

    /// Sounds to pass the pointer over, `count` tiles in one row spread to
    /// its ends: each fills a bar for the wait chosen, then plays. `demo`
    /// finds this demo again inside the view `V` that owns it.
    pub(crate) fn render<V: 'static>(
        &self,
        delay_ms: u32,
        count: usize,
        demo: fn(&mut V) -> &mut HoverDemo,
        cx: &mut Context<V>,
    ) -> Div {
        let delay = delay_ms as f32;
        let now = Instant::now();
        div()
            .w_full()
            .flex()
            // Spread to reach the card's end: what is left over is less than
            // a tile, shared out between them.
            .justify_between()
            .children(
                SAMPLES
                    .into_iter()
                    .take(count.clamp(1, SAMPLES.len()))
                    .enumerate()
                    .map(|(ix, name)| {
                        let started = self
                            .hovered
                            .filter(|(row, _)| *row == ix)
                            .map(|(_, since)| since);
                        let waited = started.map(|since| (now - since).as_secs_f32() * 1000.0);
                        let status: AnyElement = match waited {
                            None => div().h(px(14.0)).into_any_element(),
                            Some(ms) if ms >= delay => div()
                                .h(px(14.0))
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .text_xs()
                                .text_color(rgb(theme::accent()))
                                .child(equalizer(ms / 1000.0))
                                .child(t!("Tocando"))
                                .into_any_element(),
                            Some(ms) => div()
                                .h(px(14.0))
                                .w_full()
                                .px(px(12.0))
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .w_full()
                                        .h(px(4.0))
                                        .rounded_full()
                                        .bg(rgb(theme::bg_hover()))
                                        .child(
                                            div()
                                                .h_full()
                                                .rounded_full()
                                                .bg(rgb(theme::accent()))
                                                .w(gpui::relative((ms / delay).min(1.0))),
                                        ),
                                )
                                .into_any_element(),
                        };
                        div()
                            .id(ElementId::NamedInteger(
                                "hover-audio-demo".into(),
                                ix as u64,
                            ))
                            .size(px(HOVER_TILE))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap(px(6.0))
                            .rounded(px(10.0))
                            .border_1()
                            .map(|this| {
                                if started.is_some() {
                                    this.border_color(rgb(theme::accent()))
                                        .bg(rgb(theme::accent_subtle()))
                                } else {
                                    this.border_color(rgb(theme::border())).bg(rgb(theme::bg()))
                                }
                            })
                            .on_hover(cx.listener(
                                move |view: &mut V, hovered: &bool, _window, cx| {
                                    let state = demo(view);
                                    if *hovered {
                                        state.hovered = Some((ix, Instant::now()));
                                    } else if state.hovered.is_some_and(|(row, _)| row == ix) {
                                        state.hovered = None;
                                    }
                                    // Leaving one sound for another silences the first at
                                    // once, as in the listing.
                                    if state.played.take().is_some() {
                                        crate::explorer::hover_audio::stop();
                                    }
                                    cx.notify();
                                },
                            ))
                            .child(
                                Icon::new(Icon::empty())
                                    .path("icons/music.svg")
                                    .size_6()
                                    .text_color(rgb(theme::fg_secondary())),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .px(px(8.0))
                                    .text_xs()
                                    .text_center()
                                    .text_color(rgb(theme::fg()))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(name),
                            )
                            .child(status)
                    }),
            )
    }
}

/// The media preview's sounds, by the names their tiles show.
const SAMPLES: [&str; 14] = [
    "chuva.wav",
    "porta.wav",
    "passos.wav",
    "sino.wav",
    "vento.wav",
    "clique.wav",
    "trovão.wav",
    "risada.wav",
    "teclado.wav",
    "pássaros.wav",
    "aplausos.wav",
    "água.wav",
    "motor.wav",
    "apito.wav",
];

/// The file of sample `ix`, made on first use in the temp folder: short
/// sounds synthesized here, so nothing is shipped or downloaded for them.
fn sample_file(ix: usize) -> std::io::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join("prime-files-amostras");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{ix:02}.wav"));
    let bytes = wav(&synth(ix));
    // Rewritten when the recipe changed (another length).
    if std::fs::metadata(&path).map(|meta| meta.len()).ok() != Some(bytes.len() as u64) {
        std::fs::write(&path, bytes)?;
    }
    Ok(path)
}

const RATE: u32 = 22_050;

// 16-bit mono PCM in a RIFF header.
fn wav(samples: &[f32]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&((sample.clamp(-1.0, 1.0) * 32_000.0) as i16).to_le_bytes());
    }
    out
}

// A decaying sine burst at `at` seconds: a low thud or a short tick.
fn hit(out: &mut [f32], at: f32, freq: f32, decay: f32, gain: f32) {
    let start = (at * RATE as f32) as usize;
    for (k, slot) in out.iter_mut().enumerate().skip(start) {
        let dt = (k - start) as f32 / RATE as f32;
        let env = (-dt * decay).exp();
        if env < 0.001 {
            break;
        }
        *slot += (dt * freq * std::f32::consts::TAU).sin() * env * gain;
    }
}

// A tone sliding from `from` Hz by `slide` Hz a second, `length` long.
fn sweep(out: &mut [f32], at: f32, from: f32, slide: f32, length: f32, decay: f32, gain: f32) {
    let start = (at * RATE as f32) as usize;
    let mut phase = 0.0f32;
    for (k, slot) in out
        .iter_mut()
        .enumerate()
        .skip(start)
        .take((length * RATE as f32) as usize)
    {
        let dt = (k - start) as f32 / RATE as f32;
        phase += (from + dt * slide) / RATE as f32;
        let shape = if decay > 0.0 {
            (-dt * decay).exp()
        } else {
            (dt / length * std::f32::consts::PI).sin()
        };
        *slot += (phase * std::f32::consts::TAU).sin() * shape * gain;
    }
}

// Sample `ix`, about a second and a half of it. Rough on purpose: enough to
// tell one tile from the next by ear.
fn synth(ix: usize) -> Vec<f32> {
    use std::f32::consts::{PI, TAU};
    let len = (RATE as f32 * 1.6) as usize;
    let mut out = vec![0.0f32; len];
    let mut seed = 0x9E37_79B9u32.wrapping_mul(ix as u32 + 1) | 1;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    let t = |i: usize| i as f32 / RATE as f32;
    let mut low = 0.0f32;
    match ix {
        // Rain: soft hiss with drops.
        0 => {
            for slot in out.iter_mut() {
                low += (noise() - low) * 0.35;
                *slot = low * 0.25 + if noise() > 0.995 { 0.5 } else { 0.0 };
            }
        }
        // Door: two knocks, then a slam.
        1 => {
            hit(&mut out, 0.05, 90.0, 18.0, 0.8);
            hit(&mut out, 0.25, 90.0, 18.0, 0.8);
            hit(&mut out, 0.7, 60.0, 7.0, 1.0);
        }
        // Footsteps.
        2 => {
            for step in 0..5 {
                hit(&mut out, 0.05 + step as f32 * 0.3, 120.0, 25.0, 0.7);
            }
        }
        // Bell: a few partials ringing out.
        3 => {
            for (i, slot) in out.iter_mut().enumerate() {
                let x = t(i);
                *slot = ((x * 880.0 * TAU).sin()
                    + 0.5 * (x * 2210.0 * TAU).sin()
                    + 0.25 * (x * 3520.0 * TAU).sin())
                    * (-x * 2.5).exp()
                    * 0.4;
            }
        }
        // Wind: deep noise rising and falling.
        4 => {
            for (i, slot) in out.iter_mut().enumerate() {
                low += (noise() - low) * 0.04;
                *slot = low * (0.5 + 0.5 * (t(i) * 1.3 * TAU).sin()) * 1.2;
            }
        }
        // Mouse clicks.
        5 => {
            for click in [0.1, 0.25, 0.8, 0.95] {
                hit(&mut out, click, 2500.0, 300.0, 0.7);
            }
        }
        // Thunder: a long low rumble.
        6 => {
            for (i, slot) in out.iter_mut().enumerate() {
                low += (noise() - low) * 0.02;
                *slot = low * 2.5 * (-t(i) * 1.5).exp();
            }
        }
        // Laughter: "ha" a few times.
        7 => {
            for (i, slot) in out.iter_mut().enumerate() {
                let x = t(i);
                let syllable = (x * 6.0).fract();
                let on = if x < 1.3 && syllable < 0.55 {
                    (syllable / 0.55 * PI).sin()
                } else {
                    0.0
                };
                let pitch = 260.0 - x * 40.0;
                *slot = ((x * pitch * TAU).sin() + 0.4 * (x * pitch * 3.0 * TAU).sin()) * on * 0.4;
            }
        }
        // Typing.
        8 => {
            let mut at = 0.03;
            while at < 1.4 {
                let freq = 1800.0 + noise().abs() * 900.0;
                hit(&mut out, at, freq, 220.0, 0.5);
                at += 0.07 + noise().abs() * 0.08;
            }
        }
        // Birds: quick chirps sweeping up.
        9 => {
            for chirp in [0.05, 0.22, 0.6, 0.75, 1.1] {
                sweep(&mut out, chirp, 2500.0, 18_000.0, 0.09, 0.0, 0.4);
            }
        }
        // Applause: many small claps.
        10 => {
            for (i, slot) in out.iter_mut().enumerate() {
                *slot = noise() * 0.12 * (1.0 - t(i) / 1.6).max(0.0)
                    + if noise() > 0.97 { noise() * 0.5 } else { 0.0 };
            }
        }
        // Water: bubbles rising.
        11 => {
            let mut at = 0.02;
            while at < 1.4 {
                let base = 400.0 + noise().abs() * 500.0;
                sweep(&mut out, at, base, 9000.0, 0.06, 40.0, 0.4);
                at += 0.08 + noise().abs() * 0.1;
            }
        }
        // Engine: a rough low buzz.
        12 => {
            for (i, slot) in out.iter_mut().enumerate() {
                let x = t(i);
                let freq = 55.0 + 4.0 * (x * 3.0 * TAU).sin();
                *slot = ((x * freq).fract() * 2.0 - 1.0) * 0.3 + noise() * 0.03;
            }
        }
        // Whistle, with a wobble.
        _ => {
            let mut phase = 0.0f32;
            for (i, slot) in out.iter_mut().enumerate() {
                let x = t(i);
                phase += (1600.0 + 40.0 * (x * 7.0 * TAU).sin()) / RATE as f32;
                *slot =
                    (phase * TAU).sin() * 0.35 * (x * 8.0).min(1.0) * ((1.6 - x) * 8.0).min(1.0);
            }
        }
    }
    // No click at either end.
    let fade = (0.01 * RATE as f32) as usize;
    for k in 0..fade.min(len) {
        let gain = k as f32 / fade as f32;
        out[k] *= gain;
        out[len - 1 - k] *= gain;
    }
    out
}

/// A chosen entry, said back in the test.
fn chosen(label: &str) -> SharedString {
    tf!(
        "Você escolheu \"{item}\". No teste, nada acontece.",
        item = label
    )
    .into()
}

#[derive(Clone, Copy)]
enum Panel {
    Sidebar,
    Preview,
    Terminal,
}

enum TestEntry {
    Item {
        label: &'static str,
        icon: Option<&'static str>,
        submenu: bool,
        shortcut: Option<&'static str>,
        windows: bool,
    },
    Separator,
    Note(&'static str),
}

fn hides(compact: &nohrs_core::config::Compact, edge: Edge) -> bool {
    match edge {
        Edge::Top => compact.hide_tabs || compact.hide_header,
        Edge::Left => compact.hide_sidebar,
        Edge::Right => compact.hide_preview,
        Edge::Bottom => compact.hide_bottom_bar,
    }
}

// Height of the top bars that hide, and of those that stay.
fn top_hidden(compact: &nohrs_core::config::Compact) -> f32 {
    (if compact.hide_tabs { TABS_H } else { 0.0 })
        + (if compact.hide_header { HEADER_H } else { 0.0 })
}

fn top_fixed(compact: &nohrs_core::config::Compact) -> f32 {
    (if compact.hide_tabs { 0.0 } else { TABS_H })
        + (if compact.hide_header { 0.0 } else { HEADER_H })
}

// A bar sliding over the listing from its edge, `p` of the way out.
fn slide(inner: Div, edge: Edge, size: f32, p: f32) -> Div {
    let hidden = -(1.0 - p) * size;
    let strip = div()
        .absolute()
        .top_0()
        .bottom_0()
        .w(px(size))
        .overflow_hidden();
    let inner = inner.absolute().top_0().bottom_0();
    match edge {
        Edge::Left => strip.left_0().child(inner.left(px(hidden))),
        _ => strip.right_0().child(inner.right(px(hidden))),
    }
}

fn relative_center() -> gpui::DefiniteLength {
    gpui::relative(0.42).into()
}

// ---------------------------------------------------------------------------
// The preview window's parts: grey lines where words would be.

pub(crate) fn mini_window(height: f32) -> Div {
    div()
        .relative()
        .w_full()
        .h(px(height))
        .flex()
        .flex_col()
        .rounded(px(10.0))
        .overflow_hidden()
        .border_1()
        .border_color(rgb(theme::border()))
        .bg(rgb(theme::bg()))
        .text_size(px(TINY))
}

pub(crate) fn line(width: f32, color: u32) -> Div {
    div()
        .h(px(4.0))
        .w(px(width))
        .flex_shrink_0()
        .rounded(px(2.0))
        .bg(rgb(color))
}

fn tab_strip() -> Div {
    let tab = |active: bool, width: f32| {
        div()
            .h(px(17.0))
            .w(px(width))
            .px(px(6.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .rounded_t(px(5.0))
            .when(active, |this| this.bg(rgb(theme::bg())))
            .child(div().size(px(6.0)).rounded(px(1.5)).bg(rgb(FOLDER)))
            .child(line(
                width - 26.0,
                if active {
                    theme::fg_secondary()
                } else {
                    theme::border()
                },
            ))
    };
    div()
        .h(px(TABS_H))
        .w_full()
        .flex_shrink_0()
        .flex()
        .items_end()
        .gap(px(3.0))
        .px(px(6.0))
        .bg(rgb(theme::toolbar_bg()))
        .child(tab(true, 74.0))
        .child(tab(false, 64.0))
        .child(tab(false, 56.0))
}

fn header() -> Div {
    div()
        .h(px(HEADER_H))
        .w_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(5.0))
        .px(px(7.0))
        .bg(rgb(theme::bg()))
        .border_b_1()
        .border_color(rgb(theme::border()))
        .children((0..3).map(|_| div().size(px(5.0)).rounded_full().bg(rgb(theme::muted()))))
        .child(
            div()
                .flex_1()
                .h(px(11.0))
                .rounded(px(3.0))
                .bg(rgb(theme::bg_secondary()))
                .flex()
                .items_center()
                .px(px(5.0))
                .child(line(90.0, theme::border())),
        )
}

fn vertical_tabs() -> Div {
    div()
        .w(px(VTABS_W))
        .h_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(5.0))
        .pt(px(6.0))
        .bg(rgb(theme::toolbar_bg()))
        .border_r_1()
        .border_color(rgb(theme::border()))
        .children((0..3).map(|ix| {
            div()
                .size(px(16.0))
                .rounded(px(4.0))
                .flex()
                .items_center()
                .justify_center()
                .when(ix == 0, |this| this.bg(rgb(theme::accent_subtle())))
                .child(div().size(px(7.0)).rounded(px(1.5)).bg(rgb(FOLDER)))
        }))
}

fn titled(title: &'static str, body: Div) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(5.0))
        .p(px(6.0))
        .bg(rgb(theme::toolbar_bg()))
        .child(
            div()
                .text_color(rgb(theme::muted()))
                .whitespace_nowrap()
                .child(title),
        )
        .child(body)
}

fn sidebar(tags: bool) -> Div {
    let place = |width: f32, active: bool| {
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(3.0))
            .py(px(2.0))
            .rounded(px(3.0))
            .when(active, |this| this.bg(rgb(theme::accent_subtle())))
            .child(div().size(px(6.0)).rounded(px(1.5)).bg(rgb(FOLDER)))
            .child(line(width, theme::border()))
    };
    titled(
        t!("Barra lateral"),
        div()
            .flex()
            .flex_col()
            .gap(px(1.0))
            .child(place(46.0, false))
            .child(place(38.0, true))
            .child(place(52.0, false))
            .child(place(34.0, false))
            .when(tags, |this| {
                this.child(div().h(px(6.0))).children(
                    [0xEF4444, 0x3B82F6, 0x22C55E].into_iter().map(|color| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .px(px(3.0))
                            .py(px(2.0))
                            .child(div().size(px(6.0)).rounded_full().bg(rgb(color)))
                            .child(line(30.0, theme::border()))
                    }),
                )
            }),
    )
}

fn preview_panel() -> Div {
    titled(
        t!("Visualizador"),
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .w_full()
                    .h(px(46.0))
                    .rounded(px(4.0))
                    .bg(rgb(theme::accent_subtle())),
            )
            .child(line(60.0, theme::border()))
            .child(line(40.0, theme::border())),
    )
}

fn terminal() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .p(px(6.0))
        .bg(rgb(0x111214))
        .child(
            div()
                .flex()
                .gap(px(4.0))
                .child(div().text_color(rgb(0x9A9CA3)).child(t!("Terminal")))
                .child(div().text_color(rgb(0x6B6E76)).child(t!("(quando aberto)"))),
        )
        .child(
            div()
                .text_color(rgb(0x22C55E))
                .font_family("Consolas")
                .child("> _"),
        )
}

fn listing() -> Div {
    let row = |width: f32, folder: bool, selected: bool| {
        div()
            .flex()
            .items_center()
            .gap(px(5.0))
            .px(px(4.0))
            .py(px(3.0))
            .rounded(px(3.0))
            .when(selected, |this| this.bg(rgb(theme::accent_subtle())))
            .child(div().size(px(7.0)).rounded(px(1.5)).bg(rgb(if folder {
                FOLDER
            } else {
                theme::muted()
            })))
            .child(line(
                width,
                if selected {
                    theme::accent()
                } else {
                    theme::border()
                },
            ))
    };
    div()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .gap(px(1.0))
        .p(px(6.0))
        .bg(rgb(theme::bg()))
        .overflow_hidden()
        .child(
            div()
                .flex()
                .gap(px(30.0))
                .px(px(4.0))
                .pb(px(3.0))
                .child(line(26.0, theme::muted()))
                .child(line(18.0, theme::muted())),
        )
        .child(row(70.0, true, false))
        .child(row(54.0, true, false))
        .child(row(88.0, true, true))
        .child(row(62.0, false, false))
        .child(row(76.0, false, false))
        .child(row(48.0, false, false))
        .child(row(66.0, false, false))
}

fn bottom_bar() -> Div {
    div()
        .h(px(BOTTOM_H))
        .w_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_between()
        .px(px(7.0))
        .bg(rgb(theme::toolbar_bg()))
        .border_t_1()
        .border_color(rgb(theme::border()))
        .child(line(60.0, theme::border()))
        .child(line(34.0, theme::border()))
}

// Four bars bobbing: playing, without a sound.
fn equalizer(time: f32) -> Div {
    div()
        .h(px(12.0))
        .flex()
        .items_end()
        .gap(px(2.0))
        .children((0..4).map(|ix| {
            let phase = time * 9.0 + ix as f32 * 1.7;
            let height = 3.0 + (phase.sin() * 0.5 + 0.5) * 9.0;
            div()
                .w(px(3.0))
                .h(px(height))
                .rounded(px(1.0))
                .bg(rgb(theme::accent()))
        }))
}
