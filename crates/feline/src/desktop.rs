// SPDX-License-Identifier: GPL-3.0-or-later
//! Installing Feline's menu entry and icons for the current user.
//!
//! Why this exists: on Wayland, a window can't send its own icon. The desktop
//! looks up the window's app ID ([`APP_ID`]) and finds a `.desktop` file of the
//! same name, whose `Icon=` names an icon in the icon theme. So the icon only
//! appears once these files are installed.
//!
//! Everything goes into `~/.local/share` (no root, nothing system-wide):
//! - `applications/<APP_ID>.desktop`
//! - `icons/hicolor/<size>/apps/<APP_ID>.png`

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use feline_core::paths::{self, APP_ID};
use feline_core::process::{RunError, run_captured};

/// The menu entry template from `assets/`. `Exec=` is replaced on install.
const DESKTOP_TEMPLATE: &str =
    include_str!("../../../assets/io.github.TerrariaLoverForever.Feline.desktop");

/// Icons compiled into the binary, so installing needs no source checkout.
const ICONS: &[(u32, &[u8])] = &[
    (
        16,
        include_bytes!("../../../assets/icons/hicolor/16x16/apps/feline.png"),
    ),
    (
        32,
        include_bytes!("../../../assets/icons/hicolor/32x32/apps/feline.png"),
    ),
    (
        48,
        include_bytes!("../../../assets/icons/hicolor/48x48/apps/feline.png"),
    ),
    (
        64,
        include_bytes!("../../../assets/icons/hicolor/64x64/apps/feline.png"),
    ),
    (
        128,
        include_bytes!("../../../assets/icons/hicolor/128x128/apps/feline.png"),
    ),
    (
        256,
        include_bytes!("../../../assets/icons/hicolor/256x256/apps/feline.png"),
    ),
    (
        512,
        include_bytes!("../../../assets/icons/hicolor/512x512/apps/feline.png"),
    ),
];

/// What `install` did, for printing.
pub struct Report {
    pub files: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// What the menu-entry commands say on systems without freedesktop menus.
const NOT_HERE: &str = "menu entries are a Linux desktop feature; \
     on Windows, Feline's Start menu shortcut comes with its installer";

pub fn install() -> Result<Report, String> {
    if cfg!(windows) {
        return Err(NOT_HERE.to_owned());
    }
    let data = paths::data_home().ok_or("couldn't find your home folder ($HOME is not set)")?;
    let exec = quote_exec(&exec_path()?)?;

    let mut files = Vec::new();
    let entry = data.join("applications").join(format!("{APP_ID}.desktop"));
    write(&entry, desktop_entry(&exec).as_bytes())?;
    files.push(entry);

    for (size, png) in ICONS {
        let path = icon_path(&data, *size);
        write(&path, png)?;
        files.push(path);
    }

    Ok(Report {
        files,
        warnings: refresh_caches(&data),
    })
}

pub fn uninstall() -> Result<Vec<PathBuf>, String> {
    if cfg!(windows) {
        return Err(NOT_HERE.to_owned());
    }
    let data = paths::data_home().ok_or("couldn't find your home folder ($HOME is not set)")?;
    let mut removed = Vec::new();
    let mut targets = vec![data.join("applications").join(format!("{APP_ID}.desktop"))];
    targets.extend(ICONS.iter().map(|(size, _)| icon_path(&data, *size)));
    for path in targets {
        match fs::remove_file(&path) {
            Ok(()) => removed.push(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("couldn't remove {}: {e}", path.display())),
        }
    }
    Ok(removed)
}

/// The program Feline's menu entries should start: normally this Feline
/// binary. A launcher script can set `FELINE_EXEC` to its own absolute path,
/// so the entries keep pointing at the script (for example one that always
/// starts the newest build of a source checkout).
fn exec_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("FELINE_EXEC").map(PathBuf::from)
        && path.is_absolute()
        && path.is_file()
    {
        return Ok(path);
    }
    std::env::current_exe()
        .map_err(|e| format!("couldn't find the path of the running Feline program: {e}"))
}

/// The file name of Feline's hidden link-handler entry. It's separate from
/// the menu entry, so Feline only becomes a candidate for `roblox://` links
/// when the user switches link handling on.
pub const LINK_ENTRY: &str = "io.github.TerrariaLoverForever.Feline.links.desktop";

fn link_entry_path() -> Result<PathBuf, String> {
    let data = paths::data_home().ok_or("couldn't find your home folder ($HOME is not set)")?;
    Ok(data.join("applications").join(LINK_ENTRY))
}

