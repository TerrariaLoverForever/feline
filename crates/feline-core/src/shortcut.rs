// SPDX-License-Identifier: GPL-3.0-or-later
//! Desktop shortcuts that start one game straight away (Bloxstrap's game
//! shortcuts). The shortcut runs Feline with the game's `roblox://` link, so
//! the game starts the way a link from roblox.com would.
//!
//! Windows: a `.lnk` on the desktop with the game's icon (made by Windows'
//! own shortcut maker, `WScript.Shell`). Linux: a `.desktop` file in the app
//! menu and on the desktop.

use std::fs;
use std::path::{Path, PathBuf};

use crate::png;

/// A file name made from a game's name: no characters Windows or Linux
/// refuse, not empty, not too long.
pub fn file_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { ' ' } else { c })
        .collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean = clean.trim_matches('.').chars().take(60).collect::<String>();
    if clean.is_empty() { "Roblox game".to_owned() } else { clean }
}

/// The link the shortcut opens.
pub fn game_link(place: u64) -> String {
    format!("roblox://experiences/start?placeId={place}")
}

/// A Windows `.ico` holding one PNG picture (Windows Vista and later read
/// those), up to 256 × 256.
pub fn ico_from_png(image: &png::Rgba) -> Vec<u8> {
    let square = png::fit_square(image, image.width.max(image.height).clamp(16, 256));
    let data = png::encode(&square);
    let size = |v: u32| if v >= 256 { 0u8 } else { v as u8 };
    let mut ico = vec![0, 0, 1, 0, 1, 0];
    ico.extend_from_slice(&[size(square.width), size(square.height), 0, 0]);
    ico.extend_from_slice(&1u16.to_le_bytes()); // planes
    ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(&data);
    ico
}

