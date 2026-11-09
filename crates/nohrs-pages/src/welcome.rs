//! The welcome screen: the first thing Prime Files shows, and what Settings ›
//! About opens again. Seven short steps, named in a bar along the top so the
//! person sees where they are and can jump back: appearance; how the window
//! is laid out (panels, compact mode); what a folder shows (order, hidden
//! items, tabs, the menu); sound; the parts of Windows it takes over; search;
//! and a summary of all of it.
//!
//! The steps that change how the window looks have a small Prime Files beside
//! them — the same previews the settings page draws — so a choice shows before
//! the person reaches the real window. On a narrow window it sits above the
//! choices instead.
//!
//! Like the settings page it never applies anything itself: choices go to
//! `config.toml` or the registry, and the root shows them back. Finishing (or
//! skipping) writes `ui.welcome_done = true`, which is what takes the screen
//! away; Settings sets it back to false to bring it again.

use gpui::{
    AnyElement, Context, Div, ElementId, FocusHandle, IntoElement, KeyDownEvent, Render, ScrollHandle, SharedString,
    Stateful, Window, div, point, prelude::*, px, rgb,
};
use gpui_component::Icon;
use nohrs_core::config::{self, Config, DockSide, OpenFromOtherApps, SortOrder, ThemeMode};
use nohrs_core::i18n::{self, Lang};
use nohrs_core::telemetry::LogErr;
use nohrs_core::{t, tf};
use nohrs_ui::theme::theme;

use crate::explorer::everything::{Everything, mft};
use crate::settings::preview::{self, CompactDemo, HoverDemo};

// Window widths where the layout changes: the preview moves into a column
// of its own, and the step bar names every step.
const WIDE: f32 = 980.0;
const NAMED_STEPS: f32 = 900.0;
const NARROW: f32 = 600.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Look,
    Window,
    Files,
    Sound,
    System,
    Search,
    Done,
}

const STEPS: [Step; 7] = [Step::Look, Step::Window, Step::Files, Step::Sound, Step::System, Step::Search, Step::Done];

/// What the step of the system's own parts is called: on Windows it is
/// Windows, on the other systems it is the desktop.
#[cfg(windows)]
const SYSTEM_LABEL: &str = "Windows";
#[cfg(not(windows))]
const SYSTEM_LABEL: &str = "Sistema";

impl Step {
    fn label(self) -> &'static str {
        match self {
            Self::Look => t!("Aparência"),
            Self::Window => t!("Janela"),
            Self::Files => t!("Pastas"),
            Self::Sound => t!("Mídia"),
            Self::System => SYSTEM_LABEL,
            Self::Search => t!("Busca"),
            Self::Done => t!("Pronto"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Look => "icons/palette.svg",
            Self::Window => "icons/layout-dashboard.svg",
            Self::Files => "icons/folder-open.svg",
            Self::Sound => "icons/music.svg",
            Self::System => "icons/app-window.svg",
            Self::Search => "icons/search.svg",
            Self::Done => "icons/circle-check.svg",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Look => t!("Bem-vindo ao Prime Files"),
            Self::Window => t!("Como a janela se organiza"),
            Self::Files => t!("O que cada pasta mostra"),
            Self::Sound => t!("Som e vídeo"),
            Self::System => t!("O seu explorador padrão"),
            Self::Search => t!("Achar qualquer arquivo"),
            Self::Done => t!("Tudo pronto"),
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::Look => t!("Um explorador de arquivos rápido para o Windows. Vamos deixá-lo do seu jeito em menos de um minuto: cada escolha vale na hora, e a janela de exemplo mostra como fica."),
            Self::Window => t!("Onde ficam a barra lateral e o visualizador, e se as barras somem para deixar só os arquivos na tela."),
            Self::Files => t!("A ordem dos arquivos, os itens ocultos, as abas e o botão direito."),
            Self::Sound => t!("Ouça um som sem abrir: deixe o mouse parado em cima dele."),
            Self::System => t!("Escolha se as pastas do Windows abrem aqui. Dá para desfazer a qualquer momento, e nada muda para os outros usuários do computador."),
            Self::Search => t!("Um atalho que abre a busca de qualquer programa, e um índice do computador inteiro."),
            Self::Done => t!("Você pode mudar qualquer uma destas escolhas depois, em Configurações."),
        }
    }

    // Whether the step draws a window of example beside its choices.
    fn has_preview(self) -> bool {
        !matches!(self, Self::Done)
    }
}

/// A line of the summaries: a choice and what it is now.
enum Value {
    // A switch; `None` while the system has not answered, or for something
    // that is not installed.
    Switch(Option<bool>),
    Text(String),
    NotInstalled,
}

/// The welcome screen.
pub struct WelcomePage {
    step: Step,
    config: Config,
    // `None` until the registry or Listary's file has answered.
    default_explorer: Option<bool>,
    starts_with_windows: Option<bool>,
    listary: Option<crate::listary::State>,
    // The last change that failed, shown above the buttons.
    error: Option<String>,
    // A change under way (Listary restarts, the registry is written).
    busy: bool,
    // The previews that follow the pointer.
    compact_demo: CompactDemo,
    hover_demo: HoverDemo,
    // The choices' column, sent back to the top on each step.
    scroll: ScrollHandle,
    // Waiting for a key combination for the quick search, as Settings does;
    // the page takes the keyboard while it waits.
    recording_quick: bool,
    focus: FocusHandle,
}

