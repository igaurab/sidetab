use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Edge {
    Left,
    Right,
}

impl Edge {
    pub fn is_left(self) -> bool {
        self == Edge::Left
    }
}

/// What a cycling shortcut (Alt+Tab / Super+Tab) shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CycleScope {
    AllWorkspaces,
    CurrentWorkspace,
    /// The shortcut does nothing (remove the Hyprland binding to fully
    /// return the key to other uses).
    Disabled,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeVariant {
    /// Take colors from the current Omarchy theme (which follows the
    /// wallpaper), falling back to `System` when Omarchy isn't installed.
    Omarchy,
    /// Follow the desktop's color-scheme preference (checked on every reveal).
    System,
    Light,
    Dark,
}

/// Reads org.gnome.desktop.interface color-scheme ("prefer-dark" => dark),
/// which Omarchy's theme switcher keeps in sync.
pub fn system_prefers_dark() -> bool {
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("prefer-dark"))
        .unwrap_or(false)
}

/// Colors are 0xRRGGBBAA.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub background: u32,
    pub border: u32,
    pub text: u32,
    pub dim_text: u32,
    pub accent: u32,
    pub accent_text: u32,
    /// Whether this is a dark palette, for the few colors derived from it
    /// (menu borders, the settings window's own chrome).
    pub dark: bool,
}

pub const LIGHT: Palette = Palette {
    background: 0xf2f2f2ff,
    border: 0x00000022,
    text: 0x2b2b2bff,
    dim_text: 0x8a8a8aff,
    accent: 0x2c6fefff,
    accent_text: 0xffffffff,
    dark: false,
};

pub const DARK: Palette = Palette {
    background: 0x232323ff,
    border: 0xffffff1e,
    text: 0xe5e7ebff,
    dim_text: 0x9ca3afff,
    accent: 0x2c6fefff,
    accent_text: 0xffffffff,
    dark: true,
};

/// Mix `b` into `a` by `t` (0..1), keeping `a`'s alpha.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |shift: u32| {
        let ca = ((a >> shift) & 0xff) as f32;
        let cb = ((b >> shift) & 0xff) as f32;
        (ca + (cb - ca) * t).round().clamp(0.0, 255.0) as u32
    };
    (lerp(24) << 24) | (lerp(16) << 16) | (lerp(8) << 8) | (a & 0xff)
}

/// Perceived brightness of a 0xRRGGBBAA color, 0..1 (alpha ignored).
pub fn luma(c: u32) -> f32 {
    let ch = |shift: u32| ((c >> shift) & 0xff) as f32 / 255.0;
    0.2126 * ch(24) + 0.7152 * ch(16) + 0.0722 * ch(8)
}

/// The Omarchy theme directory, if Omarchy is installed and has a theme
/// selected. It's a symlink Omarchy re-points on every theme switch, so
/// this is re-read on each panel reveal rather than cached.
///
/// Omarchy 4 moved the `current` pointer out of the config directory into
/// `~/.local/state`; Omarchy 3 kept it under `~/.config`. Try the newer
/// location first so a machine carrying a stale v3 leftover still themes
/// from the live one.
fn omarchy_theme_dir() -> Option<PathBuf> {
    let candidates = [
        dirs::state_dir().map(|d| d.join("omarchy/current/theme")),
        dirs::config_dir().map(|d| d.join("omarchy/current/theme")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|dir| dir.join("colors.toml").exists())
}

/// The subset of an Omarchy theme's `colors.toml` the panel uses.
#[derive(Deserialize)]
struct OmarchyColors {
    accent: Option<String>,
    foreground: Option<String>,
    background: Option<String>,
    /// Omarchy 4: `"light"` or `"dark"`. Absent on Omarchy 3 themes, which
    /// signal light mode with a `light.mode` file instead.
    mode: Option<String>,
}

/// Build a palette from the current Omarchy theme.
pub fn omarchy_palette() -> Option<Palette> {
    let dir = omarchy_theme_dir()?;
    let colors: OmarchyColors =
        toml::from_str(&std::fs::read_to_string(dir.join("colors.toml")).ok()?).ok()?;
    let dark = match colors.mode.as_deref() {
        Some(mode) => !mode.trim().eq_ignore_ascii_case("light"),
        None => !dir.join("light.mode").exists(),
    };
    let mut p = if dark { DARK } else { LIGHT };
    p.dark = dark;

    if let Some(bg) = colors.background.as_deref().and_then(parse_hex) {
        p.background = bg;
        p.border = if dark { 0xffffff1e } else { 0x00000022 };
    }
    if let Some(fg) = colors.foreground.as_deref().and_then(parse_hex) {
        p.text = fg;
        // Group headers fade the foreground toward the background rather
        // than using the theme's own dim tone (color8), which on several
        // themes is too dark against the panel to read.
        p.dim_text = mix(fg, p.background, 0.42);
    }
    if let Some(accent) = colors.accent.as_deref().and_then(parse_hex) {
        p.accent = accent;
        // Omarchy accents are often pastel; pick the readable label color
        p.accent_text = if luma(accent) > 0.55 {
            mix(0x000000ff, accent, 0.12)
        } else {
            0xffffffff
        };
    }
    Some(p)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub variant: ThemeVariant,
    /// Optional hex overrides like "#2c6fef" or "#f2f2f2cc"
    pub background: Option<String>,
    pub accent: Option<String>,
    pub text: Option<String>,
    pub dim_text: Option<String>,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            variant: ThemeVariant::Omarchy,
            background: None,
            accent: None,
            text: None,
            dim_text: None,
        }
    }
}

