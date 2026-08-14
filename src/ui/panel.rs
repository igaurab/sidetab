//! The switcher panel: a Contexts-style sidebar. Owns the presence state
//! machine — every reveal/hide/focus transition funnels through here.

use crate::config::{Config, Palette, SidebarMode};
use crate::daemon::Msg;
use crate::hypr::ctl;
use crate::hypr::events::HyprEvent;
use crate::icons::IconResolver;
use crate::windows::{self, Group, WinEntry};
use gpui::{
    div, ease_in_out, img, prelude::*, px, rgba, Animation, AnimationExt as _, Context,
    FocusHandle, KeyDownEvent, MouseButton, Window,
};
use std::time::Duration;

pub const NOFOCUS_TAG: &str = "sidetab-nofocus";

/// Visible width of the collapsed always-visible sidebar: enough for the
/// row icons and the first few characters, like Contexts' collapsed state.
const COMPACT_PX: f64 = 76.0;
/// Duration of the client-side expand/collapse tween.
const CARD_ANIM_MS: u64 = 160;

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

pub struct Switcher {
    cfg: Config,
    palette: Palette,
    entries: Vec<WinEntry>,
    groups: Vec<Group>,
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
    fullscreen_active: bool,
    dirty: bool,
    /// active content-card width tween (from, to)
    card_anim: Option<(f32, f32)>,
    anim_gen: u64,
    /// the real window is currently at compact width
    compact_window: bool,
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
            order: Vec::new(),
            filtered: Vec::new(),
            query: String::new(),
            selected: 0,
            mode: Mode::Hidden,
            scope: Scope::All,
            address: None,
            fullscreen_active: ctl::active_window_fullscreen(),
            dirty: true,
            card_anim: None,
            anim_gen: 0,
            compact_window: false,
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

    fn always_visible(&self) -> bool {
        self.cfg.sidebar == SidebarMode::AlwaysVisible
    }

    fn set_nofocus(&self, on: bool) {
        if let Some(addr) = &self.address {
            let sign = if on { '+' } else { '-' };
            let _ = ctl::dispatch(&format!("tagwindow {sign}{NOFOCUS_TAG} address:{addr}"));
        }
    }

