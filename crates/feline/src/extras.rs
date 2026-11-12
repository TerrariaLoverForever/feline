// SPDX-License-Identifier: GPL-3.0-or-later
//! The just-for-fun options (About → Make it yours, Desk cat, Music, Achievements)
//! and "clear when Feline closes" (Settings → Privacy): showing them,
//! changing them, and putting them to work (the accent colour on the
//! `Theme`, the season on the Home page, the desk cat, the achievements).

use feline_core::achievements::{self, Facts};
use feline_core::history::{History, local_offset_seconds, now};
use feline_core::look::{self, ACCENTS, Neutrals, Palette, Season, SeasonChoice};
use feline_core::settings::Settings;
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

use crate::gui::{AchievementTile, AppWindow, Extras, Shared, Theme, lock};

fn colour(rgb: u32) -> Color {
    Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// 0xRRGGBBAA.
fn rgba(c: u32) -> Color {
    Color::from_argb_u8(c as u8, (c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8)
}

fn colour_alpha(rgb: u32, alpha: u8) -> Color {
    Color::from_argb_u8(alpha, (rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// The season showing now, by the settings.
fn current_season(settings: &Settings) -> Option<Season> {
    settings.season.season_at(now(), local_offset_seconds())
}

/// Puts everything on screen: colours, season, the options' values, the cat
/// preview and the achievements. Also starts/stops the desk cat.
pub fn refresh(ui: &AppWindow, shared: &Shared) {
    let settings = lock(shared).settings.clone();
    let extras = ui.global::<Extras>();

    // Colours: the den's neutrals for light or dark, and the accent family.
    let season = current_season(&settings);
    let accent = look::effective_accent(&settings.accent, season);
    let light = settings.appearance.is_light();
    let p = Palette::for_mode(accent, light);
    let n = Neutrals::for_mode(light);
    let theme = ui.global::<Theme>();
    theme.set_dark(!light);
    theme.set_bg(colour(n.bg));
    theme.set_sidebar(colour(n.sidebar));
    theme.set_surface(colour(n.surface));
    theme.set_surface_raised(colour(n.surface_raised));
    theme.set_surface_hover(colour(n.surface_hover));
    theme.set_border(colour(n.border));
    theme.set_border_strong(colour(n.border_strong));
    theme.set_inset(colour(n.inset));
    theme.set_hero_mid(colour(n.hero_mid));
    theme.set_hero_end(colour(n.hero_end));
    theme.set_wash(rgba(n.wash_a));
    theme.set_wash_hover(rgba(n.wash_hover_a));
    theme.set_track(rgba(n.track_a));
    theme.set_scrim(rgba(n.scrim_a));
    theme.set_text(colour(n.text));
    theme.set_text_muted(colour(n.text_muted));
    theme.set_text_faint(colour(n.text_faint));
    theme.set_success(colour(n.success));
    theme.set_warning(colour(n.warning));
    theme.set_danger(colour(n.danger));
    theme.set_info(colour(n.info));
    theme.set_lift_color(rgba(n.lift_a));
    theme.set_pop_color(rgba(n.pop_a));
    theme.set_accent(colour(p.accent));
    theme.set_accent_strong(colour(p.strong));
    theme.set_accent_soft(colour_alpha(p.accent, if light { 0x24 } else { 0x1f }));
    theme.set_accent_light(colour(p.light));
    theme.set_accent_hover(colour(p.hover));
    theme.set_accent_deep(colour(p.deep));
    theme.set_accent_deeper(colour(p.deeper));
    theme.set_lavender(colour(p.lavender));
    theme.set_accent_ink(colour(if light { p.lavender } else { p.accent }));
    theme.set_on_accent(colour(p.on_accent));
    extras.set_appearance_index(
        look::Appearance::ALL.iter().position(|a| *a == settings.appearance).unwrap_or(0) as i32,
    );

    // Look options.
    let names: Vec<SharedString> = ACCENTS.iter().map(|(_, name, _)| (*name).into()).collect();
    let swatches: Vec<Color> = ACCENTS.iter().map(|(_, _, c)| colour(*c)).collect();
    extras.set_accent_names(ModelRc::new(VecModel::from(names)));
    extras.set_accent_swatches(ModelRc::new(VecModel::from(swatches)));
    let index = ACCENTS
        .iter()
        .position(|(name, _, _)| *name == settings.accent)
        .unwrap_or(ACCENTS.len());
    extras.set_accent_index(index as i32);
    if settings.accent.starts_with('#') {
        extras.set_accent_hex(settings.accent.clone().into());
    }
    let mut seasons: Vec<SharedString> = vec!["Follow the calendar".into(), "Off".into()];
    seasons.extend(Season::ALL.iter().map(|s| format!("Always: {}", s.title()).into()));
    extras.set_season_names(ModelRc::new(VecModel::from(seasons)));
    extras.set_season_index(match settings.season {
        SeasonChoice::Auto => 0,
        SeasonChoice::Off => 1,
        SeasonChoice::Always(s) => 2 + Season::ALL.iter().position(|x| *x == s).unwrap_or(0) as i32,
    });
    extras.set_season_on(season.is_some());
    if let Some(season) = season {
        let title = season.title();
        let short = title.split(" (").next().unwrap_or(title);
        extras.set_season_title(short.into());
        extras.set_season_greeting(season.greeting().into());
        extras.set_sprinkle(colour(season.sprinkle()));
    }

    // Desk cat.
    let cat = settings.cat;
    extras.set_cat_supported(crate::desk_cat::SUPPORTED);
    extras.set_cat_on(cat.on);
    extras.set_cat_coat_index(
        feline_core::cat::Coat::ALL.iter().position(|c| *c == cat.coat).unwrap_or(0) as i32,
    );
    extras.set_cat_size_index(cat.scale.clamp(2, 4) as i32 - 2);
    extras.set_cat_when_index(i32::from(cat.only_playing));
    extras.set_cat_preview(crate::desk_cat::preview(cat.coat, accent));
    crate::desk_cat::configure(cat, accent);

    // Clear on exit.
    let c = settings.clear_on_exit;
    extras.set_exit_roblox_logs(c.roblox_logs);
    extras.set_exit_feline_logs(c.feline_logs);
    extras.set_exit_cache(c.roblox_cache);
    extras.set_exit_history(c.history);

    // Music.
    crate::music::refresh(ui, &settings);

    extras.set_achievements_on(settings.achievements);
    if settings.achievements {
        show_achievements(ui, &settings, false);
    }
}

/// Changes one option (see `Extras.set` in extras.slint) and saves.
pub fn set(ui: &AppWindow, shared: &Shared, which: &str, value: i32) {
    if crate::music::set(shared, which, value) {
        save(shared);
        crate::music::refresh(ui, &lock(shared).settings.clone());
        return;
    }
    {
        let mut state = lock(shared);
        let s = &mut state.settings;
        let on = value != 0;
        match which {
            "accent" => match ACCENTS.get(value.max(0) as usize) {
                Some((name, _, _)) => s.accent = (*name).to_owned(),
                None => return,
            },
            "season" => {
                s.season = match value {
                    0 => SeasonChoice::Auto,
                    1 => SeasonChoice::Off,
                    i => Season::ALL
                        .get((i - 2).max(0) as usize)
                        .map_or(SeasonChoice::Auto, |s| SeasonChoice::Always(*s)),
                }
            }
            "appearance" => {
                s.appearance = look::Appearance::ALL
                    .get(value.max(0) as usize)
                    .copied()
                    .unwrap_or_default();
            }
            "cat" => s.cat.on = on,
            "cat-coat" => {
                s.cat.coat = feline_core::cat::Coat::ALL
                    .get(value.max(0) as usize)
                    .copied()
                    .unwrap_or_default();
            }
            "cat-size" => s.cat.scale = (value.clamp(0, 2) + 2) as u32,
            "cat-when" => s.cat.only_playing = on,
            "achievements" => s.achievements = on,
            "exit-roblox-logs" => s.clear_on_exit.roblox_logs = on,
            "exit-feline-logs" => s.clear_on_exit.feline_logs = on,
            "exit-cache" => s.clear_on_exit.roblox_cache = on,
            "exit-history" => s.clear_on_exit.history = on,
            _ => return,
        }
    }
    ui.global::<Extras>().set_accent_message(SharedString::new());
    save(shared);
    refresh(ui, shared);
}

/// Your own accent colour, as typed.
pub fn set_accent_hex(ui: &AppWindow, shared: &Shared, text: &str) {
    let extras = ui.global::<Extras>();
    let Some(rgb) = look::parse_hex(text) else {
        extras.set_accent_message(
            "That's not a colour Feline knows. Try # and six hex digits, like #ff8fb1.".into(),
        );
        return;
    };
    extras.set_accent_message(SharedString::new());
    lock(shared).settings.accent = look::hex(rgb);
    save(shared);
    refresh(ui, shared);
}

fn save(shared: &Shared) {
    let settings = lock(shared).settings.clone();
    if let Some(path) = Settings::path()
        && let Err(e) = settings.save_to(&path)
    {
        eprintln!("feline: couldn't save the settings: {e}");
    }
}

/// "10 Nov 2026".
fn date_label(when: u64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (year, month, day) = look::civil_date(when, local_offset_seconds());
    format!("{day} {} {year}", MONTHS[(month as usize).saturating_sub(1).min(11)])
}

/// Works out the achievements (remembering new ones) and shows them. With
/// `announce`, new ones also get a notification.
pub fn show_achievements(ui: &AppWindow, settings: &Settings, announce: bool) {
    let history = History::load();
    let mut facts = Facts::from_history(&history, local_offset_seconds());
    facts.mods_on = !settings.mods.is_empty();
    facts.desk_cat = settings.cat.on && crate::desk_cat::SUPPORTED;
    facts.tidied = feline_core::housekeeping::last_tidy().is_some();
    facts.own_colour = settings.accent != "violet";
    let (new, unlocked) = achievements::update(&facts);
    if announce && !new.is_empty() {
        let title = if new.len() == 1 {
            format!("Achievement unlocked: {}", new[0].title)
        } else {
            format!("{} achievements unlocked!", new.len())
        };
        let text = new.iter().map(|a| a.title).collect::<Vec<_>>().join(", ");
        feline_core::launch::notify(&title, &text);
    }
    let tiles: Vec<AchievementTile> = achievements::ALL
        .iter()
        .map(|a| {
            let when = unlocked.get(a.id).copied();
            AchievementTile {
                title: a.title.into(),
                text: a.text.into(),
                unlocked: when.is_some(),
                when: when.map(date_label).unwrap_or_default().into(),
            }
        })
        .collect();
    let count = tiles.iter().filter(|t| t.unlocked).count();
    let extras = ui.global::<Extras>();
    extras.set_achievements_count(
        format!("{count} of {} unlocked", achievements::ALL.len()).into(),
    );
    extras.set_achievements(ModelRc::new(VecModel::from(tiles)));
}

/// A game ended: new achievements, and the cat goes back to sleep if it
/// only comes out while playing.
pub fn game_ended(ui: &AppWindow, shared: &Shared) {
    crate::desk_cat::set_playing(false);
    // "Rejoin" / "Server hop" on Home, now that the last server is known.
    ui.set_can_rejoin(feline_core::roblox_tools::LastServer::load().is_some());
    let settings = lock(shared).settings.clone();
    if settings.achievements {
        show_achievements(ui, &settings, true);
    }
}

/// Feline is closing: the cat goes home, then "clear when Feline closes"
/// (when `clear`).
pub fn on_exit(shared: &Shared, clear: bool) {
    crate::desk_cat::shutdown();
    crate::music::shutdown();
    if !clear {
        return;
    }
    let (choice, busy) = {
        let state = lock(shared);
        (state.settings.clear_on_exit, state.busy)
    };
    // A game Feline started may still be running (e.g. "close Feline after
    // starting"): its files are left alone; the next close clears them.
    if !choice.any() || busy {
        return;
    }
    let roots = crate::home_page::log_roots(shared);
    let result = feline_core::housekeeping::clear_on_exit(choice, &roots);
    for line in result.skipped {
        eprintln!("feline: not cleared on exit: {line}");
    }
}
