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
    /// top-left position in logical layout coords
    pub at: [i64; 2],
    pub size: [i64; 2],
    pub class: String,
    #[serde(rename = "initialClass")]
    pub initial_class: String,
    pub title: String,
    pub workspace: WorkspaceRef,
    #[serde(rename = "focusHistoryID")]
    pub focus_history_id: i64,
    pub mapped: bool,
    pub hidden: bool,
    #[serde(default)]
    pub pid: i64,
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
        // focusing the panel on a fullscreen workspace must never transfer
        // fullscreen onto the panel itself (mouse use over fullscreen apps)
        "sync_fullscreen off, match:class sidetab",
        "float on, match:class sidetab-settings",
        "size 620 600, match:class sidetab-settings",
        "center on, match:class sidetab-settings",
    ];
    batch(
        &rules
            .iter()
            .map(|r| format!("keyword windowrule {r}"))
            .collect::<Vec<_>>(),
    )
}

/// Kill the window's border via a per-window prop. Unlike keyword-applied
/// windowrules (wiped on every config reload until we re-apply them), a
/// setprop sticks to the window, so the panel can never flash the theme's
/// focus border.
pub fn remove_border(address: &str) -> Result<()> {
    request(&format!("setprop address:{address} bordersize 0")).map(|_| ())
}

fn no_warps_enabled() -> bool {
    request("getoption cursor:no_warps")
        .map(|s| s.contains("int: 1"))
        .unwrap_or(false)
}

/// Dispatch without letting Hyprland warp the cursor to the focused
/// window (restores the user's own no_warps setting afterwards).
fn dispatch_no_warp(args: &str) -> Result<()> {
    if no_warps_enabled() {
        dispatch(args)
    } else {
        batch(&[
            "keyword cursor:no_warps true".to_string(),
            format!("dispatch {args}"),
            "keyword cursor:no_warps false".to_string(),
        ])
    }
}

pub fn focus_window(address: &str) -> Result<()> {
    dispatch_no_warp(&format!("focuswindow address:{address}"))
}

pub fn focus_current_or_last() -> Result<()> {
    dispatch_no_warp("focuscurrentorlast")
}

/// Raise a window to the top of the stack. Hyprland marks raised windows
/// as allowed-over-fullscreen, so this is how both the panel and switch
/// targets stay visible above a fullscreen window.
pub fn raise_window(address: &str) -> Result<()> {
    dispatch(&format!("alterzorder top,address:{address}"))
}