    /// Kick off a client-side width tween of the content card. Hyprland
    /// never animates our window (that leaves black residue where the
    /// buffer doesn't cover the animated frame); instead gpui animates the
    /// card and the real window is resized instantly at the ends.
    fn start_card_anim(&mut self, from: f32, to: f32, resize_after: bool, cx: &mut Context<Self>) {
        self.anim_gen += 1;
        let generation = self.anim_gen;
        self.card_anim = Some((from, to));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(CARD_ANIM_MS + 30))
                .await;
            this.update(cx, |this, cx| {
                if this.anim_gen == generation {
                    this.card_anim = None;
                    if resize_after {
                        this.place(false);
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- data ----

    fn refresh(&mut self) {
        self.entries = windows::fetch();
        if self.scope == Scope::Workspace {
            if let Ok(mon) = ctl::focused_monitor() {
                self.entries
                    .retain(|e| e.workspace_id == mon.active_workspace.id);
            }
        }
        self.groups = windows::group(&self.entries, &self.cfg.pinned);
        self.order = windows::display_order(&self.groups);
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
            if self.groups.is_empty() {
                h += ROW_H;
            }
        }
        h
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
        let w = self.cfg.width as f64;
        let h = (self.content_height() as f64 + 4.0).min(mh - 16.0);
        let strip = if self.fullscreen_active {
            -8.0 // park fully offscreen
        } else {
            self.cfg.hover_strip_px as f64
        };
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
        // resting state in always-visible mode: a narrow panel at the edge
        // showing icons + a few characters (Contexts' collapsed sidebar),
        // resized rather than slid so the icon column stays visible
        let (x, w, compact) = if revealed {
            (x_shown, w, false)
        } else if self.always_visible() && !self.fullscreen_active {
            let (mw, _) = mon.logical_size();
            let cw = COMPACT_PX as i64;
            let x = if self.cfg.edge.is_left() {
                mon.x
            } else {
                mon.x + mw as i64 - cw
            };
            (x, cw, true)
        } else {
            (x_hidden, w, false)
        };
        self.compact_window = compact;
        let _ = ctl::batch(&[
            format!("dispatch resizewindowpixel exact {w} {h},address:{addr}"),
            format!("dispatch movewindowpixel exact {x} {y},address:{addr}"),
        ]);
    }

    /// True while resting as the narrow always-visible sidebar. During the
    /// collapse tween this stays false so the content keeps its full
    /// styling and is clipped smoothly instead of reflowing mid-animation.
    fn compact(&self) -> bool {
        self.always_visible()
            && matches!(self.mode, Mode::Hidden | Mode::HoverPending)
            && self.card_anim.is_none()
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
        // expanding the compact sidebar tweens the card; the centered
        // cycling overlay pops instantly
        let animate = self.compact_window && !self.centered();
        self.place(true);
        if animate {
            self.start_card_anim(COMPACT_PX as f32, self.cfg.width, false, cx);
        }
        // visible panels must receive pointer input (hover + clicks),
        // which Hyprland withholds from no_focus windows
        self.set_nofocus(false);
        cx.notify();
    }

    fn end_interaction(&mut self) {
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
        if self.always_visible() && !self.fullscreen_active {
            let was_expanded = !self.compact_window;
            self.set_nofocus(true);
            self.mode = Mode::Hidden;
            self.end_interaction();
            self.refresh();
            self.selected = self.mru_position();
            if was_expanded {
                // tween the card down first; the window shrinks afterwards
                self.start_card_anim(self.cfg.width, COMPACT_PX as f32, true, cx);
            } else {
                self.place(false);
            }
            cx.notify();
        } else {
            self.hide_now(cx);
        }
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
                if self.fullscreen_active {
                    return;
                }
                let (Some((cur_x, cur_y)), Some(mon)) = (ctl::cursor_pos(), self.monitor())
                else {
                    return;
                };
                let (_, _, y, _, h) = self.geometry(&mon);
                let zone = if self.always_visible() {
                    COMPACT_PX + 2.0
                } else {
                    (self.cfg.hover_strip_px as f64).max(6.0)
                };
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

    /// In always-visible mode the parked panel's compact strip is on
    /// screen, so its list must stay current even while "hidden".
    fn refresh_compact(&mut self, cx: &mut Context<Self>) {
        if self.always_visible() && self.mode == Mode::Hidden && self.card_anim.is_none() {
            self.refresh();
            self.selected = self.mru_position();
            self.place(false);
            cx.notify();
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

    fn focus_selected_and_hide(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.selected_entry() {
            let _ = ctl::focus_window(&entry.address);
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
                let scope = if cmd.ends_with("-ws") {
                    Scope::Workspace
                } else {
                    Scope::All
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
                Mode::Hidden | Mode::HoverPending => {
                    self.dirty = true;
                    self.refresh_compact(cx);
                }
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
                    self.refresh_compact(cx);
                }
            }
            HyprEvent::FullscreenChanged(active) => {
                self.fullscreen_active = active;
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
            if self.mode == Mode::Hidden && !self.fullscreen_active && self.hover_armed {
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

    pub fn preview_end(&mut self, cx: &mut Context<Self>) {
        if self.mode == Mode::Revealed {
            self.rest(cx);
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

    fn letter_tile(&self, entry: &WinEntry) -> gpui::Div {
        let name = windows::app_name(&entry.class);
        let letter = name.chars().next().unwrap_or('?').to_string();
        let hash: u32 = entry
            .class
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
        let show_digit = position < 9 && !self.searching() && !self.compact();

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
                None => self.letter_tile(entry).into_any_element(),
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
                    let _ = ctl::focus_window(&address);
                    this.rest(cx);
                }),
            )
    }
}

impl Render for Switcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let left = self.cfg.edge.is_left();
        let searching = self.searching();
        let compact = self.compact();

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
        } else if self.groups.is_empty() {
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
                list = list.child(
                    div()
                        .h(px(GROUP_HEADER_H))
                        .flex_none()
                        .px(px(8.))
                        .flex()
                        .items_end()
                        .pb(px(3.))
                        .text_size(px(11.))
                        .text_color(rgba(p.dim_text))
                        .child(if compact {
                            windows::short_group_label(&g.label)
                        } else {
                            g.label.clone()
                        }),
                );
                for entry_ix in g.rows {
                    list = list.child(self.render_row(pos, entry_ix, cx));
                    pos += 1;
                }
            }
        }

        let card = div()
            .id("root")
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
                    .when(self.mode != Mode::Search && !self.compact(), |d| {
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
            .child(list);

        // The card tweens its width during compact<->full transitions while
        // the real window is resized instantly at the endpoints — Hyprland
        // animating our window leaves black residue, gpui doesn't.
        let outer = div()
            .size_full()
            .flex()
            .when(!left, |d| d.justify_end());
        match self.card_anim {
            Some((from, to)) => outer
                .child(card.with_animation(
                    ("card-anim", self.anim_gen as usize),
                    Animation::new(Duration::from_millis(CARD_ANIM_MS)).with_easing(ease_in_out),
                    move |card, t| card.w(px(from + (to - from) * t)),
                ))
                .into_any_element(),
            None => outer.child(card.w_full()).into_any_element(),
        }
    }
}
