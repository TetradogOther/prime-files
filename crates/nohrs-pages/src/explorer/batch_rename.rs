//! Renaming several items at once: F2 (or Rename in the menu) with more than
//! one item selected. A dialog with two ways of naming them — one name with a
//! number after it (`Foto (1).jpg`, `Foto (2).jpg`, as the Explorer does), or
//! a piece of text traded for another in every name — and a list showing
//! each old name next to the new one before anything is touched.
//!
//! The whole batch is one step of Ctrl+Z.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::Sizable as _;
use gpui_component::WindowExt as _;
use gpui_component::dialog::DialogButtonProps;
use gpui_component::input::{Input, InputState};
use nohrs_core::i18n;
use nohrs_core::telemetry::LogErr as _;
use nohrs_core::{k, t, tf};
use nohrs_services::fs::ops;
use nohrs_ui::theme::theme;

use super::ExplorerPane;

/// How the new names are made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Rule {
    /// `base (n).ext`, counting from `start` in the order the items are
    /// listed; each keeps its own extension.
    Numbered { base: String, start: usize },
    /// Every `find` in the name (capitals ignored) becomes `with`.
    Replace { find: String, with: String },
}

/// The new name of each item, in order. `items` are (name, is a folder).
pub(crate) fn new_names(items: &[(String, bool)], rule: &Rule) -> Vec<String> {
    match rule {
        Rule::Numbered { base, start } => {
            let base = base.trim();
            items
                .iter()
                .enumerate()
                .map(|(ix, (name, is_dir))| {
                    if base.is_empty() {
                        return name.clone();
                    }
                    let extension = if *is_dir { "" } else { extension_of(name) };
                    format!("{base} ({}){extension}", start + ix)
                })
                .collect()
        }
        Rule::Replace { find, with } => items
            .iter()
            .map(|(name, _)| {
                if find.is_empty() {
                    name.clone()
                } else {
                    replace_ignoring_case(name, find, with)
                }
            })
            .collect(),
    }
}

// `.jpg` of `foto.jpg`; nothing for `README` or `.gitignore`, whose dot
// starts the name.
fn extension_of(name: &str) -> &str {
    match name.rfind('.') {
        Some(0) | None => "",
        Some(dot) => &name[dot..],
    }
}

