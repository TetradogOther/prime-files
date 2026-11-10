use super::super::types::ViewMode;
use crate::explorer::ExplorerPane;
use gpui::prelude::*;
use gpui::*;
use gpui_component::Sizable;
use gpui_component::input::Input;
use gpui_component::list::ListItem;
use gpui_component::{Icon, IconName};
use nohrs_core::i18n;
use nohrs_core::{k, t};
use nohrs_ui::theme::theme;
use std::time::Duration;

/// Renders the explorer header with navigation buttons, the breadcrumb path
/// bar, and view-mode controls.
pub fn render(
    page: &mut ExplorerPane,
    _window: &mut Window,
    cx: &mut Context<ExplorerPane>,
) -> impl IntoElement + use<> {
    // This PC is an address, not a path: one crumb with its name. Under a
    // drive, a crumb for it comes first, as in Explorer, leading back to it.
    let location = page.location_name(cx);
    if location.is_none() && !continues_below(&page.crumb_trail, &page.cwd) {
        page.crumb_trail = page.cwd.clone();
    }
    // The crumbs run to the deepest folder of the trail; the ones past the
    // folder on screen are dimmed, and clicking one goes back down there.
    let parts = if location.is_some() {
        Vec::new()
    } else {
        path_parts(&page.crumb_trail)
    };
    let current_ix = if location.is_some() {
        0
    } else {
        path_parts(&page.cwd).len().saturating_sub(1)
    };
    let under_this_pc = location.is_none() && starts_with_drive(&page.cwd);

    // Five crumbs at most, always including the folder on screen.
    let start_idx = if parts.len() > 5 {
        (parts.len() - 5).min(current_ix)
    } else {
        0
    };
    let display_parts = parts[start_idx..].to_vec();
    let is_truncated = start_idx > 0;

    let mut crumbs = Vec::new();
    let mut crumb_id = 0;
    page.crumb_bounds.borrow_mut().clear();
    // On a share, Network and the server come first, as in Explorer, each
    // leading back to its list.
    let unc_server = crate::explorer::network::server_of(&page.cwd).map(str::to_string);
    if unc_server.is_some() {
        push_crumb(
            &mut crumbs,
            page,
            &mut crumb_id,
            i18n::tr(crate::explorer::network::NETWORK_NAME),
            theme::fg_secondary(),
            None,
            Some(Box::new(|this, _, window, cx| {
                this.change_dir(crate::explorer::network::NETWORK.to_string(), window, cx)
            })),
            cx,
        );
    }
    if let Some(name) = location {
        push_crumb(
            &mut crumbs,
            page,
            &mut crumb_id,
            name,
            theme::fg_secondary(),
            None,
            None,
            cx,
        );
    } else if under_this_pc {
        // A computer, as the Explorer draws it there: the name took a
        // crumb's worth of room on every path of every drive.
        push_crumb(
            &mut crumbs,
            page,
            &mut crumb_id,
            "\u{e7f4}",
            theme::fg_secondary(),
            Some(theme::icon_font()),
            Some(Box::new(|this, _, window, cx| {
                this.change_dir(crate::explorer::places::THIS_PC.to_string(), window, cx)
            })),
            cx,
        );
    } else if let Some(server) = unc_server.clone() {
        let address = format!("\\\\{server}");
        push_crumb(
            &mut crumbs,
            page,
            &mut crumb_id,
            server,
            theme::fg_secondary(),
            None,
            Some(Box::new(move |this, _, window, cx| {
                this.change_dir(address.clone(), window, cx)
            })),
            cx,
        );
    }

    if is_truncated {
        push_crumb(
            &mut crumbs,
            page,
            &mut crumb_id,
            "…",
            theme::fg_secondary(),
            None,
            None,
            cx,
        );
    }

    for (display_i, p) in display_parts.iter().enumerate() {
        let actual_i = start_idx + display_i;
        let text = if p.is_empty() {
            String::from("/")
        } else if actual_i == 0 && unc_server.is_some() {
            // `\\nas\edicao\`: the server has its crumb already.
            p.trim_end_matches('\\')
                .rsplit('\\')
                .next()
                .unwrap_or(p)
                .to_string()
        } else {
            p.clone()
        };

        // `PathBuf::push` knows the root component already ends in a separator;
        // joining the strings by hand produced "//tmp" for every crumb under "/".
        let mut prefix = std::path::PathBuf::new();
        for (j, part) in parts.iter().enumerate() {
            prefix.push(part);
            if j >= actual_i {
                break;
            }
        }
        let mut path_here = prefix.to_string_lossy().to_string();
        if path_here.is_empty() {
            path_here = page.cwd.clone();
        }

        let (color, opacity) = match actual_i.cmp(&current_ix) {
            std::cmp::Ordering::Less => (theme::fg_secondary(), 1.0),
            std::cmp::Ordering::Equal => (theme::fg(), 1.0),
            std::cmp::Ordering::Greater => (theme::fg_secondary(), 0.55),
        };
        push_crumb(
            &mut crumbs,
            page,
            &mut crumb_id,
            text,
            mix_rgb(theme::bg(), color, opacity),
            None,
            Some(Box::new(move |this, _, window, cx| {
                this.change_dir(path_here.clone(), window, cx)
            })),
            cx,
        );
    }

    let can_go_back = page.history_index > 0;
    let can_go_forward = page.history_index + 1 < page.history.len();
    let can_go_up = crate::explorer::navigation::parent_location(&page.cwd).is_some();

    // Store search_visible for use in search toggle style
    let preview_visible = page.preview_visible;
    let entry_count = page.filtered_entries.len();

    let framed = page.framed.header;
    div()
        .when(!framed, |this| this.bg(rgb(theme::bg())).border_b_1())
        .border_color(rgb(theme::border()))
        .flex()
        .items_center()
        .text_color(rgb(theme::fg()))
        .px(px(16.0))
        .py(px(10.0))
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .flex_shrink_0()
                .child(
                    ListItem::new("nav-back")
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .when(!can_go_back, |this| this.opacity(0.35))
                        .when(can_go_back, |this| {
                            this.on_click(
                                cx.listener(|view, _, window, cx| view.go_back(window, cx)),
                            )
                        })
                        .child(
                            Icon::new(IconName::ArrowLeft)
                                .size_4()
                                .text_color(rgb(theme::gray_600())),
                        ),
                )
                .child(
                    ListItem::new("nav-forward")
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .when(!can_go_forward, |this| this.opacity(0.35))
                        .when(can_go_forward, |this| {
                            this.on_click(
                                cx.listener(|view, _, window, cx| view.go_forward(window, cx)),
                            )
                        })
                        .child(
                            Icon::new(IconName::ArrowRight)
                                .size_4()
                                .text_color(rgb(theme::gray_600())),
                        ),
                )
                // Up and refresh beside back and forward, in Explorer's order.
                .child(
                    ListItem::new("nav-up")
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .when(!can_go_up, |this| this.opacity(0.35))
                        .when(can_go_up, |this| {
                            this.on_click(cx.listener(|view, _, window, cx| view.go_up(window, cx)))
                        })
                        .child(
                            Icon::new(Icon::empty())
                                .path("icons/arrow-up.svg")
                                .size_4()
                                .text_color(rgb(theme::gray_600())),
                        ),
                )
                .child(
                    ListItem::new("nav-refresh")
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .on_click(cx.listener(|view, _, _, cx| view.reload(cx)))
                        .child(
                            Icon::new(Icon::empty())
                                .path("icons/refresh-cw.svg")
                                .size_4()
                                .text_color(rgb(theme::gray_600())),
                        ),
                )
                .child(
                    div()
                        .w(px(1.0))
                        .h(px(20.0))
                        .bg(rgb(theme::border()))
                        .mx(px(6.0)),
                ),
        )
        .child(match &page.path_edit {
            Some(state) => div()
                .flex_1()
                .min_w(px(0.0))
                .child(Input::new(&state.input).small().text_sm())
                .into_any_element(),
            // A click on the bar outside the crumbs turns it into the full
            // path, selected, as File Pilot's and the Explorer's do; a
            // double click anywhere, crumbs included, too. A single click on
            // a crumb navigates.
            None => div()
                .id("path-bar")
                .flex_1()
                .h(px(28.0))
                .flex()
                .items_center()
                .overflow_hidden()
                .min_w(px(0.0))
                .cursor_text()
                .on_click(cx.listener(|this, _event: &ClickEvent, window, cx| {
                    this.begin_path_edit(window, cx);
                }))
                .child(
                    div()
                        .relative()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_sm()
                        .children(crumbs)
                        .child(crumb_hover_guard(cx.entity())),
                )
                .into_any_element(),
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .flex_shrink_0()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme::fg_secondary()))
                        .whitespace_nowrap()
                        .mr(px(4.0))
                        .child(i18n::plural(entry_count, k!("{n} item"), k!("{n} itens"))),
                )
                .child(render_open_in(page, cx))
                .child(render_view_mode_toggle(page, cx))
                .child(
                    // The preview switch sits up here, beside the view modes,
                    // where it is seen: the preview is one of the explorer's
                    // most used parts. The filter menu went to the bottom bar.
                    ListItem::new("preview-toggle")
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.preview_visible = !view.preview_visible;
                            cx.notify();
                        }))
                        .child(
                            Icon::new(Icon::empty())
                                .path("icons/panel-right.svg")
                                .size_4()
                                .text_color(if preview_visible {
                                    rgb(theme::accent())
                                } else {
                                    rgb(theme::gray_600())
                                }),
                        ),
                ),
        )
}

