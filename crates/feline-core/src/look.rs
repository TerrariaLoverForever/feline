// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline's look: the accent colour (a preset or your own) and the seasonal
//! themes (a colour and a greeting for the time of year).
//!
//! Only colours and words: nothing here animates, so the launcher still
//! costs nothing while you play.

/// The accent colour presets: settings name, shown name, 0xRRGGBB.
pub const ACCENTS: [(&str, &str, u32); 8] = [
    ("violet", "Feline violet", 0x8f7ef7),
    ("blue", "Sky blue", 0x5b9cf5),
    ("teal", "Teal", 0x3fbfc4),
    ("green", "Catnip green", 0x62c97a),
    ("pink", "Pink", 0xf07aa8),
    ("orange", "Ginger", 0xf0954a),
    ("red", "Red", 0xee6070),
    ("gold", "Gold", 0xe0b24a),
];

/// Feline's own accent.
pub const VIOLET: u32 = 0x8f7ef7;

/// An accent setting: a preset name or `#rrggbb` → its colour. Unknown
/// values are Feline's violet.
pub fn accent_colour(setting: &str) -> u32 {
    if let Some(hex) = setting.strip_prefix('#') {
        return parse_hex(hex).unwrap_or(VIOLET);
    }
    ACCENTS
        .iter()
        .find(|(name, _, _)| *name == setting)
        .map_or(VIOLET, |(_, _, colour)| *colour)
}

/// `rrggbb` (or `rgb`) → 0xRRGGBB.
pub fn parse_hex(hex: &str) -> Option<u32> {
    let hex = hex.trim().trim_start_matches('#');
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        6 => u32::from_str_radix(hex, 16).ok(),
        3 => {
            let v = u32::from_str_radix(hex, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
            Some((r * 17) << 16 | (g * 17) << 8 | (b * 17))
        }
        _ => None,
    }
}

/// The colours the interface derives from one accent (all 0xRRGGBB, plus
/// alpha where noted as 0xRRGGBBAA).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub accent: u32,
    /// Pressed buttons, the end of the Play button's gradient.
    pub strong: u32,
    /// The start of the Play button's gradient, switch knobs.
    pub light: u32,
    /// The Play button when hovered.
    pub hover: u32,
    /// Pale text on dark backgrounds.
    pub lavender: u32,
    /// Very dark tinted surfaces (cards with a hint of the accent).
    pub deep: u32,
    pub deeper: u32,
    /// Readable text on the accent: white, or near-black on pale accents.
    pub on_accent: u32,
}

fn channels(c: u32) -> [f32; 3] {
    [((c >> 16) & 0xff) as f32, ((c >> 8) & 0xff) as f32, (c & 0xff) as f32]
}

fn pack(c: [f32; 3]) -> u32 {
    let ch = |v: f32| v.round().clamp(0.0, 255.0) as u32;
    ch(c[0]) << 16 | ch(c[1]) << 8 | ch(c[2])
}

/// `a` mixed with `b`: 0 = all `a`, 1 = all `b`.
pub fn mix(a: u32, b: u32, amount: f32) -> u32 {
    let (a, b) = (channels(a), channels(b));
    pack([0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * amount))
}

