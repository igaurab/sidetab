//! Window class -> app icon PNG path.
//!
//! Resolution: class/initialClass -> .desktop entry (filename stem or
//! StartupWMClass, case-insensitive, reverse-DNS aware) -> Icon= name ->
//! freedesktop icon theme lookup. SVG results are rasterized once into
//! ~/.cache/sidetab/icons/ because gpui renders PNG reliably.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct IconResolver {
    /// lowercase desktop-entry key -> Icon= value
    desktop_index: HashMap<String, String>,
    themes: Vec<String>,
    cache_dir: PathBuf,
    memo: Mutex<HashMap<String, Option<PathBuf>>>,
}

fn data_dirs() -> Vec<PathBuf> {
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

fn parse_desktop_entry(path: &Path) -> Option<(Option<String>, Option<String>)> {
    let content = std::fs::read_to_string(path).ok()?;
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
        if let Some(v) = line.strip_prefix("Icon=") {
            icon.get_or_insert_with(|| v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("StartupWMClass=") {
            wm_class.get_or_insert_with(|| v.trim().to_string());
        }
    }
    Some((icon, wm_class))
}

fn current_icon_theme() -> Option<String> {
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let theme = s.trim().trim_matches('\'').trim_matches('"').to_string();
    (!theme.is_empty()).then_some(theme)
}

impl IconResolver {
    pub fn new() -> Self {
        let mut desktop_index = HashMap::new();
        for dir in data_dirs() {
            let apps = dir.join("applications");
            let Ok(read) = std::fs::read_dir(&apps) else {
                continue;
            };
            for entry in read.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                    continue;
                }
                let Some((Some(icon), wm_class)) = parse_desktop_entry(&path) else {
                    continue;
                };
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    desktop_index
                        .entry(stem.to_lowercase())
                        .or_insert_with(|| icon.clone());
                    // reverse-DNS stems also match by their last segment
                    if let Some(last) = stem.rsplit('.').next() {
                        desktop_index
                            .entry(last.to_lowercase())
                            .or_insert_with(|| icon.clone());
                    }
                }
                if let Some(wm) = wm_class {
                    desktop_index.entry(wm.to_lowercase()).or_insert(icon);
                }
            }
        }

        let mut themes = Vec::new();
        if let Some(t) = current_icon_theme() {
            themes.push(t);
        }
        for t in ["hicolor", "Adwaita", "breeze"] {
            if !themes.iter().any(|x| x == t) {
                themes.push(t.to_string());
            }
        }

        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("sidetab/icons");
        let _ = std::fs::create_dir_all(&cache_dir);

        IconResolver {
            desktop_index,
            themes,
            cache_dir,
            memo: Mutex::new(HashMap::new()),
        }
    }

    /// Icon= value for a window class, trying exact and reverse-DNS matches.
    fn icon_name_for_class(&self, class: &str) -> Option<String> {
        let lc = class.to_lowercase();
        if let Some(icon) = self.desktop_index.get(&lc) {
            return Some(icon.clone());
        }
        if let Some(last) = lc.rsplit('.').next() {
            if let Some(icon) = self.desktop_index.get(last) {
                return Some(icon.clone());
            }
        }
        None
    }

    fn lookup_in_themes(&self, name: &str) -> Option<PathBuf> {
        // Absolute Icon= paths are used as-is.
        if name.starts_with('/') {
            let p = PathBuf::from(name);
            return p.exists().then_some(p);
        }
        for theme in &self.themes {
            if let Some(p) = freedesktop_icons::lookup(name)
                .with_theme(theme)
                .with_size(64)
                .find()
            {
                return Some(p);
            }
        }
        for ext in ["png", "svg"] {
            let p = PathBuf::from(format!("/usr/share/pixmaps/{name}.{ext}"));
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    /// Rasterize SVGs to a cached 64px PNG; pass PNGs through.
    fn to_png(&self, path: PathBuf, icon_name: &str) -> Option<PathBuf> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("png") => Some(path),
            Some("svg") => {
                let safe: String = icon_name
                    .chars()
                    .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
                    .collect();
                let out = self.cache_dir.join(format!("{safe}-64.png"));
                if out.exists() {
                    return Some(out);
                }
                let data = std::fs::read(&path).ok()?;
                let tree =
                    resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
                let size = tree.size();
                let scale = 64.0 / size.width().max(size.height());
                let mut pixmap = resvg::tiny_skia::Pixmap::new(64, 64)?;
                resvg::render(
                    &tree,
                    resvg::tiny_skia::Transform::from_scale(scale, scale),
                    &mut pixmap.as_mut(),
                );
                pixmap.save_png(&out).ok()?;
                Some(out)
            }
            _ => None,
        }
    }

    /// PNG path for a window's app icon, or None (caller draws a letter tile).
    pub fn resolve(&self, class: &str, initial_class: &str) -> Option<PathBuf> {
        let key = format!("{class}|{initial_class}");
        if let Some(hit) = self.memo.lock().unwrap().get(&key) {
            return hit.clone();
        }
        let mut result = None;
        for candidate_class in [class, initial_class] {
            if candidate_class.is_empty() {
                continue;
            }
            let names = [
                self.icon_name_for_class(candidate_class),
                Some(candidate_class.to_lowercase()),
                Some(candidate_class.to_string()),
            ];
            for name in names.into_iter().flatten() {
                if let Some(path) = self.lookup_in_themes(&name) {
                    if let Some(png) = self.to_png(path, &name) {
                        result = Some(png);
                        break;
                    }
                }
            }
            if result.is_some() {
                break;
            }
        }
        self.memo.lock().unwrap().insert(key, result.clone());
        result
    }
}
