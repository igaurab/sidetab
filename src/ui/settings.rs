//! Settings window, laid out like the macOS Contexts preferences: a
//! navigation sidebar on the left, the selected section's controls on the
//! right. Lives in the daemon; every change applies to the panel
//! immediately and persists to ~/.config/sidetab/config.toml.

use crate::apps::{class_matches, DesktopApp};
use crate::config::{
    mix, Config, CycleScope, Edge, ThemeVariant, OVERLAY_WIDTH_MAX, OVERLAY_WIDTH_MIN, WIDTH_MAX,
    WIDTH_MIN,
};
use crate::icons::IconResolver;
use crate::ui::panel::Switcher;
use gpui::{
    canvas, div, img, prelude::*, px, rgba, svg, Animation, AnimationExt, Bounds, Context,
    Entity, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Window,
};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Panel,
    Shortcuts,
    Appearance,
    PinnedApps,
    About,
}

const SECTIONS: [(Section, &str, &str); 5] = [
    (Section::Panel, "icons/panel.svg", "Panel"),
    (Section::Shortcuts, "icons/keys.svg", "Window Switching"),
    (Section::Appearance, "icons/appearance.svg", "Appearance"),
    (Section::PinnedApps, "icons/pin.svg", "Apps"),
    (Section::About, "icons/info.svg", "About"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    Slider,
    Preview,
    Width,
    OverlayWidth,
}


pub struct Settings {
    cfg: Config,
    panel: Entity<Switcher>,
    active: Section,
    /// every installed application (from .desktop entries), sorted by name
    apps: Vec<DesktopApp>,
    /// filter typed into the pinned-apps search bar
    query: String,
    icons: IconResolver,
    focus_handle: FocusHandle,
    dragging: Option<Drag>,
    track_bounds: Option<Bounds<Pixels>>,
    preview_bounds: Option<Bounds<Pixels>>,
    width_track_bounds: Option<Bounds<Pixels>>,
    overlay_track_bounds: Option<Bounds<Pixels>>,
    /// Result of the last "Install shortcuts" click, shown under the button.
    bindings_status: Option<String>,
}

impl Settings {
    pub fn new(cfg: Config, panel: Entity<Switcher>, cx: &mut Context<Self>) -> Self {
        Settings {
            cfg,
            panel,
            active: Section::Panel,
            apps: crate::apps::installed(),
            query: String::new(),
            icons: IconResolver::new(),
            focus_handle: cx.focus_handle(),
            dragging: None,
            track_bounds: None,
            preview_bounds: None,
            width_track_bounds: None,
            overlay_track_bounds: None,
            bindings_status: None,
        }
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let _ = self.cfg.save();
        self.apply_live(cx);
    }

    /// Push to the panel without persisting (used mid-drag).
    fn apply_live(&mut self, cx: &mut Context<Self>) {
        let cfg = self.cfg.clone();
        self.panel.update(cx, |panel, cx| panel.update_config(cfg, cx));
        cx.notify();
    }

    fn mutate(&mut self, f: impl FnOnce(&mut Config), cx: &mut Context<Self>) {
        f(&mut self.cfg);
        self.apply(cx);
    }

    // ---- position fine-tuning drag ----

    fn drag_update(&mut self, pos: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        match self.dragging {
            Some(Drag::Slider) => {
                let Some(b) = self.track_bounds else { return };
                let usable = (f32::from(b.size.width) - 16.0).max(1.0);
                let frac = ((f32::from(pos.x) - f32::from(b.origin.x) - 8.0) / usable).clamp(0.0, 1.0);
                self.cfg.v_pos = frac;
            }
            Some(Drag::Preview) => {
                let Some(b) = self.preview_bounds else { return };
                let frac = ((f32::from(pos.y) - f32::from(b.origin.y)) / f32::from(b.size.height).max(1.0)).clamp(0.0, 1.0);
                let left = f32::from(pos.x) < f32::from(b.origin.x) + f32::from(b.size.width) / 2.0;
                self.cfg.v_pos = frac;
                self.cfg.edge = if left { Edge::Left } else { Edge::Right };
            }
            Some(drag @ (Drag::Width | Drag::OverlayWidth)) => {
                let overlay = drag == Drag::OverlayWidth;
                let bounds = if overlay {
                    self.overlay_track_bounds
                } else {
                    self.width_track_bounds
                };
                let Some(b) = bounds else { return };
                let (min, max) = if overlay {
                    (OVERLAY_WIDTH_MIN, OVERLAY_WIDTH_MAX)
                } else {
                    (WIDTH_MIN, WIDTH_MAX)
                };
                let usable = (f32::from(b.size.width) - 16.0).max(1.0);
                let frac =
                    ((f32::from(pos.x) - f32::from(b.origin.x) - 8.0) / usable).clamp(0.0, 1.0);
                // snap to 10px steps within [min, max]
                let w = ((min + (max - min) * frac) / 10.0).round() * 10.0;
                if overlay {
                    self.cfg.overlay_width = w;
                } else {
                    self.cfg.width = w;
                }
                // light path: only the content card re-renders per event
                let cfg = self.cfg.clone();
                self.panel
                    .update(cx, |panel, cx| panel.preview_width(cfg, w, max, overlay, cx));
                cx.notify();
                return;
            }
            None => return,
        }
        // light path: move the revealed panel without park/reveal churn
        let cfg = self.cfg.clone();
        self.panel
            .update(cx, |panel, cx| panel.preview_position(cfg, cx));
        cx.notify();
    }

    fn drag_end(&mut self, cx: &mut Context<Self>) {
        if self.dragging.take().is_some() {
            let _ = self.cfg.save();
            self.panel.update(cx, |panel, cx| panel.preview_end(cx));
            cx.notify();
        }
    }
}

// palette for the settings window itself (opaque, follows panel theme)
struct Ui {
    bg: u32,
    nav_bg: u32,
    row: u32,
    text: u32,
    dim: u32,
    accent: u32,
    accent_text: u32,
}

fn ui(cfg: &Config) -> Ui {
    let p = cfg.theme.palette();
    let dark = p.dark;
    Ui {
        // tinted toward the panel's own background so the settings window
        // reads as part of the same theme (Omarchy colors included)
        bg: if dark {
            mix(0x1f2023ff, p.background, 0.5)
        } else {
            mix(0xf4f4f5ff, p.background, 0.5)
        },
        nav_bg: if dark {
            mix(0x18191bff, p.background, 0.35)
        } else {
            mix(0xe9e9ebff, p.background, 0.35)
        },
        row: if dark { 0xffffff10 } else { 0x00000010 },
        text: p.text,
        dim: p.dim_text,
        accent: p.accent,
        accent_text: p.accent_text,
    }
}

impl Settings {
    fn chip(
        &self,
        id: (&'static str, usize),
        label: String,
        active: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        u: &Ui,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .px(px(10.))
            .h(px(26.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(12.))
            .when(active, |d| d.bg(rgba(u.accent)).text_color(rgba(u.accent_text)))
            .when(!active, |d| d.bg(rgba(u.row)).text_color(rgba(u.text)))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| on_click(this, cx)),
            )
            .child(label)
    }

    fn stepper(
        &self,
        id: &'static str,
        value: String,
        dec: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        inc: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        u: &Ui,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let btn = |d: gpui::Stateful<gpui::Div>| {
            d.w(px(24.))
                .h(px(24.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
        };
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                btn(div().id((id, 0usize)).bg(rgba(u.row)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| dec(this, cx)),
                    )
                    .child("−"),
            )
            .child(
                div()
                    .w(px(64.))
                    .text_align(gpui::TextAlign::Center)
                    .child(value),
            )
            .child(
                btn(div().id((id, 1usize)).bg(rgba(u.row)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| inc(this, cx)),
                    )
                    .child("+"),
            )
    }

    /// A width slider. The readout is a plain-language size name rather than
    /// a pixel count — the exact number means little to anyone who isn't
    /// eyeballing it against the live preview anyway. Used for both the
    /// sidebar width and the Alt-Tab window width; `drag` picks which one
    /// the pointer is steering and which track bounds to record.
    fn width_slider(
        &self,
        id: &'static str,
        value: f32,
        min: f32,
        max: f32,
        drag: Drag,
        u: &Ui,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let frac = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let entity = cx.entity();
        div()
            .flex()
            .flex_col()
            .items_end()
            .gap(px(2.))
            .child(
                div()
                    .id(id)
                    .w(px(180.))
                    .h(px(20.))
                    .relative()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            this.dragging = Some(drag);
                            this.drag_update(ev.position, cx);
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(8.))
                            .left(px(8.))
                            .right(px(8.))
                            .h(px(4.))
                            .rounded(px(2.))
                            .bg(rgba(u.row)),
                    )
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                entity.update(cx, |this, _| match drag {
                                    Drag::OverlayWidth => {
                                        this.overlay_track_bounds = Some(bounds)
                                    }
                                    _ => this.width_track_bounds = Some(bounds),
                                })
                            },
                            |_, _, _, _| {},
                        )
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(3.))
                            .left(gpui::relative(frac))
                            .ml(px(-14.0 * frac))
                            .w(px(14.))
                            .h(px(14.))
                            .rounded(px(7.))
                            .bg(rgba(u.accent)),
                    ),
            )
            .child(
                // a fixed scale under the track, so the size reads off the
                // knob's position instead of a number nobody can picture
                div()
                    .w(px(180.))
                    .flex()
                    .justify_between()
                    .px(px(4.))
                    .text_size(px(10.))
                    .text_color(rgba(u.dim))
                    .child("Small")
                    .child("Medium")
                    .child("Large"),
            )
    }

    fn row(&self, label: &'static str, control: impl IntoElement, u: &Ui) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(42.))
            .child(div().text_color(rgba(u.text)).child(label))
            .child(control)
    }

    fn heading(&self, label: &'static str, u: &Ui) -> impl IntoElement {
        div()
            .pb(px(10.))
            .text_size(px(16.))
            .text_color(rgba(u.text))
            .child(label)
    }

    fn hint(&self, text: String, u: &Ui) -> impl IntoElement {
        div()
            .pt(px(4.))
            .text_size(px(11.))
            .text_color(rgba(u.dim))
            .child(text)
    }

    fn scope_chips(
        &self,
        id: &'static str,
        current: CycleScope,
        set: impl Fn(&mut Config, CycleScope) + Clone + 'static,
        u: &Ui,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        div().flex().gap(px(6.)).children(
            [
                (CycleScope::AllWorkspaces, "All workspaces"),
                (CycleScope::CurrentWorkspace, "Current workspace"),
                (CycleScope::Disabled, "Disabled"),
            ]
            .into_iter()
            .enumerate()
            .map(|(ix, (scope, label))| {
                let set = set.clone();
                self.chip(
                    (id, ix),
                    label.to_string(),
                    current == scope,
                    move |this, cx| this.mutate(|c| set(c, scope), cx),
                    u,
                    cx,
                )
            }),
        )
    }

    fn shortcuts_pane(&self, u: &Ui, cx: &mut Context<Self>) -> gpui::Div {
        let cfg = self.cfg.clone();
        div()
            .flex()
            .flex_col()
            .child(self.heading("Window Switching", u))
            .child(div().pb(px(6.)).text_color(rgba(u.text)).child("Alt + Tab cycles"))
            .child(self.scope_chips("alttab", cfg.alt_tab, |c, s| c.alt_tab = s, u, cx))
            .child(
                div()
                    .pt(px(14.))
                    .pb(px(6.))
                    .text_color(rgba(u.text))
                    .child("Super + Tab cycles"),
            )
            .child(self.scope_chips("supertab", cfg.super_tab, |c, s| c.super_tab = s, u, cx))
            .child(self.hint(
                "Choose what each shortcut cycles through. Disabled restores \
                 the stock behavior: Alt+Tab cycles windows the plain \
                 Hyprland way (no panel), Super+Tab switches to the next \
                 workspace."
                    .to_string(),
                u,
            ))
            .child(self.row(
                "Alt-Tab window size",
                self.width_slider(
                    "overlayslider",
                    cfg.overlay_width,
                    OVERLAY_WIDTH_MIN,
                    OVERLAY_WIDTH_MAX,
                    Drag::OverlayWidth,
                    u,
                    cx,
                ),
                u,
            ))
            .child(self.hint(
                "How wide the window both shortcuts pop up in the middle of \
                 the screen is. It's set separately from the sidebar, so it \
                 can stay roomy enough for long window titles even with a \
                 narrow sidebar."
                    .to_string(),
                u,
            ))
            .child(self.bindings_block(u, cx))
    }

    /// The one part of setup that isn't automatic: the shortcuts have to
    /// exist in the Hyprland config before any of the above does anything.
    fn bindings_block(&self, u: &Ui, cx: &mut Context<Self>) -> gpui::Div {
        let plan = crate::bindings::plan().ok();
        let installed = plan.as_ref().is_some_and(|p| p.installed());
        let (where_, style) = match &plan {
            Some(p) => (
                p.file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("your Hyprland config")
                    .to_string(),
                p.style.label(),
            ),
            None => ("your Hyprland config".to_string(), "Hyprland config"),
        };

        div()
            .flex()
            .flex_col()
            .pt(px(18.))
            .child(self.heading("Keyboard shortcuts", u))
            .child(self.row(
                "Alt+Tab and Super+Tab",
                // Installing is a one-time edit guarded by a marker, so once
                // it's in there is no second action to offer — just say so.
                if installed {
                    div()
                        .text_size(px(12.))
                        .text_color(rgba(u.dim))
                        .child("Installed")
                        .into_any_element()
                } else {
                    self.chip(
                    ("installbindings", 0),
                    "Install".to_string(),
                    true,
                    |this, cx| {
                        this.bindings_status = Some(match crate::bindings::install() {
                            Ok(crate::bindings::Outcome::AlreadyInstalled { .. }) => {
                                "Already installed — nothing to do.".to_string()
                            }
                            Ok(crate::bindings::Outcome::Installed {
                                file, reloaded, ..
                            }) => {
                                let name = file
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("your config")
                                    .to_string();
                                if reloaded {
                                    format!("Added to {name} and reloaded — try Alt+Tab.")
                                } else {
                                    format!("Added to {name}. Run 'hyprctl reload' to apply.")
                                }
                            }
                            Err(e) => format!("Couldn't install: {e}"),
                        });
                        cx.notify();
                    },
                    u,
                    cx,
                )
                .into_any_element()
                },
                u,
            ))
            .child(self.hint(
                match &self.bindings_status {
                    Some(msg) => msg.clone(),
                    None if installed => format!(
                        "The shortcuts are wired up in {where_}. To change or \
                         remove them, edit the sidetab block in that file."
                    ),
                    None => format!(
                        "Alt+Tab and Super+Tab do nothing until the shortcuts are \
                         in your Hyprland config. This adds them to {where_} \
                         (detected: {style}), backs the file up first, and starts \
                         the daemon on login."
                    ),
                },
                u,
            ))
    }

    fn delays_block(&self, u: &Ui, cx: &mut Context<Self>) -> gpui::Div {
        let cfg = self.cfg.clone();
        div()
            .flex()
            .flex_col()
            .pt(px(6.))
            .child(self.row(
                "Show delay",
                self.stepper(
                    "showdelay",
                    format!("{}ms", cfg.show_delay_ms),
                    |this, cx| {
                        this.mutate(|c| c.show_delay_ms = c.show_delay_ms.saturating_sub(30), cx)
                    },
                    |this, cx| this.mutate(|c| c.show_delay_ms = (c.show_delay_ms + 30).min(600), cx),
                    u,
                    cx,
                ),
                u,
            ))
            .child(self.row(
                "Hide delay",
                self.stepper(
                    "hidedelay",
                    format!("{}ms", cfg.hide_delay_ms),
                    |this, cx| {
                        this.mutate(|c| c.hide_delay_ms = c.hide_delay_ms.saturating_sub(50), cx)
                    },
                    |this, cx| {
                        this.mutate(|c| c.hide_delay_ms = (c.hide_delay_ms + 50).min(2000), cx)
                    },
                    u,
                    cx,
                ),
                u,
            ))
            .child(self.row(
                "Hover zone",
                self.stepper(
                    "hoverzone",
                    format!("{}px", cfg.hover_strip_px as i64),
                    |this, cx| {
                        this.mutate(|c| c.hover_strip_px = (c.hover_strip_px - 2.0).max(2.0), cx)
                    },
                    |this, cx| {
                        this.mutate(|c| c.hover_strip_px = (c.hover_strip_px + 2.0).min(24.0), cx)
                    },
                    u,
                    cx,
                ),
                u,
            ))
            .child(self.hint(
                "Show and hide delay control how quickly the sidebar reveals \
                 when hovered and hides after the cursor leaves. Hover zone \
                 is the width of the invisible strip at the screen edge that \
                 triggers the reveal — nothing is drawn there."
                    .to_string(),
                u,
            ))
    }

    fn panel_pane(&self, u: &Ui, cx: &mut Context<Self>) -> gpui::Div {
        let cfg = self.cfg.clone();
        let edge_row = div().flex().gap(px(6.)).children(
            [(Edge::Left, "Left"), (Edge::Right, "Right")]
                .into_iter()
                .enumerate()
                .map(|(ix, (edge, label))| {
                    self.chip(
                        ("edge", ix),
                        label.to_string(),
                        cfg.edge == edge,
                        move |this, cx| this.mutate(|c| c.edge = edge, cx),
                        u,
                        cx,
                    )
                }),
        );

        // -- fine-tune slider --
        let frac = cfg.v_frac();
        let entity = cx.entity();
        let track = div()
            .id("vslider")
            .flex_1()
            .h(px(20.))
            .relative()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.dragging = Some(Drag::Slider);
                    this.drag_update(ev.position, cx);
                }),
            )
            .child(
                div()
                    .absolute()
                    .top(px(8.))
                    .left(px(8.))
                    .right(px(8.))
                    .h(px(4.))
                    .rounded(px(2.))
                    .bg(rgba(u.row)),
            )
            .child(
                canvas(
                    // bounds are only needed to map drag positions
                    move |bounds, _, cx| {
                        entity.update(cx, |this, _| this.track_bounds = Some(bounds))
                    },
                    |_, _, _, _| {},
                )
                .size_full(),
            )
            .child(
                // percentage-positioned so no measured bounds are needed:
                // at 0% the knob hugs the left end, at 100% the right end
                div()
                    .absolute()
                    .top(px(3.))
                    .left(gpui::relative(frac))
                    .ml(px(-14.0 * frac))
                    .w(px(14.))
                    .h(px(14.))
                    .rounded(px(7.))
                    .bg(rgba(u.accent)),
            );

        // -- mini screen preview --
        let (pw, ph) = (176.0_f32, 99.0_f32);
        let bar_h = 30.0_f32;
        let bar_y = (ph - bar_h) * frac;
        let bar_x = if cfg.edge.is_left() { 3.0 } else { pw - 9.0 - 3.0 };
        let entity = cx.entity();
        let preview = div()
            .id("posprev")
            .w(px(pw))
            .h(px(ph))
            .flex_none()
            .relative()
            .rounded(px(6.))
            .bg(rgba(u.row))
            .border_1()
            .border_color(rgba(u.row))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.dragging = Some(Drag::Preview);
                    this.drag_update(ev.position, cx);
                }),
            )
            .child(
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |this, _| {
                            this.preview_bounds = Some(bounds);
                        })
                    },
                    |_, _, _, _| {},
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(bar_x))
                    .top(px(bar_y))
                    .w(px(9.))
                    .h(px(bar_h))
                    .rounded(px(2.))
                    .bg(rgba(u.accent)),
            );

        div()
            .flex()
            .flex_col()
            .child(self.heading("Panel", u))
            .child(self.row(
                "Sidebar size",
                self.width_slider(
                    "wslider",
                    cfg.width,
                    WIDTH_MIN,
                    WIDTH_MAX,
                    Drag::Width,
                    u,
                    cx,
                ),
                u,
            ))
            .child(div().pt(px(8.)).pb(px(4.)).text_color(rgba(u.text)).child("Edge"))
            .child(edge_row)
            .child(
                div()
                    .pt(px(12.))
                    .pb(px(4.))
                    .text_color(rgba(u.text))
                    .child("Placement"),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(14.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(div().flex().items_center().gap(px(8.)).child(track).child(
                                div().w(px(40.)).flex_none().text_size(px(11.)).text_color(rgba(u.dim)).child(
                                    format!("{}%", (frac * 100.0) as i64),
                                ),
                            ))
                            .child(self.hint(
                                "Drag the slider — or the panel inside the mini \
                                 screen — to place it anywhere along the edge. \
                                 The real panel previews live while you drag."
                                    .to_string(),
                                u,
                            )),
                    )
                    .child(preview),
            )
            .child(self.delays_block(u, cx))
    }

    fn appearance_pane(&self, u: &Ui, cx: &mut Context<Self>) -> gpui::Div {
        let cfg = self.cfg.clone();
        div()
            .flex()
            .flex_col()
            .child(self.heading("Appearance", u))
            .child(div().pb(px(6.)).text_color(rgba(u.text)).child("Theme"))
            .child(
                div().flex().gap(px(6.)).children(
                    [
                        (ThemeVariant::Omarchy, "Omarchy"),
                        (ThemeVariant::System, "System"),
                        (ThemeVariant::Light, "Light"),
                        (ThemeVariant::Dark, "Dark"),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(ix, (variant, label))| {
                        self.chip(
                            ("theme", ix),
                            label.to_string(),
                            cfg.theme.variant == variant,
                            move |this, cx| this.mutate(|c| c.theme.variant = variant, cx),
                            u,
                            cx,
                        )
                    }),
                ),
            )
            .child(self.hint(
                "Omarchy takes its colors from your current Omarchy theme and \
                 follows every theme switch (falling back to System when \
                 Omarchy isn't installed). System follows your desktop's \
                 light/dark preference. Colors can be overridden in the config \
                 file."
                    .to_string(),
                u,
            ))
    }

    /// Best icon for an installed app: its Icon= name, else its class.
    fn app_icon(&self, app: &DesktopApp) -> Option<PathBuf> {
        app.icon
            .as_deref()
            .and_then(|name| self.icons.resolve_name(name))
            .or_else(|| self.icons.resolve(&app.class, ""))
    }

    fn letter_tile(&self, name: &str, class: &str) -> gpui::Div {
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

    fn section_label(&self, label: &'static str, u: &Ui) -> impl IntoElement {
        div()
            .pt(px(4.))
            .pb(px(6.))
            .flex_none()
            .text_size(px(11.))
            .text_color(rgba(u.dim))
            .child(label)
    }

    /// One row in the pinned-apps section: icon, name, pin toggle.
    /// `disabled` greys out the pin button (pin limit reached).
    fn app_row(
        &self,
        id: (&'static str, usize),
        icon: Option<PathBuf>,
        name: String,
        class: String,
        pinned: bool,
        disabled: bool,
        u: &Ui,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tile = self.letter_tile(&name, &class);
        let pin_btn = div()
            .id((id.0, id.1 + 100_000))
            .w(px(24.))
            .h(px(24.))
            .flex_none()
            .rounded(px(5.))
            .flex()
            .items_center()
            .justify_center()
            .when(!disabled, |d| {
                d.cursor_pointer().hover(|d| d.bg(rgba(u.row))).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.mutate(
                            |c| {
                                let before = c.pinned.len();
                                c.pinned.retain(|p| !class_matches(p, &class));
                                if c.pinned.len() == before
                                    && before < crate::config::MAX_PINNED
                                {
                                    c.pinned.push(class.clone());
                                }
                            },
                            cx,
                        )
                    }),
                )
            })
            .when(disabled, |d| d.opacity(0.3))
            .child(
                svg()
                    .path("icons/pin.svg")
                    .w(px(13.))
                    .h(px(13.))
                    .text_color(rgba(if pinned { u.accent } else { u.dim })),
            );
        div()
            .id(id)
            .h(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .rounded(px(6.))
            .hover(|d| d.bg(rgba(u.row)))
            .child(match icon {
                Some(path) => img(path).w(px(18.)).h(px(18.)).flex_none().into_any_element(),
                None => tile.into_any_element(),
            })
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(rgba(u.text))
                    .child(name),
            )
            .child(pin_btn)
    }

    fn pinned_pane(&self, u: &Ui, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let cfg = self.cfg.clone();
        let q = self.query.to_lowercase();

        let mut pane = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .child(self.heading("Apps", u))
            .child(
                div()
                    .pb(px(6.))
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(rgba(u.dim))
                    .child(
                        "Pinned apps appear as icons in the panel header — click one \
                         to open the app.",
                    ),
            )
            .child(
                div()
                    .pb(px(10.))
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(rgba(if cfg.pinned.len() >= crate::config::MAX_PINNED {
                        u.accent
                    } else {
                        u.dim
                    }))
                    .child(format!(
                        "Up to {} apps can be pinned ({} of {} used).",
                        crate::config::MAX_PINNED,
                        cfg.pinned.len(),
                        crate::config::MAX_PINNED,
                    )),
            );

        // the Pinned section only exists once something is pinned
        if !cfg.pinned.is_empty() {
            pane = pane
                .child(self.section_label("Pinned", u))
                .children(cfg.pinned.iter().enumerate().map(|(ix, pin)| {
                    let app = self.apps.iter().find(|a| class_matches(pin, &a.class));
                    let name = app
                        .map(|a| a.name.clone())
                        .unwrap_or_else(|| crate::windows::app_name(pin));
                    let icon = app
                        .and_then(|a| self.app_icon(a))
                        .or_else(|| self.icons.resolve(pin, ""));
                    self.app_row(("pinned", ix), icon, name, pin.clone(), true, false, u, cx)
                }))
                .child(div().h(px(8.)).flex_none());
        }

        let search = div()
            .id("appsearch")
            .h(px(30.))
            .flex_none()
            .rounded(px(6.))
            .bg(rgba(u.row))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle);
                    cx.notify();
                }),
            )
            .child(
                svg()
                    .path("icons/search.svg")
                    .w(px(12.))
                    .h(px(12.))
                    .flex_none()
                    .text_color(rgba(u.dim)),
            )
            .child({
                let focused = self.focus_handle.is_focused(window);
                let caret = div()
                    .w(px(1.5))
                    .h(px(15.))
                    .flex_none()
                    .bg(rgba(u.text))
                    .with_animation(
                        "caret-blink",
                        Animation::new(Duration::from_millis(1000)).repeat(),
                        |el, t| el.opacity(if t < 0.5 { 1.0 } else { 0.0 }),
                    );
                let mut field = div().flex().items_center();
                if !self.query.is_empty() {
                    field = field.child(div().text_color(rgba(u.text)).child(self.query.clone()));
                }
                if focused {
                    field = field.child(caret);
                }
                if self.query.is_empty() {
                    field = field.child(
                        div()
                            .pl(px(2.))
                            .text_color(rgba(u.dim))
                            .child("Search applications…"),
                    );
                }
                field
            });

        let matches: Vec<usize> = self
            .apps
            .iter()
            .enumerate()
            .filter(|(_, a)| q.is_empty() || a.name.to_lowercase().contains(&q))
            .map(|(ix, _)| ix)
            .collect();

        let mut list = div()
            .id("appslist")
            .flex_1()
            .min_h(px(0.))
            .mt(px(6.))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if matches.is_empty() {
            list = list.child(
                div()
                    .pt(px(10.))
                    .px(px(8.))
                    .text_color(rgba(u.dim))
                    .child("No matching applications"),
            );
        } else {
            let at_limit = cfg.pinned.len() >= crate::config::MAX_PINNED;
            for app_ix in matches {
                let app = &self.apps[app_ix];
                let pinned = cfg.pinned.iter().any(|p| class_matches(p, &app.class));
                list = list.child(self.app_row(
                    ("app", app_ix),
                    self.app_icon(app),
                    app.name.clone(),
                    app.class.clone(),
                    pinned,
                    at_limit && !pinned,
                    u,
                    cx,
                ));
            }
        }

        pane.child(self.section_label("All Applications", u))
            .child(search)
            .child(list)
    }

    fn about_pane(&self, u: &Ui) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.))
            .pt(px(20.))
            .children(
                crate::icons::app_logo_png(128)
                    .map(|p| img(p).w(px(64.)).h(px(64.)).flex_none()),
            )
            .child(
                div()
                    .pt(px(8.))
                    .text_size(px(16.))
                    .text_color(rgba(u.text))
                    .child(format!("Sidetab {}", env!("CARGO_PKG_VERSION"))),
            )
            .child(
                div()
                    .text_color(rgba(u.dim))
                    .child("A Contexts-style window switcher for Hyprland."),
            )
            .child(div().text_color(rgba(u.dim)).child("github.com/igaurab/sidetab"))
            .child(self.hint(
                format!("Config: {}", crate::config::config_path().display()),
                u,
            ))
    }
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let u = ui(&self.cfg);
        let active = self.active;

        let nav = div()
            .w(px(186.))
            .flex_none()
            .h_full()
            .bg(rgba(u.nav_bg))
            .flex()
            .flex_col()
            .p(px(10.))
            .gap(px(2.))
            .child(
                div()
                    .px(px(8.))
                    .pt(px(4.))
                    .pb(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .children(
                        crate::icons::app_logo_png(64)
                            .map(|p| img(p).w(px(20.)).h(px(20.)).flex_none()),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgba(u.text))
                            .child("Sidetab"),
                    ),
            )
            .children(SECTIONS.iter().enumerate().map(|(ix, &(section, icon, label))| {
                let selected = active == section;
                div()
                    .id(("nav", ix))
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(rgba(u.accent)).text_color(rgba(u.accent_text)))
                    .when(!selected, |d| d.text_color(rgba(u.text)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.active = section;
                            if section == Section::PinnedApps {
                                // fresh app list, and keyboard focus for the search bar
                                this.apps = crate::apps::installed();
                                window.focus(&this.focus_handle);
                            }
                            cx.notify();
                        }),
                    )
                    .child(
                        svg()
                            .path(icon)
                            .w(px(14.))
                            .h(px(14.))
                            .flex_none()
                            .text_color(rgba(if selected { u.accent_text } else { u.dim })),
                    )
                    .child(label)
            }));

        let pane = match self.active {
            Section::Panel => self.panel_pane(&u, cx),
            Section::Shortcuts => self.shortcuts_pane(&u, cx),
            Section::Appearance => self.appearance_pane(&u, cx),
            Section::PinnedApps => self.pinned_pane(&u, window, cx),
            Section::About => self.about_pane(&u),
        };

        div()
            .id("settings-root")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                // the pinned-apps search bar is the only text input
                if this.active != Section::PinnedApps {
                    return;
                }
                match ev.keystroke.key.as_str() {
                    "backspace" => {
                        this.query.pop();
                        cx.notify();
                    }
                    "escape" => {
                        if !this.query.is_empty() {
                            this.query.clear();
                            cx.notify();
                        }
                    }
                    _ => {
                        if let Some(ch) = ev
                            .keystroke
                            .key_char
                            .as_ref()
                            .filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
                        {
                            this.query.push_str(ch);
                            cx.notify();
                        }
                    }
                }
            }))
            .size_full()
            .flex()
            .bg(rgba(u.bg))
            .text_color(rgba(u.text))
            .text_size(px(13.))
            .font_family(
                self.cfg
                    .font
                    .clone()
                    .unwrap_or_else(|| "Liberation Sans".into()),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if this.dragging.is_some() && ev.pressed_button == Some(MouseButton::Left) {
                    this.drag_update(ev.position, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.drag_end(cx)),
            )
            .child(nav)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.)) // let content wrap instead of overflowing
                    .h_full()
                    .flex()
                    .flex_col()
                    .p(px(18.))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .pb(px(4.))
                            .child(
                                div()
                                    .id("close")
                                    .w(px(24.))
                                    .h(px(24.))
                                    .rounded(px(6.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(rgba(u.row))
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|_, _, window, _| {
                                            window.remove_window();
                                        }),
                                    )
                                    .child(
                                        svg()
                                            .path("icons/x.svg")
                                            .w(px(12.))
                                            .h(px(12.))
                                            .text_color(rgba(u.dim)),
                                    ),
                            ),
                    )
                    .child(pane),
            )
    }
}
