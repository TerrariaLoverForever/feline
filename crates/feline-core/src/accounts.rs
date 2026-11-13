// SPDX-License-Identifier: GPL-3.0-or-later
//! Several Roblox accounts for Feline's runtime, and switching between them.
//!
//! The runtime logs in with one session cookie, `<runtime data>/roblox-cookie`
//! (see feline-runtime's login.rs). An account here is a copy of such a
//! cookie with the name Roblox gave for it:
//!
//! ```text
//! <runtime data>/accounts/<user id>.cookie   the session (only you can read it)
//! <runtime data>/accounts/<user id>.json     {"name": …, "displayName": …}
//! ```
//!
//! Switching copies an account's cookie over `roblox-cookie`; the runtime
//! uses it at its next start. Nothing is sent anywhere except to Roblox,
//! which is asked whose session a cookie is. Signing out in the Roblox app
//! itself ends that session at Roblox, so its copy here stops working.

use std::path::{Path, PathBuf};

use serde_json::Value;

const COOKIE_NAME: &str = ".ROBLOSECURITY";

/// A Roblox account, as Roblox names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: u64,
    pub name: String,
    pub display_name: String,
}

/// A saved account, and whether it's the one the runtime logs in with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub account: Account,
    pub active: bool,
}

/// Why Roblox didn't say whose session a cookie is.
#[derive(Debug, PartialEq, Eq)]
pub enum WhoError {
    /// Roblox doesn't accept the session any more.
    Expired,
    /// Couldn't ask (offline, Roblox down, unexpected answer).
    Other(String),
}

/// Feline's runtime data folder.
fn runtime_dir() -> Option<PathBuf> {
    Some(crate::paths::data_home()?.join("feline").join("runtime"))
}

fn accounts_dir(runtime: &Path) -> PathBuf {
    runtime.join("accounts")
}

fn active_cookie_path(runtime: &Path) -> PathBuf {
    runtime.join("roblox-cookie")
}

/// The cookie's value without its name; None unless it's printable text
/// with no separators (so it can't smuggle anything into a header).
fn clean(value: &str) -> Option<&str> {
    let value = value.trim();
    let value = value.strip_prefix(COOKIE_NAME).and_then(|v| v.strip_prefix('=')).unwrap_or(value);
    (value.len() >= 20
        && value.len() <= 8192
        && value.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b';' | b',' | b'=' | b'"' | b'\\')))
    .then_some(value)
}

/// Writes `text` to `path` so that only you can read it.
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let temporary = path.with_extension("new");
    let _ = std::fs::remove_file(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|e| format!("couldn't write {}: {e}", temporary.display()))?;
    file.write_all(text.as_bytes()).and_then(|()| file.sync_all()).map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, path).map_err(|e| e.to_string())
}

fn read_cookie(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    clean(&text).map(str::to_owned)
}

/// The saved accounts in `runtime`, sorted by name.
fn list_in(runtime: &Path) -> Vec<Saved> {
    let active = read_cookie(&active_cookie_path(runtime));
    let Ok(entries) = std::fs::read_dir(accounts_dir(runtime)) else {
        return Vec::new();
    };
    let mut saved: Vec<Saved> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != "cookie" {
                return None;
            }
            let id: u64 = path.file_stem()?.to_str()?.parse().ok()?;
            let cookie = read_cookie(&path)?;
            let about: Value = std::fs::read_to_string(path.with_extension("json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or(Value::Null);
            let text = |key: &str| about.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
            Some(Saved {
                account: Account {
                    id,
                    name: text("name"),
                    display_name: text("displayName"),
                },
                active: active.as_deref() == Some(cookie.as_str()),
            })
        })
        .collect();
    saved.sort_by_key(|s| (s.account.name.to_lowercase(), s.account.id));
    saved
}

/// Saves `cookie` as `account`'s (replacing an older session of the same
/// account), without changing who the runtime logs in as.
fn store_in(runtime: &Path, account: &Account, cookie: &str) -> Result<(), String> {
    let cookie = clean(cookie).ok_or("that isn't a Roblox session cookie")?;
    let dir = accounts_dir(runtime);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    write_private(&dir.join(format!("{}.cookie", account.id)), cookie)?;
    let about = serde_json::json!({ "name": account.name, "displayName": account.display_name });
    write_private(&dir.join(format!("{}.json", account.id)), &about.to_string())
}

/// Makes the saved account `id` the one the runtime logs in with.
fn switch_in(runtime: &Path, id: u64) -> Result<(), String> {
    let cookie = read_cookie(&accounts_dir(runtime).join(format!("{id}.cookie"))).ok_or("that account isn't saved any more")?;
    write_private(&active_cookie_path(runtime), &cookie)
}

