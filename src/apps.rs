//! Installed-application index built from .desktop entries. Backs the
//! pinned-apps picker in settings and dock-style launching from the panel.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DesktopApp {
    /// Display name (Name=).
    pub name: String,
    /// Best guess at the window class: StartupWMClass= if present,
    /// otherwise the desktop-file stem. Stored in config when pinned.
    pub class: String,
    /// Exec= with %-field codes stripped, ready to hand to `dispatch exec`.
    pub exec: String,
    /// Icon= name, if any.
    pub icon: Option<String>,
}

/// XDG data dirs that may hold applications/ and icons/, most specific first.
pub(crate) fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home_data) = dirs::data_dir() {
        dirs.push(home_data);
    }
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/share/flatpak/exports/share"));
    }
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    let xdg = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for d in xdg.split(':').filter(|d| !d.is_empty()) {
        dirs.push(PathBuf::from(d));
    }
    dirs
}

/// Whether a pinned class and a window/app class refer to the same app.
/// Case-insensitive, and reverse-DNS ids match by their last segment
/// ("org.mozilla.firefox" == "firefox").
pub fn class_matches(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    if a == b {
        return true;
    }
    let last = |s: &str| s.rsplit('.').next().unwrap_or(s).to_string();
    !a.is_empty() && !b.is_empty() && last(&a) == last(&b)
}

fn parse_app(path: &Path) -> Option<DesktopApp> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut name = None;
    let mut exec = None;
    let mut icon = None;
    let mut wm_class = None;
    let mut in_main_section = false;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main_section = line == "[Desktop Entry]";
            continue;
        }
        if !in_main_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Name" => name.get_or_insert_with(|| value.to_string()),
            "Exec" => exec.get_or_insert_with(|| value.to_string()),
            "Icon" => icon.get_or_insert_with(|| value.to_string()),
            "StartupWMClass" => wm_class.get_or_insert_with(|| value.to_string()),
            // launchers and terminal apps don't belong in a dock
            "Type" if value != "Application" => return None,
            "NoDisplay" | "Hidden" | "Terminal" if value == "true" => return None,
            _ => continue,
        };
    }
    let stem = path.file_stem()?.to_str()?.to_string();
    // strip %f/%u/... field codes; they only matter when opening files
    let exec = exec?
        .split_whitespace()
        .filter(|tok| !(tok.len() == 2 && tok.starts_with('%')))
        .collect::<Vec<_>>()
        .join(" ");
    Some(DesktopApp {
        name: name?,
        class: wm_class.unwrap_or(stem),
        exec,
        icon,
    })
}

/// Every launchable installed application, deduped by desktop id
/// (earlier data dirs win) and sorted by display name.
pub fn installed() -> Vec<DesktopApp> {
    let mut seen = std::collections::HashSet::new();
    let mut apps = Vec::new();
    for dir in data_dirs() {
        let Ok(read) = std::fs::read_dir(dir.join("applications")) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !seen.insert(stem.to_lowercase()) {
                continue;
            }
            if let Some(app) = parse_app(&path) {
                apps.push(app);
            }
        }
    }
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps
}