type CrumbAction =
    Box<dyn Fn(&mut ExplorerPane, &ClickEvent, &mut Window, &mut Context<ExplorerPane>)>;

fn push_crumb(
    crumbs: &mut Vec<AnyElement>,
    page: &ExplorerPane,
    id: &mut usize,
    label: impl Into<SharedString>,
    color: u32,
    font_family: Option<&'static str>,
    action: Option<CrumbAction>,
    cx: &mut Context<ExplorerPane>,
) {
    if !crumbs.is_empty() {
        crumbs.push(
            Icon::new(IconName::ChevronRight)
                .size_3p5()
                .text_color(rgb(theme::muted()))
                .into_any_element(),
        );
    }
    crumbs.push(path_crumb(page, *id, label, color, font_family, action, cx));
    *id += 1;
}

fn path_crumb(
    page: &ExplorerPane,
    id: usize,
    label: impl Into<SharedString>,
    color: u32,
    font_family: Option<&'static str>,
    action: Option<CrumbAction>,
    cx: &mut Context<ExplorerPane>,
) -> AnyElement {
    let clickable = action.is_some();
    let item = div()
        .id(("path-crumb", id))
        .px(px(4.0))
        .py(px(3.0))
        .rounded(px(5.0))
        .text_sm()
        .text_color(rgb(color))
        .when_some(font_family, |item, family| item.font_family(family))
        .child(label.into());
    if !clickable {
        return item.into_any_element();
    }
    // Where it is drawn, for `crumb_hover_guard` to tell which crumb the
    // pointer is on. gpui's own hover events were not used: a crumb switching
    // to its animated form reported "hover ended" with the pointer still on
    // it, and the listing swallowed the move that left the bar downwards.
    let bounds_list = page.crumb_bounds.clone();
    let item = item
        .relative()
        .cursor_pointer()
        .child(
            canvas(
                move |bounds, _, _| {
                    let mut list = bounds_list.borrow_mut();
                    if list.len() <= id {
                        list.resize(id + 1, Bounds::default());
                    }
                    list[id] = bounds;
                },
                |_, _, _, _| {},
            )
            // Pinned to the crumb's corner: an absolute box without insets
            // sits where the text flow leaves it, below the label.
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .on_click(cx.listener(move |page, event, window, cx| {
            cx.stop_propagation();
            if let Some(action) = &action {
                action(page, event, window, cx);
            }
        }));
    let Some((_, generation)) = page
        .crumb_hover_animation
        .filter(|(animated, _)| *animated == id && page.hovered_crumb == Some(id))
    else {
        return item.into_any_element();
    };
    // The highlight fades in rather than switching on: the background from
    // clear to the accent tint and the text from its colour to the accent.
    let tint = theme::accent_subtle();
    let accent = theme::accent();
    item.with_animation(
        (
            ElementId::from(("path-crumb-hover", id)),
            generation.to_string(),
        ),
        Animation::new(Duration::from_millis(160)).with_easing(ease_out_quint()),
        move |item, progress| {
            let alpha = (progress.clamp(0.0, 1.0) * 255.0).round() as u32;
            item.bg(rgba((tint << 8) | alpha))
                .text_color(rgb(mix_rgb(color, accent, progress)))
        },
    )
    .into_any_element()
}

// Clears the crumb highlight once the pointer is anywhere off the crumbs. In
// the capture phase, which every move goes through: the listing consumes the
// moves over it, so a crumb's own "hover ended" never came when the pointer
// left the bar downwards and the highlight stayed lit.
fn crumb_hover_guard(entity: Entity<ExplorerPane>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, (), window, _cx| {
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                entity.update(cx, |page, cx| {
                    let under = page
                        .crumb_bounds
                        .borrow()
                        .iter()
                        .position(|bounds| bounds.contains(&event.position));
                    set_crumb_hover(page, under, cx);
                });
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

fn set_crumb_hover(page: &mut ExplorerPane, next: Option<usize>, cx: &mut Context<ExplorerPane>) {
    if page.hovered_crumb == next {
        return;
    }
    page.hovered_crumb = next;
    if let Some(id) = next {
        let generation = page
            .crumb_hover_animation
            .map_or(1, |(_, generation)| generation.wrapping_add(1));
        page.crumb_hover_animation = Some((id, generation));
    }
    cx.notify();
}

fn mix_rgb(from: u32, to: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let from = ((from >> shift) & 0xff) as f32;
        let to = ((to >> shift) & 0xff) as f32;
        (from + (to - from) * amount).round() as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

// "Open in": the folder on screen in the Command Prompt, PowerShell,
// Windows Terminal (when installed) or Windows' own Explorer, each with the
// shortcut it was given in Settings › Shortcuts.
fn render_open_in(
    page: &mut ExplorerPane,
    cx: &mut Context<ExplorerPane>,
) -> impl IntoElement + use<> {
    use crate::explorer::commands::{OpenIn, has_windows_terminal};
    use nohrs_core::shortcuts::{Action, display};
    let open = page.open_in_menu;
    let mut items: Vec<(OpenIn, &'static str, &'static str, Option<String>)> = vec![
        (
            OpenIn::CommandPrompt,
            "icons/square-terminal.svg",
            t!("Prompt de Comando"),
            display(Action::OpenCommandPrompt),
        ),
        (
            OpenIn::PowerShell,
            "icons/square-terminal.svg",
            "PowerShell",
            display(Action::OpenPowerShell),
        ),
    ];
    if has_windows_terminal() {
        items.push((
            OpenIn::WindowsTerminal,
            "icons/app-terminal.svg",
            "Windows Terminal",
            None,
        ));
    }
    items.push((
        OpenIn::FileExplorer,
        "icons/folder-open.svg",
        t!("Explorador do Windows"),
        display(Action::OpenFileExplorer),
    ));
    div()
        .relative()
        .flex_shrink_0()
        .child(
            div()
                .id("open-in")
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(|style| style.bg(rgb(theme::bg_hover())))
                .when(open, |style| style.bg(rgb(theme::bg_hover())))
                .tooltip(|window, cx| {
                    gpui_component::tooltip::Tooltip::new(t!("Abrir a pasta em outro programa"))
                        .build(window, cx)
                })
                .on_click(cx.listener(|page, _, _, cx| {
                    page.open_in_menu = !page.open_in_menu;
                    cx.notify();
                }))
                .child(
                    Icon::new(Icon::empty())
                        .path("icons/external-link.svg")
                        .size_4()
                        .text_color(rgb(if open {
                            theme::accent()
                        } else {
                            theme::gray_600()
                        })),
                ),
        )
        .when(open, |this| {
            this.child(deferred(
                div()
                    .id("open-in-menu")
                    .absolute()
                    .top(px(34.0))
                    .right_0()
                    .min_w(px(240.0))
                    .p(px(4.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .bg(rgb(theme::bg_secondary()))
                    .shadow_xl()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|page, _, _, cx| {
                        page.open_in_menu = false;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .px(px(8.0))
                            .pt(px(4.0))
                            .pb(px(2.0))
                            .text_xs()
                            .text_color(rgb(theme::muted()))
                            .child(t!("Abrir esta pasta no")),
                    )
                    .children(items.into_iter().map(|(program, icon, label, keys)| {
                        div()
                            .id(SharedString::from(format!("open-in-{program:?}")))
                            .h(px(30.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_sm()
                            .text_color(rgb(theme::fg()))
                            .hover(|style| style.bg(rgb(theme::bg_hover())))
                            .on_click(cx.listener(move |page, _, _, cx| page.open_in(program, cx)))
                            .child(
                                Icon::new(Icon::empty())
                                    .path(icon)
                                    .size_4()
                                    .text_color(rgb(theme::fg_secondary())),
                            )
                            .child(div().flex_1().child(label))
                            .children(keys.map(|keys| {
                                div().text_xs().text_color(rgb(theme::muted())).child(keys)
                            }))
                    })),
            ))
        })
}

fn render_view_mode_toggle(
    page: &mut ExplorerPane,
    cx: &mut Context<ExplorerPane>,
) -> impl IntoElement + use<> {
    div()
        .flex()
        .items_center()
        .gap_1()
        .p(px(2.0))
        .rounded(px(8.0))
        .bg(rgb(theme::bg_secondary()))
        .border_1()
        .border_color(rgb(theme::border()))
        // `list.svg` ships with this crate but has no `IconName` variant, so it
        // is loaded by asset path.
        .child(view_mode_button(
            page,
            ViewMode::List,
            "view-mode-list",
            Icon::new(Icon::empty()).path(SharedString::from("icons/list.svg")),
            t!("Lista"),
            cx,
        ))
        .child(view_mode_button(
            page,
            ViewMode::Grid,
            "view-mode-grid",
            Icon::new(IconName::LayoutDashboard),
            t!("Grade"),
            cx,
        ))
}

fn view_mode_button(
    page: &mut ExplorerPane,
    mode: ViewMode,
    id: &'static str,
    icon: Icon,
    label: &'static str,
    cx: &mut Context<ExplorerPane>,
) -> impl IntoElement + use<> {
    // Tree and List are views of the list side, so its button stays lit.
    let is_active = (page.view_mode == ViewMode::Grid) == (mode == ViewMode::Grid);
    // The active segment lifts onto white so its accent icon reads against the
    // group's tinted track, rather than accent-on-gray.
    ListItem::new(id)
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(6.0))
        .when(is_active, |this| this.bg(rgb(theme::bg())).shadow_sm())
        .on_click(cx.listener(move |this, _, _, cx| this.set_view_mode(mode, cx)))
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(icon.size_4().text_color(if is_active {
                    rgb(theme::accent())
                } else {
                    rgb(theme::gray_500())
                }))
                .child(
                    div()
                        .text_xs()
                        .when(is_active, |this| this.font_weight(gpui::FontWeight::MEDIUM))
                        .text_color(if is_active {
                            rgb(theme::fg())
                        } else {
                            rgb(theme::fg_secondary())
                        })
                        .child(label),
                ),
        )
}

// `C:\...`: a path on a lettered drive, which Explorer shows under This PC.
// Whether `trail` is `cwd` or a folder somewhere below it, so the crumbs of
// the trail still lead through the folder on screen. Windows paths compare
// without case.
fn continues_below(trail: &str, cwd: &str) -> bool {
    if trail.is_empty() {
        return false;
    }
    let trail = trail.to_lowercase();
    let cwd = cwd.to_lowercase();
    let cwd = cwd.trim_end_matches(['\\', '/']);
    trail.trim_end_matches(['\\', '/']) == cwd
        || trail
            .strip_prefix(cwd)
            .is_some_and(|rest| rest.starts_with(['\\', '/']))
}

fn starts_with_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn path_parts(path: &str) -> Vec<String> {
    use std::path::Component;
    let mut parts: Vec<String> = Vec::new();
    let mut after_prefix = false;
    for c in std::path::Path::new(path).components() {
        match (c, parts.last_mut()) {
            // Windows splits `C:\` (and `\\server\share\`) into a prefix and a
            // `\` root. Shown apart they are two crumbs where the user sees one
            // drive, and the first navigates to `C:`, which Windows reads as the
            // current directory on that drive rather than its root.
            (Component::RootDir, Some(prefix)) if after_prefix => {
                prefix.push(std::path::MAIN_SEPARATOR);
            }
            _ => parts.push(c.as_os_str().to_string_lossy().to_string()),
        }
        after_prefix = matches!(c, Component::Prefix(_));
    }
    if parts.is_empty() {
        parts.push(path.to_string());
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::{continues_below, path_parts};

    #[test]
    fn the_trail_survives_going_up_and_not_another_branch() {
        assert!(continues_below(r"C:\Edição\SFX\Awkhard", r"C:\Edição"));
        assert!(continues_below(r"C:\Edição\SFX", r"c:\edição\sfx"));
        assert!(continues_below(r"C:\Edição\SFX", r"C:\"));
        assert!(!continues_below(r"C:\Edição\SFX", r"C:\Edição\Música"));
        assert!(!continues_below(r"C:\Edição2\SFX", r"C:\Edição"));
        assert!(!continues_below("", r"C:\Edição"));
    }

    #[cfg(windows)]
    #[test]
    fn drive_and_root_are_one_crumb() {
        assert_eq!(path_parts(r"C:\Users\ada"), ["C:\\", "Users", "ada"]);
        assert_eq!(path_parts(r"C:\"), ["C:\\"]);
        assert_eq!(
            path_parts(r"\\server\share\docs"),
            [r"\\server\share\", "docs"]
        );
    }

    #[cfg(windows)]
    #[test]
    fn up_goes_to_the_parent_and_from_a_drive_to_this_pc() {
        use crate::explorer::navigation::parent_location;
        use crate::explorer::places::THIS_PC;
        assert_eq!(
            parent_location(r"C:\Users\ada").as_deref(),
            Some(r"C:\Users")
        );
        assert_eq!(parent_location(r"C:\Users").as_deref(), Some(r"C:\"));
        assert_eq!(parent_location(r"C:\").as_deref(), Some(THIS_PC));
        assert_eq!(parent_location(r"d:").as_deref(), Some(THIS_PC));
        assert_eq!(parent_location(THIS_PC), None);
    }

    #[cfg(unix)]
    #[test]
    fn root_stays_its_own_crumb() {
        assert_eq!(path_parts("/tmp/ada"), ["/", "tmp", "ada"]);
    }
}