/// Forgets the saved account `id`; if the runtime was logged in with it,
/// that login goes too. True if there was such an account.
fn remove_in(runtime: &Path, id: u64) -> bool {
    let dir = accounts_dir(runtime);
    let path = dir.join(format!("{id}.cookie"));
    let was_active = list_in(runtime).iter().any(|s| s.account.id == id && s.active);
    let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
    let removed = std::fs::remove_file(path).is_ok();
    if was_active {
        let _ = std::fs::remove_file(active_cookie_path(runtime));
    }
    removed
}

/// The saved accounts.
pub fn list() -> Vec<Saved> {
    runtime_dir().as_deref().map(list_in).unwrap_or_default()
}

/// Whether the runtime has a login that isn't one of the saved accounts
/// (you signed in before accounts existed, or in the runtime's own window).
pub fn unsaved_login() -> bool {
    let Some(runtime) = runtime_dir() else {
        return false;
    };
    read_cookie(&active_cookie_path(&runtime)).is_some() && !list_in(&runtime).iter().any(|s| s.active)
}

/// `{"id":1,"name":"…","displayName":"…"}`.
fn parse_account(json: &str) -> Option<Account> {
    let value: Value = serde_json::from_str(json).ok()?;
    Some(Account {
        id: value.get("id")?.as_u64().filter(|&id| id > 0)?,
        name: value.get("name")?.as_str()?.to_owned(),
        display_name: value.get("displayName").and_then(Value::as_str).unwrap_or_default().to_owned(),
    })
}

/// Asks Roblox whose session `cookie` is.
pub fn who(cookie: &str) -> Result<Account, WhoError> {
    let cookie = clean(cookie).ok_or_else(|| WhoError::Other("that isn't a Roblox session cookie".into()))?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .get("https://users.roblox.com/v1/users/authenticated")
        .header("Cookie", format!("{COOKIE_NAME}={cookie}"))
        .call()
        .map_err(|e| WhoError::Other(e.to_string()))?;
    match response.status().as_u16() {
        200 => {}
        401 | 403 => return Err(WhoError::Expired),
        status => return Err(WhoError::Other(format!("Roblox answered {status}"))),
    }
    let body = response.body_mut().read_to_string().map_err(|e| WhoError::Other(e.to_string()))?;
    parse_account(&body).ok_or_else(|| WhoError::Other("unexpected answer from Roblox".into()))
}

/// Saves the runtime's current login as an account, if it isn't one yet.
/// The account it turned out to be.
pub fn save_current() -> Result<Option<Account>, String> {
    let runtime = runtime_dir().ok_or("no data folder")?;
    if list_in(&runtime).iter().any(|s| s.active) {
        return Ok(None);
    }
    let Some(cookie) = read_cookie(&active_cookie_path(&runtime)) else {
        return Ok(None);
    };
    match who(&cookie) {
        Ok(account) => {
            store_in(&runtime, &account, &cookie)?;
            Ok(Some(account))
        }
        Err(WhoError::Expired) => Err("Roblox doesn't accept the current login any more".into()),
        Err(WhoError::Other(e)) => Err(e),
    }
}

/// Opens Feline's login window (Roblox's own website) and waits for you to
/// sign in. The new account is saved and becomes the one in use.
pub fn add_with_login_window() -> Result<Account, String> {
    use std::io::Read;
    let runtime = runtime_dir().ok_or("no data folder")?;
    // Keep the login in use now, so it can be switched back to.
    let _ = save_current();
    let beside = std::env::current_exe().ok().map(|exe| exe.with_file_name("feline-login")).filter(|p| p.is_file());
    let program = beside.unwrap_or_else(|| "feline-login".into());
    let mut child = std::process::Command::new(&program)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("the login window ({}) didn't open: {e}", program.display()))?;
    let mut cookie = String::new();
    let read = child.stdout.take().ok_or("no pipe from the login window")?.read_to_string(&mut cookie);
    drop(child.stdin.take());
    let status = child.wait().map_err(|e| e.to_string())?;
    if read.is_err() || !status.success() || clean(&cookie).is_none() {
        return Err("the login window was closed without signing in".into());
    }
    let account = match who(&cookie) {
        Ok(a) => a,
        Err(WhoError::Expired) => return Err("Roblox didn't accept that login".into()),
        Err(WhoError::Other(e)) => return Err(e),
    };
    std::fs::create_dir_all(&runtime).map_err(|e| e.to_string())?;
    store_in(&runtime, &account, &cookie)?;
    switch_in(&runtime, account.id)?;
    Ok(account)
}

/// Makes the saved account `id` the one in use, after checking with Roblox
/// that its session still works (an ended one is removed).
pub fn switch(id: u64) -> Result<Account, String> {
    let runtime = runtime_dir().ok_or("no data folder")?;
    // Whoever is in use now stays in the list.
    let _ = save_current();
    let cookie = read_cookie(&accounts_dir(&runtime).join(format!("{id}.cookie"))).ok_or("that account isn't saved any more")?;
    let account = match who(&cookie) {
        Ok(a) => Some(a),
        Err(WhoError::Expired) => {
            remove_in(&runtime, id);
            return Err("Roblox ended that account's session (signed out somewhere else?). Add it again".into());
        }
        // Offline: switch anyway; the runtime checks again when it starts.
        Err(WhoError::Other(_)) => None,
    };
    switch_in(&runtime, id)?;
    let saved = list_in(&runtime).into_iter().find(|s| s.account.id == id).map(|s| s.account);
    account.or(saved).ok_or_else(|| "that account isn't saved any more".into())
}

