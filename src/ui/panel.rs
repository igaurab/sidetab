//! The switcher panel: a Contexts-style sidebar. Owns the presence state
//! machine — every reveal/hide/focus transition funnels through here.

use crate::config::{Config, CycleScope, Palette};
use crate::daemon::Msg;
use crate::hypr::ctl;
use crate::hypr::events::HyprEvent;
use crate::icons::IconResolver;
use crate::windows::{self, Group, WinEntry};
use gpui::{
    div, img, prelude::*, px, rgba, Context, FocusHandle, KeyDownEvent, MouseButton, Window,
};
use std::time::Duration;

pub const NOFOCUS_TAG: &str = "sidetab-nofocus";

const ROW_H: f32 = 26.0;
const GROUP_HEADER_H: f32 = 24.0;
const PANEL_HEADER_H: f32 = 30.0;
const PAD_V: f32 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Hidden,
    /// Mouse is on the edge strip; reveal scheduled.
    HoverPending,
    /// Visible without keyboard focus (hover or `toggle`).
    Revealed,
    /// Alt is held; visible (or reveal pending), driven by next/prev/commit.
    Cycling,
    /// Keyboard-focused with a search field.
    Search,
}

/// Which windows a cycling session includes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    All,
    /// Only the currently active workspace (Super+Tab).
    Workspace,
}

/// A pinned app, shown as a dock-style launcher row below the window list.
/// Every click launches it (a new instance if the app allows one; a
/// single-instance app will just surface its existing window itself).
#[derive(Debug, Clone)]
struct Launcher {
    class: String,
    name: String,
    exec: Option<String>,
    running: bool,
}

/// Right-click context menu on a row. On a window row (`address` set) it
/// offers pinning that window and pinning its app; on a launcher row only
/// unpinning the app.
#[derive(Debug, Clone)]
struct RowMenu {
    class: String,
    address: Option<String>,
    position: gpui::Point<gpui::Pixels>,
}

pub struct Switcher {
    cfg: Config,
    palette: Palette,
    entries: Vec<WinEntry>,
    groups: Vec<Group>,
    /// installed-app index, for resolving pinned classes to name/exec
    apps: Vec<crate::apps::DesktopApp>,
    /// dock rows for every pinned app (grouped view only)
    launchers: Vec<Launcher>,
    /// window addresses pinned to the top "Pinned" group (session-only:
    /// addresses die with their windows, so persisting them is pointless)
    pinned_windows: Vec<String>,
    /// open right-click menu, if any
    menu: Option<RowMenu>,
    /// entry indices in on-screen order (group by group)
    order: Vec<usize>,
    /// entry indices when a search query is active
    filtered: Vec<usize>,
    query: String,
    /// position within the current visible order
    selected: usize,
    mode: Mode,
    scope: Scope,
    /// our own Hyprland window address (discovered after mapping)
    address: Option<String>,
    /// live width-drag: window sits at WIDTH_MAX, card renders this wide
    width_preview: Option<f32>,
    /// windows we floated to lift them above a fullscreen sibling, with
    /// their original tiled geometry; re-tiled in place when coverage ends
    temp_floated: Vec<(String, Option<([i64; 2], [i64; 2])>)>,
    fullscreen_active: bool,
    dirty: bool,
    /// the current reveal came from edge hover (auto-hides on leave)
    hover_originated: bool,
    /// consecutive polls with the cursor outside the revealed panel
    outside_polls: u8,
    /// hover reveal is suppressed until the cursor has left the trigger
    /// zone once (prevents instant re-expansion right after switching)
    hover_armed: bool,
    reveal_gen: u64,
    hide_gen: u64,
    icons: IconResolver,
    pub focus_handle: FocusHandle,
}

