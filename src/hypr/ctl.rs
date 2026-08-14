//! Command socket (.socket.sock): queries and dispatchers, no `hyprctl`
//! process spawn per call.

use anyhow::{Context as _, Result};
use serde::Deserialize;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

fn request(cmd: &str) -> Result<String> {
    let dir = super::instance_dir().context("no running Hyprland instance found")?;
    let mut stream = UnixStream::connect(dir.join(".socket.sock"))?;
    stream.write_all(cmd.as_bytes())?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    Ok(buf)
}

pub fn dispatch(args: &str) -> Result<()> {
    request(&format!("dispatch {args}")).map(|_| ())
}

/// One connection, many commands.
pub fn batch(cmds: &[String]) -> Result<()> {
    if cmds.is_empty() {
        return Ok(());
    }
    request(&format!("[[BATCH]]{}", cmds.join(";"))).map(|_| ())
}

#[derive(Deserialize, Debug, Clone)]
pub struct Client {
    pub address: String,
    pub class: String,
    #[serde(rename = "initialClass")]
    pub initial_class: String,
    pub title: String,
    pub workspace: WorkspaceRef,
    #[serde(rename = "focusHistoryID")]
    pub focus_history_id: i64,
    pub mapped: bool,
    pub hidden: bool,
    pub pinned: bool,
    pub floating: bool,
    pub monitor: i64,
    /// 0 = not fullscreen (bitmask: 1 maximized, 2 fullscreen)
    pub fullscreen: i64,
}

#[derive(Deserialize, Debug, Clone)]
pub struct WorkspaceRef {
    pub id: i64,
    pub name: String,
}

pub fn clients() -> Result<Vec<Client>> {
    let raw = request("j/clients")?;
    Ok(serde_json::from_str(&raw)?)
}

#[derive(Deserialize, Debug, Clone)]
pub struct Monitor {
    pub id: i64,
    pub name: String,
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    pub scale: f64,
    pub focused: bool,
    #[serde(rename = "activeWorkspace")]
    pub active_workspace: WorkspaceRef,
}

impl Monitor {
    /// Size in Hyprland's logical layout coordinates (what movewindowpixel uses).
    pub fn logical_size(&self) -> (f64, f64) {
        (self.width as f64 / self.scale, self.height as f64 / self.scale)
    }
}

pub fn monitors() -> Result<Vec<Monitor>> {
    let raw = request("j/monitors")?;
    Ok(serde_json::from_str(&raw)?)
}

pub fn focused_monitor() -> Result<Monitor> {
    let mons = monitors()?;
    mons.iter()
        .find(|m| m.focused)
        .or(mons.first())
        .cloned()
        .context("no monitors")
}

#[derive(Deserialize, Debug, Clone)]
pub struct ActiveWindow {
    pub address: Option<String>,
    #[serde(default)]
    pub fullscreen: i64,
}

/// True only for real fullscreen (bit 2). Bit 1 is "maximized", which does
/// not cover pinned floating windows, so the hover strip must stay usable.
pub fn active_window_fullscreen() -> bool {
    request("j/activewindow")
        .ok()
        .and_then(|raw| serde_json::from_str::<ActiveWindow>(&raw).ok())
        .map(|w| w.fullscreen & 2 != 0)
        .unwrap_or(false)
}

/// Cursor position in logical layout coordinates.
pub fn cursor_pos() -> Option<(f64, f64)> {
    let raw = request("cursorpos").ok()?;
    let (x, y) = raw.trim().split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

pub fn active_address() -> Option<String> {
    request("j/activewindow")
        .ok()
        .and_then(|raw| serde_json::from_str::<ActiveWindow>(&raw).ok())
        .and_then(|w| w.address)
}

/// Rules that make the panel behave: floating, pinned across workspaces,
/// chromeless, instant, and never taking keyboard focus (hover must not
/// steal focus with follow_mouse=1). Re-applied on configreloaded.
pub fn apply_panel_rules() -> Result<()> {
    let rules = [
        "float on, match:class sidetab",
        "pin on, match:class sidetab",
        "border_size 0, match:class sidetab",
        "no_anim on, match:class sidetab",
        "no_shadow on, match:class sidetab",
        "rounding 0, match:class sidetab",
        // no_focus is tag-scoped so the daemon can lift it for search mode
        // (tagwindow +/- sidetab-nofocus); a class rule could never be lifted.
        "no_focus on, match:tag sidetab-nofocus",
        "float on, match:class sidetab-settings",
        "size 620 460, match:class sidetab-settings",
        "center on, match:class sidetab-settings",
    ];
    batch(
        &rules
            .iter()
            .map(|r| format!("keyword windowrule {r}"))
            .collect::<Vec<_>>(),
    )
}

pub fn focus_window(address: &str) -> Result<()> {
    dispatch(&format!("focuswindow address:{address}"))
}
