/// Modern theme tokens inspired by macOS and modern file explorers
// Consumers import this as `theme::theme` (e.g. `use nohrs_ui::theme::theme`);
// the inner module name is part of the established public path.
#[allow(clippy::module_inception)]
pub mod theme {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    // The tokens are read during render, on every frame, by code that has no
    // `cx` to hand in some places (free helper functions building elements). A
    // process-wide flag keeps them plain calls; the appearance is chosen once at
    // startup, before the first window opens, so no frame ever reads it torn.
    static DARK: AtomicBool = AtomicBool::new(false);

    /// Selects the palette every token below reads from.
    pub fn set_dark(dark: bool) {
        DARK.store(dark, Ordering::Relaxed);
    }

    /// Whether the dark palette is the one in use.
    pub fn is_dark() -> bool {
        DARK.load(Ordering::Relaxed)
    }

    /// The font holding the system's icon glyphs (caption buttons, player).
    /// Segoe Fluent Icons ships only with Windows 11; Windows 10 has the same
    /// code points in Segoe MDL2 Assets, and naming a font that is not
    /// installed draws every glyph as an empty box.
    pub fn icon_font() -> &'static str {
        static FONT: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
        FONT.get_or_init(|| {
            let windows = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
            let fluent = std::path::Path::new(&windows)
                .join("Fonts")
                .join("SegoeIcons.ttf");
            if fluent.is_file() {
                "Segoe Fluent Icons"
            } else {
                "Segoe MDL2 Assets"
            }
        })
    }

    // The accent the user picked, as `0x01rrggbb`; zero means the palette's
    // own orange. The marker byte keeps black (`#000000`) distinguishable from
    // "nothing picked".
    static ACCENT: AtomicU32 = AtomicU32::new(0);
    const ACCENT_SET: u32 = 0x0100_0000;

    /// The accents the settings page offers, by their `config.toml` name.
    /// Orange is the palette's own and carries no colour here: the two
    /// palettes tune it separately, which a single base colour cannot.
    pub const NAMED_ACCENTS: &[(&str, Option<u32>)] = &[
        ("orange", None),
        ("yellow", Some(0xEAB308)),
        ("lime", Some(0x84CC16)),
        ("green", Some(0x22C55E)),
        ("teal", Some(0x14B8A6)),
        ("cyan", Some(0x06B6D4)),
        ("blue", Some(0x3B82F6)),
        ("indigo", Some(0x6366F1)),
        ("purple", Some(0x8B5CF6)),
        ("magenta", Some(0xD946EF)),
        ("pink", Some(0xEC4899)),
        ("red", Some(0xEF4444)),
        ("brown", Some(0xA16B47)),
        ("gray", Some(0x94A3B8)),
    ];

    /// The colour a `theme.accent` value names: one of [`NAMED_ACCENTS`] or a
    /// `#rrggbb` hex. `None` for orange and for anything unrecognized, so a
    /// typo leaves the window looking as it always did.
    pub fn parse_accent(value: &str) -> Option<u32> {
        let value = value.trim();
        if let Some(hex) = value.strip_prefix('#') {
            return (hex.len() == 6)
                .then(|| u32::from_str_radix(hex, 16).ok())
                .flatten();
        }
        NAMED_ACCENTS
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(value))
            .and_then(|(_, color)| *color)
    }

    /// Replaces the palette's accent with `color`, or restores it with `None`.
    pub fn set_accent(color: Option<u32>) {
        ACCENT.store(
            color.map_or(0, |color| ACCENT_SET | (color & 0xFF_FFFF)),
            Ordering::Relaxed,
        );
    }

    fn picked_accent() -> Option<u32> {
        let value = ACCENT.load(Ordering::Relaxed);
        (value & ACCENT_SET != 0).then_some(value & 0xFF_FFFF)
    }

    // Linear blend of two `0xrrggbb` colours, `amount` of the way to `to`.
    fn mix(from: u32, to: u32, amount: f32) -> u32 {
        let channel = |shift: u32| {
            let a = ((from >> shift) & 0xFF) as f32;
            let b = ((to >> shift) & 0xFF) as f32;
            ((a + (b - a) * amount).round() as u32).min(0xFF) << shift
        };
        channel(16) | channel(8) | channel(0)
    }

    // Relative luminance, enough to choose black or white text on a fill.
    fn luminance(color: u32) -> f32 {
        let linear = |shift: u32| {
            let c = ((color >> shift) & 0xFF) as f32 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(16) + 0.7152 * linear(8) + 0.0722 * linear(0)
    }

    // A picked accent, shaded for the palette in use the way the orange is:
    // darker on white so text on it still reads, lighter on the dark grays.
    fn shaded_accent(base: u32) -> u32 {
        if is_dark() {
            mix(base, 0xFFFFFF, 0.12)
        } else {
            mix(base, 0x000000, 0.18)
        }
    }

    struct Palette {
        gray_50: u32,
        gray_100: u32,
        gray_200: u32,
        gray_300: u32,
        gray_400: u32,
        gray_500: u32,
        gray_600: u32,
        gray_700: u32,
        gray_800: u32,
        gray_900: u32,
        bg: u32,
        bg_secondary: u32,
        bg_hover: u32,
        fg: u32,
        fg_secondary: u32,
        muted: u32,
        border: u32,
        border_hover: u32,
        toolbar_bg: u32,
        toolbar_hover: u32,
        toolbar_text: u32,
        toolbar_active_bg: u32,
        toolbar_border: u32,
        accent: u32,
        accent_hover: u32,
        accent_subtle: u32,
        accent_fg: u32,
        danger: u32,
        warning: u32,
        preview_backdrop: u32,
    }

    const LIGHT: Palette = Palette {
        gray_50: 0xF9FAFB,
        gray_100: 0xF3F4F6,
        gray_200: 0xE5E7EB,
        gray_300: 0xD1D5DB,
        gray_400: 0x9CA3AF,
        gray_500: 0x6B7280,
        gray_600: 0x4B5563,
        gray_700: 0x374151,
        gray_800: 0x1F2937,
        gray_900: 0x111827,
        bg: 0xFFFFFF,
        bg_secondary: 0xF9FAFB,
        bg_hover: 0xF3F4F6,
        fg: 0x111827,
        fg_secondary: 0x6B7280,
        muted: 0x9CA3AF,
        border: 0xE5E7EB,
        border_hover: 0xD1D5DB,
        // Chrome surfaces (nav rail, tab bar, status bar) share one tone so the
        // window reads as a frame around the white content areas.
        toolbar_bg: 0xF3F4F6,
        toolbar_hover: 0xE5E7EB,
        toolbar_text: 0x4B5563,
        toolbar_active_bg: 0xFFFFFF,
        toolbar_border: 0xE5E7EB,
        // Accent ramp, derived from the burnt orange in the app icon. `accent`
        // is dark enough to carry `accent_fg` text at WCAG AA (4.9:1), which
        // the filled controls (search button, active scope chips) rely on.
        accent: 0xB85214,
        accent_hover: 0x93400F,
        accent_subtle: 0xFDF1E9,
        accent_fg: 0xFFFFFF,
        danger: 0xDC2626,
        warning: 0xB45309,
        preview_backdrop: 0x1F2937,
    };

    // Graphite rather than black: pure black behind near-white text is the
    // harshest contrast a screen can show and smears on OLED. The text stops
    // short of white for the same reason, and every step keeps the chrome a
    // shade darker than the content so the frame reads the same way it does in
    // the light palette.
    const DARK_PALETTE: Palette = Palette {
        // The gray ramp is inverted, since its callers mean "faint" by the low
        // end and "strong" by the high end, not "light" and "dark".
        gray_50: 0x232428,
        gray_100: 0x2A2C30,
        gray_200: 0x33353A,
        gray_300: 0x43464D,
        gray_400: 0x6E7179,
        gray_500: 0x9A9CA3,
        gray_600: 0xB4B6BC,
        gray_700: 0xC8CACF,
        gray_800: 0xDCDDE0,
        gray_900: 0xE6E7EA,
        bg: 0x1E1F22,
        bg_secondary: 0x232428,
        bg_hover: 0x2A2C30,
        fg: 0xDCDDE0,
        fg_secondary: 0x9A9CA3,
        muted: 0x6E7179,
        border: 0x2F3135,
        border_hover: 0x43464D,
        toolbar_bg: 0x18191C,
        toolbar_hover: 0x2A2C30,
        toolbar_text: 0xB4B6BC,
        toolbar_active_bg: 0x2A2C30,
        toolbar_border: 0x2A2C30,
        // The icon's orange lifted until it reads on graphite; at that
        // lightness white text on it would fall under AA, so filled controls
        // carry the background tone instead (about 6:1).
        accent: 0xE0803F,
        accent_hover: 0xEE9558,
        accent_subtle: 0x3A2B21,
        accent_fg: 0x1B1C1F,
        danger: 0xF07171,
        warning: 0xE0A33A,
        preview_backdrop: 0x141517,
    };

    fn palette() -> &'static Palette {
        if is_dark() { &DARK_PALETTE } else { &LIGHT }
    }

    macro_rules! tokens {
        ($($(#[$doc:meta])* $name:ident;)*) => {
            $(
                $(#[$doc])*
                pub fn $name() -> u32 {
                    palette().$name
                }
            )*
        };
    }

    /// Primary accent color.
    pub fn accent() -> u32 {
        picked_accent().map_or(palette().accent, shaded_accent)
    }

    /// The palette's own accent (the orange), whatever the user picked; for
    /// the swatch that offers it back.
    pub fn palette_accent() -> u32 {
        palette().accent
    }

    /// Accent color on hover / pressed.
    pub fn accent_hover() -> u32 {
        match picked_accent() {
            Some(base) if is_dark() => mix(shaded_accent(base), 0xFFFFFF, 0.15),
            Some(base) => mix(shaded_accent(base), 0x000000, 0.2),
            None => palette().accent_hover,
        }
    }

    /// Faint accent tint, for selected rows and active chip backgrounds.
    pub fn accent_subtle() -> u32 {
        match picked_accent() {
            Some(base) if is_dark() => mix(palette().bg, base, 0.2),
            Some(base) => mix(palette().bg, base, 0.1),
            None => palette().accent_subtle,
        }
    }

    /// Text and icon color on top of an `accent`-filled surface.
    pub fn accent_fg() -> u32 {
        match picked_accent() {
            Some(_) if luminance(accent()) > 0.4 => 0x1B1C1F,
            Some(_) => 0xFFFFFF,
            None => palette().accent_fg,
        }
    }

    tokens! {
        /// Faintest step of the gray ramp.
        gray_50;
        /// Faint gray, used for subtle backgrounds.
        gray_100;
        /// Gray used for default borders.
        gray_200;
        /// Gray used for borders on hover.
        gray_300;
        /// Gray used for muted text.
        gray_400;
        /// Gray used for secondary text.
        gray_500;
        /// Gray used for primary text.
        gray_600;
        /// Gray used for high-emphasis text.
        gray_700;
        /// Second-strongest step of the gray ramp.
        gray_800;
        /// Strongest step of the gray ramp.
        gray_900;
        /// Primary surface background color.
        bg;
        /// Secondary surface background color.
        bg_secondary;
        /// Background color for hovered surfaces.
        bg_hover;
        /// Primary foreground (text) color.
        fg;
        /// Secondary foreground (text) color for less prominent content.
        fg_secondary;
        /// Muted foreground color for de-emphasized content.
        muted;
        /// Default border color.
        border;
        /// Border color on hover.
        border_hover;
        /// Toolbar background color.
        toolbar_bg;
        /// Toolbar item background color on hover.
        toolbar_hover;
        /// Toolbar text color.
        toolbar_text;
        /// Background color of the active toolbar item.
        toolbar_active_bg;
        /// Toolbar border color.
        toolbar_border;
        /// Danger color, used for error messages and destructive actions.
        danger;
        /// Warning color, for an operation that did what was asked and left
        /// something unfinished. Amber rather than a lighter red: the distance
        /// from `danger` is the whole point, since the two say opposite things
        /// about whether the thing the user asked for happened.
        warning;
        /// Backdrop behind an image preview; deliberately dark so transparent
        /// and light-colored images stay legible.
        preview_backdrop;
    }

    /// Text color of the active toolbar item.
    pub fn toolbar_active_text() -> u32 {
        accent()
    }

    /// Status bar background; matches the other chrome surfaces.
    pub fn footer_bg() -> u32 {
        toolbar_bg()
    }

    /// Opacity of the scrim painted behind a modal dialog. `gpui_component`
    /// ships 5%, which is invisible against this theme's white surfaces and
    /// leaves confirmations looking like floating panels rather than modals.
    pub const MODAL_SCRIM_ALPHA: f32 = 0.40;
}
