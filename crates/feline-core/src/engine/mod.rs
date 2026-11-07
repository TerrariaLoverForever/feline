// SPDX-License-Identifier: GPL-3.0-or-later
//! Roblox's Android app for Feline's runtime: finding the latest version,
//! downloading it, checking it's really Roblox's, installing it next to the
//! previous one, and going back if a new version doesn't work.

pub mod apkpure;
pub mod apksig;
pub mod zip;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// SHA-256 of Roblox's APK signing certificate ("Roblox Corporation",
/// San Mateo; 1024-bit RSA from 2010), as `apksigner` prints it. A
/// download signed by anything else is rejected.
pub const ROBLOX_CERT_SHA256: &str =
    "44932ea35a17a267372d71b54d1a0cb3da0dca5113e94406ae2fe18090ba1477";

const LIBRARY: &str = "lib/x86_64/libroblox.so";
/// Bigger downloads are refused (the APK is about 240 MB).
const MAX_DOWNLOAD: u64 = 1 << 30;

/// `~/.local/share/feline/engine`: where the runtime looks first.
pub fn engine_dir() -> Option<PathBuf> {
    Some(crate::paths::data_home()?.join("feline").join("engine"))
}

fn versions_dir(engine: &Path) -> PathBuf {
    engine.join("versions")
}

/// The version `current` points at, if Feline installed it.
pub fn current_version() -> Option<String> {
    current_in(&engine_dir()?)
}

fn current_in(engine: &Path) -> Option<String> {
    let target = std::fs::read_link(engine.join("current")).ok()?;
    Some(target.file_name()?.to_string_lossy().into_owned())
}

/// What `update` is doing, for a progress display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Progress {
    Checking,
    /// Bytes so far, and the total if known.
    Downloading(u64, Option<u64>),
    Verifying,
    Installing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    UpToDate(String),
    /// A new version, and the one it replaced.
    Installed {
        version: String,
        previous: Option<String>,
    },
}

/// Installs the newest Roblox version, if it's newer than the current one
/// and not one that already failed here.
pub fn update(progress: &mut dyn FnMut(Progress)) -> Result<Outcome, String> {
    let engine = engine_dir().ok_or("no home folder")?;
    progress(Progress::Checking);
    let current = current_in(&engine);
    let releases = apkpure::releases()?;
    let latest = releases
        .iter()
        .find(|r| !versions_dir(&engine).join(&r.version).join("bad").exists())
        .ok_or("only versions that failed here are available")?;
    if let Some(cur) = &current
        && apkpure::version_key(cur) >= apkpure::version_key(&latest.version)
    {
        return Ok(Outcome::UpToDate(cur.clone()));
    }
    install(&engine, latest, progress)?;
    Ok(Outcome::Installed {
        version: latest.version.clone(),
        previous: current,
    })
}

fn install(
    engine: &Path,
    release: &apkpure::Release,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), String> {
    let versions = versions_dir(engine);
    std::fs::create_dir_all(&versions).map_err(|e| e.to_string())?;
    // Already downloaded and checked before: just switch to it.
    let existing = versions.join(&release.version);
    if existing.join("base.apk").is_file() && existing.join("libroblox.so").is_file() {
        progress(Progress::Installing);
        switch_to(engine, &release.version)?;
        prune(engine);
        return Ok(());
    }
    let download = versions.join(format!(".{}.download", release.version));
    fetch(&release.url, &download, progress)?;
    progress(Progress::Verifying);
    let data = std::fs::read(&download).map_err(|e| e.to_string())?;
    let unpacked = unpack(&data, release.xapk);
    let _ = std::fs::remove_file(&download);
    let (base, library) = unpacked?;
    progress(Progress::Installing);
    let staging = versions.join(format!(".{}.new", release.version));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    std::fs::write(staging.join("base.apk"), &base).map_err(|e| e.to_string())?;
    std::fs::write(staging.join("libroblox.so"), &library).map_err(|e| e.to_string())?;
    let target = versions.join(&release.version);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&staging, &target).map_err(|e| e.to_string())?;
    switch_to(engine, &release.version)?;
    prune(engine);
    Ok(())
}