impl WelcomePage {
    /// A welcome screen on its first step, reading what the system has now.
    pub fn new(config: Config, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            step: Step::Look,
            config,
            default_explorer: None,
            starts_with_windows: None,
            listary: None,
            error: None,
            busy: false,
            compact_demo: CompactDemo::default(),
            hover_demo: HoverDemo::default(),
            scroll: ScrollHandle::new(),
            recording_quick: false,
            focus: cx.focus_handle(),
        };
        page.refresh(cx);
        page
    }

    /// Takes the config the window runs on, so the choices show it.
    pub fn set_config(&mut self, config: Config, cx: &mut Context<Self>) {
        self.config = config;
        cx.notify();
    }

    // Asks the registry and Listary, off the UI thread.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let read = cx.background_spawn(async {
            (
                crate::default_explorer::is_default(),
                crate::default_explorer::starts_with_windows(),
                crate::listary::state(),
            )
        });
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            let (is_default, starts, listary) = read.await;
            this.update(cx, |page, cx| {
                page.default_explorer = Some(is_default);
                page.starts_with_windows = Some(starts);
                page.listary = Some(listary);
                page.busy = false;
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    fn write(&mut self, section: &'static str, key: &'static str, value: String, cx: &mut Context<Self>) {
        let path = config::paths::config_file();
        let write = cx.background_spawn(async move { config::set_value(&path, section, key, &value) });
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            if let Err(error) = write.await {
                this.update(cx, |page, cx| {
                    page.error = Some(tf!("Não foi possível salvar {section}.{key}: {error}", section = section, key = key, error = error));
                    cx.notify();
                })
                .log_err();
            }
        })
        .detach();
    }

    // Runs a system change (registry, Listary) in the background, then reads
    // everything again so the switches show what really happened.
    fn change(&mut self, run: impl FnOnce(&std::path::Path) -> std::io::Result<()> + Send + 'static, cx: &mut Context<Self>) {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(error) => {
                self.error = Some(tf!("Não foi possível achar o executável do Prime Files: {error}", error = error));
                cx.notify();
                return;
            }
        };
        self.busy = true;
        self.error = None;
        cx.notify();
        let task = cx.background_spawn(async move { run(&exe) });
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            let outcome = task.await;
            this.update(cx, |page, cx| {
                if let Err(error) = outcome {
                    page.error = Some(error.to_string());
                }
                page.refresh(cx);
            })
            .log_err();
        })
        .detach();
    }

    fn go_to(&mut self, step: Step, cx: &mut Context<Self>) {
        // The sounds' tiles go away under the pointer without saying so.
        self.hover_demo.stop();
        self.recording_quick = false;
        self.step = step;
        self.scroll.set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        self.hover_demo.stop();
        self.write("ui", "welcome_done", "true".into(), cx);
    }

    // -----------------------------------------------------------------------
    // Controls

    // A switch of `config.toml`: flips `[section] key` from `on`.
    fn config_card(
        &self,
        id: &'static str,
        icon: &'static str,
        title: &'static str,
        text: &'static str,
        on: bool,
        (section, key): (&'static str, &'static str),
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        option_card(id, icon, title, text, Some(on), false, cx.listener(move |page, _, _, cx| {
            page.write(section, key, (!on).to_string(), cx);
        }))
    }

    // Exclusive options, each writing its TOML `value` to `[section] key`.
    fn choices(
        &self,
        id: &'static str,
        (section, key): (&'static str, &'static str),
        options: Vec<(SharedString, bool, String)>,
        cx: &mut Context<Self>,
    ) -> Div {
        div().w_full().flex().flex_wrap().gap(px(6.0)).children(options.into_iter().enumerate().map(
            |(ix, (label, selected, value))| {
                pill(ElementId::NamedInteger(id.into(), ix as u64), label, selected).when(!selected, |this| {
                    this.on_click(cx.listener(move |page, _, _, cx| page.write(section, key, value.clone(), cx)))
                })
            },
        ))
    }

    fn static_choices(
        &self,
        id: &'static str,
        target: (&'static str, &'static str),
        options: &[(&'static str, bool, &'static str)],
        cx: &mut Context<Self>,
    ) -> Div {
        let options =
            options.iter().map(|&(label, selected, value)| (SharedString::from(label), selected, value.to_string())).collect();
        self.choices(id, target, options, cx)
    }

    fn dock_choices(&self, id: &'static str, key: &'static str, side: DockSide, cx: &mut Context<Self>) -> Div {
        self.static_choices(
            id,
            ("layout", key),
            &[
                (t!("Esquerda"), side == DockSide::Left, "\"left\""),
                (t!("Direita"), side == DockSide::Right, "\"right\""),
                (t!("Em cima"), side == DockSide::Top, "\"top\""),
                (t!("Embaixo"), side == DockSide::Bottom, "\"bottom\""),
            ],
            cx,
        )
    }

    // -----------------------------------------------------------------------
    // Steps

    fn step_look(&self, cx: &mut Context<Self>) -> Div {
        let chosen = Lang::parse(&self.config.ui.language);
        let mut languages: Vec<(SharedString, bool, String)> =
            vec![(t!("Automático").into(), chosen.is_none(), "\"auto\"".to_string())];
        for lang in Lang::ALL {
            languages.push((lang.native_name().into(), chosen == Some(lang), format!("\"{}\"", lang.code())));
        }
        let mode = self.config.theme.mode;
        steps_column()
            .child(choice_card(
                "icons/case-sensitive.svg",
                t!("Idioma"),
                tf!(
                    "Automático segue o formato regional do Windows; neste computador, {language}.",
                    language = i18n::detected().native_name()
                ),
                self.choices("welcome-lang", ("ui", "language"), languages, cx),
            ))
            .child(choice_card(
                "icons/eye.svg",
                t!("Tema"),
                t!("Igual ao Windows troca sozinho quando o Windows troca.").to_string(),
                div()
                    .w_full()
                    .flex()
                    .gap(px(10.0))
                    .child(theme_tile("welcome-dark", t!("Escuro"), mode == ThemeMode::Dark, (0x1E1F22, 0x2A2C30, 0xDCDDE0), cx, "\"dark\""))
                    .child(theme_tile("welcome-light", t!("Claro"), mode == ThemeMode::Light, (0xFFFFFF, 0xF3F4F6, 0x1F2937), cx, "\"light\""))
                    .child(theme_tile("welcome-system", t!("Igual ao Windows"), mode == ThemeMode::System, (0x1E1F22, 0xF3F4F6, 0x9A9CA3), cx, "\"system\"")),
            ))
            .child(choice_card(
                "icons/palette.svg",
                t!("Cor de destaque"),
                t!("A cor da seleção, dos botões e das abas. Qualquer outra cor: em Configurações › Aparência.").to_string(),
                self.accents(cx),
            ))
    }

    // The named accents as swatches, as in Settings › Appearance; a custom
    // colour set there shows first, chosen.
    fn accents(&self, cx: &mut Context<Self>) -> Div {
        let accent = self.config.theme.accent.trim().to_string();
        let custom = accent.starts_with('#').then(|| theme::parse_accent(&accent)).flatten();
        div()
            .w_full()
            .flex()
            .flex_wrap()
            .gap(px(4.0))
            .children(custom.map(|color| swatch("welcome-accent-custom".into(), color, t!("Personalizada"), true)))
            .children(theme::NAMED_ACCENTS.iter().zip(crate::settings::ACCENT_LABELS).enumerate().map(
                |(index, (&(name, color), &label))| {
                    let selected = accent.eq_ignore_ascii_case(name);
                    swatch(
                        ElementId::NamedInteger("welcome-accent".into(), index as u64),
                        color.unwrap_or_else(theme::palette_accent),
                        i18n::tr(label),
                        selected,
                    )
                    .when(!selected, |this| {
                        this.on_click(cx.listener(move |page, _, _, cx| {
                            page.write("theme", "accent", format!("\"{name}\""), cx);
                        }))
                    })
                },
            ))
    }

    fn step_window(&self, cx: &mut Context<Self>) -> Div {
        let layout = &self.config.layout;
        let compact = &self.config.compact;
        steps_column()
            .child(group_label(t!("Painéis")))
            .child(choice_card(
                "icons/columns.svg",
                t!("Barra lateral"),
                t!("Acesso rápido, unidades e tags.").to_string(),
                self.dock_choices("welcome-dock-sidebar", "sidebar", layout.sidebar, cx),
            ))
            .child(choice_card(
                "icons/panel-right.svg",
                t!("Visualizador"),
                t!("Mostra o arquivo selecionado: imagem, vídeo, som, PDF, texto.").to_string(),
                self.dock_choices("welcome-dock-preview", "preview", layout.preview, cx),
            ))
            .child(self.config_card(
                "welcome-vertical-tabs",
                "icons/list.svg",
                t!("Abas verticais"),
                t!("As abas numa coluna à esquerda, como no Zen. Estreita, mostra só os ícones."),
                layout.vertical_tabs,
                ("layout", "vertical_tabs"),
                cx,
            ))
            .child(note(t!("Depois, também dá para puxar uma borda da lista e escolher onde abrir cada painel.")))
            .child(group_label(t!("Modo compacto")))
            .child(self.config_card(
                "welcome-compact",
                "icons/layout-dashboard.svg",
                t!("Modo compacto"),
                t!("Esconde as barras escolhidas, deixando só a lista; cada uma desliza para fora quando o mouse encosta na borda dela, como no Zen Browser."),
                compact.enabled,
                ("compact", "enabled"),
                cx,
            ))
            .when(compact.enabled, |this| {
                let toggles: Vec<(&'static str, bool, &'static str)> = vec![
                    (t!("Abas"), compact.hide_tabs, "hide_tabs"),
                    (t!("Barra de rota"), compact.hide_header, "hide_header"),
                    (t!("Barra lateral"), compact.hide_sidebar, "hide_sidebar"),
                    (t!("Visualizador"), compact.hide_preview, "hide_preview"),
                    (t!("Barra de baixo"), compact.hide_bottom_bar, "hide_bottom_bar"),
                ];
                this.child(choice_card(
                    "icons/eye.svg",
                    t!("O que esconder"),
                    t!("Somem até o mouse encostar na borda.").to_string(),
                    div().w_full().flex().flex_wrap().gap(px(6.0)).children(toggles.into_iter().enumerate().map(
                        |(ix, (label, on, key))| {
                            toggle_pill(ElementId::NamedInteger("welcome-compact-hide".into(), ix as u64), label, on)
                                .on_click(cx.listener(move |page, _, _, cx| page.write("compact", key, (!on).to_string(), cx)))
                        },
                    )),
                ))
                .child(choice_card(
                    "icons/layers.svg",
                    t!("Como as barras aparecem"),
                    t!("Por cima da lista ou empurrando ela.").to_string(),
                    self.static_choices(
                        "welcome-compact-overlay",
                        ("compact", "overlay"),
                        &[(t!("Por cima"), compact.overlay, "true"), (t!("Empurrando"), !compact.overlay, "false")],
                        cx,
                    ),
                ))
            })
    }

    fn step_files(&self, cx: &mut Context<Self>) -> Div {
        let ui = &self.config.ui;
        let explorer = &self.config.explorer;
        steps_column()
            .child(group_label(t!("Em cada pasta")))
            .child(choice_card(
                "icons/arrow-up-down.svg",
                t!("Ordenação padrão"),
                t!("Como cada pasta é ordenada ao abrir.").to_string(),
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(self.static_choices(
                        "welcome-sort",
                        ("ui", "default_sort"),
                        &[
                            (t!("Nome"), ui.default_sort == SortOrder::Name, "\"name\""),
                            (t!("Modificado"), ui.default_sort == SortOrder::Modified, "\"modified\""),
                            (t!("Tamanho"), ui.default_sort == SortOrder::Size, "\"size\""),
                            (t!("Tipo"), ui.default_sort == SortOrder::Kind, "\"kind\""),
                        ],
                        cx,
                    ))
                    .child(self.static_choices(
                        "welcome-sort-direction",
                        ("ui", "default_sort_descending"),
                        &[
                            (t!("Crescente"), !ui.default_sort_descending, "false"),
                            (t!("Decrescente"), ui.default_sort_descending, "true"),
                        ],
                        cx,
                    )),
            ))
            .child(self.config_card(
                "welcome-hidden",
                "icons/eye.svg",
                t!("Mostrar itens ocultos"),
                t!("Arquivos e pastas marcados como ocultos."),
                ui.show_hidden,
                ("ui", "show_hidden"),
                cx,
            ))
            .child(group_label(t!("Abas")))
            .child(self.config_card(
                "welcome-restore-tabs",
                "icons/refresh-cw.svg",
                t!("Reabrir as abas ao iniciar"),
                t!("Volta com as abas da última vez. Vale na próxima vez que o Prime Files abrir."),
                explorer.restore_tabs,
                ("explorer", "restore_tabs"),
                cx,
            ))
            .child({
                // Said the other way round from the key, as in Settings.
                let front = !explorer.background_tabs;
                option_card(
                    "welcome-front-tabs",
                    "icons/external-link.svg",
                    t!("Ir para a aba nova"),
                    t!("O botão do meio abre a pasta numa aba nova e já mostra ela. Desligado, a aba abre atrás da atual, como num navegador."),
                    Some(front),
                    false,
                    cx.listener(move |page, _, _, cx| page.write("explorer", "background_tabs", front.to_string(), cx)),
                )
            })
            .child(group_label(t!("Botão direito")))
            // The classic menu is the Explorer's own: on the other systems
            // there is no such menu to open.
            .when(cfg!(windows), |this| {
                this.child(self.config_card(
                    "welcome-classic-menu",
                    "icons/menu-square.svg",
                    t!("Menu clássico do Windows"),
                    t!("O botão direito abre direto o menu antigo do Windows, com Renomear, Copiar e Colar escritos, sem passar por \"Mostrar mais opções\"."),
                    self.config.menu.classic,
                    ("menu", "classic"),
                    cx,
                ))
            })
    }

    fn step_sound(&self, cx: &mut Context<Self>) -> Div {
        let ui = &self.config.ui;
        let delay = ui.hover_audio_delay_ms;
        steps_column()
            .child(self.config_card(
                "welcome-hover-audio",
                "icons/music.svg",
                t!("Tocar ao passar o mouse"),
                t!("Deixe o mouse parado sobre um arquivo de som na lista para ouvi-lo; tirar o mouse para. Um som por vez, no volume do visualizador."),
                ui.hover_audio,
                ("ui", "hover_audio"),
                cx,
            ))
            .when(ui.hover_audio, |this| {
                this.child(choice_card(
                    "icons/clock.svg",
                    t!("Tempo parado antes de tocar"),
                    t!("Curto toca enquanto você passa pela lista; longo só toca o arquivo em que você parou.").to_string(),
                    self.static_choices(
                        "welcome-hover-delay",
                        ("ui", "hover_audio_delay_ms"),
                        &[
                            (t!("Na hora"), delay == 0, "0"),
                            ("0,2 s", delay == 200, "200"),
                            ("0,4 s", delay == 400, "400"),
                            ("0,7 s", delay == 700, "700"),
                            ("1 s", delay == 1000, "1000"),
                            ("2 s", delay == 2000, "2000"),
                        ],
                        cx,
                    ),
                ))
            })
            .child(self.config_card(
                "welcome-autoplay",
                "icons/film.svg",
                t!("Tocar ao abrir no visualizador"),
                t!("Vídeos e sons começam a tocar assim que o visualizador os abre."),
                ui.preview_autoplay,
                ("ui", "preview_autoplay"),
                cx,
            ))
            .child(self.config_card(
                "welcome-waveforms",
                "icons/gallery-vertical-end.svg",
                t!("Forma de onda nos ícones"),
                t!("No modo Ícones, arquivos de som mostram a forma de onda no lugar do ícone."),
                ui.audio_waveforms,
                ("ui", "audio_waveforms"),
                cx,
            ))
    }

    fn step_system(&self, cx: &mut Context<Self>) -> Div {
        let is_default = self.default_explorer;
        let starts = self.starts_with_windows;
        let reveal = self.config.explorer.reveal_from_other_apps;
        let opens = self.config.explorer.open_from_other_apps;
        steps_column()
            .child(option_card(
                "welcome-default",
                "icons/folder-open.svg",
                t!("Usar o Prime Files como explorador padrão"),
                t!("Pastas, unidades, Win+E e o ícone do Explorador na barra de tarefas passam a abrir aqui."),
                is_default,
                self.busy,
                cx.listener(move |page, _, _, cx| {
                    let enable = !is_default.unwrap_or(false);
                    page.change(
                        move |exe| {
                            if enable {
                                crate::default_explorer::enable(exe)
                            } else {
                                crate::default_explorer::disable()
                            }
                        },
                        cx,
                    );
                }),
            ))
            .when(cfg!(windows), |this| {
                this.child(option_card(
                    "welcome-reveal",
                    "icons/film.svg",
                    t!("\"Mostrar na pasta\" abre aqui"),
                    t!("Premiere (Revelar no Explorador), Chrome, Edge e Firefox (Mostrar na pasta, depois de baixar) e o menu Iniciar (Abrir local do arquivo) abrem a pasta no Prime Files, com o arquivo selecionado e piscando em destaque."),
                    Some(reveal),
                    false,
                    cx.listener(move |page, _, _, cx| {
                        page.write("explorer", "reveal_from_other_apps", (!reveal).to_string(), cx);
                    }),
                ))
            })
            .child(choice_card(
                "icons/layers.svg",
                t!("Pastas abertas de outros programas"),
                t!("Onde abre uma pasta mandada pela Área de Trabalho, pelo Win+E ou por outro programa.").to_string(),
                self.static_choices(
                    "welcome-open-from",
                    ("explorer", "open_from_other_apps"),
                    &[
                        (t!("Nova aba"), opens == OpenFromOtherApps::NewTab, "\"new-tab\""),
                        (t!("Aba atual"), opens == OpenFromOtherApps::CurrentTab, "\"current-tab\""),
                        (t!("Nova janela"), opens == OpenFromOtherApps::NewWindow, "\"new-window\""),
                    ],
                    cx,
                ),
            ))
            .child(option_card(
                "welcome-autostart",
                "icons/clock.svg",
                t!("Iniciar com o Windows"),
                t!("Abre minimizado quando você entra no Windows, para que \"Mostrar na pasta\" de qualquer programa caia aqui na hora."),
                starts,
                self.busy,
                cx.listener(move |page, _, _, cx| {
                    let enable = !starts.unwrap_or(false);
                    page.change(move |exe| crate::default_explorer::set_starts_with_windows(exe, enable), cx);
                }),
            ))
            .when(cfg!(windows) && reveal && starts == Some(false), |this| {
                this.child(note(t!("\"Mostrar na pasta\" só chega aqui enquanto o Prime Files está aberto. Para valer sempre, ligue \"Iniciar com o Windows\".")))
            })
            .child(self.config_card(
                "welcome-updates",
                "icons/refresh-cw.svg",
                t!("Atualizar automaticamente"),
                t!("Procura uma versão nova ao abrir, baixa em segundo plano e instala quando você fecha o Prime Files."),
                self.config.updates.automatic,
                ("updates", "automatic"),
                cx,
            ))
    }

    fn step_search(&self, cx: &mut Context<Self>) -> Div {
        let mft_on = mft::state() == mft::State::On;
        let (mft_busy, mft_error) =
            cx.try_global::<Everything>().map_or((false, None), |state| (state.mft_busy, state.mft_error.clone()));
        let listary_card: AnyElement = match self.listary {
            Some(crate::listary::State::NotInstalled) => info_card(
                "icons/search.svg",
                "Listary",
                t!("O Listary não está instalado. Se você instalar depois, ligue em Configurações › Integração com o Windows."),
            )
            .into_any_element(),
            state => {
                let ours = state == Some(crate::listary::State::Ours);
                option_card(
                    "welcome-listary",
                    "icons/search.svg",
                    t!("Abrir as pastas do Listary aqui"),
                    t!("As pastas que você abre pelo Listary (Ctrl duas vezes) abrem no Prime Files. O Listary reinicia ao mudar."),
                    state.map(|_| ours),
                    self.busy,
                    cx.listener(move |page, _, _, cx| {
                        let enable = !ours;
                        page.change(
                            move |exe| {
                                if enable {
                                    crate::listary::enable(exe)
                                } else {
                                    crate::listary::disable()
                                }
                            },
                            cx,
                        );
                    }),
                )
                .into_any_element()
            }
        };
        steps_column()
            .child(self.quick_search_card(cx))
            // The instant index reads the NTFS tables of a Windows disk; on
            // the other systems the index is built by walking the folders.
            .when(cfg!(windows), |this| {
                this.child(option_card(
                    "welcome-mft",
                    "icons/database.svg",
                    t!("Indexação instantânea"),
                    t!("Lê a tabela dos discos NTFS como o Everything: o índice do computador fica pronto em segundos, também depois de reiniciar. Instala um serviço do Windows, com permissão de administrador uma vez."),
                    Some(mft_on),
                    mft_busy,
                    cx.listener(move |_page, _, _, cx| mft::set(!mft_on, cx)),
                ))
            })
            .when_some(mft_error, |this, error| this.child(note_text(tf!("Não deu certo: {error}", error = error))))
            // The Listary is a Windows program.
            .when(cfg!(windows), |this| this.child(listary_card))
            .child(note(t!("Dentro da janela: Ctrl+E busca no computador inteiro e Ctrl+T abre a busca numa aba nova.")))
    }

    // A key pressed while the quick search waits for its shortcut, by the
    // same rules as Settings: a combination (Ctrl, Alt or Win with a key, or
    // an F key) becomes it; Esc gives up.
    fn record_quick_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if !self.recording_quick {
            return;
        }
        cx.stop_propagation();
        let keystroke = &event.keystroke;
        let key = keystroke.key.to_lowercase();
        if nohrs_core::shortcuts::MODIFIER_KEYS.contains(&key.as_str()) {
            return;
        }
        let modifiers = keystroke.modifiers;
        self.recording_quick = false;
        if key == "escape" && !(modifiers.control || modifiers.alt || modifiers.shift) {
            cx.notify();
            return;
        }
        match crate::quick_keys::from_keystroke(modifiers.control, modifiers.shift, modifiers.alt, modifiers.platform, &key) {
            Some(setting) => {
                self.error = None;
                self.write("launcher", "hotkey", format!("\"{setting}\""), cx);
            }
            None => {
                self.error =
                    Some(t!("Esse atalho não serve: use Ctrl, Alt ou Win com uma tecla, ou uma tecla F sozinha.").to_string());
            }
        }
        cx.notify();
    }

    // The shortcut that opens Prime Files' search from any program, as
    // Listary does: the usual ones as chips, and any other combination
    // recorded, as in Settings.
    fn quick_search_card(&self, cx: &mut Context<Self>) -> Div {
        let current = crate::quick_keys::resolve(&self.config.launcher.hotkey);
        let listary_installed = !matches!(self.listary, Some(crate::listary::State::NotInstalled) | None);
        let presets: Vec<(SharedString, bool, String)> = quick_presets()
            .into_iter()
            .map(|(value, label)| (SharedString::from(label), value == current, format!("\"{value}\"")))
            .collect();
        let custom = !quick_presets().iter().any(|(value, _)| *value == current);
        let recording = self.recording_quick;
        let record_label: SharedString = if recording {
            t!("Aperte o atalho… (Esc desiste)").into()
        } else if custom {
            crate::quick_keys::describe(&current).into()
        } else {
            t!("Gravar outro…").into()
        };
        let record = pill("welcome-quick-record".into(), record_label, custom || recording)
            .cursor_pointer()
            .on_click(cx.listener(|page, _, window, cx| {
                page.recording_quick = !page.recording_quick;
                if page.recording_quick {
                    window.focus(&page.focus);
                }
                cx.notify();
            }));
        choice_card(
            "icons/keyboard.svg",
            t!("Busca rápida, o Listary do Prime Files"),
            t!("Um atalho abre a busca do computador por cima de qualquer programa. Numa janela de abrir ou salvar arquivo, o que você escolher vai para ela. Ou grave qualquer outra combinação.").to_string(),
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(self.choices("welcome-quick", ("launcher", "hotkey"), presets, cx).child(record))
                .when(listary_installed && current == "double-ctrl", |this| {
                    this.child(note(t!("O Listary também abre com Ctrl duas vezes: os dois vão abrir juntos. Shift 2x evita isso.")))
                }),
        )
        .when(current != "off", |this| this.border_color(rgb(theme::accent())))
    }

    fn step_done(&self, cx: &mut Context<Self>) -> Div {
        steps_column()
            .children(STEPS.into_iter().filter(|step| *step != Step::Done).map(|step| {
                summary_card(step.label(), step.icon(), self.summary(step)).child(
                    div()
                        .id(ElementId::NamedInteger("welcome-change".into(), step as u64))
                        .absolute()
                        .top(px(14.0))
                        .right(px(14.0))
                        .px(px(10.0))
                        .h(px(26.0))
                        .flex()
                        .items_center()
                        .rounded(px(6.0))
                        .text_xs()
                        .text_color(rgb(theme::accent()))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(theme::bg_hover())))
                        .on_click(cx.listener(move |page, _, _, cx| page.go_to(step, cx)))
                        .child(t!("Alterar")),
                )
            }))
            .child(note(t!("Dicas: Espaço mostra o arquivo grande, os botões laterais do mouse voltam e avançam, Ctrl+E busca no computador inteiro, e Configurações › Atalhos muda qualquer tecla.")))
    }

    // What a step's choices are now, for the summaries.
    fn summary(&self, step: Step) -> Vec<(&'static str, Value)> {
        let config = &self.config;
        let on = |value: bool| Value::Switch(Some(value));
        let side = |side: DockSide| {
            Value::Text(
                match side {
                    DockSide::Left => t!("Esquerda"),
                    DockSide::Right => t!("Direita"),
                    DockSide::Top => t!("Em cima"),
                    DockSide::Bottom => t!("Embaixo"),
                }
                .to_string(),
            )
        };
        match step {
            Step::Look => {
                let language = match Lang::parse(&config.ui.language) {
                    Some(lang) => lang.native_name().to_string(),
                    None => t!("Automático").to_string(),
                };
                let mode = match config.theme.mode {
                    ThemeMode::Dark => t!("Escuro"),
                    ThemeMode::Light => t!("Claro"),
                    ThemeMode::System => t!("Igual ao Windows"),
                };
                let accent = config.theme.accent.trim();
                let accent = theme::NAMED_ACCENTS
                    .iter()
                    .zip(crate::settings::ACCENT_LABELS)
                    .find(|((name, _), _)| name.eq_ignore_ascii_case(accent))
                    .map_or(t!("Personalizada"), |(_, label)| i18n::tr(label));
                vec![
                    (t!("Idioma"), Value::Text(language)),
                    (t!("Tema"), Value::Text(mode.to_string())),
                    (t!("Cor de destaque"), Value::Text(accent.to_string())),
                ]
            }
            Step::Window => vec![
                (t!("Barra lateral"), side(config.layout.sidebar)),
                (t!("Visualizador"), side(config.layout.preview)),
                (t!("Abas verticais"), on(config.layout.vertical_tabs)),
                (t!("Modo compacto"), on(config.compact.enabled)),
            ],
            Step::Files => {
                let sort = match config.ui.default_sort {
                    SortOrder::Name => t!("Nome"),
                    SortOrder::Modified => t!("Modificado"),
                    SortOrder::Size => t!("Tamanho"),
                    SortOrder::Kind => t!("Tipo"),
                };
                let direction = if config.ui.default_sort_descending { t!("Decrescente") } else { t!("Crescente") };
                vec![
                    (t!("Ordenação padrão"), Value::Text(format!("{sort} · {direction}"))),
                    (t!("Mostrar itens ocultos"), on(config.ui.show_hidden)),
                    (t!("Reabrir as abas ao iniciar"), on(config.explorer.restore_tabs)),
                    (t!("Ir para a aba nova"), on(!config.explorer.background_tabs)),
                    (t!("Menu clássico do Windows"), on(config.menu.classic)),
                ]
            }
            Step::Sound => {
                let hover = if config.ui.hover_audio {
                    Value::Text(tf!("Ligado, depois de {time}", time = delay_label(config.ui.hover_audio_delay_ms)))
                } else {
                    on(false)
                };
                vec![
                    (t!("Tocar ao passar o mouse"), hover),
                    (t!("Tocar ao abrir no visualizador"), on(config.ui.preview_autoplay)),
                    (t!("Forma de onda nos ícones"), on(config.ui.audio_waveforms)),
                ]
            }
            Step::System => {
                let opens = match config.explorer.open_from_other_apps {
                    OpenFromOtherApps::NewTab => t!("Nova aba"),
                    OpenFromOtherApps::CurrentTab => t!("Aba atual"),
                    OpenFromOtherApps::NewWindow => t!("Nova janela"),
                };
                vec![
                    (t!("Explorador padrão do Windows"), Value::Switch(self.default_explorer)),
                    (t!("\"Mostrar na pasta\" abre aqui"), on(config.explorer.reveal_from_other_apps)),
                    (t!("Pastas abertas de outros programas"), Value::Text(opens.to_string())),
                    (t!("Iniciar com o Windows"), Value::Switch(self.starts_with_windows)),
                    (t!("Atualizar automaticamente"), on(config.updates.automatic)),
                ]
            }
            Step::Search => {
                let current = crate::quick_keys::resolve(&config.launcher.hotkey);
                let quick = match quick_presets().into_iter().find(|(value, _)| *value == current) {
                    Some(("off", _)) => on(false),
                    Some((_, label)) => Value::Text(label.to_string()),
                    None => Value::Text(crate::quick_keys::describe(&current)),
                };
                let listary = match self.listary {
                    Some(crate::listary::State::Ours) => on(true),
                    Some(crate::listary::State::NotInstalled) => Value::NotInstalled,
                    None => Value::Switch(None),
                    Some(_) => on(false),
                };
                vec![
                    (t!("Busca rápida"), quick),
                    (t!("Indexação instantânea"), on(mft::state() == mft::State::On)),
                    ("Listary", listary),
                ]
            }
            Step::Done => Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Previews

    fn preview(&self, step: Step, width: f32, cx: &mut Context<Self>) -> Div {
        let config = &self.config;
        let body: Div = match step {
            Step::Look => div().flex().flex_col().child(preview::layout_window(&config.layout, 250.0)),
            Step::Window => div()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(preview::layout_window(&config.layout, 250.0))
                .when(config.compact.enabled, |this| {
                    this.child(preview_caption(
                        t!("Modo compacto"),
                        t!("Passe o mouse nas bordas da janela de exemplo: as barras escondidas saem com a espera e a animação escolhidas."),
                    ))
                    .child(self.compact_demo.render(
                        &config.compact,
                        220.0,
                        |page: &mut Self| &mut page.compact_demo,
                        |page| &page.config.compact,
                        cx,
                    ))
                }),
            Step::Files => sort_preview(config),
            Step::Sound => div()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(icons_preview(config.ui.audio_waveforms))
                .map(|this| {
                    if config.ui.hover_audio {
                        this.child(preview_caption(
                            t!("Testar o tempo"),
                            t!("Passe o mouse pelos arquivos: cada um começa a tocar depois do tempo escolhido."),
                        ))
                        .child(self.hover_demo.render(
                            config.ui.hover_audio_delay_ms,
                            HoverDemo::fitting(width),
                            |page: &mut Self| &mut page.hover_demo,
                            cx,
                        ))
                    } else {
                        this
                    }
                }),
            Step::System | Step::Search => summary_card(t!("Como fica"), "icons/eye.svg", self.summary(step)),
            Step::Done => div(),
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(theme::muted()))
                    .child(Icon::new(Icon::empty()).path("icons/eye.svg").size_3())
                    .child(t!("Prévia")),
            )
            .child(body)
    }

    // -----------------------------------------------------------------------
    // Frame

    // The steps by name, done ones checked; any of them can be clicked. On a
    // narrow window only the current one keeps its name.
    fn step_bar(&self, named: bool, narrow: bool, pad: f32, cx: &mut Context<Self>) -> Div {
        let current = STEPS.iter().position(|step| *step == self.step).unwrap_or(0);
        // Seven circles, their links and one name fit 360 px only this tight.
        let gap = if narrow { 4.0 } else { 8.0 };
        let mut row = div().w_full().max_w(px(1100.0)).mx_auto().flex().items_center().gap(px(gap));
        for (ix, step) in STEPS.into_iter().enumerate() {
            if ix > 0 {
                row = row.child(
                    div()
                        .flex_1()
                        .min_w(px(6.0))
                        .h(px(1.0))
                        .bg(rgb(if ix <= current { theme::accent() } else { theme::border() })),
                );
            }
            let done = ix < current;
            let here = ix == current;
            row = row.child(
                div()
                    .id(ElementId::NamedInteger("welcome-step".into(), ix as u64))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(if narrow { 2.0 } else { 4.0 }))
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::bg_hover())))
                    .on_click(cx.listener(move |page, _, _, cx| page.go_to(step, cx)))
                    .child(
                        div()
                            .size(px(24.0))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .map(|this| {
                                if done {
                                    this.bg(rgb(theme::accent())).text_color(rgb(theme::accent_fg()))
                                } else if here {
                                    this.border_2().border_color(rgb(theme::accent())).text_color(rgb(theme::accent()))
                                } else {
                                    this.border_1().border_color(rgb(theme::border_hover())).text_color(rgb(theme::muted()))
                                }
                            })
                            .map(|this| {
                                if done {
                                    this.child(Icon::new(Icon::empty()).path("icons/check.svg").size_3())
                                } else {
                                    this.child((ix + 1).to_string())
                                }
                            }),
                    )
                    .when(named || here, |this| {
                        this.child(
                            div()
                                .text_sm()
                                .whitespace_nowrap()
                                .text_color(rgb(if here { theme::fg() } else { theme::fg_secondary() }))
                                .when(here, |text| text.font_weight(gpui::FontWeight::SEMIBOLD))
                                .child(step.label()),
                        )
                    }),
            );
        }
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .px(px(pad))
            .py(px(10.0))
            .border_b_1()
            .border_color(rgb(theme::border()))
            .child(row)
    }

    fn footer(&self, narrow: bool, pad: f32, cx: &mut Context<Self>) -> Div {
        let index = STEPS.iter().position(|step| *step == self.step).unwrap_or(0);
        let last = index == STEPS.len() - 1;
        let next = STEPS.get(index + 1).copied();
        let previous = index.checked_sub(1).and_then(|ix| STEPS.get(ix)).copied();
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .px(px(pad))
            .py(px(12.0))
            .border_t_1()
            .border_color(rgb(theme::border()))
            .bg(rgb(theme::toolbar_bg()))
            .child(
                div()
                    .w_full()
                    .max_w(px(1100.0))
                    .mx_auto()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .when(!last, |this| {
                        this.child(
                            div()
                                .id("welcome-skip")
                                .flex_shrink_0()
                                .px(px(8.0))
                                .h(px(36.0))
                                .flex()
                                .items_center()
                                .rounded(px(8.0))
                                .text_sm()
                                .text_color(rgb(theme::fg_secondary()))
                                .cursor_pointer()
                                .hover(|style| style.text_color(rgb(theme::fg())).bg(rgb(theme::bg_hover())))
                                .on_click(cx.listener(|page, _, _, cx| page.finish(cx)))
                                .child(t!("Pular")),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_xs()
                            .text_color(rgb(theme::muted()))
                            .when(!narrow, |this| {
                                this.child(if last {
                                    t!("Esta tela volta por Configurações › Sobre › Abrir de novo.")
                                } else {
                                    t!("Pular mantém o que você já escolheu. Esta tela volta por Configurações › Sobre.")
                                })
                            }),
                    )
                    .when(!narrow, |this| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(rgb(theme::muted()))
                                .child(tf!("{n} de {total}", n = index + 1, total = STEPS.len())),
                        )
                    })
                    .when_some(previous, |this, previous| {
                        this.child(
                            secondary_button("welcome-back", t!("Voltar"))
                                .on_click(cx.listener(move |page, _, _, cx| page.go_to(previous, cx))),
                        )
                    })
                    .child(
                        primary_button("welcome-next", if last { t!("Começar a usar") } else { t!("Próximo") }, !last)
                            .on_click(cx.listener(move |page, _, _, cx| match next {
                                Some(next) => page.go_to(next, cx),
                                None => page.finish(cx),
                            })),
                    ),
            )
    }
}