/// Relative brightness, 0..1 (sRGB, roughly).
fn luminance(c: u32) -> f32 {
    let [r, g, b] = channels(c);
    (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
}

/// WCAG contrast ratio between two colours (1 to 21).
pub fn contrast(a: u32, b: u32) -> f32 {
    let lin = |c: u32| {
        let [r, g, b] = channels(c).map(|v| {
            let v = v / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        });
        0.2126 * r + 0.7152 * g + 0.0722 * b
    };
    let (x, y) = (lin(a), lin(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

impl Palette {
    pub fn from_accent(accent: u32) -> Self {
        // Feline's own violet keeps its hand-picked shades exactly.
        if accent == VIOLET {
            return Self {
                accent,
                strong: 0x7a67f2,
                light: 0x9988fa,
                hover: 0xa596ff,
                lavender: 0xcdc5ff,
                deep: 0x231f3d,
                deeper: 0x2a2148,
                on_accent: 0xffffff,
            };
        }
        const SURFACE: u32 = 0x15161e;
        Self {
            accent,
            strong: mix(accent, 0x000000, 0.12),
            light: mix(accent, 0xffffff, 0.08),
            hover: mix(accent, 0xffffff, 0.16),
            lavender: mix(accent, 0xffffff, 0.62),
            deep: mix(SURFACE, accent, 0.14),
            deeper: mix(SURFACE, accent, 0.2),
            on_accent: if luminance(accent) > 0.72 { 0x15161e } else { 0xffffff },
        }
    }

    /// The accent family for the light theme: the tinted surfaces become
    /// pale washes, and "lavender" (accent-coloured text and icons) gets
    /// darker so it stays readable on white.
    pub fn from_accent_light(accent: u32) -> Self {
        let dark = Self::from_accent(accent);
        // Darken toward the light den's ink until it reads on white (4.5:1).
        let mut amount = 0.3;
        let mut ink = mix(accent, 0x1c1b26, amount);
        while contrast(ink, 0xffffff) < 4.6 && amount < 0.9 {
            amount += 0.05;
            ink = mix(accent, 0x1c1b26, amount);
        }
        Self {
            lavender: ink,
            deep: mix(0xffffff, accent, 0.13),
            deeper: mix(0xffffff, accent, 0.2),
            ..dark
        }
    }

    /// The accent family for `mode`.
    pub fn for_mode(accent: u32, light: bool) -> Self {
        if light { Self::from_accent_light(accent) } else { Self::from_accent(accent) }
    }
}

/// Light or dark.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Appearance {
    /// The Night-Lit Den (Feline's own look).
    #[default]
    Dark,
    Light,
    /// Whatever the system is set to.
    System,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Self::Dark, Self::Light, Self::System];

    pub fn parse(value: &str) -> Self {
        match value {
            "light" => Self::Light,
            "system" => Self::System,
            _ => Self::Dark,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::System => "system",
        }
    }

    /// Whether this means the light theme right now.
    pub fn is_light(self) -> bool {
        match self {
            Self::Dark => false,
            Self::Light => true,
            Self::System => system_prefers_light(),
        }
    }
}

/// Whether the system asks apps for a light look. Windows: Settings →
/// Personalization → Colors. Linux: the desktop's `color-scheme` setting
/// (GNOME, and KDE through its GTK bridge); dark when it doesn't say.
pub fn system_prefers_light() -> bool {
    #[cfg(windows)]
    {
        feline_win::apps_use_light_theme().unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
            .ok()
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains("prefer-light"))
    }
}

/// Feline's neutral colours (everything but the accent), 0xRRGGBB or
/// 0xRRGGBBAA where the name says `_a`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Neutrals {
    pub bg: u32,
    pub sidebar: u32,
    pub surface: u32,
    pub surface_raised: u32,
    pub surface_hover: u32,
    pub border: u32,
    pub border_strong: u32,
    pub inset: u32,
    pub hero_mid: u32,
    pub hero_end: u32,
    pub wash_a: u32,
    pub wash_hover_a: u32,
    pub track_a: u32,
    pub scrim_a: u32,
    pub text: u32,
    pub text_muted: u32,
    pub text_faint: u32,
    pub success: u32,
    pub warning: u32,
    pub danger: u32,
    pub info: u32,
    pub lift_a: u32,
    pub pop_a: u32,
}

impl Neutrals {
    /// The Night-Lit Den: cool slate, a few shades apart.
    pub const DARK: Neutrals = Neutrals {
        bg: 0x111218,
        sidebar: 0x0d0e13,
        surface: 0x171820,
        surface_raised: 0x1d1f29,
        surface_hover: 0x242633,
        border: 0x252735,
        border_strong: 0x343749,
        inset: 0x0b0c10,
        hero_mid: 0x1a1a28,
        hero_end: 0x15161e,
        wash_a: 0xffffff08,
        wash_hover_a: 0xffffff12,
        track_a: 0xffffff0c,
        scrim_a: 0x05030acc,
        text: 0xebebf3,
        text_muted: 0xa5a7ba,
        text_faint: 0x6d7086,
        success: 0x5fcf98,
        warning: 0xe9b860,
        danger: 0xef6f80,
        info: 0x67b4f0,
        lift_a: 0x00000059,
        pop_a: 0x00000073,
    };