/// Streams `url` into `path`.
fn fetch(url: &str, path: &Path, progress: &mut dyn FnMut(Progress)) -> Result<(), String> {
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(15)))
        .build()
        .new_agent();
    let mut response = agent
        .get(url)
        .call()
        .map_err(|e| format!("download: {e}"))?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
    if total.is_some_and(|t| t > MAX_DOWNLOAD) {
        return Err("the download is unexpectedly large".into());
    }
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD)
        .reader();
    let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut buffer = vec![0u8; 1 << 16];
    let (mut done, mut shown) = (0u64, 0u64);
    loop {
        let n = reader
            .read(&mut buffer)
            .map_err(|e| format!("download: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if done - shown >= 1 << 20 {
            shown = done;
            progress(Progress::Downloading(done, total));
        }
    }
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(())
}

/// Checks an APK is Roblox's, by its signature and pinned certificate.
fn check_signed(apk: &[u8]) -> Result<(), String> {
    let signer = apksig::verify(apk)?;
    if signer.certificate_sha256 != ROBLOX_CERT_SHA256 {
        return Err(format!(
            "signed by someone other than Roblox (certificate {})",
            signer.certificate_sha256
        ));
    }
    Ok(())
}

/// (base APK, x86_64 engine library) from a download: one APK with the
/// library inside, or an XAPK bundle of split APKs. Every APK used is
/// signature-checked first.
fn unpack(data: &[u8], xapk: bool) -> Result<(Vec<u8>, Vec<u8>), String> {
    {
        let entries = zip::entries(data).ok_or("the download isn't a zip file")?;
        if !xapk && entries.contains_key("AndroidManifest.xml") {
            check_signed(data)?;
            let lib = entries.get(LIBRARY).ok_or("the APK has no x86_64 engine")?;
            return Ok((data.to_vec(), zip::read(data, lib)?));
        }
        // XAPK: split APKs inside a zip.
        let (mut base, mut library) = (None, None);
        for (name, entry) in &entries {
            if !name.ends_with(".apk") {
                continue;
            }
            let apk = zip::read(data, entry)?;
            let inner = zip::entries(&apk).ok_or("a split isn't a zip file")?;
            if let Some(lib) = inner.get(LIBRARY) {
                check_signed(&apk)?;
                library = Some(zip::read(&apk, lib)?);
            }
            if inner.contains_key("classes.dex") {
                check_signed(&apk)?;
                base = Some(apk);
            }
        }
        Ok((
            base.ok_or("the bundle has no base APK")?,
            library.ok_or("the bundle has no x86_64 engine")?,
        ))
    }
}