impl Render for WelcomePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let step = self.step;
        if step == Step::Window && self.config.compact.enabled {
            self.compact_demo.tick(&self.config.compact, window);
        }
        if self.hover_demo.is_active() {
            self.hover_demo.tick(self.config.ui.hover_audio_delay_ms);
            window.request_animation_frame();
        }
        let width = f32::from(window.viewport_size().width);
        let wide = width >= WIDE;
        let narrow = width < NARROW;
        let pad = if narrow { 16.0 } else { 28.0 };
        // The sample windows of the previews have panels of fixed width
        // (about 370 px with every panel docked to a side); a column narrower
        // than that made them push the whole screen past the window's edge.
        let panel_pad = if width >= 1240.0 { 24.0 } else { 18.0 };
        let panel_width = if width >= 1240.0 { 440.0 } else { 410.0 };
        // What the previews fit in: the side column inside its padding, or
        // the choices' column.
        let preview_width = if wide { panel_width - 2.0 * panel_pad } else { (width - 2.0 * pad).min(660.0) };

        let body = match step {
            Step::Look => self.step_look(cx),
            Step::Window => self.step_window(cx),
            Step::Files => self.step_files(cx),
            Step::Sound => self.step_sound(cx),
            Step::System => self.step_system(cx),
            Step::Search => self.step_search(cx),
            Step::Done => self.step_done(cx),
        };
        let inline_preview = (!wide && step.has_preview()).then(|| self.preview(step, preview_width, cx));
        let side_preview = (wide && step.has_preview()).then(|| self.preview(step, preview_width, cx));

        // The scrolling column needs a height of its own (the row's, not its
        // content's): without `h_full` the last card ran under the footer and
        // the scroll stopped short of it.
        let choices = div()
            .id("welcome-choices")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .max_w(px(660.0))
                    .mx_auto()
                    .px(px(pad))
                    .pt(px(if narrow { 20.0 } else { 32.0 }))
                    .pb(px(if narrow { 28.0 } else { 40.0 }))
                    .flex()
                    .flex_col()
                    .gap(px(22.0))
                    .child(hero(step.icon(), step.title(), step.text(), narrow))
                    .children(inline_preview.map(|preview| div().w_full().min_w(px(0.0)).overflow_hidden().child(preview)))
                    .child(body)
                    .when_some(self.error.clone(), |this, error| {
                        this.child(div().text_sm().text_color(rgb(theme::danger())).child(error))
                    }),
            );

        // Fills the room under the title bar, never more: `size_full` here
        // took the window's height and width, so the bottom and the right
        // side fell outside it.
        div()
            .id("welcome")
            .flex_1()
            .w_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(theme::bg()))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|page, event: &KeyDownEvent, _window, cx| page.record_quick_key(event, cx)))
            .child(self.step_bar(width >= NAMED_STEPS, narrow, pad, cx))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .flex()
                    .overflow_hidden()
                    .child(choices)
                    .children(side_preview.map(|preview| {
                        div()
                            .id("welcome-preview")
                            .w(px(panel_width))
                            .h_full()
                            .min_h(px(0.0))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .overflow_x_hidden()
                            .overflow_y_scroll()
                            .border_l_1()
                            .border_color(rgb(theme::border()))
                            .bg(rgb(theme::toolbar_bg()))
                            .child(
                                div()
                                    .w_full()
                                    .min_w(px(0.0))
                                    .flex_shrink_0()
                                    .overflow_hidden()
                                    .p(px(panel_pad))
                                    .pt(px(32.0))
                                    .flex()
                                    .flex_col()
                                    .child(preview),
                            )
                    })),
            )
            .child(self.footer(narrow, pad, cx))
    }
}