    /// The same den with the lights on: paper-white surfaces on a cool grey
    /// floor, ink-dark text, status colours deep enough to read on white.
    pub const LIGHT: Neutrals = Neutrals {
        bg: 0xf3f3f7,
        sidebar: 0xe9e9f0,
        surface: 0xffffff,
        surface_raised: 0xf7f7fa,
        surface_hover: 0xeeeef4,
        border: 0xe1e1ea,
        border_strong: 0xcbcbd8,
        inset: 0xf6f6f9,
        hero_mid: 0xf7f6fc,
        hero_end: 0xfcfcfe,
        wash_a: 0x1c1b260a,
        wash_hover_a: 0x1c1b2614,
        track_a: 0x1c1b260f,
        scrim_a: 0x1c1b2680,
        text: 0x1c1b26,
        text_muted: 0x52526a,
        text_faint: 0x727289,
        success: 0x1d8a56,
        warning: 0x9a6200,
        danger: 0xc9354b,
        info: 0x1f72b8,
        lift_a: 0x1c1b261c,
        pop_a: 0x1c1b2633,
    };

    pub fn for_mode(light: bool) -> Self {
        if light { Self::LIGHT } else { Self::DARK }
    }
}

/// A time of year with its own look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Season {
    Winter,
    Hearts,
    Spring,
    Summer,
    Autumn,
    Spooky,
}

impl Season {
    pub const ALL: [Season; 6] = [
        Self::Winter,
        Self::Hearts,
        Self::Spring,
        Self::Summer,
        Self::Autumn,
        Self::Spooky,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Winter => "winter",
            Self::Hearts => "hearts",
            Self::Spring => "spring",
            Self::Summer => "summer",
            Self::Autumn => "autumn",
            Self::Spooky => "spooky",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == value)
    }

    /// The name shown in the settings.
    pub fn title(self) -> &'static str {
        match self {
            Self::Winter => "Snowy winter (Dec 1 – Jan 6)",
            Self::Hearts => "Hearts (Feb 7 – 14)",
            Self::Spring => "Spring blossom (Mar 20 – Apr 30)",
            Self::Summer => "Sunny summer (Jun 21 – Aug 31)",
            Self::Autumn => "Cosy autumn (Sep 22 – Nov 30)",
            Self::Spooky => "Spooky (Oct 20 – Nov 1)",
        }
    }

    /// The season's accent colour.
    pub fn accent(self) -> u32 {
        match self {
            Self::Winter => 0x6fb6f2,
            Self::Hearts => 0xf27aa8,
            Self::Spring => 0x5ccdb0,
            Self::Summer => 0xf0bd45,
            Self::Autumn => 0xe58450,
            Self::Spooky => 0xf28a3a,
        }
    }

    /// The colour of the little dots sprinkled on the Home page.
    pub fn sprinkle(self) -> u32 {
        match self {
            Self::Winter => 0xffffff,
            Self::Hearts => 0xff9cc2,
            Self::Spring => 0xffc4dc,
            Self::Summer => 0xffe08a,
            Self::Autumn => 0xf2a65a,
            Self::Spooky => 0xb48cff,
        }
    }

    /// A short line for the Home page.
    pub fn greeting(self) -> &'static str {
        match self {
            Self::Winter => "Snow is falling. Perfect weather to curl up and play.",
            Self::Hearts => "Sending you lots of purrs this week.",
            Self::Spring => "Spring is here: windows open, whiskers twitching.",
            Self::Summer => "Sunny days. Don't forget to stretch in a warm spot.",
            Self::Autumn => "Leaves are falling. Great season for chasing them.",
            Self::Spooky => "Spooky season! Black cats welcome.",
        }
    }

    /// The season on a (month 1–12, day 1–31) date, if any.
    pub fn on(month: u32, day: u32) -> Option<Self> {
        let md = month * 100 + day;
        match md {
            1201..=1231 | 101..=106 => Some(Self::Winter),
            207..=214 => Some(Self::Hearts),
            320..=430 => Some(Self::Spring),
            621..=831 => Some(Self::Summer),
            1020..=1101 => Some(Self::Spooky),
            922..=1130 => Some(Self::Autumn),
            _ => None,
        }
    }
}

/// The seasonal theme setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SeasonChoice {
    /// Follow the calendar.
    #[default]
    Auto,
    Off,
    /// Always this season (to try it out, or because you like it).
    Always(Season),
}

impl SeasonChoice {
    pub fn parse(value: &str) -> Self {
        match value {
            "off" => Self::Off,
            "auto" | "" => Self::Auto,
            other => Season::parse(other).map_or(Self::Auto, Self::Always),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Always(season) => season.as_str(),
        }
    }

    /// The season to show at Unix time `now` (local time via
    /// `offset_seconds`, east positive).
    pub fn season_at(self, now: u64, offset_seconds: i64) -> Option<Season> {
        match self {
            Self::Off => None,
            Self::Always(season) => Some(season),
            Self::Auto => {
                let (_, month, day) = civil_date(now, offset_seconds);
                Season::on(month, day)
            }
        }
    }
}

