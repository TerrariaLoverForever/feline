// SPDX-License-Identifier: GPL-3.0-or-later
//! Where new versions of Roblox's Android app come from: APKPure's app
//! version list. The endpoint and its headers are the ones Mocktail uses
//! (<https://github.com/komaruworld/mocktail>, Apache-2.0,
//! src/update/apkpure_provider.cc); the parsing is Feline's own.
//!
//! The answer is protobuf. Each release holds its version name (field 6),
//! then its file type (field 8: "APK" or "XAPK") and download link
//! (field 9). Downloads are only trusted after the signature check
//! (`apksig`), never because of where they came from.

const URL: &str =
    "https://api.pureapk.com/m/v3/cms/app_version?hl=en-US&package_name=com.roblox.client";
const DOWNLOAD_HOSTS: [&str; 3] = ["pureapk.com", "apkpure.com", "winudf.com"];

/// One downloadable version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// e.g. "2.746.1266".
    pub version: String,
    pub url: String,
    /// A bundle of split APKs (XAPK) rather than one APK.
    pub xapk: bool,
}

/// Asks APKPure for Roblox's releases (newest first).
pub fn releases() -> Result<Vec<Release>, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .build()
        .new_agent();
    let mut response = agent
        .get(URL)
        .header("x-cv", "3172501")
        .header("x-sv", "29")
        .header("x-gp", "1")
        .header("x-abis", "x86_64")
        .call()
        .map_err(|e| format!("APKPure: {e}"))?;
    let body = response
        .body_mut()
        .with_config()
        .limit(4 << 20)
        .read_to_vec()
        .map_err(|e| format!("APKPure: {e}"))?;
    let mut found = parse(&body);
    found.sort_by_key(|r| std::cmp::Reverse(version_key(&r.version)));
    if found.is_empty() {
        return Err("APKPure listed no downloadable Roblox version".into());
    }
    Ok(found)
}

/// "2.746.1266" → [2, 746, 1266], for ordering versions.
pub fn version_key(version: &str) -> Vec<u64> {
    version.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

fn varint(data: &[u8], at: &mut usize) -> Option<usize> {
    let mut value = 0usize;
    for shift in (0..35).step_by(7) {
        let byte = *data.get(*at)?;
        *at += 1;
        value |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn trusted(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', ':']).next().unwrap_or("");
    DOWNLOAD_HOSTS
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
}

/// The releases in APKPure's answer: each version name (`0x32` + length +
/// "N.N.N"), then the first download link after it.
pub fn parse(data: &[u8]) -> Vec<Release> {
    // Where each version name starts.
    let mut names = Vec::new();
    let mut i = 0;
    while i + 2 < data.len() {
        if data[i] == 0x32 {
            let len = data[i + 1] as usize;
            if (5..=20).contains(&len)
                && let Some(text) = data.get(i + 2..i + 2 + len)
                && let Ok(text) = std::str::from_utf8(text)
                && text.split('.').count() >= 3
                && text
                    .split('.')
                    .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            {
                names.push((i, text.to_owned()));
                i += 2 + len;
                continue;
            }
        }
        i += 1;
    }
    let mut releases = Vec::new();
    for (n, (start, version)) in names.iter().enumerate() {
        let end = names.get(n + 1).map_or(data.len(), |(s, _)| *s);
        let record = &data[*start..end];
        // field 8 (type) then field 9 (link): 0x42 len "APK"|"XAPK" 0x4a len url
        let mut j = 0;
        while j + 6 < record.len() {
            let kind = if record[j..].starts_with(b"\x42\x03APK\x4a") {
                Some((false, 6))
            } else if record[j..].starts_with(b"\x42\x04XAPK\x4a") {
                Some((true, 7))
            } else {
                None
            };
            if let Some((xapk, skip)) = kind {
                let mut at = j + skip;
                if let Some(len) = varint(record, &mut at)
                    && let Some(url) = record.get(at..at + len)
                    && let Ok(url) = std::str::from_utf8(url)
                    && trusted(url)
                {
                    releases.push(Release {
                        version: version.clone(),
                        url: url.to_owned(),
                        xapk,
                    });
                    break;
                }
            }
            j += 1;
        }
    }
    releases
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(version: &str, kind: &str, url: &str) -> Vec<u8> {
        let mut r = vec![0x2a, 3, b'1', b'2', b'3', 0x32, version.len() as u8];
        r.extend_from_slice(version.as_bytes());
        r.extend_from_slice(&[0x3a, 2, b'x', b'y', 0x42, kind.len() as u8]);
        r.extend_from_slice(kind.as_bytes());
        r.push(0x4a);
        let mut len = url.len();
        while len >= 0x80 {
            r.push((len as u8 & 0x7f) | 0x80);
            len >>= 7;
        }
        r.push(len as u8);
        r.extend_from_slice(url.as_bytes());
        r
    }

    #[test]
    fn finds_versions_and_trusted_links() {
        let long = format!("https://download.pureapk.com/b/APK/{}", "a".repeat(300));
        let mut data = record("2.746.1266", "APK", &long);
        data.extend(record("2.742.1059", "XAPK", "https://d.winudf.com/x"));
        data.extend(record("2.740.1", "APK", "https://evil.example/x"));
        let found = parse(&data);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].version, "2.746.1266");
        assert_eq!(found[0].url, long);
        assert!(!found[0].xapk);
        assert!(found[1].xapk);
        assert!(version_key("2.746.1266") > version_key("2.742.1059"));
    }
}
