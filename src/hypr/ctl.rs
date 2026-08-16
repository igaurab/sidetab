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

/// Which config parser the running Hyprland uses.
///
/// Hyprland 0.56 added a Lua config; Omarchy 4 adopts it, Omarchy 3 stays on
/// the legacy `.conf` parser. Under Lua, the socket rejects `keyword` outright
/// and reinterprets `dispatch <args>` as the Lua expression
/// `hl.dispatch(<args>)`, so both need a second spelling.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Parser {
    Legacy,
    Lua,
}

/// Probed once per process: a bare `keyword` is a no-op the legacy parser
/// answers with its own usage error, while the Lua parser answers with a
/// refusal naming "non-legacy". Nothing is mutated either way.
pub fn parser() -> Parser {
    static PARSER: std::sync::OnceLock<Parser> = std::sync::OnceLock::new();
    *PARSER.get_or_init(|| match request("keyword") {
        Ok(reply) if reply.contains("non-legacy") => Parser::Lua,
        _ => Parser::Legacy,
    })
}

/// Run a Lua chunk. Only meaningful on [`Parser::Lua`].
fn eval(code: &str) -> Result<()> {
    request(&format!("eval {code}")).map(|_| ())
}

/// Escape a Lua single-quoted string literal. Addresses and tags are
/// sidetab's own, but window titles reach `exec` lines from arbitrary apps.
fn lua_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// The dispatchers sidetab uses, spelled for whichever parser is live.
///
/// Kept as a closed enum rather than free-form strings so that adding a
/// dispatcher forces both spellings to be written.
pub enum Dsp<'a> {
    ResizeExact { w: i64, h: i64, addr: &'a str },
    MoveExact { x: i64, y: i64, addr: &'a str },
    Tag { add: bool, tag: &'a str, addr: &'a str },
    FocusWindow(&'a str),
    FocusCurrentOrLast,
    RaiseWindow(&'a str),
    BringActiveToTop,
    CloseWindow(&'a str),
    Exec(&'a str),
    CycleNext { prev: bool },
    WorkspaceRel { forward: bool },
}

impl Dsp<'_> {
    /// `dispatch <args>` for the legacy parser.
    fn legacy(&self) -> String {
        match self {
            Dsp::ResizeExact { w, h, addr } => {
                format!("dispatch resizewindowpixel exact {w} {h},address:{addr}")
            }
            Dsp::MoveExact { x, y, addr } => {
                format!("dispatch movewindowpixel exact {x} {y},address:{addr}")
            }
            Dsp::Tag { add, tag, addr } => {
                let sign = if *add { '+' } else { '-' };
                format!("dispatch tagwindow {sign}{tag} address:{addr}")
            }
            Dsp::FocusWindow(addr) => format!("dispatch focuswindow address:{addr}"),
            Dsp::FocusCurrentOrLast => "dispatch focuscurrentorlast".into(),
            Dsp::RaiseWindow(addr) => format!("dispatch alterzorder top,address:{addr}"),
            Dsp::BringActiveToTop => "dispatch bringactivetotop".into(),
            Dsp::CloseWindow(addr) => format!("dispatch closewindow address:{addr}"),
            Dsp::Exec(cmd) => format!("dispatch exec {cmd}"),
            Dsp::CycleNext { prev } => {
                if *prev {
                    "dispatch cyclenext prev".into()
                } else {
                    "dispatch cyclenext".into()
                }
            }
            Dsp::WorkspaceRel { forward } => {
                let step = if *forward { "e+1" } else { "e-1" };
                format!("dispatch workspace {step}")
            }
        }
    }

    /// The Lua expression the 0.56 socket wraps in `hl.dispatch(...)`.
    fn lua(&self) -> String {
        let win = |addr: &str| format!("window = {}", lua_str(&format!("address:{addr}")));
        match self {
            Dsp::ResizeExact { w, h, addr } => format!(
                "dispatch hl.dsp.window.resize({{ exact = true, x = {w}, y = {h}, {} }})",
                win(addr)
            ),
            Dsp::MoveExact { x, y, addr } => format!(
                "dispatch hl.dsp.window.move({{ exact = true, x = {x}, y = {y}, {} }})",
                win(addr)
            ),
            Dsp::Tag { add, tag, addr } => {
                let sign = if *add { '+' } else { '-' };
                format!(
                    "dispatch hl.dsp.window.tag({{ tag = {}, {} }})",
                    lua_str(&format!("{sign}{tag}")),
                    win(addr)
                )
            }
            Dsp::FocusWindow(addr) => format!("dispatch hl.dsp.focus({{ {} }})", win(addr)),
            // The Lua focus dispatcher has no current-or-last toggle; `last`
            // is the same motion for the one case sidetab uses it in.
            Dsp::FocusCurrentOrLast => "dispatch hl.dsp.focus({ last = true })".into(),
            Dsp::RaiseWindow(addr) => format!(
                "dispatch hl.dsp.window.alter_zorder({{ mode = 'top', {} }})",
                win(addr)
            ),
            Dsp::BringActiveToTop => "dispatch hl.dsp.window.bring_to_top()".into(),
            Dsp::CloseWindow(addr) => format!("dispatch hl.dsp.window.close({{ {} }})", win(addr)),
            Dsp::Exec(cmd) => format!("dispatch hl.dsp.exec_cmd({})", lua_str(cmd)),
            Dsp::CycleNext { prev } => {
                format!("dispatch hl.dsp.window.cycle_next({{ prev = {prev} }})")
            }
            Dsp::WorkspaceRel { forward } => {
                let step = if *forward { "e+1" } else { "e-1" };
                format!("dispatch hl.dsp.focus({{ workspace = {} }})", lua_str(step))
            }
        }
    }

    fn encode(&self) -> String {
        match parser() {
            Parser::Legacy => self.legacy(),
            Parser::Lua => self.lua(),
        }
    }
}

pub fn dispatch(d: Dsp) -> Result<()> {
    request(&d.encode()).map(|_| ())
}

/// One connection, many dispatchers.
pub fn dispatch_all(ds: &[Dsp]) -> Result<()> {
    batch(&ds.iter().map(Dsp::encode).collect::<Vec<_>>())
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
    let rounding = crate::config::CARD_ROUNDING as i64;
    // (legacy rule body, Lua rule table) — the same rule in both spellings.
    // The Lua matcher is a regex, so the class matches are anchored to keep
    // the panel's rules off the settings window (which contains "sidetab").
    let rules = [
        ("float on, match:class sidetab".to_string(),
         "{ float = true, match = { class = '^sidetab$' } }".to_string()),
        ("pin on, match:class sidetab".to_string(),
         "{ pin = true, match = { class = '^sidetab$' } }".to_string()),
        ("no_anim on, match:class sidetab".to_string(),
         "{ no_anim = true, match = { class = '^sidetab$' } }".to_string()),
        // no_focus is tag-scoped so the daemon can lift it for search mode
        // (tagwindow +/- sidetab-nofocus); a class rule could never be lifted.
        ("no_focus on, match:tag sidetab-nofocus".to_string(),
         "{ no_focus = true, match = { tag = 'sidetab-nofocus' } }".to_string()),
        // The chrome rules are tag-scoped so they can be re-asserted on a
        // live window — see remove_chrome.
        (format!("border_size 0, match:tag {CHROMELESS_TAG}"),
         format!("{{ border_size = 0, match = {{ tag = '{CHROMELESS_TAG}' }} }}")),
        (format!("no_shadow on, match:tag {CHROMELESS_TAG}"),
         format!("{{ no_shadow = true, match = {{ tag = '{CHROMELESS_TAG}' }} }}")),
        // matching the card's own radius: Hyprland clips the window's blur
        // region to this, and a square region bleeds into the card's
        // transparent corners
        (format!("rounding {rounding}, match:tag {CHROMELESS_TAG}"),
         format!("{{ rounding = {rounding}, match = {{ tag = '{CHROMELESS_TAG}' }} }}")),
        // focusing the panel on a fullscreen workspace must never transfer
        // fullscreen onto the panel itself (mouse use over fullscreen apps)
        ("sync_fullscreen off, match:class sidetab".to_string(),
         "{ sync_fullscreen = false, match = { class = '^sidetab$' } }".to_string()),
        (format!("float on, match:class {SETTINGS_CLASS}"),
         format!("{{ float = true, match = {{ class = '^{SETTINGS_CLASS}$' }} }}")),
        (format!("size 680 600, match:class {SETTINGS_CLASS}"),
         format!("{{ size = '680 600', match = {{ class = '^{SETTINGS_CLASS}$' }} }}")),
        (format!("center on, match:class {SETTINGS_CLASS}"),
         format!("{{ center = true, match = {{ class = '^{SETTINGS_CLASS}$' }} }}")),
    ];
    let cmds = match parser() {
        Parser::Legacy => rules
            .iter()
            .map(|(legacy, _)| format!("keyword windowrule {legacy}"))
            .collect::<Vec<_>>(),
        Parser::Lua => rules
            .iter()
            .map(|(_, lua)| format!("eval hl.window_rule({lua})"))
            .collect::<Vec<_>>(),
    };
    batch(&cmds)
}

pub const CHROMELESS_TAG: &str = "sidetab-chromeless";

/// Wayland app_id of the settings window (see daemon::run).
pub const SETTINGS_CLASS: &str = "sidetab-settings";

/// Strip the window's decorations: no focus border, no drop shadow, and a
/// rounding that matches the card so Hyprland's blur stays inside it.
///
/// The rules themselves are applied by [`apply_panel_rules`]; this forces
/// Hyprland to (re-)evaluate them on the live window. Rules only land on a
/// window when it maps, a config reload wipes every keyword-applied rule,
/// and re-adding a rule afterwards doesn't re-decorate an existing window —
/// `hyprctl setprop`, which used to patch a mapped window, is gone as of
/// Hyprland 0.56. Changing a window's tags does trigger a re-evaluation,
/// hence the drop-and-re-add.
pub fn remove_chrome(address: &str) -> Result<()> {
    dispatch_all(&[
        Dsp::Tag {
            add: false,
            tag: CHROMELESS_TAG,
            addr: address,
        },
        Dsp::Tag {
            add: true,
            tag: CHROMELESS_TAG,
            addr: address,
        },
    ])
}

fn no_warps_enabled() -> bool {
    request("getoption cursor:no_warps")
        // legacy prints `int: 1`; the Lua parser reports the option's real
        // bool type as `bool: true`.
        .map(|s| s.contains("int: 1") || s.contains("bool: true"))
        .unwrap_or(false)
}

/// Set `cursor:no_warps`, in whichever spelling the live parser accepts.
fn set_no_warps(on: bool) -> String {
    match parser() {
        Parser::Legacy => format!("keyword cursor:no_warps {on}"),
        Parser::Lua => format!("eval hl.config({{ cursor = {{ no_warps = {on} }} }})"),
    }
}

/// Dispatch without letting Hyprland warp the cursor to the focused
/// window (restores the user's own no_warps setting afterwards).
fn dispatch_no_warp(d: Dsp) -> Result<()> {
    if no_warps_enabled() {
        dispatch(d)
    } else {
        batch(&[set_no_warps(true), d.encode(), set_no_warps(false)])
    }
}

pub fn focus_window(address: &str) -> Result<()> {
    dispatch_no_warp(Dsp::FocusWindow(address))
}

pub fn focus_current_or_last() -> Result<()> {
    dispatch_no_warp(Dsp::FocusCurrentOrLast)
}

/// Vertical extent (y, height) of the settings window, if it's open. The
/// centered overlay preview steers around it so the slider being dragged
/// stays visible.
pub fn settings_window_band() -> Option<(f64, f64)> {
    clients()
        .ok()?
        .into_iter()
        .find(|c| c.class == SETTINGS_CLASS && c.mapped && !c.hidden)
        .map(|c| (c.at[1] as f64, c.size[1] as f64))
}

/// Raise a window to the top of the stack. Hyprland marks raised windows
/// as allowed-over-fullscreen, so this is how both the panel and switch
/// targets stay visible above a fullscreen window.
pub fn raise_window(address: &str) -> Result<()> {
    dispatch(Dsp::RaiseWindow(address))
}
