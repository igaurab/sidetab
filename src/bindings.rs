//! Installs sidetab's Hyprland keybindings (and the daemon autostart) into
//! the user's config, so a fresh install doesn't require hand-copying a
//! snippet out of the README.
//!
//! Hyprland 0.56 added a Lua config alongside the classic `.conf` one and
//! Omarchy 4 switched to it. The two are written completely differently, and
//! a block placed in the wrong one is read by nobody — which is the failure
//! this command exists to prevent. The live parser is authoritative when
//! Hyprland is running; otherwise the config files on disk decide.
//!
//! Only stock `hl.*` Lua API is emitted, never Omarchy's `o.*` helpers, so
//! the block works on any Lua config rather than Omarchy's specifically.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Marker kept in the block's first line. Presence of this string anywhere
/// in the target file means sidetab already installed its bindings.
const MARKER: &str = "sidetab:bindings";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// `hyprland.lua` — Hyprland 0.56+, Omarchy 4.
    Lua,
    /// `hyprland.conf` — the classic parser, Omarchy 3.
    Conf,
}

impl Style {
    pub fn label(self) -> &'static str {
        match self {
            Style::Lua => "Hyprland Lua config",
            Style::Conf => "Hyprland .conf config",
        }
    }
}

const LUA_BLOCK: &str = r#"
-- sidetab:bindings — window switcher (https://github.com/igaurab/sidetab)
-- ALT+TAB and SUPER+TAB are unbound first so these replace whatever the
-- distro bound them to.
hl.unbind("ALT + TAB")
hl.unbind("ALT + SHIFT + TAB")
hl.unbind("SUPER + TAB")

hl.bind("ALT + TAB", hl.dsp.exec_cmd("sidetab next"),
  { repeating = true, description = "Window switcher" })
hl.bind("ALT + SHIFT + TAB", hl.dsp.exec_cmd("sidetab prev"),
  { repeating = true, description = "Window switcher (reverse)" })
hl.bind("ALT + ALT_L", hl.dsp.exec_cmd("sidetab commit"),
  { release = true, transparent = true })

hl.bind("SUPER + TAB", hl.dsp.exec_cmd("sidetab next-ws"),
  { repeating = true, description = "Switch window on workspace" })
hl.bind("SUPER + SUPER_L", hl.dsp.exec_cmd("sidetab commit"),
  { release = true, transparent = true })

-- Optional: fuzzy window search
-- hl.bind("SUPER + SLASH", hl.dsp.exec_cmd("sidetab search"),
--   { description = "Window search" })

hl.on("hyprland.start", function()
  hl.exec_cmd("sidetab daemon")
end)
"#;

const CONF_BLOCK: &str = r#"
# sidetab:bindings — window switcher (https://github.com/igaurab/sidetab)
# ALT+TAB and SUPER+TAB are unbound first so these replace whatever the
# distro bound them to.
unbind = ALT, TAB
unbind = ALT SHIFT, TAB
unbind = SUPER, TAB

binde = ALT, TAB, exec, sidetab next
binde = ALT SHIFT, TAB, exec, sidetab prev
bindrt = ALT, ALT_L, exec, sidetab commit

binde = SUPER, TAB, exec, sidetab next-ws
bindrt = SUPER, SUPER_L, exec, sidetab commit

# Optional: fuzzy window search
# bindd = SUPER, SLASH, Window search, exec, sidetab search

exec-once = sidetab daemon
"#;

/// Where the block will be written, and in which dialect.
pub struct Plan {
    pub style: Style,
    pub file: PathBuf,
}

impl Plan {
    fn block(&self) -> &'static str {
        match self.style {
            Style::Lua => LUA_BLOCK,
            Style::Conf => CONF_BLOCK,
        }
    }

    /// True when a sidetab block is already present in the target file.
    pub fn installed(&self) -> bool {
        std::fs::read_to_string(&self.file).is_ok_and(|s| s.contains(MARKER))
    }
}

fn hypr_dir() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("could not determine XDG config directory")?
        .join("hypr"))
}

/// Which dialect the user's Hyprland actually reads.
///
/// A running Hyprland answers definitively (see [`crate::hypr::ctl::parser`]);
/// otherwise `hyprland.lua` winning over `hyprland.conf` mirrors Hyprland's
/// own preference when both exist.
fn detect_style(dir: &Path) -> Style {
    if crate::hypr::instance_dir().is_some() {
        return match crate::hypr::ctl::parser() {
            crate::hypr::ctl::Parser::Lua => Style::Lua,
            crate::hypr::ctl::Parser::Legacy => Style::Conf,
        };
    }
    if dir.join("hyprland.lua").exists() {
        Style::Lua
    } else {
        Style::Conf
    }
}

/// Pick the file to append to: a dedicated bindings file when the config is
/// split that way (Omarchy's layout), else the main config.
///
/// A `bindings.*` file is only used when the main config actually pulls it
/// in — it existing is not enough. Writing to an orphaned include would put
/// the block somewhere Hyprland never reads, which is the exact failure this
/// whole command exists to avoid.
pub fn plan() -> Result<Plan> {
    let dir = hypr_dir()?;
    let style = detect_style(&dir);
    let (split, main) = match style {
        Style::Lua => ("bindings.lua", "hyprland.lua"),
        Style::Conf => ("bindings.conf", "hyprland.conf"),
    };
    let main_path = dir.join(main);
    let split_path = dir.join(split);
    let main_text = std::fs::read_to_string(&main_path).unwrap_or_default();
    // `require("hypr.bindings")` / `source = ~/.config/hypr/bindings.conf`
    let stem = split.rsplit_once('.').map(|(s, _)| s).unwrap_or(split);
    let sourced = main_text
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with('#') && !t.starts_with("--")
        })
        .any(|l| l.contains(stem));

    let file = if split_path.exists() && sourced {
        split_path
    } else {
        main_path
    };
    Ok(Plan { style, file })
}

pub enum Outcome {
    AlreadyInstalled {
        file: PathBuf,
    },
    Installed {
        file: PathBuf,
        backup: Option<PathBuf>,
        reloaded: bool,
    },
}

/// Append the bindings block, backing up the file first. Idempotent: a file
/// already carrying the marker is left untouched.
pub fn install() -> Result<Outcome> {
    let plan = plan()?;
    if plan.installed() {
        return Ok(Outcome::AlreadyInstalled { file: plan.file });
    }

    let existing = std::fs::read_to_string(&plan.file).unwrap_or_default();
    let backup = if existing.is_empty() {
        None
    } else {
        let path = plan.file.with_extension(format!(
            "{}.sidetab.bak",
            plan.file
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("bak")
        ));
        std::fs::write(&path, &existing)
            .with_context(|| format!("writing backup {}", path.display()))?;
        Some(path)
    };

    if let Some(parent) = plan.file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    // Keep exactly one blank line between whatever was there and the block.
    let mut out = existing.trim_end().to_string();
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(plan.block());
    std::fs::write(&plan.file, out)
        .with_context(|| format!("writing {}", plan.file.display()))?;

    // Applying it is itself a manual step, so do it when Hyprland is live.
    let reloaded = crate::hypr::instance_dir().is_some() && crate::hypr::ctl::reload().is_ok();

    Ok(Outcome::Installed {
        file: plan.file,
        backup,
        reloaded,
    })
}