// Every `find` in `text`, compared without capitals, traded for `with`.
fn replace_ignoring_case(text: &str, find: &str, with: &str) -> String {
    let lower_text = text.to_lowercase();
    let lower_find = find.to_lowercase();
    // Lowercasing can change a character's length (rare, outside Latin);
    // then the positions would not line up, so compare as written.
    if lower_text.len() != text.len() || lower_find.len() != find.len() {
        return text.replace(find, with);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = 0;
    while let Some(found) = lower_text[rest..].find(&lower_find) {
        let at = rest + found;
        out.push_str(&text[rest..at]);
        out.push_str(with);
        rest = at + find.len();
    }
    out.push_str(&text[rest..]);
    out
}

/// What renaming `pairs` (path, new name) would hit before touching
/// anything: a name used twice in the batch, or one an item outside the
/// batch already has. Each item refused, with why; the rest can go.
pub(crate) fn refused(pairs: &[(PathBuf, String)]) -> Vec<(PathBuf, String)> {
    let key = |path: &Path| path.to_string_lossy().to_lowercase();
    let sources: HashSet<String> = pairs.iter().map(|(path, _)| key(path)).collect();
    let mut taken = HashSet::new();
    let mut refused = Vec::new();
    for (path, name) in pairs {
        let Some(parent) = path.parent() else {
            refused.push((path.clone(), t!("sem pasta de origem").to_string()));
            continue;
        };
        let target = parent.join(name);
        let target_key = key(&target);
        if !taken.insert(target_key.clone()) {
            refused.push((path.clone(), tf!("“{name}” sairia duas vezes", name = name)));
            continue;
        }
        // Another item of the batch leaves that name free; the item itself
        // (a change of capitals only) is not in its own way.
        if target_key != key(path) && !sources.contains(&target_key) && ops::would_conflict(&target)
        {
            refused.push((
                path.clone(),
                tf!("“{name}” já existe nesta pasta", name = name),
            ));
        }
    }
    refused
}

/// Renames each (path, new name) and says which went, as (old, new) paths.
/// Names that other items of the batch still hold are freed first through a
/// temporary name, so `a → b, b → a` works.
pub(crate) fn rename_all(
    pairs: Vec<(PathBuf, String)>,
) -> (super::file_ops::OpReport, Vec<(PathBuf, PathBuf)>) {
    let mut report = super::file_ops::OpReport::default();
    let refused = refused(&pairs);
    for (path, why) in &refused {
        report.failed(format!("{}: {why}", path.display()));
    }
    let refused: HashSet<&PathBuf> = refused.iter().map(|(path, _)| path).collect();
    let pairs: Vec<(PathBuf, String)> = pairs
        .into_iter()
        .filter(|(path, name)| {
            !refused.contains(path) && path.file_name().is_some_and(|old| old != name.as_str())
        })
        .collect();

    let key = |path: &Path| path.to_string_lossy().to_lowercase();
    let sources: HashSet<String> = pairs.iter().map(|(path, _)| key(path)).collect();
    // Where each item is now: its own path, or the temporary name it was
    // moved out of the way to.
    let mut now: Vec<Option<PathBuf>> = Vec::with_capacity(pairs.len());
    for (ix, (path, name)) in pairs.iter().enumerate() {
        let target = path.parent().map(|parent| parent.join(name));
        let blocked_by_batch = target
            .as_ref()
            .is_some_and(|target| key(target) != key(path) && sources.contains(&key(target)));
        if !blocked_by_batch {
            now.push(Some(path.clone()));
            continue;
        }
        let temporary = format!(".prime-files-renomeando-{}-{ix}", std::process::id());
        match ops::rename_in_place(path, &temporary) {
            Ok(moved) => now.push(Some(moved)),
            Err(error) => {
                report.failed(format!("{}: {error}", path.display()));
                now.push(None);
            }
        }
    }
    let mut done = Vec::new();
    for ((path, name), current) in pairs.into_iter().zip(now) {
        let Some(current) = current else { continue };
        match ops::rename_in_place(&current, &name) {
            Ok(renamed) => done.push((path, renamed)),
            Err(error) => {
                report.failed(format!("{}: {error}", path.display()));
                // Out of the temporary name, at least: back to its own.
                if current != path
                    && let Some(old) = path.file_name().and_then(|old| old.to_str())
                {
                    ops::rename_in_place(&current, old).ok();
                }
            }
        }
    }
    (report, done)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Numbered,
    Replace,
}

/// The dialog's body: the two ways, their fields and the list of names.
pub(crate) struct BatchRenameForm {
    items: Vec<(PathBuf, String, bool)>,
    mode: Mode,
    base: Entity<InputState>,
    find: Entity<InputState>,
    with: Entity<InputState>,
    _observers: Vec<Subscription>,
}

impl BatchRenameForm {
    fn new(
        items: Vec<(PathBuf, String, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The first item's name, without its extension, to start from.
        let first = items
            .first()
            .map(|(_, name, is_dir)| {
                if *is_dir {
                    name.clone()
                } else {
                    name[..name.len() - extension_of(name).len()].to_string()
                }
            })
            .unwrap_or_default();
        let base = cx.new(|cx| InputState::new(window, cx).default_value(first));
        let find = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Texto a procurar")));
        let with =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("Trocar por (vazio apaga)")));
        let observers = [&base, &find, &with]
            .into_iter()
            .map(|input| cx.observe(input, |_, _, cx| cx.notify()))
            .collect();
        base.update(cx, |state, cx| state.focus(window, cx));
        Self {
            items,
            mode: Mode::Numbered,
            base,
            find,
            with,
            _observers: observers,
        }
    }

    fn rule(&self, cx: &App) -> Rule {
        match self.mode {
            Mode::Numbered => Rule::Numbered {
                base: self.base.read(cx).value().to_string(),
                start: 1,
            },
            Mode::Replace => Rule::Replace {
                find: self.find.read(cx).value().to_string(),
                with: self.with.read(cx).value().to_string(),
            },
        }
    }

    /// Each item and its new name.
    pub(crate) fn plan(&self, cx: &App) -> Vec<(PathBuf, String)> {
        let names: Vec<(String, bool)> = self
            .items
            .iter()
            .map(|(_, name, is_dir)| (name.clone(), *is_dir))
            .collect();
        let new = new_names(&names, &self.rule(cx));
        self.items
            .iter()
            .map(|(path, _, _)| path.clone())
            .zip(new)
            .collect()
    }

    fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = mode;
        let field = if mode == Mode::Numbered {
            &self.base
        } else {
            &self.find
        };
        field.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }
}

