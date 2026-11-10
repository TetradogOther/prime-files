//! The top of the settings' first section: what a person opening the
//! settings sees first. The app and its version with the update state, the
//! three choices most people come for (theme, accent, taking over from the
//! Explorer) as pictures rather than rows, and a tile for every other
//! section, saying what is in it. Below it the section's own rows follow,
//! as on every other section.

use std::sync::{Arc, LazyLock};

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use nohrs_core::config::ThemeMode;
use nohrs_core::i18n::tr;
use nohrs_core::{t, tf};
use nohrs_ui::theme::theme;

use crate::settings::{ACCENT_LABELS, SettingsPage};

// The app's own icon, the one of its window and installer.
static APP_ICON: LazyLock<Arc<Image>> = LazyLock::new(|| {
    Arc::new(Image::from_bytes(
        ImageFormat::Png,
        include_bytes!("../../../assets/icon/prime-files.png").to_vec(),
    ))
});

// The miniature windows of the theme choice, in fixed colours: they show
// what each theme looks like, not the theme on screen.
struct Mini {
    window: u32,
    bar: u32,
    side: u32,
    line: u32,
}

const DARK: Mini = Mini {
    window: 0x1B1C1F,
    bar: 0x26272B,
    side: 0x222327,
    line: 0x3A3B40,
};
const LIGHT: Mini = Mini {
    window: 0xF7F7F8,
    bar: 0xE9E9EC,
    side: 0xEFEFF2,
    line: 0xCFCFD5,
};

impl SettingsPage {
    /// The first section's top.
    pub(crate) fn render_home(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(22.0))
            .child(self.home_hero(cx))
            .child(heading(t!("Ajustes rápidos")))
            // Three columns or one under the other, picked from the width
            // (`home_wide`), never a wrapping row: with cards that wrap
            // inside (the colour dots), a wrapping row had every card
            // measured over and over each frame, about 5 ms of a scroll
            // frame that has 16 (07/11/2026).
            .child({
                let cards = [
                    self.theme_card(cx),
                    self.accent_card(cx),
                    self.explorer_card(cx),
                ];
                if self.home_wide() {
                    div()
                        .flex()
                        .gap(px(12.0))
                        .children(cards.map(|card| card.flex_1().min_w(px(0.0))))
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .children(cards.map(|card| card.w_full()))
                }
            })
    }

    // The app, its version, and where its updates stand.
    fn home_hero(&self, cx: &mut Context<Self>) -> Div {
        use crate::updater::Status;
        let status = self.updater_status(cx);
        let (pill, tone) = match &status {
            Status::NotInstalled => (
                Some(t!("Cópia portátil").to_string()),
                theme::fg_secondary(),
            ),
            Status::Idle => (None, theme::fg_secondary()),
            Status::Checking => (Some(t!("Procurando…").to_string()), theme::fg_secondary()),
            // Green on either theme: the palette has no colour for "fine".
            Status::UpToDate => (Some(t!("Atualizado").to_string()), 0x46A758),
            Status::Downloading(version) => (
                Some(tf!("Baixando a versão {version}…", version = version)),
                theme::accent(),
            ),
            Status::Ready { .. } => (Some(t!("Atualização pronta").to_string()), theme::accent()),
            Status::Failed(_) => (Some(t!("Falha ao atualizar").to_string()), theme::danger()),
        };
        let busy = matches!(status, Status::Checking | Status::Downloading(_));
        let can_check = !busy && status != Status::NotInstalled;
        let ready = matches!(status, Status::Ready { .. });
        div()
            .flex()
            .items_center()
            .gap(px(18.0))
            .p(px(20.0))
            .rounded(px(14.0))
            .bg(rgb(theme::bg_secondary()))
            .border_1()
            .border_color(rgb(theme::border()))
            .child(img(APP_ICON.clone()).size(px(60.0)).flex_shrink_0())
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme::fg()))
                            .child("Prime Files"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.0))
                            .text_sm()
                            .text_color(rgb(theme::fg_secondary()))
                            .child(tf!("Versão {version}", version = env!("CARGO_PKG_VERSION")))
                            .when_some(pill, |this, pill| {
                                this.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .px(px(8.0))
                                        .py(px(2.0))
                                        .rounded_full()
                                        .bg(rgb(theme::bg()))
                                        .border_1()
                                        .border_color(rgb(theme::border()))
                                        .text_xs()
                                        .text_color(rgb(theme::fg()))
                                        .child(div().size(px(7.0)).rounded_full().bg(rgb(tone)))
                                        .child(pill),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap(px(8.0))
                    .when(ready, |this| {
                        this.child(
                            primary_button("home-install-update", t!("Atualizar agora")).on_click(
                                cx.listener(|page, _event, _window, cx| page.install_update(cx)),
                            ),
                        )
                    })
                    .when(can_check && !ready, |this| {
                        this.child(
                            secondary_button("home-check-updates", t!("Procurar atualizações"))
                                .on_click(
                                    cx.listener(|page, _event, _window, cx| page.check_updates(cx)),
                                ),
                        )
                    }),
            )
    }

    // Dark, light or as Windows, each a miniature window.
    fn theme_card(&self, cx: &mut Context<Self>) -> Div {
        let mode = self.config().theme.mode;
        card(t!("Tema"), None).child(
            div()
                .flex()
                .gap(px(10.0))
                .child(self.theme_tile("home-theme-dark", t!("Escuro"), ThemeMode::Dark, mode, cx))
                .child(self.theme_tile("home-theme-light", t!("Claro"), ThemeMode::Light, mode, cx))
                .child(self.theme_tile(
                    "home-theme-system",
                    t!("Igual ao Windows"),
                    ThemeMode::System,
                    mode,
                    cx,
                )),
        )
    }

    fn theme_tile(
        &self,
        id: &'static str,
        label: &'static str,
        tile: ThemeMode,
        current: ThemeMode,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let selected = tile == current;
        let value = match tile {
            ThemeMode::Dark => "\"dark\"",
            ThemeMode::Light => "\"light\"",
            ThemeMode::System => "\"system\"",
        };
        let picture = match tile {
            ThemeMode::Dark => mini_window(&DARK).into_any_element(),
            ThemeMode::Light => mini_window(&LIGHT).into_any_element(),
            // One window, its left half dark and its right half light:
            // whichever Windows is set to.
            ThemeMode::System => div()
                .size_full()
                .relative()
                .child(mini_window(&DARK))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .w(relative(0.5))
                        .h_full()
                        .overflow_hidden()
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .right_0()
                                .w(relative(2.0))
                                .h_full()
                                .child(mini_window(&LIGHT)),
                        ),
                )
                .into_any_element(),
        };
        div()
            .id(id)
            .flex_1()
            .min_w(px(72.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .cursor_pointer()
            .on_click(cx.listener(move |page, _event, _window, cx| {
                page.write_config("theme", "mode", value, cx)
            }))
            .child(
                div()
                    .h(px(64.0))
                    .w_full()
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .border_2()
                    .border_color(rgb(if selected {
                        theme::accent()
                    } else {
                        theme::border()
                    }))
                    .hover(|style| style.border_color(rgb(theme::accent())))
                    .child(picture),
            )
            .child(
                div()
                    .text_xs()
                    .text_center()
                    .text_color(rgb(if selected {
                        theme::fg()
                    } else {
                        theme::fg_secondary()
                    }))
                    .when(selected, |this| this.font_weight(FontWeight::MEDIUM))
                    .child(label),
            )
    }

    // The accent, as dots: the selected one ringed.
    fn accent_card(&self, cx: &mut Context<Self>) -> Div {
        let accent = self.config().theme.accent.trim().to_string();
        let dots =
            theme::NAMED_ACCENTS
                .iter()
                .zip(ACCENT_LABELS)
                .map(|(&(name, color), &label)| {
                    let selected = accent.eq_ignore_ascii_case(name);
                    let swatch = color.unwrap_or_else(theme::palette_accent);
                    let tip: SharedString = tr(label).into();
                    div()
                        .id(ElementId::Name(SharedString::from(format!(
                            "home-accent-{name}"
                        ))))
                        .size(px(30.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .border_2()
                        .border_color(rgb(if selected {
                            theme::fg()
                        } else {
                            theme::bg_secondary()
                        }))
                        .cursor_pointer()
                        .tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                        })
                        .on_click(cx.listener(move |page, _event, _window, cx| {
                            page.write_config("theme", "accent", &format!("\"{name}\""), cx);
                        }))
                        .child(div().size(px(22.0)).rounded_full().bg(rgb(swatch)))
                });
        card(
            t!("Cor de destaque"),
            Some(t!("Seleção, abas ativas e botões.")),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .children(dots)
                .child(self.custom_accent(true, cx)),
        )
    }

    // Taking folders over from Windows' Explorer, the one switch most
    // people install a file manager for.
    fn explorer_card(&self, cx: &mut Context<Self>) -> Div {
        card(
            t!("Explorador padrão"),
            Some(t!(
                "Pastas, Win+E e o ícone do Explorador na barra de tarefas abrem no Prime Files."
            )),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .child(self.default_explorer_switch(cx)),
        )
    }
}

