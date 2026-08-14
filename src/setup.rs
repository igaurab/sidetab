//! Desktop integration for cargo-install users: drops the app icon and a
//! .desktop launcher into the per-user XDG data directory so sidetab shows
//! up in application menus. Idempotent — files are only rewritten when
//! their content actually changed.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const ICON_SVG: &str = include_str!("../assets/sidetab.svg");
const DESKTOP_ENTRY: &str = include_str!("../assets/sidetab.desktop");

/// `~/.local/share/icons/hicolor/scalable/apps/sidetab.svg`
pub fn icon_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("icons/hicolor/scalable/apps/sidetab.svg"))
}

/// `~/.local/share/applications/sidetab.desktop`
pub fn desktop_entry_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("applications/sidetab.desktop"))
}

/// Write `content` to `path` unless it is already there verbatim.
/// Returns true if the file was (re)written.
fn write_if_changed(path: &Path, content: &str) -> Result<bool> {
    if std::fs::read_to_string(path).is_ok_and(|cur| cur == content) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

/// Best-effort refresh of desktop caches; missing tools and failures are ignored.
fn refresh_caches(data_dir: &Path) {
    use std::process::{Command, Stdio};
    let _ = Command::new("gtk-update-icon-cache")
        .arg("-q")
        .arg(data_dir.join("icons/hicolor"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = Command::new("update-desktop-database")
        .arg("-q")
        .arg(data_dir.join("applications"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Install the sidetab icon and desktop entry into the user's XDG data
/// directory. Safe to call on every startup: it touches nothing when the
/// installed files are already up to date.
pub fn install_desktop_integration() -> Result<()> {
    let data_dir = dirs::data_dir().context("could not determine XDG data directory")?;
    let icon = data_dir.join("icons/hicolor/scalable/apps/sidetab.svg");
    let entry = data_dir.join("applications/sidetab.desktop");

    let wrote_icon = write_if_changed(&icon, ICON_SVG)?;
    let wrote_entry = write_if_changed(&entry, DESKTOP_ENTRY)?;
    if wrote_icon || wrote_entry {
        refresh_caches(&data_dir);
    }
    Ok(())
}