const SHOWN: usize = 8;

impl Render for BatchRenameForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let plan = self.plan(cx);
        let refused: HashSet<PathBuf> = refused(&plan).into_iter().map(|(path, _)| path).collect();
        let mode_chip = |id: &'static str,
                         label: &'static str,
                         mode: Mode,
                         cx: &mut Context<Self>| {
            let on = self.mode == mode;
            div()
                .id(id)
                .h(px(28.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .rounded_full()
                .border_1()
                .text_sm()
                .map(|this| {
                    if on {
                        this.bg(rgb(theme::accent()))
                            .border_color(rgb(theme::accent()))
                            .text_color(rgb(theme::accent_fg()))
                    } else {
                        this.cursor_pointer()
                            .border_color(rgb(theme::border_hover()))
                            .text_color(rgb(theme::fg()))
                            .hover(|style| style.bg(rgb(theme::bg_hover())))
                    }
                })
                .on_click(cx.listener(move |form, _, window, cx| form.set_mode(mode, window, cx)))
                .child(label)
        };
        let field = |label: &'static str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme::fg_secondary()))
                        .child(label),
                )
                .child(Input::new(input).small())
        };
        let hidden = plan.len().saturating_sub(SHOWN);
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(mode_chip(
                        "batch-rename-numbered",
                        t!("Nome e número"),
                        Mode::Numbered,
                        cx,
                    ))
                    .child(mode_chip(
                        "batch-rename-replace",
                        t!("Trocar texto"),
                        Mode::Replace,
                        cx,
                    )),
            )
            .map(|this| match self.mode {
                Mode::Numbered => this.child(field(t!("Novo nome"), &self.base)).child(
                    div().text_xs().text_color(rgb(theme::muted())).child(t!(
                        "Cada item fica “Nome (1)”, “Nome (2)”…, com a extensão que já tinha."
                    )),
                ),
                Mode::Replace => this
                    .child(field(t!("Procurar"), &self.find))
                    .child(field(t!("Trocar por"), &self.with)),
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .p(px(8.0))
                    .rounded(px(6.0))
                    .bg(rgb(theme::bg_secondary()))
                    .children(plan.iter().take(SHOWN).zip(&self.items).map(
                        |((path, new), (_, old, _))| {
                            let changed = new != old;
                            let bad = refused.contains(path);
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .text_xs()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_color(rgb(theme::fg_secondary()))
                                        .child(old.clone()),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_color(rgb(theme::muted()))
                                        .child("→"),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_color(rgb(if bad {
                                            theme::danger()
                                        } else if changed {
                                            theme::accent()
                                        } else {
                                            theme::muted()
                                        }))
                                        .child(new.clone()),
                                )
                        },
                    ))
                    .when(hidden > 0, |this| {
                        this.child(
                            div()
                                .pt(px(2.0))
                                .text_xs()
                                .text_color(rgb(theme::muted()))
                                .child(i18n::plural(
                                    hidden,
                                    k!("e mais {n} item"),
                                    k!("e mais {n} itens"),
                                )),
                        )
                    }),
            )
            .when(!refused.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme::danger()))
                        .child(i18n::plural(
                            refused.len(),
                            k!("{n} item fica com o nome antigo: o novo nome já está em uso"),
                            k!("{n} itens ficam com o nome antigo: o novo nome já está em uso"),
                        )),
                )
            })
    }
}

impl ExplorerPane {
    /// F2 or Rename: the field on the row for one item, the dialog for a
    /// selection of several that includes `row`.
    pub(crate) fn rename_at(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.selected_paths();
        let row_selected = self
            .filtered_entries
            .get(row)
            .is_some_and(|entry| self.selection.contains(&entry.path));
        if selected.len() > 1 && row_selected {
            self.open_batch_rename(window, cx);
        } else {
            self.begin_rename(row, window, cx);
        }
    }

