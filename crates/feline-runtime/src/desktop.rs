// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Roblox's desktop layout for the app.
//!
//! The Android app picks its layout from an "app policy" Roblox's servers
//! send, cached in its local storage (`appStorage.json`, `AppConfiguration`,
//! keys `GUAC:<user ID or -1>:app-policy`). For Feline it says
//! `PlatformGroup: "Tablet"`: a bottom bar, no Play button when hovering a
//! game, no avatar next to Robux, and a reduced chat. Here the cached
//! policies are switched to the desktop layout and the same policy is
//! handed to the engine as the client setting
//! `FStringAppConfigurationOverrideAppPolicy`, so a fresh one from the
//! servers doesn't undo it.
//!
//! The keys changed, and turning off the Charts page that can leave the
//! app blank, follow mocktail's `roblox_desktop_app_policy.cc` (Apache-2.0,
//! <https://github.com/komaruworld/mocktail>).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

const POLICY_SUFFIX: &str = ":app-policy";
const OVERRIDE: &str = "FStringAppConfigurationOverrideAppPolicy";

/// The app's local storage file under the runtime's data folder.
pub fn app_storage(runtime: &Path) -> PathBuf {
    runtime.join("files/appData/LocalStorage/appStorage.json")
}

/// The desktop layout's settings in one policy.
fn make_desktop(policy: &mut Map<String, Value>) {
    policy.insert("PlatformGroup".into(), json!("Desktop"));
    policy.insert("UseGridHomePage".into(), json!(true));
    policy.insert("UseGridPageLayout".into(), json!(true));
    policy.insert("SystemBarPlacement".into(), json!("Left"));
    policy.insert("ShouldSystemBarUsuallyBePresent".into(), json!(true));
    policy.insert("DevicePreferencesPersistentPresenceVariant".into(), json!("windows"));
    policy.insert("ShowUncheckedBadge".into(), json!(false));
}

/// Switches every cached policy in `storage` (appStorage.json's text) to the
/// desktop layout. Returns the new file text (None if nothing changed) and
/// the policy to hand to the engine (the user's, else the guest one; None
/// before Roblox has sent any).
fn update_storage(storage: &str, user_id: Option<u64>) -> (Option<String>, Option<String>) {
    let Ok(Value::Object(mut root)) = serde_json::from_str::<Value>(storage) else {
        return (None, None);
    };
    let Some(Value::String(encoded)) = root.get("AppConfiguration") else {
        return (None, None);
    };
    let Ok(Value::Object(mut configurations)) = serde_json::from_str::<Value>(encoded) else {
        return (None, None);
    };
    let user_key = user_id.map(|id| format!("GUAC:{id}{POLICY_SUFFIX}"));
    let mut chosen: Option<String> = None;
    let mut guest: Option<String> = None;
    for (key, value) in configurations.iter_mut() {
        if !key.ends_with(POLICY_SUFFIX) {
            continue;
        }
        let Some(Ok(Value::Object(mut policy))) =
            value.as_str().map(serde_json::from_str::<Value>)
        else {
            continue;
        };
        make_desktop(&mut policy);
        let text = Value::Object(policy).to_string();
        if Some(key) == user_key.as_ref() {
            chosen = Some(text.clone());
        } else if key == "GUAC:-1:app-policy" {
            guest = Some(text.clone());
        }
        *value = Value::String(text);
    }
    let policy = chosen.or(guest);
    let encoded_new = Value::Object(configurations).to_string();
    if root.get("AppConfiguration").and_then(Value::as_str) == Some(encoded_new.as_str()) {
        return (None, policy);
    }
    root.insert("AppConfiguration".into(), Value::String(encoded_new));
    (Some(Value::Object(root).to_string()), policy)
}

/// Switches the app's cached layout to the desktop one, and returns the
/// policy for [`add_to_settings`]. Problems are logged, never fatal.
pub fn apply(runtime: &Path, user_id: Option<u64>) -> Option<String> {
    let path = app_storage(runtime);
    let text = std::fs::read_to_string(&path).ok()?;
    let (new_text, policy) = update_storage(&text, user_id);
    if let Some(new_text) = new_text {
        let temporary = path.with_extension("feline-new");
        let written = std::fs::write(&temporary, new_text)
            .and_then(|()| std::fs::rename(&temporary, &path));
        match written {
            Ok(()) => eprintln!("feline-runtime: the app uses Roblox's desktop layout"),
            Err(e) => eprintln!("feline-runtime: couldn't switch the app's layout: {e}"),
        }
    }
    policy
}

/// Adds the desktop policy to the client settings JSON (`{"applicationSettings":
/// {…}}` or a plain settings object), keeping settings the user set.
pub fn add_to_settings(settings: &str, policy: &str) -> String {
    let Ok(Value::Object(mut root)) = serde_json::from_str::<Value>(settings) else {
        return settings.to_owned();
    };
    let target = match root.get_mut("applicationSettings") {
        Some(Value::Object(inner)) => inner,
        _ => &mut root,
    };
    target
        .entry(OVERRIDE)
        .or_insert_with(|| Value::String(policy.to_owned()));
    // The newer Charts page can leave the whole app blank (mocktail).
    target
        .entry("FFlagLuaAppChartsAppPage")
        .or_insert_with(|| json!("False"));
    Value::Object(root).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage() -> String {
        let policy = json!({"PlatformGroup": "Tablet", "SystemBarPlacement": "Bottom", "Other": 1});
        let configurations = json!({
            "GUAC:-1:app-policy": policy.to_string(),
            "GUAC:42:app-policy": policy.to_string(),
            "Unrelated": "x",
        });
        json!({"AppConfiguration": configurations.to_string(), "RobloxLocaleId": "en_us"}).to_string()
    }

    #[test]
    fn policies_become_desktop() {
        let (new_text, policy) = update_storage(&storage(), Some(42));
        let new_text = new_text.expect("changed");
        let root: Value = serde_json::from_str(&new_text).unwrap();
        assert_eq!(root["RobloxLocaleId"], "en_us");
        let configurations: Value =
            serde_json::from_str(root["AppConfiguration"].as_str().unwrap()).unwrap();
        assert_eq!(configurations["Unrelated"], "x");
        for key in ["GUAC:-1:app-policy", "GUAC:42:app-policy"] {
            let p: Value = serde_json::from_str(configurations[key].as_str().unwrap()).unwrap();
            assert_eq!(p["PlatformGroup"], "Desktop");
            assert_eq!(p["SystemBarPlacement"], "Left");
            assert_eq!(p["Other"], 1);
        }
        let policy: Value = serde_json::from_str(&policy.unwrap()).unwrap();
        assert_eq!(policy["PlatformGroup"], "Desktop");
        // A second pass changes nothing.
        assert_eq!(update_storage(&new_text, Some(42)).0, None);
    }

    #[test]
    fn settings_get_the_override_without_replacing_the_users() {
        let merged = add_to_settings(r#"{"applicationSettings":{"FFlagX":"True"}}"#, "{}");
        let v: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["applicationSettings"][OVERRIDE], "{}");
        assert_eq!(v["applicationSettings"]["FFlagX"], "True");
        let mine = add_to_settings(r#"{"FFlagLuaAppChartsAppPage":"True"}"#, "{}");
        let v: Value = serde_json::from_str(&mine).unwrap();
        assert_eq!(v["FFlagLuaAppChartsAppPage"], "True");
    }
}
