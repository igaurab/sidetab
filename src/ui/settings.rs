//! Settings window: lives in the daemon, applies every change to the panel
//! immediately and persists it to ~/.config/sidetab/config.toml.

use crate::config::{Config, Position, ThemeVariant};
use crate::ui::panel::Switcher;
use gpui::{div, prelude::*, px, rgba, Context, Entity, MouseButton, Window};

pub struct Settings {
    cfg: Config,
    panel: Entity<Switcher>,
    /// distinct app classes of open windows plus already-pinned ones
    known_apps: Vec<String>,
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
            known_apps,
        }
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let _ = self.cfg.save();
        let cfg = self.cfg.clone();
        self.panel.update(cx, |panel, cx| panel.update_config(cfg, cx));
        cx.notify();
    }

    fn mutate(&mut self, f: impl FnOnce(&mut Config), cx: &mut Context<Self>) {
        f(&mut self.cfg);
        self.apply(cx);
    }
}

// palette for the settings window itself (opaque, follows panel theme)
struct Ui {
    bg: u32,
    row: u32,
    text: u32,
    dim: u32,
    accent: u32,
    accent_text: u32,
}

fn ui(cfg: &Config) -> Ui {
    let p = cfg.theme.palette();
    let dark = crate::config::system_prefers_dark();
    let dark = match cfg.theme.variant {
        ThemeVariant::Dark => true,
        ThemeVariant::Light => false,
        ThemeVariant::System => dark,
    };
    Ui {
        bg: if dark { 0x1f2023ff } else { 0xf4f4f5ff },
        row: if dark { 0xffffff10 } else { 0x00000008 },
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

    fn toggle(
        &self,
        id: &'static str,
        on: bool,
        flip: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        u: &Ui,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id((id, 0usize))
            .w(px(40.))
            .h(px(22.))
            .rounded(px(11.))
            .p(px(2.))
            .bg(rgba(if on { u.accent } else { u.row }))
            .flex()
            .when(on, |d| d.justify_end())
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| flip(this, cx)),
            )
            .child(
                div()
                    .w(px(18.))
                    .h(px(18.))
                    .rounded(px(9.))
                    .bg(gpui::white()),
            )
    }

    fn row(
        &self,
        label: &'static str,
        control: impl IntoElement,
        u: &Ui,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(40.))
            .child(div().text_color(rgba(u.text)).child(label))
            .child(control)
    }

    fn section(&self, label: &'static str, u: &Ui) -> impl IntoElement {
        div()
            .pt(px(14.))
            .pb(px(2.))
            .text_size(px(11.))
            .text_color(rgba(u.dim))
            .child(label.to_uppercase())
    }
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let u = ui(&self.cfg);
        let cfg = self.cfg.clone();

        let position_grid = div().flex().flex_col().gap(px(6.)).children(
            [
                &Position::ALL[..3], // left column entries rendered as a row
                &Position::ALL[3..],
            ]
            .into_iter()
            .enumerate()
            .map(|(row_ix, row)| {
                div().flex().gap(px(6.)).children(row.iter().enumerate().map(
                    |(col_ix, &pos)| {
                        let active = cfg.position == pos;
                        self.chip(
                            ("pos", row_ix * 3 + col_ix),
                            pos.label().to_string(),
                            active,
                            move |this, cx| this.mutate(|c| c.position = pos, cx),
                            &u,
                            cx,
                        )
                    },
                ))
            }),
        );

        let theme_row = div().flex().gap(px(6.)).children(
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
                    &u,
                    cx,
                )
            }),
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgba(u.bg))
            .text_color(rgba(u.text))
            .text_size(px(13.))
            .font_family(
                self.cfg
                    .font
                    .clone()
                    .unwrap_or_else(|| "Liberation Sans".into()),
            )
            .p(px(18.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pb(px(6.))
                    .child(div().text_size(px(15.)).child("Sidetab Settings"))
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
                            .child("✕"),
                    ),
            )
            .child(self.section("Panel", &u))
            .child(self.row(
                "Width",
                self.stepper(
                    "width",
                    format!("{}px", cfg.width as i64),
                    |this, cx| this.mutate(|c| c.width = (c.width - 20.0).max(240.0), cx),
                    |this, cx| this.mutate(|c| c.width = (c.width + 20.0).min(640.0), cx),
                    &u,
                    cx,
                ),
                &u,
            ))
            .child(div().pt(px(6.)).child(position_grid))
            .child(self.section("Hover reveal", &u))
            .child(self.row(
                "Reveal on edge hover",
                self.toggle(
                    "hover",
                    cfg.hover_reveal,
                    |this, cx| this.mutate(|c| c.hover_reveal = !c.hover_reveal, cx),
                    &u,
                    cx,
                ),
                &u,
            ))
            .child(self.row(
                "Edge strip width",
                self.stepper(
                    "strip",
                    format!("{}px", cfg.hover_strip_px as i64),
                    |this, cx| {
                        this.mutate(|c| c.hover_strip_px = (c.hover_strip_px - 1.0).max(2.0), cx)
                    },
                    |this, cx| {
                        this.mutate(|c| c.hover_strip_px = (c.hover_strip_px + 1.0).min(16.0), cx)
                    },
                    &u,
                    cx,
                ),
                &u,
            ))
            .child(self.row(
                "Show delay",
                self.stepper(
                    "showdelay",
                    format!("{}ms", cfg.show_delay_ms),
                    |this, cx| this.mutate(|c| c.show_delay_ms = c.show_delay_ms.saturating_sub(30), cx),
                    |this, cx| this.mutate(|c| c.show_delay_ms = (c.show_delay_ms + 30).min(600), cx),
                    &u,
                    cx,
                ),
                &u,
            ))
            .child(self.row(
                "Hide delay",
                self.stepper(
                    "hidedelay",
                    format!("{}ms", cfg.hide_delay_ms),
                    |this, cx| this.mutate(|c| c.hide_delay_ms = c.hide_delay_ms.saturating_sub(50), cx),
                    |this, cx| this.mutate(|c| c.hide_delay_ms = (c.hide_delay_ms + 50).min(2000), cx),
                    &u,
                    cx,
                ),
                &u,
            ))
            .child(self.section("Appearance", &u))
            .child(div().pt(px(6.)).child(theme_row))
            .child(self.section("Pinned apps", &u))
            .child(
                div()
                    .pt(px(6.))
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .children(self.known_apps.clone().into_iter().enumerate().map(
                        |(ix, class)| {
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
                                &u,
                                cx,
                            )
                        },
                    )),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(u.dim))
                    .child(format!(
                        "Config: {}",
                        crate::config::config_path().display()
                    )),
            )
            .child({
                let _ = window;
                div()
            })
    }
}