/// Adds the hidden entry that opens `roblox://` and `roblox-player:` links
/// with this Feline binary.
pub fn install_link_entry() -> Result<(), String> {
    let exe = exec_path()?;
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Feline\n\
         Comment=Opens Roblox links with your chosen runtime\n\
         Exec={} %u\n\
         Icon={APP_ID}\n\
         NoDisplay=true\n\
         Terminal=false\n\
         MimeType=x-scheme-handler/roblox;x-scheme-handler/roblox-player;\n",
        quote_exec(&exe)?
    );
    let path = link_entry_path()?;
    write(&path, entry.as_bytes())?;
    if let Some(dir) = path.parent() {
        // Optional: desktops usually notice new entries by themselves.
        let _ = std::process::Command::new("update-desktop-database")
            .arg(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    Ok(())
}

pub fn uninstall_link_entry() -> Result<(), String> {
    let path = link_entry_path()?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("couldn't remove {}: {e}", path.display())),
    }
}

fn icon_path(data: &Path, size: u32) -> PathBuf {
    data.join(format!("icons/hicolor/{size}x{size}/apps/{APP_ID}.png"))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    }
    fs::write(path, bytes).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

/// The template with its `Exec=` line pointing at this Feline binary.
fn desktop_entry(exec: &str) -> String {
    DESKTOP_TEMPLATE
        .lines()
        .map(|line| {
            if line.starts_with("Exec=") {
                format!("Exec={exec}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Writes a path for a desktop entry's `Exec=` key, following the Desktop
/// Entry spec: `%` is doubled (it otherwise starts a field code like `%u`),
/// and the path is quoted only if it contains a reserved character (space,
/// quotes, `$`, `~`, …), with `"` `` ` `` `$` `\` backslash-escaped inside.
///
/// Quoting only when needed matters: `xdg-mime` (xdg-utils 1.2) takes the
/// first word of `Exec=` with any quote marks still attached, can't find a
/// program by that name, and silently skips the entry.
fn quote_exec(path: &Path) -> Result<String, String> {
    let text = path
        .to_str()
        .ok_or("the Feline program's path isn't valid UTF-8, so it can't go in a menu entry")?;
    if text.chars().any(char::is_control) {
        return Err("the Feline program's path contains control characters".into());
    }
    const RESERVED: &[char] = &[
        ' ', '\t', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`',
    ];
    if !text.contains(RESERVED) {
        return Ok(text.replace('%', "%%"));
    }
    let mut quoted = String::from("\"");
    for c in text.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

/// Asks the desktop to notice the new files. These tools are optional and
/// desktops usually notice by themselves, so a missing tool isn't an error;
/// a tool that fails is reported as a warning.
fn refresh_caches(data: &Path) -> Vec<String> {
    // Deliberately no `gtk-update-icon-cache`: a cache file in the user's
    // icon folder hides icons that other programs (Steam, …) add later
    // without updating it. Without a cache, GTK simply scans the folder.
    let tools: [(&str, Vec<String>); 2] = [
        (
            "update-desktop-database",
            vec![data.join("applications").display().to_string()],
        ),
        ("kbuildsycoca6", vec![]),
    ];
    let mut warnings = Vec::new();
    for (tool, args) in tools {
        let mut cmd = std::process::Command::new(tool);
        cmd.args(&args);
        match run_captured(cmd, Duration::from_secs(30)) {
            Ok(out) if out.status.success() => {}
            Ok(out) => warnings.push(format!(
                "`{tool}` reported a problem (exit code {:?})",
                out.status.code()
            )),
            Err(RunError::NotFound { .. }) => {}
            Err(e) => warnings.push(e.to_string()),
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_quoting_follows_the_spec() {
        // No reserved characters: no quotes (xdg-utils needs that).
        assert_eq!(
            quote_exec(Path::new("/home/me/feline")).unwrap(),
            "/home/me/feline"
        );
        assert_eq!(
            quote_exec(Path::new("/opt/100%/feline")).unwrap(),
            "/opt/100%%/feline"
        );
        assert_eq!(
            quote_exec(Path::new("/home/me/My Apps/100%/$x/feline")).unwrap(),
            "\"/home/me/My Apps/100%%/\\$x/feline\""
        );
        assert!(quote_exec(Path::new("/bad\npath")).is_err());
    }

    #[test]
    fn entry_points_exec_at_the_binary_and_keeps_the_rest() {
        let entry = desktop_entry("\"/opt/feline\"");
        assert!(entry.contains("\nExec=\"/opt/feline\"\n"));
        assert!(entry.contains(&format!("Icon={APP_ID}")));
        assert!(entry.contains(&format!("StartupWMClass={APP_ID}")));
        assert_eq!(entry.matches("Exec=").count(), 1);
    }
}