    /// Opens the dialog that renames the whole selection.
    pub(crate) fn open_batch_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items: Vec<(PathBuf, String, bool)> = self
            .filtered_entries
            .iter()
            .filter(|entry| self.selection.contains(&entry.path))
            .map(|entry| {
                (
                    PathBuf::from(&entry.path),
                    entry.name.clone(),
                    entry.kind == "dir",
                )
            })
            .collect();
        if items.len() < 2 {
            return;
        }
        let count = items.len();
        let form = cx.new(|cx| BatchRenameForm::new(items, window, cx));
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let form = form.clone();
            let weak = weak.clone();
            dialog
                .title(tf!("Renomear {count} itens", count = count))
                .w(px(520.0))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("Renomear"))
                        .cancel_text(t!("Cancelar")),
                )
                .child(form.clone())
                .on_ok(move |_, _window, cx| {
                    let plan = form.read(cx).plan(cx);
                    weak.update(cx, |pane, cx| pane.rename_batch(plan, cx))
                        .log_err();
                    true
                })
        });
    }

    /// Renames each (path, new name) off the UI thread, as one step of Ctrl+Z.
    pub(crate) fn rename_batch(&mut self, plan: Vec<(PathBuf, String)>, cx: &mut Context<Self>) {
        let total = plan.len();
        let label = i18n::plural(total, k!("{n} item renomeado"), k!("{n} itens renomeados"));
        self.run_fs_op_with(
            total,
            label,
            cx,
            move || rename_all(plan),
            |done, cx| {
                for (old, new) in &done {
                    super::tags::Tags::moved(&old.to_string_lossy(), &new.to_string_lossy(), cx);
                }
                super::file_ops::remember_renames(done.clone());
                done.into_iter().map(|(_, new)| new).collect()
            },
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::disallowed_methods)]
mod tests {
    // Not a glob: it would bring gpui's own `test` macro over the standard one.
    use super::{Rule, new_names, refused, rename_all};

    fn items(names: &[&str]) -> Vec<(String, bool)> {
        names
            .iter()
            .map(|name| (name.to_string(), !name.contains('.')))
            .collect()
    }

    #[test]
    fn numbered_names_keep_each_extension() {
        let rule = Rule::Numbered {
            base: "Viagem".into(),
            start: 1,
        };
        assert_eq!(
            new_names(&items(&["a.jpg", "b.PNG", "pasta", ".gitignore"]), &rule),
            [
                "Viagem (1).jpg",
                "Viagem (2).PNG",
                "Viagem (3)",
                "Viagem (4)"
            ]
        );
    }

    #[test]
    fn an_empty_name_changes_nothing() {
        let rule = Rule::Numbered {
            base: "  ".into(),
            start: 1,
        };
        assert_eq!(new_names(&items(&["a.jpg"]), &rule), ["a.jpg"]);
    }

    #[test]
    fn replace_ignores_capitals_and_keeps_the_rest() {
        let rule = Rule::Replace {
            find: "img".into(),
            with: "Foto".into(),
        };
        assert_eq!(
            new_names(&items(&["IMG_01.jpg", "img-img.jpg", "outro.jpg"]), &rule),
            ["Foto_01.jpg", "Foto-Foto.jpg", "outro.jpg"]
        );
    }

    #[test]
    fn a_swap_inside_the_batch_goes_through() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "A").unwrap();
        std::fs::write(dir.path().join("b.txt"), "B").unwrap();
        let (report, done) = rename_all(vec![
            (dir.path().join("a.txt"), "b.txt".into()),
            (dir.path().join("b.txt"), "a.txt".into()),
        ]);
        assert!(report.failures().is_empty(), "{:?}", report.failures());
        assert_eq!(done.len(), 2);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "B"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "A"
        );
    }

    #[test]
    fn a_name_outside_the_batch_is_never_written_over() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "A").unwrap();
        std::fs::write(dir.path().join("b.txt"), "B").unwrap();
        std::fs::write(dir.path().join("fora.txt"), "FORA").unwrap();
        let (report, done) = rename_all(vec![
            (dir.path().join("a.txt"), "fora.txt".into()),
            (dir.path().join("b.txt"), "c.txt".into()),
        ]);
        assert!(!report.failures().is_empty());
        assert_eq!(done.len(), 1, "the other one still goes");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("fora.txt")).unwrap(),
            "FORA"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "A"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("c.txt")).unwrap(),
            "B"
        );
    }

    #[test]
    fn the_same_new_name_twice_is_refused_once() {
        let dir = tempfile::tempdir().unwrap();
        let pairs = vec![
            (dir.path().join("a.txt"), "x.txt".to_string()),
            (dir.path().join("b.txt"), "X.txt".to_string()),
        ];
        let refused = refused(&pairs);
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].0, dir.path().join("b.txt"));
    }
}
