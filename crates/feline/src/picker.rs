// SPDX-License-Identifier: GPL-3.0-or-later
//! Asking the user for a file, and opening a file with its usual app.

use std::path::{Path, PathBuf};

/// Shows the system's file dialog. `patterns` are like `["*.ogg", "*.mp3"]`.
/// Windows: the standard "Open" dialog. Linux: zenity or kdialog, whichever
/// is installed. `None` if cancelled or no dialog is available.
pub fn pick_file(title: &str, kind: &str, patterns: &[&str]) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        feline_win::window::pick_file(title, &[(kind, &patterns.join(";"))])
    }
    #[cfg(not(windows))]
    {
        use std::process::Command;
        let zenity = Command::new("zenity")
            .arg("--file-selection")
            .arg(format!("--title={title}"))
            .arg(format!("--file-filter={kind} | {}", patterns.join(" ")))
            .output();
        let output = match zenity {
            Ok(out) => out,
            Err(_) => Command::new("kdialog")
                .args(["--title", title, "--getopenfilename", "."])
                .arg(format!("{} ({kind})", patterns.join(" ")))
                .output()
                .ok()?,
        };
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (output.status.success() && !path.is_empty()).then(|| PathBuf::from(path))
    }
}

/// Opens a file with the app the system uses for it (e.g. a music player).
pub fn open_with_default_app(file: &Path) -> std::io::Result<()> {
    let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
    std::process::Command::new(opener)
        .arg(file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
}