impl crate::Page for WelcomePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        <Self as Render>::render(self, window, cx).into_any_element()
    }
}

// The quick search's shortcuts offered here, by their `launcher.hotkey`.
fn quick_presets() -> [(&'static str, &'static str); 5] {
    [
        ("off", t!("Desligado")),
        ("double-ctrl", "Ctrl 2x"),
        ("double-shift", "Shift 2x"),
        ("ctrl+space", t!("Ctrl+Espaço")),
        ("shift+space", t!("Shift+Espaço")),
    ]
}

fn delay_label(ms: u32) -> String {
    match ms {
        0 => t!("Na hora").to_string(),
        ms if ms % 1000 == 0 => format!("{} s", ms / 1000),
        ms => format!("{},{} s", ms / 1000, (ms % 1000) / 100),
    }
}

// ---------------------------------------------------------------------------
// Pieces

fn steps_column() -> Div {
    div().w_full().flex().flex_col().gap(px(12.0))
}

fn hero(icon: &'static str, title: &'static str, text: &'static str, narrow: bool) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(16.0))
        .child(
            div()
                .size(px(if narrow { 44.0 } else { 52.0 }))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(14.0))
                .bg(rgb(theme::accent_subtle()))
                .text_color(rgb(theme::accent()))
                .child(Icon::new(Icon::empty()).path(icon).size_6()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .map(|this| if narrow { this.text_xl() } else { this.text_2xl() })
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(theme::fg()))
                        .child(title),
                )
                .child(div().text_sm().line_height(px(20.0)).text_color(rgb(theme::fg_secondary())).child(text)),
        )
}