/// Points `current` (and the runtime's `libroblox.so`/`base.apk` links
/// through it) at `version`, atomically.
fn switch_to(engine: &Path, version: &str) -> Result<(), String> {
    use std::os::unix::fs::symlink;
    let link = engine.join(".current.new");
    let _ = std::fs::remove_file(&link);
    symlink(Path::new("versions").join(version), &link).map_err(|e| e.to_string())?;
    std::fs::rename(&link, engine.join("current")).map_err(|e| e.to_string())?;
    for name in ["libroblox.so", "base.apk"] {
        let path = engine.join(name);
        let wanted = Path::new("current").join(name);
        if std::fs::read_link(&path).ok().as_deref() != Some(wanted.as_path()) {
            let _ = std::fs::remove_file(&path);
            symlink(&wanted, &path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Keeps the current version and the newest working other one.
fn prune(engine: &Path) {
    let current = current_in(engine);
    let Ok(list) = std::fs::read_dir(versions_dir(engine)) else {
        return;
    };
    let mut others: Vec<String> = list
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && Some(n) != current.as_ref())
        .collect();
    others.sort_by_key(|v| std::cmp::Reverse(apkpure::version_key(v)));
    let keep = others
        .iter()
        .find(|v| versions_dir(engine).join(v).join("ok").exists())
        .cloned();
    for v in others {
        if Some(&v) != keep.as_ref() {
            let _ = std::fs::remove_dir_all(versions_dir(engine).join(&v));
        }
    }
}

/// How many times the runtime started a version (it writes `starts`).
fn starts(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("starts"))
        .map(|s| s.len())
        .unwrap_or(0)
}

/// If the current version was started twice and never reached Roblox's app
/// (the runtime writes `ok` when it does), marks it bad and goes back to the
/// newest version that worked. A note for the user, if it did.
pub fn roll_back_if_broken() -> Option<String> {
    roll_back_in(&engine_dir()?)
}

fn roll_back_in(engine: &Path) -> Option<String> {
    let engine = engine.to_path_buf();
    let current = current_in(&engine)?;
    let dir = versions_dir(&engine).join(&current);
    if dir.join("ok").exists() || starts(&dir) < 2 {
        return None;
    }
    let mut good: Vec<String> = std::fs::read_dir(versions_dir(&engine))
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|v| *v != current && versions_dir(&engine).join(v).join("ok").exists())
        .collect();
    good.sort_by_key(|v| std::cmp::Reverse(apkpure::version_key(v)));
    let _ = std::fs::write(dir.join("bad"), "");
    let Some(back) = good.first().cloned() else {
        // No installed version worked yet: remove the links, so the runtime
        // uses an engine copied by hand (~/feline-engine), if there is one.
        for name in ["current", "libroblox.so", "base.apk"] {
            let _ = std::fs::remove_file(engine.join(name));
        }
        return Some(format!(
            "Roblox {current} didn't start with Feline's runtime, so Feline stopped using it."
        ));
    };
    switch_to(&engine, &back).ok()?;
    Some(format!(
        "Roblox {current} didn't start with Feline's runtime, so Feline went back to {back}."
    ))
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn version(engine: &Path, v: &str, ok: bool) {
        let dir = versions_dir(engine).join(v);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("libroblox.so"), v).unwrap();
        std::fs::write(dir.join("base.apk"), v).unwrap();
        if ok {
            std::fs::write(dir.join("ok"), "").unwrap();
        }
    }

    #[test]
    fn switching_rolling_back_and_pruning() {
        let engine = std::env::temp_dir().join(format!(
            "feline-test-roblox-versions-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&engine);
        version(&engine, "2.742.1059", true);
        version(&engine, "2.740.1", true);
        version(&engine, "2.746.1266", false);
        switch_to(&engine, "2.746.1266").unwrap();
        assert_eq!(current_in(&engine).as_deref(), Some("2.746.1266"));
        assert_eq!(
            std::fs::read_to_string(engine.join("libroblox.so")).unwrap(),
            "2.746.1266"
        );
        // Keeps the current one and the newest that worked.
        prune(&engine);
        assert!(!versions_dir(&engine).join("2.740.1").exists());
        assert!(versions_dir(&engine).join("2.742.1059").exists());
        // Started once without reaching the app: not yet.
        std::fs::write(versions_dir(&engine).join("2.746.1266/starts"), "x").unwrap();
        assert!(roll_back_in(&engine).is_none());
        // Twice: back to the working version, the new one marked bad.
        std::fs::write(versions_dir(&engine).join("2.746.1266/starts"), "xx").unwrap();
        assert!(roll_back_in(&engine).is_some());
        assert_eq!(current_in(&engine).as_deref(), Some("2.742.1059"));
        assert_eq!(
            std::fs::read_to_string(engine.join("base.apk")).unwrap(),
            "2.742.1059"
        );
        assert!(versions_dir(&engine).join("2.746.1266/bad").exists());
        let _ = std::fs::remove_dir_all(&engine);
    }
}

#[cfg(test)]
mod tests {
    /// Verifies the installed base.apk (needs ~/feline-engine/base.apk).
    #[test]
    #[ignore]
    fn installed_apk_is_signed() {
        let path =
            std::path::PathBuf::from(std::env::var("HOME").unwrap()).join("feline-engine/base.apk");
        let apk = std::fs::read(path).unwrap();
        let signer = super::apksig::verify(&apk).unwrap();
        println!("certificate sha256: {}", signer.certificate_sha256);
        // The engine inside it is the installed one (compared by hash:
        // never print these buffers, they're over 100 MB).
        let (_, library) = super::unpack(&apk, false)
            .map_err(|e| e.to_string())
            .unwrap();
        let installed = std::fs::read(
            std::path::PathBuf::from(std::env::var("HOME").unwrap())
                .join("feline-engine/libroblox.so"),
        )
        .unwrap();
        let hash = |b: &[u8]| {
            ring::digest::digest(&ring::digest::SHA256, b)
                .as_ref()
                .to_vec()
        };
        assert!(
            hash(&library) == hash(&installed),
            "extracted engine differs"
        );
        let mut tampered = apk;
        tampered[1000] ^= 1;
        assert!(super::apksig::verify(&tampered).is_err());
    }
}

#[cfg(test)]
mod live_tests {
    /// Parses a saved APKPure answer (FELINE_APKPURE_SAMPLE=path).
    #[test]
    #[ignore]
    fn saved_answer_parses() {
        let path = std::env::var("FELINE_APKPURE_SAMPLE").unwrap();
        let found = super::apkpure::parse(&std::fs::read(path).unwrap());
        for r in found.iter().take(3) {
            println!("{} xapk={} {}…", r.version, r.xapk, &r.url[..60]);
        }
        assert!(!found.is_empty());
    }
}