impl Switcher {
    pub fn new(cfg: Config, cx: &mut Context<Self>) -> Self {
        // Hyprland delivers no pointer input to no_focus windows, so the
        // parked edge strip cannot see hover events. Poll the cursor
        // instead while hidden; once revealed the tag is lifted and real
        // pointer events take over.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(180))
                .await;
            if this.update(cx, |this, cx| this.poll_edge(cx)).is_err() {
                return;
            }
        })
        .detach();
        let palette = cfg.theme.palette();
        Switcher {
            cfg,
            palette,
            entries: Vec::new(),
            groups: Vec::new(),
            apps: crate::apps::installed(),
            launchers: Vec::new(),
            pinned_windows: Vec::new(),
            menu: None,
            order: Vec::new(),
            filtered: Vec::new(),
            query: String::new(),
            selected: 0,
            mode: Mode::Hidden,
            scope: Scope::All,
            address: None,
            width_preview: None,
            temp_floated: Vec::new(),
            fullscreen_active: ctl::active_window_fullscreen(),
            dirty: true,
            hover_originated: false,
            outside_polls: 0,
            hover_armed: true,
            reveal_gen: 0,
            hide_gen: 0,
            icons: IconResolver::new(),
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn set_address(&mut self, address: String, cx: &mut Context<Self>) {
        self.address = Some(address);
        self.set_nofocus(true);
        // the freshly mapped window may have grabbed focus before the
        // nofocus tag landed — give it back
        if ctl::active_address().as_ref() == self.address.as_ref() {
            let _ = ctl::focus_current_or_last();
        }
        self.rest(cx);
    }

    fn set_nofocus(&self, on: bool) {
        if let Some(addr) = &self.address {
            let sign = if on { '+' } else { '-' };
            let _ = ctl::dispatch(&format!("tagwindow {sign}{NOFOCUS_TAG} address:{addr}"));
        }
    }


    // ---- data ----

    fn refresh(&mut self) {
        self.entries = windows::fetch();
        // safety net: if no fullscreen window exists anymore (e.g. it was
        // closed, which emits no fullscreen event), restore temp floats
        if !self.temp_floated.is_empty() && !self.entries.iter().any(|e| e.fullscreen) {
            self.restore_temp_floated();
            self.entries = windows::fetch();
        }
        // drop pins whose window closed (checked against ALL windows,
        // before any workspace filtering, so other-workspace pins survive)
        self.pinned_windows
            .retain(|a| self.entries.iter().any(|e| &e.address == a));
        if self.scope == Scope::Workspace {
            if let Ok(mon) = ctl::focused_monitor() {
                self.entries
                    .retain(|e| e.workspace_id == mon.active_workspace.id);
            }
        }
        self.groups = windows::group(&self.entries, &self.pinned_windows);
        self.order = windows::display_order(&self.groups);
        let has_window = |pin: &str| {
            self.entries.iter().any(|e| {
                crate::apps::class_matches(pin, &e.class)
                    || crate::apps::class_matches(pin, &e.initial_class)
            })
        };
        self.launchers = self
            .cfg
            .pinned
            .iter()
            .map(|pin| {
                let app = self
                    .apps
                    .iter()
                    .find(|a| crate::apps::class_matches(pin, &a.class));
                Launcher {
                    class: pin.clone(),
                    name: app
                        .map(|a| a.name.clone())
                        .unwrap_or_else(|| windows::app_name(pin)),
                    exec: app.map(|a| a.exec.clone()),
                    running: has_window(pin),
                }
            })
            .collect();
        self.dirty = false;
        if !self.query.is_empty() {
            self.filtered = windows::filter(&self.entries, &self.query);
        }
        let len = self.visible_order().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    fn visible_order(&self) -> &[usize] {
        if self.searching() {
            &self.filtered
        } else {
            &self.order
        }
    }

    fn searching(&self) -> bool {
        self.mode == Mode::Search && !self.query.is_empty()
    }

    fn selected_entry(&self) -> Option<&WinEntry> {
        self.visible_order()
            .get(self.selected)
            .map(|&i| &self.entries[i])
    }

    /// Display position of the most-recently-used *other* window.
    fn mru_position(&self) -> usize {
        let target = self
            .entries
            .iter()
            .filter(|e| e.focus_history_id != 0)
            .min_by_key(|e| e.focus_history_id)
            .map(|e| e.address.clone());
        match target {
            Some(addr) => self
                .order
                .iter()
                .position(|&i| self.entries[i].address == addr)
                .unwrap_or(0),
            None => 0,
        }
    }

    // ---- geometry ----

    fn monitor(&self) -> Option<ctl::Monitor> {
        ctl::focused_monitor().ok()
    }

    fn content_height(&self) -> f32 {
        let mut h = PAD_V * 2.0 + PANEL_HEADER_H;
        if self.searching() {
            h += self.filtered.len().max(1) as f32 * ROW_H;
        } else {
            for g in &self.groups {
                h += GROUP_HEADER_H + g.rows.len() as f32 * ROW_H;
            }
            if self.show_launchers() {
                h += GROUP_HEADER_H + self.launchers.len() as f32 * ROW_H;
            }
            if self.groups.is_empty() && !self.show_launchers() {
                h += ROW_H;
            }
        }
        h
    }

    /// The pinned dock appears in the docked grouped view, not in the
    /// centered cycling overlay or search results.
    fn show_launchers(&self) -> bool {
        !self.launchers.is_empty() && !self.centered() && !self.searching()
    }

    /// Pin the app if it isn't pinned, unpin it if it is; persists.
    fn toggle_pin(&mut self, class: &str, cx: &mut Context<Self>) {
        let before = self.cfg.pinned.len();
        self.cfg
            .pinned
            .retain(|p| !crate::apps::class_matches(p, class));
        if self.cfg.pinned.len() == before {
            self.cfg.pinned.push(class.to_string());
        }
        let _ = self.cfg.save();
        self.menu = None;
        self.refresh();
        if self.mode != Mode::Hidden {
            self.place(true); // the dock changed the panel height
        }
        cx.notify();
    }

    fn is_pinned(&self, class: &str) -> bool {
        self.cfg
            .pinned
            .iter()
            .any(|p| crate::apps::class_matches(p, class))
    }

    /// Pin/unpin a single window (by address) to the top "Pinned" group.
    fn toggle_pin_window(&mut self, address: &str, cx: &mut Context<Self>) {
        let before = self.pinned_windows.len();
        self.pinned_windows.retain(|a| a != address);
        if self.pinned_windows.len() == before {
            self.pinned_windows.push(address.to_string());
        }
        self.menu = None;
        self.refresh();
        if self.mode != Mode::Hidden {
            self.place(true);
        }
        cx.notify();
    }

    fn is_pinned_window(&self, address: &str) -> bool {
        self.pinned_windows.iter().any(|a| a == address)
    }

    /// True while a cycling session should present as a centered overlay
    /// (macOS Cmd-Tab style) instead of the docked sidebar.
    fn centered(&self) -> bool {
        self.mode == Mode::Cycling
    }

    /// (revealed_x, hidden_x, y, w, h) in Hyprland logical layout coords.
    /// Height always fits the full content (clamped only by the monitor).
    fn geometry(&self, mon: &ctl::Monitor) -> (i64, i64, i64, i64, i64) {
        let (mw, mh) = mon.logical_size();
        // during a width drag the real window parks at WIDTH_MAX once and
        // only the content card resizes — per-event window resizes flicker
        let w = if self.width_preview.is_some() {
            crate::config::WIDTH_MAX as f64
        } else {
            self.cfg.width as f64
        };
        let h = (self.content_height() as f64 + 4.0).min(mh - 16.0);
        // Park entirely offscreen — hover detection is cursor-polling based,
        // so no visible sliver is needed (hover_strip_px is only the width
        // of the invisible trigger zone at the edge).
        let strip = -8.0;
        let (x_shown, x_hidden) = if self.cfg.edge.is_left() {
            (mon.x as f64, mon.x as f64 - w + strip)
        } else {
            (
                mon.x as f64 + mw - w,
                mon.x as f64 + mw - strip,
            )
        };
        let (x_shown, y) = if self.centered() {
            (
                mon.x as f64 + (mw - w) / 2.0,
                mon.y as f64 + (mh - h) / 2.0,
            )
        } else {
            // slide along the edge: 0 = top, 1 = bottom
            let frac = self.cfg.v_frac() as f64;
            (x_shown, mon.y as f64 + (mh - h).max(0.0) * frac)
        };
        (x_shown as i64, x_hidden as i64, y as i64, w as i64, h as i64)
    }

    fn place(&mut self, revealed: bool) {
        let (Some(addr), Some(mon)) = (self.address.clone(), self.monitor()) else {
            return;
        };
        let (x_shown, x_hidden, y, w, h) = self.geometry(&mon);
        let x = if revealed { x_shown } else { x_hidden };
        let _ = ctl::batch(&[
            format!("dispatch resizewindowpixel exact {w} {h},address:{addr}"),
            format!("dispatch movewindowpixel exact {x} {y},address:{addr}"),
        ]);
    }

    fn park(&mut self) {
        self.place(false);
    }

    // ---- presence transitions ----

    fn reveal_now(&mut self, cx: &mut Context<Self>) {
        if self.dirty {
            self.refresh();
        }
        self.palette = self.cfg.theme.palette();
        self.place(true);
        // raising marks the panel allowed-over-fullscreen, so it shows
        // above fullscreen windows too
        if self.fullscreen_active {
            if let Some(addr) = &self.address {
                let _ = ctl::raise_window(addr);
            }
        }
        // Visible panels normally drop no_focus so they receive pointer
        // input (hover + clicks). Over a fullscreen window the tag must
        // stay: follow_mouse would focus the panel and Hyprland transfers
        // fullscreen to newly focused windows — the panel would go
        // fullscreen. View-only there; keyboard switching still works.
        self.set_nofocus(self.fullscreen_active);
        cx.notify();
    }

    fn end_interaction(&mut self) {
        self.menu = None;
        self.scope = Scope::All;
        self.hover_originated = false;
        self.outside_polls = 0;
        self.hover_armed = false; // require leaving the zone before re-reveal
        self.query.clear();
        self.filtered.clear();
        self.reveal_gen += 1; // cancel pending reveals
        self.hide_gen += 1;
        self.dirty = true;
        // follow_mouse may have focused the panel while it was visible;
        // hand focus back if we still hold it
        if let (Some(active), Some(own)) = (ctl::active_address(), self.address.as_ref()) {
            if &active == own {
                let _ = ctl::focus_current_or_last();
            }
        }
    }

    /// Park off-screen (explicit hide, or resting state in hover mode).
    fn hide_now(&mut self, cx: &mut Context<Self>) {
        self.set_nofocus(true);
        self.mode = Mode::Hidden;
        self.end_interaction();
        self.park();
        cx.notify();
    }

    /// Return to the resting state after any interaction. In hover mode
    /// that parks nearly offscreen; in always-visible mode the same park
    /// leaves the compact sidebar (icons + workspace headers) showing.
    fn rest(&mut self, cx: &mut Context<Self>) {
        self.hide_now(cx);
    }

    /// Cursor watcher, since gpui cannot always see enter/leave here:
    /// while hidden it waits for the cursor to dwell on the edge strip,
    /// and while hover-revealed it hides once the cursor has left.
    fn poll_edge(&mut self, cx: &mut Context<Self>) {
        if self.address.is_none() {
            return;
        }
        match self.mode {
            Mode::Hidden | Mode::HoverPending => {
                let (Some((cur_x, cur_y)), Some(mon)) = (ctl::cursor_pos(), self.monitor())
                else {
                    return;
                };
                let (_, _, y, _, h) = self.geometry(&mon);
                let zone = (self.cfg.hover_strip_px as f64).max(6.0);
                let (mw, _) = mon.logical_size();
                let in_strip = cur_y >= y as f64
                    && cur_y <= (y + h) as f64
                    && if self.cfg.edge.is_left() {
                        cur_x <= mon.x as f64 + zone
                    } else {
                        cur_x >= mon.x as f64 + mw - zone
                    };
                if !self.hover_armed {
                    if !in_strip {
                        self.hover_armed = true;
                    }
                    return;
                }
                if self.mode == Mode::Hidden && in_strip {
                    self.mode = Mode::HoverPending;
                    self.schedule_reveal(self.cfg.show_delay_ms, cx);
                } else if self.mode == Mode::HoverPending && !in_strip {
                    self.mode = Mode::Hidden;
                    self.reveal_gen += 1;
                }
            }
            Mode::Revealed if self.hover_originated => {
                if self.cursor_inside_panel() {
                    self.outside_polls = 0;
                } else {
                    self.outside_polls = self.outside_polls.saturating_add(1);
                    let outside_ms = self.outside_polls as u64 * 180;
                    if outside_ms >= self.cfg.hide_delay_ms.max(200) {
                        self.hide_now(cx);
                    }
                }
            }
            _ => {}
        }
    }

    /// True if the cursor is currently inside the revealed panel rect.
    fn cursor_inside_panel(&self) -> bool {
        let (Some((cur_x, cur_y)), Some(mon)) = (ctl::cursor_pos(), self.monitor()) else {
            return false;
        };
        let (x, _, y, w, h) = self.geometry(&mon);
        cur_x >= x as f64
            && cur_x <= (x + w) as f64
            && cur_y >= y as f64
            && cur_y <= (y + h) as f64
    }

    fn schedule_reveal(&mut self, delay_ms: u64, cx: &mut Context<Self>) {
        self.reveal_gen += 1;
        let generation = self.reveal_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay_ms))
                .await;
            this.update(cx, |this, cx| {
                if this.reveal_gen == generation
                    && matches!(this.mode, Mode::HoverPending | Mode::Cycling)
                {
                    if this.mode == Mode::HoverPending {
                        this.mode = Mode::Revealed;
                        this.hover_originated = true;
                        this.outside_polls = 0;
                    }
                    this.reveal_now(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn schedule_hide(&mut self, delay_ms: u64, cx: &mut Context<Self>) {
        self.hide_gen += 1;
        let generation = self.hide_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay_ms))
                .await;
            this.update(cx, |this, cx| {
                if this.hide_gen == generation
                    && this.mode == Mode::Revealed
                    && this.hover_originated
                {
                    // spurious leave events fire when the window moves under
                    // a stationary cursor; only hide if the cursor truly left
                    if this.cursor_inside_panel() {
                        this.schedule_hide(this.cfg.hide_delay_ms.max(150), cx);
                    } else {
                        this.hide_now(cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Focus a window. If a fullscreen sibling on its workspace would keep
    /// covering it, temporarily float it on top instead — floating BEFORE
    /// focusing avoids Hyprland transferring fullscreen to the new focus.
    /// Temp-floated windows are re-tiled when returning to the fullscreen
    /// window (or when fullscreen ends).
    fn switch_to_address(&mut self, address: &str) {
        let Some(t) = self.entries.iter().find(|e| e.address == address).cloned() else {
            let _ = ctl::focus_window(address);
            return;
        };
        let covered = !t.fullscreen
            && self.entries.iter().any(|e| {
                e.fullscreen && e.workspace_id == t.workspace_id && e.address != t.address
            });
        if covered {
            if !t.floating {
                // remember the tiled geometry so the layout can be restored
                let orig = ctl::clients().ok().and_then(|cs| {
                    cs.into_iter()
                        .find(|c| c.address == address)
                        .map(|c| (c.at, c.size))
                });
                // float it at its own tiled geometry: the window lifts in
                // place, and the size stays deterministic across repeated
                // hops (Hyprland's own float sizing shrinks a little on
                // every setfloating round trip)
                let mut cmds = vec![format!("dispatch setfloating address:{address}")];
                if let Some((at, size)) = orig {
                    cmds.push(format!(
                        "dispatch resizewindowpixel exact {} {},address:{address}",
                        size[0], size[1]
                    ));
                    cmds.push(format!(
                        "dispatch movewindowpixel exact {} {},address:{address}",
                        at[0], at[1]
                    ));
                }
                let _ = ctl::batch(&cmds);
                self.temp_floated.push((address.to_string(), orig));
            }
            let _ = ctl::focus_window(address);
            let _ = ctl::raise_window(address);
        } else {
            // NOTE: temp-floated windows stay floating (hidden under the
            // fullscreen app) until fullscreen actually ends — re-tiling
            // on every hop nests them deeper into the layout each time.
            let _ = ctl::focus_window(address);
        }
    }

    /// Re-tile windows we floated to lift them over a fullscreen sibling.
    /// Each is first moved back to its original tiled spot so the layout
    /// re-inserts it where it came from.
    fn restore_temp_floated(&mut self) {
        for (addr, orig) in std::mem::take(&mut self.temp_floated) {
            if !self.entries.iter().any(|e| e.address == addr) {
                continue; // window closed meanwhile
            }
            let mut cmds = Vec::new();
            if let Some((at, size)) = orig {
                cmds.push(format!(
                    "dispatch resizewindowpixel exact {} {},address:{addr}",
                    size[0], size[1]
                ));
                cmds.push(format!(
                    "dispatch movewindowpixel exact {} {},address:{addr}",
                    at[0], at[1]
                ));
            }
            cmds.push(format!("dispatch settiled address:{addr}"));
            let _ = ctl::batch(&cmds);
        }
    }

    fn focus_selected_and_hide(&mut self, cx: &mut Context<Self>) {
        if let Some(address) = self.selected_entry().map(|e| e.address.clone()) {
            self.switch_to_address(&address);
        }
        self.rest(cx);
    }

    fn step(&mut self, delta: i64, cx: &mut Context<Self>) {
        let len = self.visible_order().len();
        if len == 0 {
            return;
        }
        self.selected = ((self.selected as i64 + delta).rem_euclid(len as i64)) as usize;
        cx.notify();
    }

    // ---- external messages ----

    pub fn handle_msg(&mut self, msg: Msg, window: &mut Window, cx: &mut Context<Self>) {
        match msg {
            Msg::Cmd(cmd) => self.handle_cmd(&cmd, window, cx),
            Msg::Event(ev) => self.handle_event(ev, cx),
        }
    }

    fn handle_cmd(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            "next" | "prev" | "next-ws" | "prev-ws" => {
                let delta: i64 = if cmd.starts_with("next") { 1 } else { -1 };
                // each shortcut's scope is user-configurable (Shortcuts
                // section in settings); Disabled swallows the command
                let behavior = if cmd.ends_with("-ws") {
                    self.cfg.super_tab
                } else {
                    self.cfg.alt_tab
                };
                let scope = match behavior {
                    CycleScope::AllWorkspaces => Scope::All,
                    CycleScope::CurrentWorkspace => Scope::Workspace,
                    CycleScope::Disabled => {
                        // hand the key back to the compositor's stock
                        // behavior: plain window cycling for Alt+Tab,
                        // workspace switching for Super+Tab (the Omarchy
                        // defaults these binds replaced)
                        if cmd.ends_with("-ws") {
                            let _ = ctl::dispatch(if delta > 0 {
                                "workspace e+1"
                            } else {
                                "workspace e-1"
                            });
                        } else {
                            let _ = ctl::dispatch(if delta > 0 {
                                "cyclenext"
                            } else {
                                "cyclenext prev"
                            });
                            let _ = ctl::dispatch("bringactivetotop");
                        }
                        return;
                    }
                };
                if self.mode == Mode::Search
                    || (self.mode == Mode::Cycling && self.scope == scope)
                {
                    self.step(delta, cx);
                } else {
                    self.scope = scope;
                    self.refresh();
                    self.mode = Mode::Cycling;
                    self.selected = if delta > 0 {
                        self.mru_position()
                    } else {
                        self.order.len().saturating_sub(1)
                    };
                    self.schedule_reveal(self.cfg.show_delay_ms, cx);
                }
            }
            "commit" => {
                if self.mode == Mode::Cycling {
                    self.focus_selected_and_hide(cx);
                }
            }
            "toggle" => {
                if self.mode == Mode::Hidden {
                    self.handle_cmd("show", window, cx);
                } else {
                    self.hide_now(cx);
                }
            }
            "show" => {
                self.scope = Scope::All;
                self.refresh();
                self.mode = Mode::Revealed;
                self.selected = self.mru_position();
                self.reveal_now(cx);
            }
            "hide" => self.hide_now(cx),
            "search" => {
                self.scope = Scope::All;
                self.refresh();
                self.mode = Mode::Search;
                self.query.clear();
                self.selected = self.mru_position();
                self.reveal_now(cx); // also lifts the nofocus tag
                if let Some(addr) = &self.address {
                    let _ = ctl::focus_window(addr);
                }
                window.focus(&self.focus_handle);
            }
            _ => {}
        }
    }

    fn handle_event(&mut self, ev: HyprEvent, cx: &mut Context<Self>) {
        match ev {
            HyprEvent::ConfigReloaded => {
                let _ = ctl::apply_panel_rules();
                if self.mode == Mode::Hidden {
                    self.set_nofocus(true);
                    self.park();
                } else {
                    self.place(true);
                }
            }
            HyprEvent::WindowsChanged => match self.mode {
Mode::Hidden | Mode::HoverPending => self.dirty = true,
                Mode::Revealed | Mode::Search => {
                    let keep = self.selected_entry().map(|e| e.address.clone());
                    self.refresh();
                    if let Some(addr) = keep {
                        if let Some(pos) = self
                            .visible_order()
                            .iter()
                            .position(|&i| self.entries[i].address == addr)
                        {
                            self.selected = pos;
                        }
                    }
                    self.place(true);
                    cx.notify();
                }
                Mode::Cycling => {}
            },
            HyprEvent::ActiveWindowChanged => {
                self.fullscreen_active = ctl::active_window_fullscreen();
                if self.mode == Mode::Hidden {
                    self.dirty = true;
                    self.park();
                }
            }
            HyprEvent::FullscreenChanged(active) => {
                self.fullscreen_active = active;
                if !active {
                    // fullscreen ended: put temporarily floated windows back
                    self.restore_temp_floated();
                }
                if self.mode == Mode::Hidden {
                    self.park();
                }
            }
            HyprEvent::WindowTitle { address } => {
                if let Some(e) = self.entries.iter_mut().find(|e| e.address == address) {
                    if let Ok(clients) = ctl::clients() {
                        if let Some(c) = clients.into_iter().find(|c| c.address == address) {
                            e.title = c.title;
                        }
                    }
                    if self.mode != Mode::Hidden {
                        cx.notify();
                    }
                } else {
                    self.dirty = true;
                }
            }
        }
    }

    // ---- input ----

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != Mode::Search {
            return;
        }
        let key = ev.keystroke.key.as_str();
        match key {
            "escape" => self.rest(cx),
            "enter" => self.focus_selected_and_hide(cx),
            "down" => self.step(1, cx),
            "up" => self.step(-1, cx),
            "tab" => {
                if ev.keystroke.modifiers.shift {
                    self.step(-1, cx)
                } else {
                    self.step(1, cx)
                }
            }
            "backspace" => {
                self.query.pop();
                self.apply_query(cx);
            }
            _ => {
                if let Some(ch) = ev
                    .keystroke
                    .key_char
                    .as_ref()
                    .filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
                {
                    if self.query.is_empty() {
                        if let Some(d) = ch.chars().next().and_then(|c| c.to_digit(10)) {
                            if d >= 1 && (d as usize) <= self.order.len().min(9) {
                                self.selected = d as usize - 1;
                                self.focus_selected_and_hide(cx);
                                return;
                            }
                        }
                    }
                    self.query.push_str(ch);
                    self.apply_query(cx);
                }
            }
        }
        let _ = window;
    }

    fn apply_query(&mut self, cx: &mut Context<Self>) {
        self.filtered = if self.query.is_empty() {
            Vec::new()
        } else {
            windows::filter(&self.entries, &self.query)
        };
        self.selected = 0;
        cx.notify();
    }

    fn on_hover_change(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            self.hide_gen += 1; // cancel pending hides
            if self.mode == Mode::Hidden && self.hover_armed {
                self.mode = Mode::HoverPending;
                self.schedule_reveal(self.cfg.show_delay_ms, cx);
            }
        } else {
            match self.mode {
                Mode::HoverPending => {
                    self.mode = Mode::Hidden;
                    self.reveal_gen += 1;
                }
                Mode::Revealed => self.schedule_hide(self.cfg.hide_delay_ms, cx),
                _ => {}
            }
        }
    }

    // ---- config (settings window will call this) ----

    /// Reveal the panel (unfocused) so settings can preview placement live.
    pub fn preview_reveal(&mut self, cx: &mut Context<Self>) {
        if matches!(self.mode, Mode::Hidden | Mode::HoverPending) {
            self.scope = Scope::All;
            self.refresh();
            self.mode = Mode::Revealed;
            self.hover_originated = false;
            self.selected = self.mru_position();
        }
        self.reveal_now(cx);
    }

    /// Live width-drag preview. The window is placed once at WIDTH_MAX;
    /// each event only re-renders the content card at the new width —
    /// resizing the real window per mouse move makes the client buffer
    /// race the compositor and flicker.
    pub fn preview_width(&mut self, w: f32, cx: &mut Context<Self>) {
        self.cfg.width = w;
        let first = self.width_preview.is_none();
        self.width_preview = Some(w);
        if first {
            self.preview_reveal(cx);
        }
        cx.notify();
    }

    /// Live position-drag preview: moves the already-revealed window
    /// without the full config-update park/reveal churn.
    pub fn preview_position(&mut self, cfg: Config, cx: &mut Context<Self>) {
        self.cfg = cfg;
        if matches!(self.mode, Mode::Hidden | Mode::HoverPending) {
            self.preview_reveal(cx);
        } else {
            self.place(true);
        }
        cx.notify();
    }

    pub fn preview_end(&mut self, cx: &mut Context<Self>) {
        self.width_preview = None;
        if self.mode == Mode::Revealed {
            self.rest(cx);
        } else {
            cx.notify();
        }
    }

    pub fn update_config(&mut self, cfg: Config, cx: &mut Context<Self>) {
        self.cfg = cfg;
        self.palette = self.cfg.theme.palette();
        match self.mode {
            // re-settle into the (possibly changed) resting state
            Mode::Hidden | Mode::Revealed => self.rest(cx),
            _ => self.place(true),
        }
        cx.notify();
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    // ---- render helpers ----

    fn letter_tile(&self, class: &str) -> gpui::Div {
        let name = windows::app_name(class);
        let letter = name.chars().next().unwrap_or('?').to_string();
        let hash: u32 = class
            .bytes()
            .fold(5381u32, |h, b| h.wrapping_mul(33).wrapping_add(b as u32));
        let hue = (hash % 360) as f32 / 360.0;
        div()
            .w(px(18.))
            .h(px(18.))
            .flex_none()
            .rounded(px(4.))
            .bg(gpui::hsla(hue, 0.55, 0.5, 1.0))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(11.))
            .text_color(gpui::white())
            .child(letter)
    }

    fn render_row(
        &self,
        position: usize,
        entry_ix: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let entry = &self.entries[entry_ix];
        let selected = position == self.selected;
        let p = self.palette;
        let title = if entry.title.is_empty() {
            windows::app_name(&entry.class)
        } else {
            entry.title.clone()
        };
        let icon = self.icons.resolve(&entry.class, &entry.initial_class);
        let address = entry.address.clone();
        let menu_class = entry.class.clone();
        let menu_address = entry.address.clone();
        let show_digit = position < 9 && !self.searching();

        div()
            .id(("row", entry_ix))
            .h(px(ROW_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .rounded(px(5.))
            .when(selected, |d| {
                d.bg(rgba(p.accent)).text_color(rgba(p.accent_text))
            })
            .child(match icon {
                Some(path) => img(path).w(px(18.)).h(px(18.)).flex_none().into_any_element(),
                None => self.letter_tile(&entry.class).into_any_element(),
            })
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(title),
            )
            .when(show_digit, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(if selected {
                            rgba(p.accent_text)
                        } else {
                            rgba(p.dim_text)
                        })
                        .child(format!("{}", position + 1)),
                )
            })
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if this.selected != position && this.mode != Mode::Cycling {
                    this.selected = position;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.switch_to_address(&address);
                    this.rest(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                    this.menu = Some(RowMenu {
                        class: menu_class.clone(),
                        address: Some(menu_address.clone()),
                        position: ev.position,
                    });
                    cx.notify();
                }),
            )
    }

    fn group_header(&self, label: String) -> gpui::Div {
        let p = self.palette;
        div()
            .h(px(GROUP_HEADER_H))
            .flex_none()
            .px(px(8.))
            .flex()
            .items_end()
            .pb(px(3.))
            .text_size(px(11.))
            .text_color(rgba(p.dim_text))
            .child(label)
    }

    /// A pinned app's dock row: every click launches the app (the app
    /// itself decides whether that means a new instance or surfacing the
    /// existing one). The accent dot marks apps with an open window.
    fn render_launcher_row(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let l = &self.launchers[ix];
        let p = self.palette;
        let icon = self.icons.resolve(&l.class, "");
        let class = l.class.clone();
        let menu_class = l.class.clone();
        let exec = l.exec.clone();
        let running = l.running;
        div()
            .id(("launch", ix))
            .h(px(ROW_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|d| d.bg(rgba(p.border)))
            .child(match icon {
                Some(path) => img(path).w(px(18.)).h(px(18.)).flex_none().into_any_element(),
                None => self.letter_tile(&class).into_any_element(),
            })
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(rgba(if running { p.text } else { p.dim_text }))
                    .child(l.name.clone()),
            )
            .when(running, |d| {
                d.child(
                    div()
                        .w(px(5.))
                        .h(px(5.))
                        .flex_none()
                        .rounded(px(2.5))
                        .bg(rgba(p.accent)),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if let Some(exec) = &exec {
                        let _ = ctl::dispatch(&format!("exec {exec}"));
                    } else if let Some(entry) = this.entries.iter().find(|e| {
                        crate::apps::class_matches(&class, &e.class)
                            || crate::apps::class_matches(&class, &e.initial_class)
                    }) {
                        // no desktop entry to launch from; at least focus
                        // the MRU window (entries are MRU-ordered)
                        let _ = ctl::focus_window(&entry.address);
                    }
                    this.rest(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                    this.menu = Some(RowMenu {
                        class: menu_class.clone(),
                        address: None,
                        position: ev.position,
                    });
                    cx.notify();
                }),
            )
    }
}

impl Render for Switcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let left = self.cfg.edge.is_left();
        let searching = self.searching();

        // no scrolling: the window is always sized to fit every row
        let mut list = div().flex_1().flex().flex_col().px(px(8.));

        if searching {
            if self.filtered.is_empty() {
                list = list.child(
                    div()
                        .h(px(ROW_H))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .text_color(rgba(p.dim_text))
                        .child("No matches"),
                );
            } else {
                let rows: Vec<usize> = self.filtered.clone();
                for (pos, entry_ix) in rows.into_iter().enumerate() {
                    list = list.child(self.render_row(pos, entry_ix, cx));
                }
            }
        } else if self.groups.is_empty() && !self.show_launchers() {
            list = list.child(
                div()
                    .h(px(ROW_H))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .text_color(rgba(p.dim_text))
                    .child("No windows"),
            );
        } else {
            let groups = self.groups.clone();
            let mut pos = 0;
            for g in groups {
                list = list.child(self.group_header(g.label.clone()));
                for entry_ix in g.rows {
                    list = list.child(self.render_row(pos, entry_ix, cx));
                    pos += 1;
                }
            }
            // the pinned-apps launcher dock sits below the window list
            if self.show_launchers() {
                list = list.child(self.group_header("Pinned Apps".to_string()));
                for ix in 0..self.launchers.len() {
                    list = list.child(self.render_launcher_row(ix, cx));
                }
            }
        }

        // right-click menu, floated over the list at the click position
        let menu_overlay = self.menu.clone().map(|m| {
            let menu_item = |id: &'static str,
                             label: &'static str,
                             on_click: Box<dyn Fn(&mut Self, &mut Context<Self>)>,
                             cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .h(px(24.))
                    .px(px(8.))
                    .rounded(px(5.))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(p.accent)).text_color(rgba(p.accent_text)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| on_click(this, cx)),
                    )
                    .child(label)
            };

            let item_count = if m.address.is_some() { 2 } else { 1 };
            let menu_h = 8.0 + item_count as f32 * 24.0;
            let x = f32::from(m.position.x).clamp(0.0, self.cfg.width - 150.0);
            let y = f32::from(m.position.y)
                .min(self.content_height() - menu_h - 8.0)
                .max(0.0);
            let mut menu = div()
                .id("rowmenu")
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(140.))
                .p(px(4.))
                .rounded(px(8.))
                .bg(rgba((p.background & 0xffffff00) | 0xff))
                .border_1()
                .border_color(rgba(p.border))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.menu = None;
                    cx.notify();
                }));

            if let Some(address) = m.address.clone() {
                let label = if self.is_pinned_window(&address) {
                    "Unpin Window"
                } else {
                    "Pin Window"
                };
                menu = menu.child(menu_item(
                    "rowmenu-pin-window",
                    label,
                    Box::new(move |this, cx| this.toggle_pin_window(&address, cx)),
                    cx,
                ));
            }
            let app_label = if self.is_pinned(&m.class) {
                "Unpin App"
            } else {
                "Pin App"
            };
            let class = m.class.clone();
            menu.child(menu_item(
                "rowmenu-pin-app",
                app_label,
                Box::new(move |this, cx| this.toggle_pin(&class, cx)),
                cx,
            ))
        });

        let card = div()
            .id("root")
            .relative()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                this.on_key(ev, window, cx)
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.on_hover_change(*hovered, cx)
            }))
            .h_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(rgba(p.background))
            .border_1()
            .border_color(rgba(p.border))
            .when(self.centered(), |d| d.rounded(px(10.)))
            .when(!self.centered() && left, |d| d.rounded_r(px(10.)))
            .when(!self.centered() && !left, |d| d.rounded_l(px(10.)))
            .pt(px(PAD_V))
            .pb(px(PAD_V))
            .text_size(px(13.))
            .text_color(rgba(p.text))
            .font_family(self.cfg.font.clone().unwrap_or_else(|| "Liberation Sans".into()))
            .child(
                div()
                    .h(px(PANEL_HEADER_H))
                    .flex_none()
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(rgba(p.dim_text))
                    .child(div().flex_1().child("Apps"))
                    // in search mode the typed query lives quietly in the
                    // header instead of a dedicated input box
                    .when(self.mode == Mode::Search, |d| {
                        d.child(div().flex_none().text_color(rgba(p.text)).child(
                            if self.query.is_empty() {
                                "type to filter".to_string()
                            } else {
                                format!("{}▏", self.query)
                            },
                        ))
                    })
                    .when(self.mode != Mode::Search, |d| {
                        d.child(
                            div()
                                .id("gear")
                                .flex_none()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_, _, _, _| {
                                        // routed through the control socket so the
                                        // daemon pump owns the settings window
                                        let _ = crate::client::send("settings");
                                    }),
                                )
                                .child(
                                    gpui::svg()
                                        .path("icons/settings.svg")
                                        .w(px(13.))
                                        .h(px(13.))
                                        .text_color(rgba(p.dim_text)),
                                ),
                        )
                    }),
            )
            .child(list)
            .children(menu_overlay);
        // during a width drag the card renders at the dragged width inside
        // the max-width window, anchored to the docked edge
        match self.width_preview {
            Some(w) => div()
                .size_full()
                .flex()
                .when(!left, |d| d.justify_end())
                .child(card.w(px(w)))
                .into_any_element(),
            None => card.w_full().into_any_element(),
        }
    }
}