fn group_label(text: &'static str) -> Div {
    div()
        .pt(px(10.0))
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(rgb(theme::muted()))
        .child(text)
}

fn preview_caption(title: &'static str, text: &'static str) -> Div {
    div()
        .pt(px(4.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).text_color(rgb(theme::fg())).child(title))
        .child(div().text_xs().line_height(px(17.0)).text_color(rgb(theme::fg_secondary())).child(text))
}

fn icon_box(icon: &'static str, lit: bool) -> Div {
    div()
        .size(px(36.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(9.0))
        .bg(rgb(if lit { theme::accent_subtle() } else { theme::bg() }))
        .text_color(rgb(if lit { theme::accent() } else { theme::fg_secondary() }))
        .child(Icon::new(Icon::empty()).path(icon).size_4())
}

fn card_frame() -> Div {
    div()
        .w_full()
        .p(px(16.0))
        .rounded(px(12.0))
        .bg(rgb(theme::bg_secondary()))
        .border_1()
        .border_color(rgb(theme::border()))
}

// The name and what it does, beside the icon.
fn card_text(title: &'static str, text: SharedString) -> Div {
    div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(3.0))
        .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).text_color(rgb(theme::fg())).child(title))
        .child(div().text_xs().line_height(px(17.0)).text_color(rgb(theme::fg_secondary())).child(text))
}