/// Forgets the saved account `id` (and signs the runtime out of it).
pub fn remove(id: u64) -> bool {
    runtime_dir().is_some_and(|r| remove_in(&r, id))
}

/// Forgets the account the runtime is logged in with, if it's a saved one.
pub fn remove_active() {
    if let Some(runtime) = runtime_dir() {
        for saved in list_in(&runtime).into_iter().filter(|s| s.active) {
            remove_in(&runtime, saved.account.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "_|WARNING:-DO-NOT-SHARE-THIS.--|_AAAAAAAAAAAAAAAAAAAA";
    const B: &str = "_|WARNING:-DO-NOT-SHARE-THIS.--|_BBBBBBBBBBBBBBBBBBBB";

    fn account(id: u64, name: &str) -> Account {
        Account {
            id,
            name: name.into(),
            display_name: format!("{name}!"),
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("feline-accounts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn cookies_are_checked() {
        assert_eq!(clean(&format!(".ROBLOSECURITY={A}\n")), Some(A));
        assert_eq!(clean(A), Some(A));
        assert_eq!(clean("short"), None);
        assert_eq!(clean(&format!("{A}; Domain=evil.test")), None);
        assert_eq!(clean(&format!("{A}\r\nX-Other: 1")), None);
    }

    #[test]
    fn roblox_answers() {
        assert_eq!(parse_account(r#"{"id":12,"name":"Kitty","displayName":"Kit"}"#), Some(Account { id: 12, name: "Kitty".into(), display_name: "Kit".into() }));
        assert_eq!(parse_account(r#"{"errors":[{"code":0}]}"#), None);
        assert_eq!(parse_account(r#"{"id":0,"name":"x"}"#), None);
    }

    #[test]
    fn save_switch_and_remove() {
        let runtime = temp("switch");
        assert!(list_in(&runtime).is_empty());
        store_in(&runtime, &account(2, "zed"), B).expect("stored");
        store_in(&runtime, &account(1, "Amy"), A).expect("stored");
        // Saving doesn't log anyone in.
        assert!(!active_cookie_path(&runtime).exists());
        let names: Vec<_> = list_in(&runtime).into_iter().map(|s| (s.account.name, s.active)).collect();
        assert_eq!(names, [("Amy".to_owned(), false), ("zed".to_owned(), false)]);

        switch_in(&runtime, 2).expect("switched");
        assert_eq!(read_cookie(&active_cookie_path(&runtime)).as_deref(), Some(B));
        let active: Vec<_> = list_in(&runtime).into_iter().filter(|s| s.active).map(|s| s.account.id).collect();
        assert_eq!(active, [2]);
        assert_eq!(list_in(&runtime)[1].account.display_name, "zed!");

        // Removing the other account leaves the login alone.
        assert!(remove_in(&runtime, 1));
        assert!(active_cookie_path(&runtime).exists());
        // Removing the one in use signs out.
        assert!(remove_in(&runtime, 2));
        assert!(!active_cookie_path(&runtime).exists());
        assert!(list_in(&runtime).is_empty());
        assert!(!remove_in(&runtime, 2));
        assert!(switch_in(&runtime, 2).is_err());
        let _ = std::fs::remove_dir_all(&runtime);
    }

    #[test]
    fn a_new_session_replaces_the_old_one() {
        let runtime = temp("replace");
        store_in(&runtime, &account(1, "Amy"), A).expect("stored");
        store_in(&runtime, &account(1, "Amy2"), B).expect("stored");
        let list = list_in(&runtime);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].account.name, "Amy2");
        assert!(store_in(&runtime, &account(3, "bad"), "nope").is_err());
        let _ = std::fs::remove_dir_all(&runtime);
    }

    #[cfg(unix)]
    #[test]
    fn only_you_can_read_them() {
        use std::os::unix::fs::PermissionsExt;
        let runtime = temp("private");
        store_in(&runtime, &account(1, "Amy"), A).expect("stored");
        switch_in(&runtime, 1).expect("switched");
        let mode = |p: PathBuf| std::fs::metadata(p).expect("exists").permissions().mode() & 0o777;
        assert_eq!(mode(accounts_dir(&runtime)), 0o700);
        assert_eq!(mode(accounts_dir(&runtime).join("1.cookie")), 0o600);
        assert_eq!(mode(active_cookie_path(&runtime)), 0o600);
        let _ = std::fs::remove_dir_all(&runtime);
    }
}
