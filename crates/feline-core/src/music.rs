// SPDX-License-Identifier: GPL-3.0-or-later
//! Background music for Feline's window (About → Music): the folder your
//! tracks live in, adding a file you have, and fetching a song from a link.
//!
//! Playback itself is in the app (it needs an audio device); this is the
//! part that only deals with files and programs, so it can be tested.
//!
//! Links are fetched with [yt-dlp](https://github.com/yt-dlp/yt-dlp), a
//! separate program the user installs (Feline doesn't ship it): it knows
//! YouTube, SoundCloud, Bandcamp and many more sites. Spotify's own audio is
//! DRM-protected and can't be downloaded by anyone, so for a Spotify track
//! Feline reads its title and artist from Spotify's page and asks yt-dlp to
//! find that song on YouTube instead.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::paths;

/// The audio files Feline can play (what the app's decoders understand).
/// No WebM/Opus: links are fetched as M4A, which every site yt-dlp knows
/// offers without needing ffmpeg.
pub const EXTENSIONS: &[&str] = &["mp3", "m4a", "mp4", "aac", "ogg", "oga", "flac", "wav"];

/// The music folder: `~/.local/share/feline/music` (Windows:
/// `%LOCALAPPDATA%\Feline\Music`). Not the config folder, which roams on
/// Windows and shouldn't carry megabytes of songs.
pub fn dir() -> Option<PathBuf> {
    #[cfg(windows)]
    return paths::state_dir().map(|d| d.join("Music"));
    #[cfg(not(windows))]
    return paths::data_home().map(|d| d.join("feline").join("music"));
}

/// True if Feline can play `path` (by its extension).
pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// The tracks in `dir`, by name (file names, sorted case-insensitively).
pub fn list(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| is_audio(&e.path()))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// A track's name without its extension, for showing.
pub fn title(name: &str) -> &str {
    Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
}

/// Copies an audio file into the music folder. Returns the track's name.
pub fn import_file(source: &Path) -> Result<String, String> {
    if !is_audio(source) {
        return Err(format!(
            "Feline can't play that kind of file. It plays {}.",
            EXTENSIONS.join(", ")
        ));
    }
    let dir = dir().ok_or("your home folder couldn't be found")?;
    fs::create_dir_all(&dir).map_err(|e| format!("couldn't make the music folder ({e})"))?;
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("that file's name can't be read")?
        .to_owned();
    let target = dir.join(&name);
    if target != source {
        fs::copy(source, &target).map_err(|e| format!("couldn't copy it ({e})"))?;
    }
    Ok(name)
}

/// Deletes a track from the music folder. Only plain names in that folder.
pub fn remove(name: &str) -> io::Result<()> {
    if name.contains(['/', '\\']) || name.starts_with('.') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a track name"));
    }
    let dir = dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no music folder"))?;
    fs::remove_file(dir.join(name))
}

/// Where a link points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// A Spotify track: its audio is DRM-protected, so it's looked up on
    /// YouTube by title and artist.
    SpotifyTrack(String),
    /// A Spotify album, playlist, artist, … (not supported: one song at a time).
    SpotifyOther,
    /// Anything else on the web: yt-dlp is asked to fetch it.
    Web(String),
}

/// Reads what the user pasted.
pub fn classify(text: &str) -> Result<Link, String> {
    let url = text.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Paste a link that starts with https://".to_owned());
    }
    if url.contains("://open.spotify.com/") || url.contains("://spotify.link/") {
        // open.spotify.com/track/ID, also /intl-xx/track/ID.
        return Ok(if url.contains("/track/") || url.contains("spotify.link/") {
            Link::SpotifyTrack(url.to_owned())
        } else {
            Link::SpotifyOther
        });
    }
    Ok(Link::Web(url.to_owned()))
}

/// "Song - song and lyrics by Artist | Spotify" → "Song Artist", a YouTube
/// search for the song. `None` if it doesn't look like a track page.
fn spotify_search_terms(page_title: &str) -> Option<String> {
    let title = decode_entities(page_title.trim());
    let title = title.strip_suffix("| Spotify").unwrap_or(&title).trim();
    let (song, artist) = match title.split_once(" - song and lyrics by ") {
        Some((song, artist)) => (song, artist),
        None => match title.split_once(" - song by ") {
            Some((song, artist)) => (song, artist),
            None => (title, ""),
        },
    };
    let terms = format!("{} {}", song.trim(), artist.trim());
    let terms = terms.trim();
    (!terms.is_empty() && terms != "Spotify").then(|| terms.to_owned())
}

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// The text between `<title>` and `</title>`.
fn html_title(html: &str) -> Option<&str> {
    let start = html.find("<title")?;
    let start = start + html[start..].find('>')? + 1;
    let end = start + html[start..].find("</title>")?;
    Some(&html[start..end])
}