// A choice with its options under the text, where a row of them always has
// the card's whole width to wrap in.
fn choice_card(icon: &'static str, title: &'static str, text: String, control: Div) -> Div {
    card_frame()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(div().flex().items_center().gap(px(12.0)).child(icon_box(icon, false)).child(card_text(title, text.into())))
        .child(control)
}

// A choice as a card: icon, name, what it does, and a switch; the whole card
// is the button.
fn option_card(
    id: &'static str,
    icon: &'static str,
    title: &'static str,
    text: &'static str,
    on: Option<bool>,
    busy: bool,
    toggle: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> Stateful<Div> {
    let known = on.is_some();
    let on = on.unwrap_or(false);
    card_frame()
        .id(id)
        .flex()
        .items_center()
        .gap(px(12.0))
        .border_color(rgb(if on { theme::accent() } else { theme::border() }))
        .when(known && !busy, |this| this.cursor_pointer().hover(|style| style.bg(rgb(theme::bg_hover()))).on_click(toggle))
        .child(icon_box(icon, on))
        .child(card_text(title, text.into()))
        .child(if known && !busy {
            switch(on).into_any_element()
        } else {
            div().flex_shrink_0().text_xs().text_color(rgb(theme::muted())).child(t!("Verificando…")).into_any_element()
        })
}

fn info_card(icon: &'static str, title: &'static str, text: &'static str) -> Div {
    card_frame().flex().items_center().gap(px(12.0)).child(icon_box(icon, false)).child(card_text(title, text.into()))
}

// A step's choices as they are now, one per line.
fn summary_card(title: &'static str, icon: &'static str, rows: Vec<(&'static str, Value)>) -> Div {
    card_frame()
        .relative()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .h(px(28.0))
                .mb(px(4.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(Icon::new(Icon::empty()).path(icon).size_4().text_color(rgb(theme::accent())))
                .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).text_color(rgb(theme::fg())).child(title)),
        )
        .children(rows.into_iter().map(|(label, value)| {
            let (color, text): (u32, SharedString) = match value {
                Value::Switch(Some(true)) => (theme::accent(), t!("Ligado").into()),
                Value::Switch(Some(false)) => (theme::muted(), t!("Desligado").into()),
                Value::Switch(None) => (theme::muted(), t!("Verificando…").into()),
                Value::NotInstalled => (theme::muted(), t!("Não instalado").into()),
                Value::Text(text) => (theme::fg(), text.into()),
            };
            div()
                .min_h(px(30.0))
                .py(px(4.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .border_t_1()
                .border_color(rgb(theme::border()))
                .child(div().flex_1().min_w(px(0.0)).text_sm().text_color(rgb(theme::fg_secondary())).child(label))
                .child(div().flex_shrink_0().text_sm().text_color(rgb(color)).child(text))
        }))
}

fn note(text: &'static str) -> Div {
    note_text(text.to_string())
}

fn note_text(text: String) -> Div {
    div()
        .w_full()
        .flex()
        .gap(px(10.0))
        .px(px(14.0))
        .py(px(10.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(rgb(theme::border()))
        .child(Icon::new(Icon::empty()).path("icons/info.svg").size_4().flex_shrink_0().text_color(rgb(theme::muted())))
        .child(div().flex_1().min_w(px(0.0)).text_xs().line_height(px(18.0)).text_color(rgb(theme::fg_secondary())).child(text))
}

fn pill(id: ElementId, label: SharedString, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .h(px(30.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .rounded_full()
        .border_1()
        .text_sm()
        .whitespace_nowrap()
        .map(|this| {
            if selected {
                this.bg(rgb(theme::accent())).border_color(rgb(theme::accent())).text_color(rgb(theme::accent_fg()))
            } else {
                this.cursor_pointer()
                    .border_color(rgb(theme::border_hover()))
                    .text_color(rgb(theme::fg()))
                    .hover(|style| style.bg(rgb(theme::bg_hover())))
            }
        })
        .child(label)
}

// An on/off chip among others that are on or off on their own: a check says
// "on" apart from "chosen".
fn toggle_pill(id: ElementId, label: &'static str, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .h(px(30.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .rounded_full()
        .border_1()
        .cursor_pointer()
        .text_sm()
        .whitespace_nowrap()
        .map(|this| {
            if on {
                this.bg(rgb(theme::accent_subtle())).border_color(rgb(theme::accent())).text_color(rgb(theme::fg()))
            } else {
                this.border_color(rgb(theme::border_hover())).text_color(rgb(theme::fg_secondary()))
            }
        })
        .hover(|style| style.bg(rgb(theme::bg_hover())))
        .when(on, |this| this.child(Icon::new(Icon::empty()).path("icons/check.svg").size_3().text_color(rgb(theme::accent()))))
        .child(label)
}

fn swatch(id: ElementId, color: u32, label: &'static str, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(64.0))
        .flex_shrink_0()
        .py(px(6.0))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(5.0))
        .rounded(px(8.0))
        .when(!selected, |this| this.cursor_pointer().hover(|style| style.bg(rgb(theme::bg_hover()))))
        .child(
            div()
                .size(px(26.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(rgb(color))
                .border_2()
                .border_color(rgb(if selected { theme::fg() } else { theme::bg_secondary() }))
                .when(selected, |this| {
                    this.child(Icon::new(Icon::empty()).path("icons/check.svg").size_3().text_color(rgb(0xFFFFFF)))
                }),
        )
        .child(
            div()
                .text_xs()
                .whitespace_nowrap()
                .text_color(rgb(if selected { theme::fg() } else { theme::fg_secondary() }))
                .child(label),
        )
}

// A theme as a little window: its background, a bar and a line of text.
fn theme_tile(
    id: &'static str,
    label: &'static str,
    selected: bool,
    (background, bar, text): (u32, u32, u32),
    cx: &mut Context<WelcomePage>,
    value: &'static str,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .p(px(8.0))
        .rounded(px(10.0))
        .border_2()
        .cursor_pointer()
        .border_color(rgb(if selected { theme::accent() } else { theme::border() }))
        .hover(|style| style.bg(rgb(theme::bg_hover())))
        .on_click(cx.listener(move |page, _, _, cx| page.write("theme", "mode", value.to_string(), cx)))
        .child(
            div()
                .h(px(56.0))
                .rounded(px(6.0))
                .bg(rgb(background))
                .border_1()
                .border_color(rgb(theme::border()))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .p(px(6.0))
                .child(div().h(px(8.0)).rounded(px(3.0)).bg(rgb(bar)))
                .child(div().w(px(40.0)).h(px(5.0)).rounded(px(3.0)).bg(rgb(text)))
                .child(div().w(px(26.0)).h(px(5.0)).rounded(px(3.0)).bg(rgb(theme::accent()))),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_sm()
                .text_color(rgb(theme::fg()))
                .when(selected, |this| {
                    this.child(Icon::new(Icon::empty()).path("icons/check.svg").size_3().flex_shrink_0().text_color(rgb(theme::accent())))
                })
                .child(div().flex_1().min_w(px(0.0)).child(label)),
        )
}

fn switch(on: bool) -> Div {
    const WIDTH: f32 = 40.0;
    const HEIGHT: f32 = 22.0;
    const KNOB: f32 = 16.0;
    let inset = (HEIGHT - KNOB) / 2.0 - 1.0;
    div()
        .relative()
        .flex_shrink_0()
        .w(px(WIDTH))
        .h(px(HEIGHT))
        .rounded_full()
        .border_1()
        .map(|track| {
            if on {
                track.bg(rgb(theme::accent())).border_color(rgb(theme::accent()))
            } else {
                track.bg(rgb(theme::bg())).border_color(rgb(theme::border_hover()))
            }
        })
        .child(
            div()
                .absolute()
                .top(px(inset))
                .left(px(if on { WIDTH - KNOB - inset - 2.0 } else { inset }))
                .size(px(KNOB))
                .rounded_full()
                .bg(rgb(if on { theme::accent_fg() } else { theme::fg_secondary() })),
        )
}

fn primary_button(id: &'static str, label: &'static str, arrow: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .h(px(36.0))
        .px(px(18.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .bg(rgb(theme::accent()))
        .text_color(rgb(theme::accent_fg()))
        .text_sm()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .hover(|style| style.bg(rgb(theme::accent_hover())))
        .child(label)
        .when(arrow, |this| this.child(Icon::new(Icon::empty()).path("icons/arrow-right.svg").size_4()))
}

fn secondary_button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .h(px(36.0))
        .px(px(14.0))
        .flex()
        .items_center()
        .rounded(px(8.0))
        .cursor_pointer()
        .border_1()
        .border_color(rgb(theme::border_hover()))
        .text_color(rgb(theme::fg()))
        .text_sm()
        .hover(|style| style.bg(rgb(theme::bg_hover())))
        .child(label)
}

// ---------------------------------------------------------------------------
// The previews only this screen draws

/// One file of the sample folder in the order preview.
struct Sample {
    name: &'static str,
    folder: bool,
    hidden: bool,
    // Days ago, and bytes.
    age: u32,
    size: u64,
    kind: &'static str,
}

const SAMPLE_FOLDER: [Sample; 8] = [
    Sample { name: "Projetos", folder: true, hidden: false, age: 1, size: 0, kind: "" },
    Sample { name: "Fotos", folder: true, hidden: false, age: 9, size: 0, kind: "" },
    Sample { name: ".cache", folder: true, hidden: true, age: 0, size: 0, kind: "" },
    Sample { name: "relatório.pdf", folder: false, hidden: false, age: 3, size: 2_400_000, kind: "PDF" },
    Sample { name: "notas.txt", folder: false, hidden: false, age: 0, size: 4_000, kind: "TXT" },
    Sample { name: "trilha.wav", folder: false, hidden: false, age: 6, size: 38_000_000, kind: "WAV" },
    Sample { name: "capa.png", folder: false, hidden: false, age: 12, size: 820_000, kind: "PNG" },
    Sample { name: "desktop.ini", folder: false, hidden: true, age: 30, size: 1_000, kind: "INI" },
];

/// A folder of example sorted as `ui.default_sort` says, folders first, its
/// hidden items shown faded when `ui.show_hidden` is on.
fn sort_preview(config: &Config) -> Div {
    let ui = &config.ui;
    let mut rows: Vec<&Sample> = SAMPLE_FOLDER.iter().filter(|sample| ui.show_hidden || !sample.hidden).collect();
    rows.sort_by(|a, b| {
        let key = match ui.default_sort {
            SortOrder::Name => a.name.trim_start_matches('.').to_lowercase().cmp(&b.name.trim_start_matches('.').to_lowercase()),
            // Newest first is "descending" in the listing too.
            SortOrder::Modified => b.age.cmp(&a.age),
            SortOrder::Size => a.size.cmp(&b.size),
            SortOrder::Kind => a.kind.cmp(b.kind).then(a.name.cmp(b.name)),
        };
        let key = if ui.default_sort_descending { key.reverse() } else { key };
        b.folder.cmp(&a.folder).then(key)
    });
    let arrow = if ui.default_sort_descending { "icons/chevron-down.svg" } else { "icons/chevron-up.svg" };
    let column = |label: &'static str, order: SortOrder, width: Option<f32>| {
        let sorted = ui.default_sort == order;
        div()
            .map(|this| match width {
                Some(width) => this.w(px(width)).flex_shrink_0().justify_end(),
                None => this.flex_1().min_w(px(0.0)),
            })
            .flex()
            .items_center()
            .gap(px(3.0))
            .text_color(rgb(if sorted { theme::accent() } else { theme::muted() }))
            .when(sorted, |this| this.font_weight(gpui::FontWeight::SEMIBOLD))
            .child(label)
            .when(sorted, |this| this.child(Icon::new(Icon::empty()).path(arrow).size_3()))
    };
    let header = div()
        .h(px(26.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_xs()
        .border_b_1()
        .border_color(rgb(theme::border()))
        .child(column(t!("Nome"), SortOrder::Name, None))
        .child(column(t!("Tipo"), SortOrder::Kind, Some(36.0)))
        .child(column(t!("Tamanho"), SortOrder::Size, Some(58.0)));
    div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(px(10.0))
        .overflow_hidden()
        .border_1()
        .border_color(rgb(theme::border()))
        .bg(rgb(theme::bg()))
        .child(header)
        .children(rows.into_iter().map(|sample| {
            div()
                .h(px(28.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_xs()
                .when(sample.hidden, |this| this.opacity(0.5))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .child(
                            Icon::new(Icon::empty())
                                .path(if sample.folder { "icons/folder.svg" } else { "icons/file.svg" })
                                .size_4()
                                .flex_shrink_0()
                                .text_color(rgb(if sample.folder { preview::FOLDER } else { theme::fg_secondary() })),
                        )
                        .child(div().text_color(rgb(theme::fg())).whitespace_nowrap().child(sample.name)),
                )
                .child(div().w(px(36.0)).flex_shrink_0().flex().justify_end().text_color(rgb(theme::fg_secondary())).child(sample.kind))
                .child(
                    div()
                        .w(px(58.0))
                        .flex_shrink_0()
                        .flex()
                        .justify_end()
                        .text_color(rgb(theme::fg_secondary()))
                        .child(if sample.folder { String::new() } else { size_label(sample.size) }),
                )
        }))
}

fn size_label(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{} KB", (bytes / 1_000).max(1))
    }
}

/// The Icons view of a few files: sounds draw their waveform or a note, as
/// `ui.audio_waveforms` says.
fn icons_preview(waveforms: bool) -> Div {
    let tile = |name: &'static str, sound: Option<u32>, icon: &'static str| {
        let picture: AnyElement = match sound {
            Some(seed) if waveforms => waveform(seed).into_any_element(),
            _ => Icon::new(Icon::empty()).path(icon).size_6().text_color(rgb(theme::fg_secondary())).into_any_element(),
        };
        div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .h(px(64.0))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .bg(rgb(theme::bg_secondary()))
                    .child(picture),
            )
            .child(
                div()
                    .flex()
                    .justify_center()
                    .text_xs()
                    .text_color(rgb(theme::fg()))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(name),
            )
    };
    div()
        .w_full()
        .p(px(10.0))
        .flex()
        .gap(px(8.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(rgb(theme::border()))
        .bg(rgb(theme::bg()))
        .child(tile("chuva.wav", Some(1), "icons/music.svg"))
        .child(tile("passos.wav", Some(2), "icons/music.svg"))
        .child(tile("trilha.wav", Some(3), "icons/music.svg"))
        .child(tile("clipe.mp4", None, "icons/film.svg"))
}

// Bars of a made-up sound, the same for the same seed.
fn waveform(seed: u32) -> Div {
    let mut state = 0x9E37_79B9u32.wrapping_mul(seed) | 1;
    div().h(px(44.0)).px(px(6.0)).flex().items_center().gap(px(1.0)).children((0..16).map(move |ix| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let envelope = 1.0 - ((ix as f32 - 6.0).abs() / 10.0).min(0.8);
        let height = 4.0 + (state % 100) as f32 / 100.0 * 36.0 * envelope;
        div().w(px(2.0)).h(px(height)).rounded(px(1.0)).bg(rgb(theme::accent()))
    }))
}
