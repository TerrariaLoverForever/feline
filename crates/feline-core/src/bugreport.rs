// SPDX-License-Identifier: GPL-3.0-or-later
//! The bug report zip: one file to attach to a bug report, with the report
//! text, Feline's settings, the enabled mods, and the newest Roblox and
//! Feline logs.
//!
//! Before anything goes in, [`redact`] hides what's personal: the user's
//! name in folder paths, and whole lines that may hold a login secret
//! (cookies, tickets, tokens). The zip is written by hand (deflate from
//! `miniz_oxide`), so no zip library is needed.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Words that mark a line as possibly holding a secret.
const SECRET_WORDS: &[&str] = &[
    "roblosecurity",
    "cookie",
    "ticket",
    "token",
    "authorization",
    "password",
    "set-cookie",
];

/// Replaces the folder after `marker` (the user's name) with `<you>`.
fn hide_after(line: &str, marker: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find(marker) {
        let start = at + marker.len();
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end = tail.find(['\\', '/', '"', '\'', ' ', ',', ';']).unwrap_or(tail.len());
        out.push_str("<you>");
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Hides personal things in log or report text.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let lower = line.to_lowercase();
        if SECRET_WORDS.iter().any(|w| lower.contains(w)) {
            out.push_str("[line hidden by Feline: it may contain a login secret]\n");
            continue;
        }
        let mut line = line.to_owned();
        for marker in ["\\Users\\", "\\users\\", "/Users/", "/home/"] {
            line = hide_after(&line, marker);
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// A small zip writer
// ---------------------------------------------------------------------------

struct Entry {
    name: String,
    crc: u32,
    size: u32,
    compressed: u32,
    offset: u32,
}

/// Builds a zip in memory.
pub struct Zip {
    data: Vec<u8>,
    entries: Vec<Entry>,
    dos_time: u16,
    dos_date: u16,
}

impl Default for Zip {
    fn default() -> Self {
        Self::new()
    }
}

impl Zip {
    pub fn new() -> Self {
        let local = crate::history::now() as i64 + crate::history::local_offset_seconds();
        let (y, m, d) = civil_from_days(local.div_euclid(86_400));
        let secs = local.rem_euclid(86_400);
        Self {
            data: Vec::new(),
            entries: Vec::new(),
            dos_time: (((secs / 3600) << 11) | ((secs % 3600 / 60) << 5) | ((secs % 60) / 2)) as u16,
            dos_date: ((((y - 1980).max(0)) << 9) | (m << 5) | d) as u16,
        }
    }

    pub fn add(&mut self, name: &str, contents: &[u8]) {
        let compressed = miniz_oxide::deflate::compress_to_vec(contents, 6);
        let entry = Entry {
            name: name.to_owned(),
            crc: crate::png::crc32(contents),
            size: contents.len() as u32,
            compressed: compressed.len() as u32,
            offset: self.data.len() as u32,
        };
        let d = &mut self.data;
        d.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        d.extend_from_slice(&20u16.to_le_bytes()); // version needed
        d.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        d.extend_from_slice(&8u16.to_le_bytes()); // deflate
        d.extend_from_slice(&self.dos_time.to_le_bytes());
        d.extend_from_slice(&self.dos_date.to_le_bytes());
        d.extend_from_slice(&entry.crc.to_le_bytes());
        d.extend_from_slice(&entry.compressed.to_le_bytes());
        d.extend_from_slice(&entry.size.to_le_bytes());
        d.extend_from_slice(&(name.len() as u16).to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(name.as_bytes());
        d.extend_from_slice(&compressed);
        self.entries.push(entry);
    }

    pub fn finish(mut self) -> Vec<u8> {
        let start = self.data.len() as u32;
        let mut directory = Vec::new();
        for e in &self.entries {
            let d = &mut directory;
            d.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            d.extend_from_slice(&20u16.to_le_bytes()); // made by
            d.extend_from_slice(&20u16.to_le_bytes()); // needed
            d.extend_from_slice(&0x0800u16.to_le_bytes());
            d.extend_from_slice(&8u16.to_le_bytes());
            d.extend_from_slice(&self.dos_time.to_le_bytes());
            d.extend_from_slice(&self.dos_date.to_le_bytes());
            d.extend_from_slice(&e.crc.to_le_bytes());
            d.extend_from_slice(&e.compressed.to_le_bytes());
            d.extend_from_slice(&e.size.to_le_bytes());
            d.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            d.extend_from_slice(&[0; 12]); // extra, comment, disk, attributes
            d.extend_from_slice(&e.offset.to_le_bytes());
            d.extend_from_slice(e.name.as_bytes());
        }
        let count = self.entries.len() as u16;
        self.data.extend_from_slice(&directory);
        let d = &mut self.data;
        d.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        d.extend_from_slice(&[0; 4]); // disk numbers
        d.extend_from_slice(&count.to_le_bytes());
        d.extend_from_slice(&count.to_le_bytes());
        d.extend_from_slice(&(directory.len() as u32).to_le_bytes());
        d.extend_from_slice(&start.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes());
        self.data
    }
}

/// (year, month, day) for days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

/// The newest `count` files in `dir` matching `keep`, newest first.
fn newest(dir: &Path, count: usize, keep: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && keep(p))
        .map(|p| {
            let t = p
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            (t, p)
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files.into_iter().take(count).map(|(_, p)| p).collect()
}

/// A log's last 2 MB, redacted.
fn log_text(path: &Path) -> Option<String> {
    const MAX: usize = 2 * 1024 * 1024;
    let bytes = std::fs::read(path).ok()?;
    let tail = &bytes[bytes.len().saturating_sub(MAX)..];
    Some(redact(&String::from_utf8_lossy(tail)))
}

/// Where the zip goes: the Downloads folder (or home).
pub fn output_dir() -> Option<PathBuf> {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE")
    } else {
        std::env::var_os("HOME")
    }
    .map(PathBuf::from)?;
    let downloads = home.join("Downloads");
    Some(if downloads.is_dir() { downloads } else { home })
}

/// Writes the zip. `report` is the report text (system, checks, detective
/// and network results); `roblox_logs` are the folders Roblox logs to.
/// The zip's path.
pub fn write(report: &str, roblox_logs: &[PathBuf]) -> Result<PathBuf, String> {
    let mut zip = Zip::new();
    zip.add(
        "README.txt",
        b"Made by Feline for a bug report. Personal things were hidden first: your user name in folder paths, and every log line that might hold a login secret. Have a look before you share it.\r\n",
    );
    zip.add("report.txt", redact(report).as_bytes());

    if let Some(settings) = crate::paths::config_dir()
        .map(|d| d.join("settings.conf"))
        .and_then(|p| std::fs::read_to_string(p).ok())
    {
        zip.add("feline-settings.txt", redact(&settings).as_bytes());
    }

    let mut taken = 0;
    for dir in roblox_logs {
        for log in newest(dir, 3, |p| {
            let name = p.to_string_lossy().to_lowercase();
            name.ends_with(".log") && !name.contains("crashhandler")
        }) {
            if taken >= 3 {
                break;
            }
            if let (Some(text), Some(name)) = (log_text(&log), log.file_name()) {
                zip.add(&format!("roblox-logs/{}", name.to_string_lossy()), text.as_bytes());
                taken += 1;
            }
        }
    }
    if let Some(dir) = crate::paths::log_dir() {
        for log in newest(&dir, 3, |_| true) {
            if let (Some(text), Some(name)) = (log_text(&log), log.file_name()) {
                zip.add(&format!("feline-logs/{}", name.to_string_lossy()), text.as_bytes());
            }
        }
    }

    let local = crate::history::now() as i64 + crate::history::local_offset_seconds();
    let (y, m, d) = civil_from_days(local.div_euclid(86_400));
    let minutes = local.rem_euclid(86_400) / 60;
    let dir = output_dir().ok_or("Feline couldn't find your Downloads folder.")?;
    let path = dir.join(format!(
        "feline-bug-report-{y}-{m:02}-{d:02}-{:02}{:02}.zip",
        minutes / 60,
        minutes % 60
    ));
    std::fs::write(&path, zip.finish()).map_err(|e| format!("Couldn't save the zip: {e}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_things_are_hidden() {
        let text = "added file C:\\Users\\елена\\AppData\\Local\\Roblox\\logs\\x.log as attachment\n\
                    Cookie: .ROBLOSECURITY=_|WARNING secret\n\
                    path /home/cat/.local/share/feline and C:\\Users\\679E~1\\AppData\n";
        let out = redact(text);
        assert!(!out.contains("елена") && !out.contains("cat/") && !out.contains("679E"), "{out}");
        assert!(out.contains("C:\\Users\\<you>\\AppData\\Local"));
        assert!(out.contains("/home/<you>/.local"));
        assert!(!out.contains("WARNING secret"));
    }

    #[test]
    fn the_zip_is_readable() {
        let mut zip = Zip::new();
        zip.add("a.txt", b"hello hello hello hello");
        zip.add("dir/b.txt", b"");
        let bytes = zip.finish();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        // End of central directory: two entries.
        let end = bytes.len() - 22;
        assert_eq!(&bytes[end..end + 4], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([bytes[end + 10], bytes[end + 11]]), 2);
        // The first entry inflates back.
        let compressed_size = u32::from_le_bytes(bytes[18..22].try_into().unwrap()) as usize;
        let name_len = u16::from_le_bytes([bytes[26], bytes[27]]) as usize;
        let body = &bytes[30 + name_len..30 + name_len + compressed_size];
        assert_eq!(miniz_oxide::inflate::decompress_to_vec(body).unwrap(), b"hello hello hello hello");
    }

    #[test]
    fn dates_convert() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(crate::presence::log::days_from_civil(2026, 11, 10)), (2026, 11, 10));
    }
}
