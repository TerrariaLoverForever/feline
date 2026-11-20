// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline updating itself, from the GitHub releases of Feline's repository.
//!
//! A release is used only if it carries the program for this system
//! (`feline.exe` on Windows, `feline` on Linux) **and** its SHA-256 (a
//! `<program>.sha256` file next to it, or `SHA256: <hex>` in the release
//! notes). The download must match it, or nothing is changed.
//!
//! On Linux a release also carries Feline's runtime and login window
//! (`feline-runtime`, `feline-login`), each with its `.sha256` file. They
//! belong to the same version as the launcher, so those of them that are
//! installed next to the launcher are updated with it. Everything is
//! downloaded and checked before the first file is replaced.
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

/// Programs released together with the launcher, updated with it when they
/// sit next to it. Feline's runtime runs Roblox's x86-64 engine, so it's
/// only built for x86-64 PCs.
pub const COMPANIONS: &[&str] = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    &["feline-runtime", "feline-login"]
} else if cfg!(target_os = "linux") {
    &["feline-login"]
} else {
    &[]
};

/// A program's file name in releases for a PC with the CPU `arch`
/// (`std::env::consts::ARCH`): x86-64 builds keep the plain names they've
/// always had (older Feline versions look for those), ARM64 Linux builds
/// end in `-aarch64`. The program keeps its plain name once installed.
pub fn release_name(name: &str, arch: &str) -> String {
    if arch == "aarch64" && !name.ends_with(".exe") {
        format!("{name}-aarch64")
    } else {
        name.to_owned()
    }
}

fn this_pc(name: &str) -> String {
    release_name(name, std::env::consts::ARCH)
}

/// One file of a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// A newer Feline, ready to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// The [`COMPANIONS`] this release carries with a checksum.
    pub companions: Vec<Download>,
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
    let program = asset(&this_pc(PROGRAM))?;
    let url = program.get("browser_download_url")?.as_str()?.to_owned();
    let size = program.get("size").and_then(Value::as_u64).unwrap_or(0);
    let sha256 = asset(&format!("{}.sha256", this_pc(PROGRAM)))
        .and_then(|a| a.get("browser_download_url")?.as_str())
        .and_then(&checksum_file)
        .and_then(|t| find_sha256(&t))
        .or_else(|| find_sha256(&notes))?;
    let companions = COMPANIONS
        .iter()
        .filter_map(|name| {
            let program = asset(&this_pc(name))?;
            let url = program.get("browser_download_url")?.as_str()?;
            let sum = asset(&format!("{}.sha256", this_pc(name)))?.get("browser_download_url")?.as_str()?;
            let sha256 = find_sha256(&checksum_file(sum)?)?;
            let size = program.get("size").and_then(Value::as_u64).unwrap_or(0);
            url.starts_with("https://").then(|| Download { name: (*name).to_owned(), url: url.to_owned(), sha256, size })
        })
        .collect();
    url.starts_with("https://").then_some(Release { version, notes, url, sha256, size, companions })
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

/// Downloads `url` and checks it against `sha256`. `progress(bytes)` is
/// called as it arrives.
fn fetch(url: &str, sha256: &str, progress: &mut dyn FnMut(u64)) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let mut reader = agent(600)
        .get(url)
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
        progress(data.len() as u64);
    }
    if sha256_hex(&data) != sha256 {
        return Err("the download doesn't match the release's checksum, so Feline didn't use it".to_owned());
    }
    Ok(data)
}

fn sha256_hex(data: &[u8]) -> String {
    let hash = ring::digest::digest(&ring::digest::SHA256, data);
    hash.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Saves `data` as a program next to `target`, ready to be renamed onto it.
fn stage(target: &Path, data: &[u8]) -> Result<PathBuf, String> {
    let mut name = target.file_name().ok_or("Feline's folder wasn't found")?.to_owned();
    name.push(".new");
    let new = target.with_file_name(name);
    fs::write(&new, data).map_err(|e| format!("couldn't save it next to Feline ({e})"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&new, fs::Permissions::from_mode(0o755));
    }
    Ok(new)
}

/// The release's companions that are installed next to `exe`: the ones an
/// update has to replace.
fn installed_companions<'a>(release: &'a Release, exe: &Path) -> Vec<(&'a Download, PathBuf)> {
    release.companions.iter().map(|c| (c, exe.with_file_name(&c.name))).filter(|(_, path)| path.is_file()).collect()
}

