// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline updating itself, from the GitHub releases of Feline's repository.
//!
//! A release is used only if it carries the program for this system
//! (`feline.exe` on Windows, `feline` on Linux) **and** its SHA-256 (a
//! `<program>.sha256` file next to it, or `SHA256: <hex>` in the release
//! notes). The download must match it, or nothing is changed.
//!
//! Installing renames the running program to `feline.old.exe` (Windows lets
//! a running program be renamed, not overwritten) and puts the new one in
//! its place; it's used from the next start.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

/// Where Feline's releases are published.
pub const REPO: &str = "TerrariaLoverForever/feline";

/// The program's file name in releases.
pub const PROGRAM: &str = if cfg!(windows) { "feline.exe" } else { "feline" };

/// A newer Feline, ready to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

fn agent(timeout: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout)))
        .user_agent(format!("Feline/{}", crate::VERSION))
        .build()
        .into()
}

/// `0.10.2` > `0.9.7`; a leading `v` and anything after `-` or `+` are
/// ignored.
pub fn newer(candidate: &str, current: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim_start_matches(['v', 'V'])
            .split(['-', '+'])
            .next()
            .unwrap_or_default()
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parts(candidate), parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// A 64-character hex SHA-256 in `text` after `SHA256:` (any case), or the
/// first word of a `.sha256` file.
fn find_sha256(text: &str) -> Option<String> {
    let hex = |w: &str| (w.len() == 64 && w.chars().all(|c| c.is_ascii_hexdigit())).then(|| w.to_ascii_lowercase());
    if let Some(first) = text.split_whitespace().next().and_then(hex) {
        return Some(first);
    }
    let lower = text.to_ascii_lowercase();
    let at = lower.find("sha256:")? + "sha256:".len();
    text[at..].split_whitespace().next().and_then(hex)
}

/// Reads GitHub's "latest release" answer. `None`: no usable program in it.
pub fn parse_release(v: &Value, checksum_file: impl Fn(&str) -> Option<String>) -> Option<Release> {
    let version = v.get("tag_name")?.as_str()?.trim_start_matches(['v', 'V']).to_owned();
    let notes = v.get("body").and_then(Value::as_str).unwrap_or_default().to_owned();
    let assets = v.get("assets")?.as_array()?;
    let asset = |name: &str| assets.iter().find(|a| a.get("name").and_then(Value::as_str) == Some(name));
    let program = asset(PROGRAM)?;
    let url = program.get("browser_download_url")?.as_str()?.to_owned();
    let size = program.get("size").and_then(Value::as_u64).unwrap_or(0);
    let sha256 = asset(&format!("{PROGRAM}.sha256"))
        .and_then(|a| a.get("browser_download_url")?.as_str())
        .and_then(checksum_file)
        .and_then(|t| find_sha256(&t))
        .or_else(|| find_sha256(&notes))?;
    url.starts_with("https://").then_some(Release { version, notes, url, sha256, size })
}

/// The newest release, if it's newer than this Feline.
pub fn check() -> Result<Option<Release>, String> {
    let text = agent(15)
        .get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => "no Feline releases are published yet".to_owned(),
            e => format!("GitHub couldn't be reached ({e})"),
        })?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|_| "GitHub answered with something unexpected".to_owned())?;
    let fetch = |url: &str| agent(15).get(url).call().ok()?.body_mut().read_to_string().ok();
    Ok(parse_release(&v, fetch).filter(|r| newer(&r.version, crate::VERSION)))
}

/// Downloads `release`, checks it, and puts it in place of the running
/// program (`exe`). The old one stays as `feline.old(.exe)`.
pub fn install(release: &Release, exe: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, String> {
    use std::io::Read as _;
    let dir = exe.parent().ok_or("Feline's folder wasn't found")?;
    let mut reader = agent(600)
        .get(&release.url)
        .call()
        .map_err(|e| format!("the download didn't start ({e})"))?
        .into_body()
        .into_reader();
    let mut data = Vec::new();
    let mut chunk = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut chunk).map_err(|e| format!("the download broke off ({e})"))?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
        if data.len() > 512 * 1024 * 1024 {
            return Err("the download is far too big; not using it".to_owned());
        }
        progress(data.len() as u64, release.size);
    }
    let hash = ring::digest::digest(&ring::digest::SHA256, &data);
    let hex: String = hash.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    if hex != release.sha256 {
        return Err("the download doesn't match the release's checksum, so Feline didn't use it".to_owned());
    }
    let new = dir.join(format!("{PROGRAM}.new"));
    fs::write(&new, &data).map_err(|e| format!("couldn't save it next to Feline ({e})"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&new, fs::Permissions::from_mode(0o755));
    }
    let old = dir.join(if cfg!(windows) { "feline.old.exe" } else { "feline.old" });
    let _ = fs::remove_file(&old);
    fs::rename(exe, &old).map_err(|e| format!("couldn't move the old Feline aside ({e})"))?;
    if let Err(e) = fs::rename(&new, exe) {
        let _ = fs::rename(&old, exe);
        return Err(format!("couldn't put the new Feline in place ({e})"));
    }
    Ok(exe.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare() {
        assert!(newer("0.2.0", "0.1.9"));
        assert!(newer("v0.10.0", "0.9.9"));
        assert!(newer("1.0", "0.99.99"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0-beta", "0.1.0"));
        assert!(!newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn releases_need_the_program_and_its_checksum() {
        let sum = "ab".repeat(32);
        let v: Value = serde_json::from_str(&format!(
            r#"{{"tag_name": "v0.2.0", "body": "Cats.\nSHA256: {sum}", "assets": [
                {{"name": "{PROGRAM}", "size": 10, "browser_download_url": "https://github.com/x/{PROGRAM}"}}]}}"#
        ))
        .unwrap();
        let r = parse_release(&v, |_| None).unwrap();
        assert_eq!((r.version.as_str(), r.sha256.as_str(), r.size), ("0.2.0", sum.as_str(), 10));
        // A checksum file wins over the notes.
        let other = "cd".repeat(32);
        let v2: Value = serde_json::from_str(&format!(
            r#"{{"tag_name": "0.3.0", "assets": [
                {{"name": "{PROGRAM}", "browser_download_url": "https://github.com/x/p"}},
                {{"name": "{PROGRAM}.sha256", "browser_download_url": "https://github.com/x/s"}}]}}"#
        ))
        .unwrap();
        assert_eq!(parse_release(&v2, |_| Some(format!("{other}  {PROGRAM}\n"))).unwrap().sha256, other);
        // No checksum anywhere: not used.
        let v3: Value = serde_json::from_str(&format!(
            r#"{{"tag_name": "0.3.0", "assets": [{{"name": "{PROGRAM}", "browser_download_url": "https://github.com/x/p"}}]}}"#
        ))
        .unwrap();
        assert!(parse_release(&v3, |_| None).is_none());
    }
}