// A group's name above it, as the section's own groups have.
fn heading(text: &'static str) -> Div {
    div()
        .px(px(4.0))
        .mb(px(-12.0))
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(theme::fg_secondary()))
        .child(text)
}

// One of the quick choices: a titled card that grows to share the row.
fn card(title: &'static str, hint: Option<&'static str>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .p(px(16.0))
        .rounded(px(12.0))
        .bg(rgb(theme::bg_secondary()))
        .border_1()
        .border_color(rgb(theme::border()))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(theme::fg()))
                        .child(title),
                )
                .when_some(hint, |this, hint| {
                    this.child(
                        div()
                            .text_xs()
                            .line_height(px(17.0))
                            .text_color(rgb(theme::fg_secondary()))
                            .child(hint),
                    )
                }),
        )
}

// A window drawn small: title bar, a sidebar and a few rows, one lit in the
// accent as a selection is.
fn mini_window(colors: &Mini) -> Div {
    let line = |width: f32, lit: bool| {
        div()
            .h(px(5.0))
            .w(relative(width))
            .rounded(px(2.0))
            .bg(rgb(if lit { theme::accent() } else { colors.line }))
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgb(colors.window))
        .child(div().h(px(10.0)).w_full().bg(rgb(colors.bar)))
        .child(
            div()
                .flex_1()
                .flex()
                .child(div().w(px(18.0)).h_full().bg(rgb(colors.side)))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .p(px(7.0))
                        .child(line(0.8, false))
                        .child(line(0.6, true))
                        .child(line(0.7, false)),
                ),
        )
}

fn primary_button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(14.0))
        .py(px(7.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .bg(rgb(theme::accent()))
        .text_color(rgb(theme::accent_fg()))
        .hover(|style| style.opacity(0.9))
        .child(label)
}

fn secondary_button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(14.0))
        .py(px(7.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .text_sm()
        .bg(rgb(theme::bg()))
        .border_1()
        .border_color(rgb(theme::border()))
        .text_color(rgb(theme::fg()))
        .hover(|style| style.bg(rgb(theme::bg_hover())))
        .child(label)
}
