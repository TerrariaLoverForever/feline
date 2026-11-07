// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Staying logged in: the Roblox session cookie.
//!
//! Logging in inside the app needs things Feline doesn't have (the login
//! page's security checks run in an Android WebView). So when the app shows
//! its Sign In or Create Account screen, Feline also opens its login window
//! (`feline-login`): Roblox's own website, which hands over only the
//! `.ROBLOSECURITY` session cookie. Feline saves it and restarts the app,
//! and at every start gives it to the engine the way the Android app's Java
//! code does:
//! `NativeSettingsInterface.nativeSetMultipleCookies("https://www.roblox.com/",
//! ".ROBLOSECURITY=…")` (as mocktail does too).
//!
//! The cookie is the login itself: anyone with it is logged in as you. It's
//! kept in `<runtime data>/roblox-cookie`, readable only by you (mode 600;
//! a file others can read is refused), and never written to a log.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Runs the login window (`feline-login`, next to this program or on the
/// `PATH`; `signup` starts it on the sign-up page) and saves the session
/// cookie it hands over through a pipe. Waits until the window closes.
pub fn open_window(runtime_data: &Path, signup: bool) -> Result<PathBuf, String> {
    use std::io::Read;
    let beside = std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name("feline-login"))
        .filter(|p| p.is_file());
    let program = beside.unwrap_or_else(|| "feline-login".into());
    let mut command = std::process::Command::new(&program);
    if signup {
        command.arg("--signup");
    }
    // Its standard input stays open while this program runs: it closes
    // the window when this program exits.
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't open the login window ({}): {e}", program.display()))?;
    let mut text = String::new();
    let read = child
        .stdout
        .take()
        .ok_or("no pipe from the login window")?
        .read_to_string(&mut text);
    drop(child.stdin.take());
    let status = child.wait().map_err(|e| e.to_string())?;
    if read.is_err() || !status.success() {
        return Err("the login window was closed without logging in".into());
    }
    let saved = save_cookie(runtime_data, &text);
    text.clear();
    saved
}

/// Where the running app keeps its data, set at start-up, so the login
/// window can save the session there.
static RUNTIME_DATA: OnceLock<PathBuf> = OnceLock::new();
/// The login window is open (only one at a time).
static WINDOW_OPEN: AtomicBool = AtomicBool::new(false);

/// Called at start-up with the runtime's data folder.
pub fn set_runtime_data(path: PathBuf) {
    let _ = RUNTIME_DATA.set(path);
}

/// The app's Sign In screen.
const LOGIN_SCREEN: &str = "Login";

/// Whether the app's screen `name` (from its `onAppReady` calls, e.g.
/// "Login" or "SinglePageSignUp") is one where you'd log in, and if so
/// whether it's for a new account.
fn login_screen(name: &str) -> Option<bool> {
    if name == LOGIN_SCREEN {
        Some(false)
    } else if name.contains("SignUp") {
        Some(true)
    } else {
        None
    }
}

/// The app showed screen `name`. On its Sign In or Create Account screen,
/// opens the login window next to the app (the in-app screen stays too),
/// and once you've logged in there, restarts the app with the session.
pub fn screen_shown(name: &str) {
    let Some(signup) = login_screen(name) else {
        return;
    };
    let Some(data) = RUNTIME_DATA.get().cloned() else {
        return;
    };
    if WINDOW_OPEN.swap(true, Ordering::SeqCst) {
        return;
    }
    eprintln!("feline-runtime: the app wants a login: opening the login window");
    let spawned = std::thread::Builder::new()
        .name("feline-login".into())
        .spawn(move || {
            match open_window(&data, signup) {
                Ok(_) => {
                    eprintln!("feline-runtime: logged in; restarting the app with the session");
                    restart();
                }
                Err(e) => eprintln!("feline-runtime: {e}"),
            }
            WINDOW_OPEN.store(false, Ordering::SeqCst);
        });
    if spawned.is_err() {
        WINDOW_OPEN.store(false, Ordering::SeqCst);
    }
}

/// Starts this program again in place (same arguments and environment),
/// so the engine starts fresh with the saved session.
fn restart() {
    use std::os::unix::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("feline-runtime: couldn't restart: own program not found");
        return;
    };
    let error = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("feline-runtime: couldn't restart: {error}");
}

/// Whose login the saved session is (Roblox's answer to "who am I").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: u64,
    pub name: String,
    pub display_name: String,
}

/// Why [`account`] has no answer.
#[derive(Debug)]
pub enum AccountError {
    /// Roblox doesn't accept the session any more (logged out elsewhere,
    /// expired): it's no use keeping it.
    Expired,
    /// Couldn't ask (offline, Roblox down, unexpected answer).
    Other(String),
}

const AUTHENTICATED_URL: &str = "https://users.roblox.com/v1/users/authenticated";

/// Asks Roblox which account `cookie` (`.ROBLOSECURITY=…`) logs in to.
pub fn account(cookie: &str) -> Result<Account, AccountError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .get(AUTHENTICATED_URL)
        .header("Cookie", cookie)
        .call()
        .map_err(|e| AccountError::Other(e.to_string()))?;
    match response.status().as_u16() {
        200 => {}
        401 | 403 => return Err(AccountError::Expired),
        status => return Err(AccountError::Other(format!("Roblox answered {status}"))),
    }
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| AccountError::Other(e.to_string()))?;
    parse_account(&body).ok_or_else(|| AccountError::Other("unexpected answer".into()))
}

/// `{"id":1,"name":"…","displayName":"…"}`.
fn parse_account(json: &str) -> Option<Account> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(Account {
        id: value.get("id")?.as_u64().filter(|&id| id > 0)?,
        name: value.get("name")?.as_str()?.to_owned(),
        display_name: value
            .get("displayName")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned(),
    })
}

/// Forgets the login (logging out of Feline).
pub fn forget_cookie(runtime_data: &Path) {
    let _ = std::fs::remove_file(cookie_path(runtime_data));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_answers() {
        assert_eq!(
            parse_account(r#"{"id":123456789,"name":"Kitty","displayName":"Kit"}"#),
            Some(Account {
                id: 123456789,
                name: "Kitty".into(),
                display_name: "Kit".into()
            })
        );
        assert_eq!(parse_account(r#"{"errors":[{"code":0}]}"#), None);
        assert_eq!(parse_account(r#"{"id":0,"name":"x"}"#), None);
    }

    #[test]
    fn login_screens() {
        assert_eq!(login_screen("Login"), Some(false));
        assert_eq!(login_screen("SinglePageSignUp"), Some(true));
        assert_eq!(login_screen("Landing"), None);
        assert_eq!(login_screen("Startup"), None);
    }

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