/// Downloads `release`, checks it, and puts it in place of the running
/// program (`exe`). The old one stays as `feline.old(.exe)`. Feline's
/// runtime and login window next to it are replaced too (Linux).
pub fn install(release: &Release, exe: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, String> {
    let dir = exe.parent().ok_or("Feline's folder wasn't found")?;
    let companions = installed_companions(release, exe);
    let total = release.size + companions.iter().map(|(c, _)| c.size).sum::<u64>();

    // Everything is downloaded and checked first: a broken download leaves
    // the installed Feline as it was.
    let data = fetch(&release.url, &release.sha256, &mut |n| progress(n, total))?;
    let mut done = data.len() as u64;
    let mut staged = Vec::new();
    for (companion, path) in &companions {
        let bytes = fetch(&companion.url, &companion.sha256, &mut |n| progress(done + n, total))
            .map_err(|e| format!("{}: {e}", companion.name))?;
        done += bytes.len() as u64;
        staged.push((stage(path, &bytes)?, path));
    }
    let new = stage(exe, &data)?;

    // A program that's running can be renamed over on Linux; it keeps
    // running from the old file.
    for (from, to) in &staged {
        fs::rename(from, to).map_err(|e| format!("couldn't put the new {} in place ({e})", to.display()))?;
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
    fn release_names_by_cpu() {
        assert_eq!(release_name("feline", "x86_64"), "feline");
        assert_eq!(release_name("feline-login", "x86_64"), "feline-login");
        assert_eq!(release_name("feline", "aarch64"), "feline-aarch64");
        assert_eq!(release_name("feline-login", "aarch64"), "feline-login-aarch64");
        assert_eq!(release_name("feline.exe", "aarch64"), "feline.exe");
    }

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
        // The release's file name for this PC (plain on x86-64, -aarch64 on ARM).
        let program = this_pc(PROGRAM);
        let sum = "ab".repeat(32);
        let v: Value = serde_json::from_str(&format!(
            r#"{{"tag_name": "v0.2.0", "body": "Cats.\nSHA256: {sum}", "assets": [
                {{"name": "{program}", "size": 10, "browser_download_url": "https://github.com/x/{program}"}}]}}"#
        ))
        .unwrap();
        let r = parse_release(&v, |_| None).unwrap();
        assert_eq!((r.version.as_str(), r.sha256.as_str(), r.size), ("0.2.0", sum.as_str(), 10));
        // A checksum file wins over the notes.
        let other = "cd".repeat(32);
        let v2: Value = serde_json::from_str(&format!(
            r#"{{"tag_name": "0.3.0", "assets": [
                {{"name": "{program}", "browser_download_url": "https://github.com/x/p"}},
                {{"name": "{program}.sha256", "browser_download_url": "https://github.com/x/s"}}]}}"#
        ))
        .unwrap();
        assert_eq!(parse_release(&v2, |_| Some(format!("{other}  {program}\n"))).unwrap().sha256, other);
        // No checksum anywhere: not used.
        let v3: Value = serde_json::from_str(&format!(
            r#"{{"tag_name": "0.3.0", "assets": [{{"name": "{program}", "browser_download_url": "https://github.com/x/p"}}]}}"#
        ))
        .unwrap();
        assert!(parse_release(&v3, |_| None).is_none());
    }

    /// Linux releases carry companions (the runtime on x86-64, the login
    /// window); only those with a checksum count, and only installed ones
    /// are replaced. Uses this PC's release names and companions.
    #[cfg(target_os = "linux")]
    #[test]
    fn companions_come_with_the_launcher() {
        let sum = "ab".repeat(32);
        let program = this_pc(PROGRAM);
        let companion = COMPANIONS[0];
        let released = this_pc(companion);
        let unsummed = this_pc("feline-unsummed");
        let v: Value = serde_json::from_str(&format!(
            r#"{{"tag_name": "v0.2.0", "assets": [
                {{"name": "{program}", "size": 10, "browser_download_url": "https://github.com/x/{program}"}},
                {{"name": "{program}.sha256", "browser_download_url": "https://github.com/x/{program}.sha256"}},
                {{"name": "{released}", "size": 20, "browser_download_url": "https://github.com/x/{released}"}},
                {{"name": "{released}.sha256", "browser_download_url": "https://github.com/x/{released}.sha256"}},
                {{"name": "{unsummed}", "size": 5, "browser_download_url": "https://github.com/x/{unsummed}"}}]}}"#,
        ))
        .unwrap();
        let r = parse_release(&v, |_| Some(format!("{sum}  file\n"))).unwrap();
        let names: Vec<&str> = r.companions.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, [companion], "only companions with a checksum are used");
        assert_eq!((r.companions[0].size, r.companions[0].sha256.as_str()), (20, sum.as_str()));

        let dir = std::env::temp_dir().join(format!("feline-selfupdate-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("feline");
        assert!(installed_companions(&r, &exe).is_empty(), "not next to the launcher: left alone");
        // Installed under its plain name, whatever the release calls it.
        fs::write(dir.join(companion), b"old").unwrap();
        let found = installed_companions(&r, &exe);
        assert_eq!(found.len(), 1);
        let new = stage(&found[0].1, b"new").unwrap();
        assert_eq!(new, dir.join(format!("{companion}.new")));
        assert_eq!(fs::read(&new).unwrap(), b"new");
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        let _ = fs::remove_dir_all(&dir);
    }
}