impl Theme {
    pub fn palette(&self) -> Palette {
        let system = || {
            if system_prefers_dark() {
                DARK
            } else {
                LIGHT
            }
        };
        let mut p = match self.variant {
            ThemeVariant::Light => LIGHT,
            ThemeVariant::Dark => DARK,
            ThemeVariant::System => system(),
            ThemeVariant::Omarchy => omarchy_palette().unwrap_or_else(system),
        };
        if let Some(c) = self.background.as_deref().and_then(parse_hex) {
            p.background = c;
            p.dark = luma(c) < 0.5;
        }
        if let Some(c) = self.accent.as_deref().and_then(parse_hex) {
            p.accent = c;
        }
        if let Some(c) = self.text.as_deref().and_then(parse_hex) {
            p.text = c;
        }
        if let Some(c) = self.dim_text.as_deref().and_then(parse_hex) {
            p.dim_text = c;
        }
        p
    }
}

/// "#rrggbb" or "#rrggbbaa" -> 0xRRGGBBAA
pub fn parse_hex(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches('#');
    match s.len() {
        6 => u32::from_str_radix(s, 16).ok().map(|v| (v << 8) | 0xff),
        8 => u32::from_str_radix(s, 16).ok(),
        _ => None,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub edge: Edge,
    /// Width of the docked sidebar (hover reveal, `show`, search).
    pub width: f32,
    /// Width of the centered Alt-Tab / Super+Tab overlay, which is sized
    /// independently of the sidebar — window titles need the room there.
    pub overlay_width: f32,
    /// Placement along the edge, 0.0 (top) to 1.0 (bottom).
    pub v_pos: f32,
    /// What `sidetab next/prev` (bound to Alt+Tab) cycles through.
    pub alt_tab: CycleScope,
    /// What `sidetab next-ws/prev-ws` (bound to Super+Tab) cycles through.
    pub super_tab: CycleScope,
    /// Width of the hover-target sliver left visible while hidden
    /// (not exposed in the GUI).
    pub hover_strip_px: f32,
    pub show_delay_ms: u64,
    pub hide_delay_ms: u64,
    /// UI font family; defaults to Liberation Sans.
    pub font: Option<String>,
    /// App classes pinned as icon launchers in the panel header.
    pub pinned: Vec<String>,
    pub theme: Theme,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            edge: Edge::Left,
            width: 320.0,
            overlay_width: 640.0,
            v_pos: 0.5,
            alt_tab: CycleScope::AllWorkspaces,
            super_tab: CycleScope::CurrentWorkspace,
            hover_strip_px: 4.0,
            show_delay_ms: 120,
            hide_delay_ms: 300,
            font: None,
            pinned: Vec::new(),
            theme: Theme::default(),
        }
    }
}

/// Panel width bounds (shared by the settings slider and live preview).
pub const WIDTH_MIN: f32 = 170.0;
pub const WIDTH_MAX: f32 = 640.0;

/// Alt-Tab overlay width bounds. The overlay is centered rather than docked,
/// so it can go wider than the sidebar to fit long window titles.
pub const OVERLAY_WIDTH_MIN: f32 = 320.0;
pub const OVERLAY_WIDTH_MAX: f32 = 1200.0;

/// Corner radius of the panel card. Hyprland has to round the *window* to
/// the same radius: it clips a blurred window's blur region to the window's
/// rounding, so a square window paints blur into the corners the card
/// leaves transparent.
pub const CARD_ROUNDING: f32 = 12.0;

/// Cap on pinned apps. The header clips icons it can't fit, so below
/// ~240px wide the last of five pins start disappearing.
pub const MAX_PINNED: usize = 5;

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("sidetab/config.toml")
}

impl Config {
    /// Placement fraction along the edge, clamped to 0 (top) .. 1 (bottom).
    pub fn v_frac(&self) -> f32 {
        self.v_pos.clamp(0.0, 1.0)
    }

    pub fn load() -> Config {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}
