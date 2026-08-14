use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    LeftTop,
    LeftCenter,
    LeftBottom,
    RightTop,
    RightCenter,
    RightBottom,
}

impl Position {
    pub fn is_left(self) -> bool {
        matches!(
            self,
            Position::LeftTop | Position::LeftCenter | Position::LeftBottom
        )
    }

    /// 0.0 = top, 0.5 = center, 1.0 = bottom
    pub fn v_align(self) -> f32 {
        match self {
            Position::LeftTop | Position::RightTop => 0.0,
            Position::LeftCenter | Position::RightCenter => 0.5,
            Position::LeftBottom | Position::RightBottom => 1.0,
        }
    }

    pub const ALL: [Position; 6] = [
        Position::LeftTop,
        Position::LeftCenter,
        Position::LeftBottom,
        Position::RightTop,
        Position::RightCenter,
        Position::RightBottom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Position::LeftTop => "Top Left",
            Position::LeftCenter => "Middle Left",
            Position::LeftBottom => "Bottom Left",
            Position::RightTop => "Top Right",
            Position::RightCenter => "Middle Right",
            Position::RightBottom => "Bottom Right",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeVariant {
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
}

pub const LIGHT: Palette = Palette {
    background: 0xf2f2f2cc,
    border: 0x00000022,
    text: 0x2b2b2bff,
    dim_text: 0x8a8a8aff,
    accent: 0x2c6fefff,
    accent_text: 0xffffffff,
};

pub const DARK: Palette = Palette {
    background: 0x232323d9,
    border: 0xffffff1e,
    text: 0xe5e7ebff,
    dim_text: 0x9ca3afff,
    accent: 0x2c6fefff,
    accent_text: 0xffffffff,
};

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
            variant: ThemeVariant::System,
            background: None,
            accent: None,
            text: None,
            dim_text: None,
        }
    }
}

impl Theme {
    pub fn palette(&self) -> Palette {
        let mut p = match self.variant {
            ThemeVariant::Light => LIGHT,
            ThemeVariant::Dark => DARK,
            ThemeVariant::System => {
                if system_prefers_dark() {
                    DARK
                } else {
                    LIGHT
                }
            }
        };
        if let Some(c) = self.background.as_deref().and_then(parse_hex) {
            p.background = c;
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
    pub position: Position,
    pub width: f32,
    pub hover_reveal: bool,
    pub hover_strip_px: f32,
    pub show_delay_ms: u64,
    pub hide_delay_ms: u64,
    /// UI font family; defaults to Liberation Sans.
    pub font: Option<String>,
    /// Window classes pinned to the top "Pinned" section of the panel.
    pub pinned: Vec<String>,
    pub theme: Theme,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            position: Position::LeftCenter,
            width: 320.0,
            hover_reveal: true,
            hover_strip_px: 4.0,
            show_delay_ms: 120,
            hide_delay_ms: 300,
            font: None,
            pinned: Vec::new(),
            theme: Theme::default(),
        }
    }
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("sidetab/config.toml")
}

impl Config {
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
