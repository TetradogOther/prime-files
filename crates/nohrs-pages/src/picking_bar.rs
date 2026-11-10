//! The bottom of the Windows Open dialog, in Nohrs while it picks for one
//! (`file_dialog`): a Name box that follows the selection and takes a name
//! or a path typed by hand, the dialog's own list of file types, and Open and
//! Cancel. The owner asked for it to work as the Windows dialog does
//! (07/11/2026); a double click on a file was the only way to send one.

use std::path::{Path, PathBuf};

use gpui::prelude::*;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Icon, IconName};
use nohrs_core::{t, tf};
use nohrs_ui::theme::theme;

use crate::explorer::StatusLevel;
use crate::file_dialog::{self, FileType};
use crate::root::RootView;

/// The bar's state while a dialog waits on Nohrs.
pub struct PickingBar {
    hwnd: isize,
    name: Entity<InputState>,
    _enter: Subscription,
    types: Vec<FileType>,
    chosen: usize,
    /// Patterns typed in the Name box (`*.png`), shown instead of the chosen
    /// type until another type is picked.
    typed: Option<String>,
    menu_open: bool,
    /// The files the Name box was last filled from: a new selection fills
    /// it again, and text typed since is left alone until then.
    shown_selection: Vec<String>,
}

// What the Name box asks for, read as the Windows dialog reads it.
#[derive(Debug, PartialEq)]
enum Typed {
    Send(Vec<String>),
    Folder(String),
    Patterns(String),
    Missing(String),
    Nothing,
}