/// Looks a Spotify track up on its public web page: "Song Artist".
fn spotify_lookup(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .user_agent(format!("Feline/{}", crate::VERSION))
        .build()
        .into();
    let html = agent
        .get(url)
        .call()
        .map_err(|e| format!("Spotify couldn't be reached ({e})"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    html_title(&html)
        .and_then(spotify_search_terms)
        .ok_or_else(|| "Feline couldn't read that song's name from Spotify".to_owned())
}

/// yt-dlp, if it's installed: on `PATH`, or (Windows) next to Feline.
pub fn ytdlp() -> Option<PathBuf> {
    let name = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };
    let beside = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join(name)));
    let in_music = dir().map(|d| d.join(name));
    // winget's links folder: on PATH, but only for programs started after
    // yt-dlp was installed, so Feline looks there itself.
    let winget = cfg!(windows)
        .then(|| paths::data_home().map(|d| d.join("Microsoft/WinGet/Links").join(name)))
        .flatten();
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join(name));
    beside.into_iter().chain(in_music).chain(winget).chain(on_path).find(|p| p.is_file())
}

/// What to tell someone without yt-dlp.
pub fn ytdlp_missing() -> String {
    if cfg!(windows) {
        "Fetching from a link needs yt-dlp. Install it with \"winget install yt-dlp\" in a terminal, then try again.".to_owned()
    } else {
        "Fetching from a link needs yt-dlp. Install it from your distribution's packages (often \"yt-dlp\"), then try again.".to_owned()
    }
}

/// Fetches the song at a link into the music folder (slow: runs on a
/// background thread). Returns the new track's name.
pub fn import_link(text: &str) -> Result<String, String> {
    let target = match classify(text)? {
        Link::SpotifyOther => {
            return Err(
                "That's a Spotify album, playlist or artist. Paste a link to a single song."
                    .to_owned(),
            );
        }
        Link::SpotifyTrack(url) => format!("ytsearch1:{} audio", spotify_lookup(&url)?),
        Link::Web(url) => url,
    };
    let ytdlp = ytdlp().ok_or_else(ytdlp_missing)?;
    let dir = dir().ok_or("your home folder couldn't be found")?;
    fs::create_dir_all(&dir).map_err(|e| format!("couldn't make the music folder ({e})"))?;

    let mut command = Command::new(ytdlp);
    command
        .arg("--no-playlist")
        .arg("--no-progress")
        .arg("--quiet")
        .arg("--no-warnings")
        // One song, not a three-hour mix.
        .args(["--max-filesize", "60M"])
        // M4A/AAC or MP3 if the site has it: playable without ffmpeg.
        .args(["-f", "bestaudio[ext=m4a]/bestaudio[ext=mp3]/best[ext=mp4]"])
        .arg("--windows-filenames")
        .args(["--trim-filenames", "120"])
        .arg("-P")
        .arg(&dir)
        .args(["-o", "%(title)s.%(ext)s"])
        .args(["--print", "after_move:filepath"])
        .arg("--")
        .arg(&target)
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output().map_err(|e| format!("yt-dlp couldn't start ({e})"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let line = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no details")
            .trim_start_matches("ERROR: ");
        if line.contains("Requested format is not available") {
            return Err("That site doesn't offer this song as M4A or MP3, which Feline can play.".to_owned());
        }
        return Err(format!("yt-dlp couldn't fetch it: {line}"));
    }
    let path = String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| PathBuf::from(l.trim()))
        .ok_or("yt-dlp didn't fetch anything (was the song too long?)")?;
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .ok_or_else(|| "yt-dlp saved a file Feline can't name".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_classified() {
        assert_eq!(
            classify(" https://open.spotify.com/track/abc?si=1 "),
            Ok(Link::SpotifyTrack("https://open.spotify.com/track/abc?si=1".into()))
        );
        assert_eq!(
            classify("https://open.spotify.com/intl-de/track/abc"),
            Ok(Link::SpotifyTrack("https://open.spotify.com/intl-de/track/abc".into()))
        );
        assert_eq!(classify("https://open.spotify.com/playlist/x"), Ok(Link::SpotifyOther));
        assert_eq!(
            classify("https://youtu.be/x"),
            Ok(Link::Web("https://youtu.be/x".into()))
        );
        assert!(classify("never gonna give you up").is_err());
        assert!(classify("-o /etc/passwd").is_err());
    }

    #[test]
    fn spotify_titles_become_searches() {
        assert_eq!(
            spotify_search_terms("Cat&#x27;s Song - song and lyrics by Some Band | Spotify").as_deref(),
            Some("Cat's Song Some Band")
        );
        assert_eq!(
            spotify_search_terms("Purr - song by Kitty | Spotify").as_deref(),
            Some("Purr Kitty")
        );
        assert_eq!(spotify_search_terms("Spotify"), None);
        assert_eq!(html_title("<html><title id=\"x\">Hi | Spotify</title>"), Some("Hi | Spotify"));
    }

    #[test]
    fn audio_files_and_titles() {
        assert!(is_audio(Path::new("a/b/Song.MP3")));
        assert!(is_audio(Path::new("x.m4a")));
        assert!(!is_audio(Path::new("x.webm")));
        assert!(!is_audio(Path::new("mp3")));
        assert_eq!(title("Nice tune.flac"), "Nice tune");
        assert!(remove("../settings.conf").is_err());
    }
}
