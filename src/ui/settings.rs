//! Settings window, laid out like the macOS Contexts preferences: a
//! navigation sidebar on the left, the selected section's controls on the
//! right. Lives in the daemon; every change applies to the panel
//! immediately and persists to ~/.config/sidetab/config.toml.

use crate::config::{Config, CycleScope, Edge, ThemeVariant};
use crate::ui::panel::Switcher;
use gpui::{
    canvas, div, img, prelude::*, px, rgba, svg, Bounds, Context, Entity, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Window,
};

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
    (Section::Shortcuts, "icons/keys.svg", "Shortcuts"),
    (Section::Appearance, "icons/appearance.svg", "Appearance"),
    (Section::PinnedApps, "icons/pin.svg", "Pinned Apps"),
    (Section::About, "icons/info.svg", "About"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    Slider,
    Preview,
}

pub struct Settings {
    cfg: Config,
    panel: Entity<Switcher>,
    active: Section,
    /// distinct app classes of open windows plus already-pinned ones
    known_apps: Vec<String>,
    dragging: Option<Drag>,
    track_bounds: Option<Bounds<Pixels>>,
    preview_bounds: Option<Bounds<Pixels>>,
}

fn known_apps(cfg: &Config) -> Vec<String> {
    let mut apps: Vec<String> = crate::windows::fetch()
        .into_iter()
        .map(|e| e.class)
        .chain(cfg.pinned.iter().cloned())
        .collect();
    apps.sort();
    apps.dedup();
    apps
}

impl Settings {
    pub fn new(cfg: Config, panel: Entity<Switcher>) -> Self {
        let known_apps = known_apps(&cfg);
        Settings {
            cfg,
            panel,
            active: Section::Panel,
            known_apps,
            dragging: None,
            track_bounds: None,
            preview_bounds: None,
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
            None => return,
        }
        self.apply_live(cx);
        // show the real panel at the new spot while dragging
        self.panel.update(cx, |panel, cx| panel.preview_reveal(cx));
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
    let dark = match cfg.theme.variant {
        ThemeVariant::Dark => true,
        ThemeVariant::Light => false,
        ThemeVariant::System => crate::config::system_prefers_dark(),
    };
    Ui {
        bg: if dark { 0x1f2023ff } else { 0xf4f4f5ff },
        nav_bg: if dark { 0x18191bff } else { 0xe9e9ebff },
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
            .child(self.heading("Shortcuts", u))
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
                "Choose what each shortcut cycles through, or disable it. \
                 Disabled shortcuts do nothing while sidetab holds the \
                 binding — remove the bind from your Hyprland config to \
                 give the key back to something else."
                    .to_string(),
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
            .child(self.hint(
                "Show and hide delay control how quickly the sidebar expands \
                 when hovered and retracts after the cursor leaves."
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
                "Width",
                self.stepper(
                    "width",
                    format!("{}px", cfg.width as i64),
                    |this, cx| this.mutate(|c| c.width = (c.width - 20.0).max(240.0), cx),
                    |this, cx| this.mutate(|c| c.width = (c.width + 20.0).min(640.0), cx),
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
                "System follows your desktop's light/dark preference. Colors \
                 can be overridden in the config file."
                    .to_string(),
                u,
            ))
    }

    fn pinned_pane(&self, u: &Ui, cx: &mut Context<Self>) -> gpui::Div {
        let cfg = self.cfg.clone();
        div()
            .flex()
            .flex_col()
            .child(self.heading("Pinned Apps", u))
            .child(
                div().flex().flex_wrap().gap(px(6.)).children(
                    self.known_apps
                        .clone()
                        .into_iter()
                        .enumerate()
                        .map(|(ix, class)| {
                            let active = cfg.pinned.contains(&class);
                            let toggled = class.clone();
                            self.chip(
                                ("pin", ix),
                                crate::windows::app_name(&class),
                                active,
                                move |this, cx| {
                                    this.mutate(
                                        |c| {
                                            if let Some(pos) =
                                                c.pinned.iter().position(|p| p == &toggled)
                                            {
                                                c.pinned.remove(pos);
                                            } else {
                                                c.pinned.push(toggled.clone());
                                            }
                                        },
                                        cx,
                                    )
                                },
                                u,
                                cx,
                            )
                        }),
                ),
            )
            .child(self.hint(
                "Pinned apps get their own section at the top of the panel. \
                 Apps listed here are the ones currently running."
                    .to_string(),
                u,
            ))
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let u = ui(&self.cfg);
        let active = self.active;

        let nav = div()
            .w(px(150.))
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
                        cx.listener(move |this, _, _, cx| {
                            this.active = section;
                            if section == Section::PinnedApps {
                                this.known_apps = known_apps(&this.cfg);
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
            Section::PinnedApps => self.pinned_pane(&u, cx),
            Section::About => self.about_pane(&u),
        };

        div()
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