impl RootView {
    /// Opens, keeps or closes the bar with the dialog, and fills the Name box
    /// from the selection. Runs on every render: the selection lives in the
    /// panes, which tell nobody when it changes.
    pub(crate) fn sync_picking_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = file_dialog::picking() else {
            if self.picking_bar.take().is_some() {
                file_dialog::set_patterns(None);
                self.refilter_listings(cx);
            }
            return;
        };
        if self
            .picking_bar
            .as_ref()
            .is_none_or(|bar| bar.hwnd != dialog.id())
        {
            self.open_picking_bar(dialog.id(), window, cx);
        }
        let pane = self.explorer_entity().read(cx).group.active_pane().clone();
        let files = pane.read(cx).picked_files();
        let Some(bar) = self.picking_bar.as_mut() else {
            return;
        };
        if files != bar.shown_selection {
            bar.shown_selection = files.clone();
            if !files.is_empty() {
                let text = names_text(&files);
                bar.name
                    .update(cx, |name, cx| name.set_value(text, window, cx));
            }
        }
    }

    fn open_picking_bar(&mut self, hwnd: isize, window: &mut Window, cx: &mut Context<Self>) {
        let name = cx.new(|cx| InputState::new(window, cx));
        let enter = cx.subscribe_in(
            &name,
            window,
            |root, _name, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    root.commit_picking(window, cx);
                }
            },
        );
        let (types, chosen) = file_dialog::file_types(hwnd);
        let empty = types.is_empty();
        file_dialog::set_patterns(types.get(chosen).and_then(|kind| kind.patterns.clone()));
        self.picking_bar = Some(PickingBar {
            hwnd,
            name,
            _enter: enter,
            types,
            chosen,
            typed: None,
            menu_open: false,
            shown_selection: Vec::new(),
        });
        self.refilter_listings(cx);
        if empty {
            // The hook sees the dialog before it has filled its list of
            // types; it is read again for a few seconds.
            cx.spawn_in(window, async move |this, cx| {
                for _ in 0..12 {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(250))
                        .await;
                    let done = this
                        .update(cx, |root, cx| {
                            let Some(bar) =
                                root.picking_bar.as_mut().filter(|bar| bar.hwnd == hwnd)
                            else {
                                return true;
                            };
                            let (types, chosen) = file_dialog::file_types(hwnd);
                            if types.is_empty() {
                                return false;
                            }
                            if bar.typed.is_none() {
                                file_dialog::set_patterns(
                                    types.get(chosen).and_then(|kind| kind.patterns.clone()),
                                );
                            }
                            bar.types = types;
                            bar.chosen = chosen;
                            root.refilter_listings(cx);
                            cx.notify();
                            true
                        })
                        .unwrap_or(true);
                    if done {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    // Every listing shows files by the chosen type, as the dialog does.
    fn refilter_listings(&mut self, cx: &mut Context<Self>) {
        let tabs = self.explorer_entity().read(cx).group.all_tabs();
        for tab in tabs {
            tab.update(cx, |pane, cx| {
                pane.apply_filter();
                cx.notify();
            });
        }
    }

    // Open, or Enter in the Name box.
    fn commit_picking(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = self.picking_bar.as_ref() else {
            return;
        };
        let text = bar.name.read(cx).value().trim().to_string();
        let pane = self.explorer_entity().read(cx).group.active_pane().clone();
        let (cwd, selected) = {
            let pane = pane.read(cx);
            (pane.cwd.clone(), pane.picked_files())
        };
        match resolve(&text, &cwd, &selected) {
            Typed::Send(paths) => {
                pane.update(cx, |pane, cx| pane.send_to_dialog(paths, window, cx))
            }
            Typed::Folder(folder) => {
                if let Some(bar) = self.picking_bar.as_ref() {
                    bar.name
                        .update(cx, |name, cx| name.set_value("", window, cx));
                }
                pane.update(cx, |pane, cx| pane.change_dir(folder, window, cx));
            }
            Typed::Patterns(patterns) => {
                file_dialog::set_patterns(file_dialog::parse_typed_patterns(&patterns));
                if let Some(bar) = self.picking_bar.as_mut() {
                    bar.typed = Some(patterns);
                }
                self.refilter_listings(cx);
            }
            Typed::Missing(name) => pane.update(cx, |pane, cx| {
                pane.set_status(
                    StatusLevel::Warning,
                    tf!("Não foi encontrado: {name}", name = name),
                );
                cx.notify();
            }),
            Typed::Nothing => {}
        }
        cx.notify();
    }

    fn choose_file_type(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(bar) = self.picking_bar.as_mut() else {
            return;
        };
        bar.menu_open = false;
        if let Some(kind) = bar.types.get(index) {
            bar.chosen = index;
            bar.typed = None;
            file_dialog::set_patterns(kind.patterns.clone());
        }
        self.refilter_listings(cx);
        cx.notify();
    }

    /// The bar, while a dialog waits on Nohrs.
    pub(crate) fn render_picking_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bar = self.picking_bar.as_ref()?;
        let type_label = match &bar.typed {
            Some(patterns) => patterns.clone(),
            None => bar
                .types
                .get(bar.chosen)
                .map(|kind| kind.label.clone())
                .unwrap_or_else(|| t!("Todos os arquivos").to_string()),
        };
        let menu = bar.menu_open.then(|| {
            let items: Vec<AnyElement> = if bar.types.is_empty() {
                vec![type_item(0, t!("Todos os arquivos").to_string(), true, cx)]
            } else {
                bar.types
                    .iter()
                    .enumerate()
                    .map(|(index, kind)| {
                        type_item(
                            index,
                            kind.label.clone(),
                            bar.typed.is_none() && index == bar.chosen,
                            cx,
                        )
                    })
                    .collect()
            };
            deferred(
                div()
                    .id("picking-type-menu")
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(36.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .bg(rgb(theme::bg()))
                    .shadow_lg()
                    .occlude()
                    .children(items),
            )
            .with_priority(2)
        });
        let element = div()
            .id("picking-bar")
            .flex_shrink_0()
            .px(px(16.0))
            .pt(px(10.0))
            .pb(px(12.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .border_t_1()
            .border_color(rgb(theme::border()))
            .bg(rgb(theme::toolbar_bg()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_sm()
                            .text_color(rgb(theme::fg_secondary()))
                            .child(format!("{}:", t!("Nome"))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .h(px(32.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(rgb(theme::border()))
                            .bg(rgb(theme::bg()))
                            .text_sm()
                            .child(Input::new(&bar.name).appearance(false)),
                    )
                    .child(
                        div()
                            .relative()
                            .w(px(260.0))
                            .flex_shrink_0()
                            .child(
                                div()
                                    .id("picking-type")
                                    .h(px(32.0))
                                    .px(px(10.0))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(6.0))
                                    .rounded(px(6.0))
                                    .border_1()
                                    .border_color(rgb(theme::border()))
                                    .bg(rgb(theme::bg()))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(theme::bg_hover())))
                                    .on_click(cx.listener(|root, _, _, cx| {
                                        if let Some(bar) = root.picking_bar.as_mut() {
                                            bar.menu_open = !bar.menu_open;
                                        }
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .text_sm()
                                            .text_color(rgb(theme::fg()))
                                            .child(type_label),
                                    )
                                    .child(
                                        Icon::new(IconName::ChevronDown)
                                            .size_3()
                                            .text_color(rgb(theme::muted())),
                                    ),
                            )
                            .children(menu),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    // The whole computer, as Listary's bar under the dialog:
                    // the Ctrl+T box, where a file chosen is sent at once.
                    .child(
                        div()
                            .id("picking-search")
                            .flex_1()
                            .max_w(px(420.0))
                            .h(px(30.0))
                            .px(px(10.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(rgb(theme::border()))
                            .bg(rgb(theme::bg()))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(theme::bg_hover())))
                            .on_click(cx.listener(|root, _, window, cx| {
                                root.explorer_entity().update(cx, |page, cx| {
                                    if page.palette.is_none() {
                                        page.toggle_palette(window, cx);
                                    }
                                });
                            }))
                            .child(
                                Icon::new(IconName::Search)
                                    .size_4()
                                    .text_color(rgb(theme::muted())),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_sm()
                                    .text_color(rgb(theme::muted()))
                                    .child(t!("Buscar no computador…")),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(rgb(theme::muted()))
                                    .child("Ctrl+T"),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        bar_button("picking-open", t!("Abrir").to_string(), true).on_click(
                            cx.listener(|root, _, window, cx| root.commit_picking(window, cx)),
                        ),
                    )
                    .child(
                        bar_button("picking-cancel", t!("Cancelar").to_string(), false).on_click(
                            cx.listener(|_root, _, _, cx| {
                                file_dialog::cancel();
                                cx.notify();
                            }),
                        ),
                    ),
            );
        Some(element.into_any_element())
    }
}

fn type_item(index: usize, label: String, chosen: bool, cx: &mut Context<RootView>) -> AnyElement {
    div()
        .id(("picking-type-item", index))
        .px(px(10.0))
        .py(px(6.0))
        .text_sm()
        .text_color(rgb(theme::fg()))
        .cursor_pointer()
        .when(chosen, |item| item.bg(rgb(theme::accent_subtle())))
        .hover(|style| style.bg(rgb(theme::bg_hover())))
        .on_click(cx.listener(move |root, _, _, cx| root.choose_file_type(index, cx)))
        .child(label)
        .into_any_element()
}

fn bar_button(id: &'static str, label: String, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(30.0))
        .min_w(px(96.0))
        .px(px(14.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .text_sm()
        .cursor_pointer()
        .when(primary, |button| {
            button
                .bg(rgb(theme::accent()))
                .text_color(rgb(theme::accent_fg()))
                .hover(|style| style.bg(rgb(theme::accent_hover())))
        })
        .when(!primary, |button| {
            button
                .border_1()
                .border_color(rgb(theme::border()))
                .text_color(rgb(theme::fg()))
                .hover(|style| style.bg(rgb(theme::bg_hover())))
        })
        .child(label)
}

// The Name box's text for the selected files: one name, or several in quotes,
// as the Windows dialog writes them.
fn names_text(files: &[String]) -> String {
    let name = |path: &String| {
        Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone())
    };
    match files {
        [single] => name(single),
        many => many
            .iter()
            .map(|path| format!("\"{}\"", name(path)))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

// What the Name box asks for. Empty sends the selection; `*` or `?` filters;
// a folder goes there; names (quoted when several) are read against `cwd`
// unless they are whole paths.
fn resolve(text: &str, cwd: &str, selected: &[String]) -> Typed {
    if text.is_empty() {
        return if selected.is_empty() {
            Typed::Nothing
        } else {
            Typed::Send(selected.to_vec())
        };
    }
    if text.contains('*') || text.contains('?') {
        return Typed::Patterns(text.to_string());
    }
    let names: Vec<String> = if text.contains('"') {
        text.split('"')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        vec![text.to_string()]
    };
    let mut paths = Vec::with_capacity(names.len());
    for name in &names {
        let path = if Path::new(name).is_absolute() {
            PathBuf::from(name)
        } else {
            Path::new(cwd).join(name)
        };
        if names.len() == 1 && path.is_dir() {
            return Typed::Folder(path.to_string_lossy().into_owned());
        }
        if !path.is_file() {
            return Typed::Missing(name.clone());
        }
        paths.push(path.to_string_lossy().into_owned());
    }
    Typed::Send(paths)
}

#[cfg(test)]
mod tests {
    use super::{Typed, names_text, resolve};

    #[test]
    fn the_name_box_reads_as_the_windows_dialog_does() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        std::fs::write(dir.path().join("a.png"), b"x").unwrap();
        std::fs::write(dir.path().join("b c.png"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let a = dir.path().join("a.png").to_string_lossy().into_owned();
        let b = dir.path().join("b c.png").to_string_lossy().into_owned();

        assert_eq!(resolve("", &cwd, &[]), Typed::Nothing);
        assert_eq!(
            resolve("", &cwd, std::slice::from_ref(&a)),
            Typed::Send(vec![a.clone()])
        );
        assert_eq!(resolve("a.png", &cwd, &[]), Typed::Send(vec![a.clone()]));
        assert_eq!(
            resolve(&a, "C:\\elsewhere", &[]),
            Typed::Send(vec![a.clone()])
        );
        assert_eq!(
            resolve("\"a.png\" \"b c.png\"", &cwd, &[]),
            Typed::Send(vec![a.clone(), b.clone()])
        );
        assert_eq!(
            resolve("sub", &cwd, &[]),
            Typed::Folder(dir.path().join("sub").to_string_lossy().into_owned())
        );
        assert_eq!(resolve("*.png", &cwd, &[]), Typed::Patterns("*.png".into()));
        assert_eq!(
            resolve("nada.txt", &cwd, &[]),
            Typed::Missing("nada.txt".into())
        );
        assert_eq!(names_text(&[a.clone()]), "a.png");
        assert_eq!(names_text(&[a, b]), "\"a.png\" \"b c.png\"");
    }
}