/// (year, month 1–12, day 1–31) of Unix time `now`, local time.
pub fn civil_date(now: u64, offset_seconds: i64) -> (i64, u32, u32) {
    let days = (now as i64 + offset_seconds).div_euclid(86_400);
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The accent to use: a season's colour replaces Feline's violet, but never
/// a colour you picked yourself.
pub fn effective_accent(accent_setting: &str, season: Option<Season>) -> u32 {
    let chosen = accent_colour(accent_setting);
    match season {
        Some(season) if chosen == VIOLET => season.accent(),
        _ => chosen,
    }
}

/// `0xRRGGBB` → `#rrggbb`.
pub fn hex(colour: u32) -> String {
    format!("#{:06x}", colour & 0xff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_dens_keep_text_readable() {
        for n in [Neutrals::DARK, Neutrals::LIGHT] {
            assert!(contrast(n.text, n.surface) >= 7.0);
            assert!(contrast(n.text_muted, n.surface) >= 4.5, "{n:?}");
            assert!(contrast(n.text_faint, n.surface) >= 3.0, "{n:?}");
        }
        // Accent "ink" on white stays readable for every preset.
        for (_, _, accent) in ACCENTS {
            let p = Palette::from_accent_light(accent);
            assert!(contrast(p.lavender, Neutrals::LIGHT.surface) >= 4.5, "{accent:06x}");
        }
        for a in Appearance::ALL {
            assert_eq!(Appearance::parse(a.as_str()), a);
        }
        assert_eq!(Appearance::parse("purple"), Appearance::Dark);
    }

    #[test]
    fn accents_and_hex() {
        assert_eq!(accent_colour("violet"), VIOLET);
        assert_eq!(accent_colour("teal"), 0x3fbfc4);
        assert_eq!(accent_colour("#112233"), 0x112233);
        assert_eq!(accent_colour("#abc"), 0xaabbcc);
        assert_eq!(accent_colour("#nope"), VIOLET);
        assert_eq!(accent_colour("whatever"), VIOLET);
        assert_eq!(hex(0x0a0b0c), "#0a0b0c");
        assert_eq!(parse_hex("12345"), None);
    }

    #[test]
    fn palette_keeps_violet_exact_and_derives_others() {
        assert_eq!(Palette::from_accent(VIOLET).strong, 0x7a67f2);
        let p = Palette::from_accent(0x3fbfc4);
        assert!(luminance(p.strong) < luminance(p.accent));
        assert!(luminance(p.lavender) > luminance(p.accent));
        assert_eq!(Palette::from_accent(0xffffff).on_accent, 0x15161e);
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
    }

    #[test]
    fn dates_and_seasons() {
        // 2026-11-10 12:00 UTC.
        assert_eq!(civil_date(1_794_312_000, 0), (2026, 11, 10));
        assert_eq!(civil_date(0, 0), (1970, 1, 1));
        // 2024-02-29 is a leap day.
        assert_eq!(civil_date(1_709_164_800, 0), (2024, 2, 29));
        // Just before midnight UTC is already the next day two hours east.
        assert_eq!(civil_date(1_794_268_799, 7200), (2026, 11, 10));
        assert_eq!(Season::on(12, 24), Some(Season::Winter));
        assert_eq!(Season::on(1, 6), Some(Season::Winter));
        assert_eq!(Season::on(1, 7), None);
        assert_eq!(Season::on(10, 31), Some(Season::Spooky));
        assert_eq!(Season::on(11, 10), Some(Season::Autumn));
        assert_eq!(Season::on(2, 14), Some(Season::Hearts));
        assert_eq!(Season::on(5, 15), None);
        assert_eq!(SeasonChoice::Off.season_at(1_794_312_000, 0), None);
        assert_eq!(
            SeasonChoice::Auto.season_at(1_794_312_000, 0),
            Some(Season::Autumn)
        );
        for choice in [SeasonChoice::Auto, SeasonChoice::Off, SeasonChoice::Always(Season::Spring)] {
            assert_eq!(SeasonChoice::parse(choice.as_str()), choice);
        }
    }

    #[test]
    fn seasons_only_replace_the_default_accent() {
        assert_eq!(effective_accent("violet", Some(Season::Winter)), Season::Winter.accent());
        assert_eq!(effective_accent("teal", Some(Season::Winter)), 0x3fbfc4);
        assert_eq!(effective_accent("violet", None), VIOLET);
    }
}
