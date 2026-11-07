// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Staying logged in: the Roblox session cookie.
//!
//! Logging in inside the app needs things Feline doesn't have (the login
//! page's security checks run in an Android WebView). Instead you log in
//! once on Roblox's own website, in Feline's login window, which keeps only
//! the `.ROBLOSECURITY` session cookie, and at every start Feline gives it
//! to the engine the way the Android app's Java code does:
//! `NativeSettingsInterface.nativeSetMultipleCookies("https://www.roblox.com/",
//! ".ROBLOSECURITY=…")` (as mocktail does too).
//!
//! The cookie is the login itself: anyone with it is logged in as you. It's
//! kept in `<runtime data>/roblox-cookie`, readable only by you (mode 600;
//! a file others can read is refused), and never written to a log.

use std::path::{Path, PathBuf};

const COOKIE_NAME: &str = ".ROBLOSECURITY";

/// Where the cookie is kept.
pub fn cookie_path(runtime_data: &Path) -> PathBuf {
    runtime_data.join("roblox-cookie")
}

/// A cookie value Feline accepts: printable ASCII with no separators, so it
/// can't smuggle in other cookies or attributes (`;`, `,`, spaces, `=` are
/// refused). Roblox's values are long `_|WARNING:-…|_` tokens.
fn valid_value(value: &str) -> bool {
    value.len() >= 20
        && value.len() <= 8192
        && value
            .bytes()
            .all(|b| b.is_ascii_graphic() && !matches!(b, b';' | b',' | b'=' | b'"' | b'\\'))
}

/// The saved cookie as a `Cookie:` pair (`.ROBLOSECURITY=…`), if there is a
/// usable one. Explains in the log (without the value) why not otherwise.
pub fn saved_cookie(runtime_data: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let path = cookie_path(runtime_data);
    let metadata = std::fs::metadata(&path).ok()?;
    if metadata.permissions().mode() & 0o077 != 0 {
        eprintln!(
            "feline-runtime: not using {}: other users can read it (chmod 600 it, or log in again)",
            path.display()
        );
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let value = text.trim();
    let value = value
        .strip_prefix(COOKIE_NAME)
        .and_then(|v| v.strip_prefix('='))
        .unwrap_or(value);
    if !valid_value(value) {
        eprintln!(
            "feline-runtime: not using {}: it doesn't hold a Roblox session cookie",
            path.display()
        );
        return None;
    }
    Some(format!("{COOKIE_NAME}={value}"))
}

/// Saves `value` (the cookie's value, or `.ROBLOSECURITY=value`) for the
/// next starts, readable only by you.
pub fn save_cookie(runtime_data: &Path, value: &str) -> Result<PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let value = value.trim();
    let value = value
        .strip_prefix(COOKIE_NAME)
        .and_then(|v| v.strip_prefix('='))
        .unwrap_or(value);
    if !valid_value(value) {
        return Err("that isn't a Roblox session cookie".into());
    }
    std::fs::create_dir_all(runtime_data).map_err(|e| e.to_string())?;
    let path = cookie_path(runtime_data);
    // Written to a new private file, then moved into place.
    let temporary = path.with_extension("new");
    let _ = std::fs::remove_file(&temporary);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| format!("couldn't write {}: {e}", temporary.display()))?;
    file.write_all(value.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("couldn't write {}: {e}", temporary.display()))?;
    std::fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Forgets the login (logging out of Feline).
pub fn forget_cookie(runtime_data: &Path) {
    let _ = std::fs::remove_file(cookie_path(runtime_data));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_that_could_inject_are_refused() {
        assert!(valid_value(
            "_|WARNING:-DO-NOT-SHARE-THIS.--|_ABCDEF0123456789"
        ));
        assert!(!valid_value("short"));
        assert!(!valid_value(
            "_|WARNING:-DO-NOT-SHARE-THIS|_abc; Domain=evil.test"
        ));
        assert!(!valid_value("_|WARNING:-DO-NOT-SHARE-THIS|_abc, other=1"));
        assert!(!valid_value("_|WARNING:-DO-NOT-SHARE-THIS|_abc other"));
    }

    #[test]
    fn saved_privately_and_read_back() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("feline-login-test-{}", std::process::id()));
        let value = "_|WARNING:-DO-NOT-SHARE-THIS.--|_TESTVALUE0123456789";
        let path = save_cookie(&dir, &format!(".ROBLOSECURITY={value}")).expect("saved");
        let mode = std::fs::metadata(&path)
            .expect("exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(saved_cookie(&dir), Some(format!(".ROBLOSECURITY={value}")));
        // Readable by others: refused.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert_eq!(saved_cookie(&dir), None);
        forget_cookie(&dir);
        assert_eq!(saved_cookie(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