/// Makes a desktop shortcut that starts `place` with Feline (`feline` is
/// Feline's program). `icon` is the game's picture (PNG), if Feline has it.
/// Returns where the shortcut is.
pub fn make(place: u64, name: &str, icon: Option<&Path>, feline: &Path) -> Result<PathBuf, String> {
    let name = file_name(name);
    let icons = crate::paths::state_dir().ok_or("no folder for Feline's state")?.join("shortcut-icons");
    fs::create_dir_all(&icons).map_err(|e| e.to_string())?;
    let picture = icon.and_then(|p| fs::read(p).ok()).and_then(|d| png::decode(&d).ok());
    #[cfg(windows)]
    {
        let icon_file = match picture {
            Some(image) => {
                let file = icons.join(format!("{place}.ico"));
                fs::write(&file, ico_from_png(&image)).map_err(|e| e.to_string())?;
                file
            }
            None => feline.to_owned(),
        };
        windows_shortcut(&name, feline, &game_link(place), &icon_file)
    }
    #[cfg(not(windows))]
    {
        let icon_file = match picture {
            Some(image) => {
                let file = icons.join(format!("{place}.png"));
                fs::write(&file, png::encode(&png::fit_square(&image, 256))).map_err(|e| e.to_string())?;
                file.to_string_lossy().into_owned()
            }
            None => "feline".to_owned(),
        };
        let entry = desktop_entry(&name, feline, &game_link(place), &icon_file);
        let file_name = format!("feline-game-{place}.desktop");
        let apps = crate::paths::data_home().ok_or("no home folder")?.join("applications");
        fs::create_dir_all(&apps).map_err(|e| e.to_string())?;
        fs::write(apps.join(&file_name), &entry).map_err(|e| e.to_string())?;
        // And on the desktop, where there is one (marked as trusted so it
        // runs with a double-click).
        let desktop = linux_desktop_dir();
        let on_desktop = desktop.as_ref().map(|d| d.join(&file_name));
        if let Some(path) = &on_desktop {
            use std::os::unix::fs::PermissionsExt;
            fs::write(path, &entry).map_err(|e| e.to_string())?;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
            let _ = std::process::Command::new("gio")
                .args(["set", &path.to_string_lossy(), "metadata::trusted", "true"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        Ok(on_desktop.unwrap_or_else(|| apps.join(file_name)))
    }
}

/// A `.desktop` file that starts the game.
pub fn desktop_entry(name: &str, feline: &Path, link: &str, icon: &str) -> String {
    // The Exec key's quoting: arguments in double quotes, with `"`, `` ` ``,
    // `$` and `\` escaped.
    let quote = |s: &str| {
        let mut out = String::from("\"");
        for c in s.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('"');
        out
    };
    format!(
        "[Desktop Entry]\nType=Application\nName={name}\nComment=Play {name} with Feline\nExec={} {}\nIcon={icon}\nTerminal=false\nCategories=Game;\n",
        quote(&feline.to_string_lossy()),
        quote(link).replace('%', "%%"),
    )
}

#[cfg(not(windows))]
fn linux_desktop_dir() -> Option<PathBuf> {
    // `xdg-user-dir DESKTOP` knows a renamed (or translated) desktop.
    let out = std::process::Command::new("xdg-user-dir").arg("DESKTOP").output().ok()?;
    let dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    (dir.is_dir() && dir != home).then_some(dir)
}

/// Makes the `.lnk` with Windows' own shortcut maker. Every value goes to
/// PowerShell as an environment variable, never inside the script text, so
/// nothing in a game's name can change what the script does.
#[cfg(windows)]
fn windows_shortcut(name: &str, target: &Path, link: &str, icon: &Path) -> Result<PathBuf, String> {
    use std::os::windows::process::CommandExt;
    // The path comes back in UTF-8: the desktop's can have any letters.
    const SCRIPT: &str = "[Console]::OutputEncoding=[Text.Encoding]::UTF8;\
        $d=[Environment]::GetFolderPath('Desktop');\
        $p=Join-Path $d ($env:FELINE_NAME + '.lnk');\
        $s=(New-Object -ComObject WScript.Shell).CreateShortcut($p);\
        $s.TargetPath=$env:FELINE_TARGET;$s.Arguments=$env:FELINE_ARGS;\
        $s.IconLocation=$env:FELINE_ICON + ',0';$s.Description=$env:FELINE_DESC;\
        $s.WorkingDirectory=[IO.Path]::GetDirectoryName($env:FELINE_TARGET);\
        $s.Save();Write-Output $p";
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", SCRIPT])
        .env("FELINE_NAME", name)
        .env("FELINE_TARGET", target)
        .env("FELINE_ARGS", format!("\"{link}\""))
        .env("FELINE_ICON", icon)
        .env("FELINE_DESC", format!("Play {name} with Feline"))
        .creation_flags(0x0800_0000) // no console window
        .output()
        .map_err(|e| format!("PowerShell couldn't start: {e}"))?;
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    if output.status.success() && path.is_file() {
        Ok(path)
    } else {
        Err(format!("Windows didn't make the shortcut: {}", String::from_utf8_lossy(&output.stderr).trim()))
    }
}

// ---------------------------------------------------------------------------
// Starting with the session (Linux: an autostart entry)
// ---------------------------------------------------------------------------

/// `~/.config/autostart/feline.desktop`.
#[cfg(not(windows))]
fn autostart_file() -> Option<PathBuf> {
    Some(crate::paths::config_dir()?.parent()?.join("autostart").join("feline.desktop"))
}

/// Starts Feline (minimized) when you log in, or not any more. Windows
/// does this in the registry (feline-win); this is the Linux way.
#[cfg(not(windows))]
pub fn set_autostart(on: bool, feline: &Path) -> Result<(), String> {
    let file = autostart_file().ok_or("no home folder")?;
    if !on {
        let _ = fs::remove_file(&file);
        return Ok(());
    }
    fs::create_dir_all(file.parent().unwrap_or(&file)).map_err(|e| e.to_string())?;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Feline\nComment=Feline, ready in the background\nExec=\"{}\" --minimized\nIcon=feline\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        feline.display()
    );
    fs::write(file, entry).map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn autostart_enabled() -> bool {
    autostart_file().is_some_and(|f| f.is_file())
}

/// The file type Roblox place files get (`.rbxl`, `.rbxlx`).
#[cfg(not(windows))]
const PLACE_TYPE: &str = "application/x-roblox-place";

/// Linux: place files open with Feline (`on`), or with `previous` (what
/// opened them before, as `xdg-mime` names it). Returns the handler before.
#[cfg(not(windows))]
pub fn set_place_files(on: bool, feline: &Path, previous: Option<&str>) -> Result<Option<String>, String> {
    use std::process::{Command, Stdio};
    let data = crate::paths::data_home().ok_or("no home folder")?;
    let mime_dir = data.join("mime");
    let mime = mime_dir.to_string_lossy().into_owned();
    let package = mime_dir.join("packages").join("feline-roblox-place.xml");
    let entry = data.join("applications").join("feline-studio.desktop");
    let run = |program: &str, args: &[&str]| {
        Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()
    };
    let current = run("xdg-mime", &["query", "default", PLACE_TYPE])
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|h| !h.is_empty() && h != "feline-studio.desktop");
    if on {
        fs::create_dir_all(package.parent().unwrap_or(&mime_dir)).map_err(|e| e.to_string())?;
        fs::write(
            &package,
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\">\n  <mime-type type=\"{PLACE_TYPE}\">\n    <comment>Roblox place</comment>\n    <glob pattern=\"*.rbxl\"/>\n    <glob pattern=\"*.rbxlx\"/>\n  </mime-type>\n</mime-info>\n"
            ),
        )
        .map_err(|e| e.to_string())?;
        fs::create_dir_all(entry.parent().unwrap_or(&data)).map_err(|e| e.to_string())?;
        fs::write(
            &entry,
            format!(
                "[Desktop Entry]\nType=Application\nName=Roblox Studio (Feline)\nExec=\"{}\" --studio %f\nIcon=feline\nTerminal=false\nNoDisplay=true\nMimeType={PLACE_TYPE};\n",
                feline.display()
            ),
        )
        .map_err(|e| e.to_string())?;
        run("update-mime-database", &[mime.as_str()]);
        run("xdg-mime", &["default", "feline-studio.desktop", PLACE_TYPE]);
    } else {
        let _ = fs::remove_file(&entry);
        let _ = fs::remove_file(&package);
        run("update-mime-database", &[mime.as_str()]);
        if let Some(previous) = previous.filter(|p| !p.is_empty()) {
            run("xdg-mime", &["default", previous, PLACE_TYPE]);
        }
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_file_names() {
        assert_eq!(file_name("Cat Café: Tycoon / 2 *NEW*"), "Cat Café Tycoon 2 NEW");
        assert_eq!(file_name("..."), "Roblox game");
        assert_eq!(file_name(&"a".repeat(100)).len(), 60);
    }

    #[test]
    fn icons_hold_a_png() {
        let image = png::Rgba { width: 512, height: 512, pixels: vec![200; 512 * 512 * 4] };
        let ico = ico_from_png(&image);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 1, 0]);
        assert_eq!(ico[6], 0, "256 is written as 0");
        assert!(ico[22..].starts_with(b"\x89PNG"));
    }

    #[test]
    fn desktop_entries_quote_the_link() {
        let entry = desktop_entry("Cat Café", Path::new("/usr/bin/feline"), &game_link(42), "feline");
        assert!(entry.contains("Exec=\"/usr/bin/feline\" \"roblox://experiences/start?placeId=42\""), "{entry}");
        assert!(entry.contains("Name=Cat Café"));
    }
}
