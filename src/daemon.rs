//! The resident process: owns the gpui panel window, the control socket,
//! and the Hyprland event stream.

use crate::client;
use crate::config::Config;
use crate::hypr::{ctl, events, events::HyprEvent};
use crate::ui::panel::Switcher;
use anyhow::{bail, Result};
use futures::StreamExt;
use gpui::{
    prelude::*, px, size, App, Application, Bounds, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions,
};
use std::io::BufRead;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

pub enum Msg {
    Cmd(String),
    Event(HyprEvent),
}

fn bind_control_socket() -> Result<UnixListener> {
    let path = client::socket_path();
    if UnixStream::connect(&path).is_ok() {
        bail!("sidetab daemon is already running");
    }
    let _ = std::fs::remove_file(&path); // stale socket from a crash
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

fn spawn_control_listener(
    listener: UnixListener,
) -> futures::channel::mpsc::UnboundedReceiver<String> {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    std::thread::Builder::new()
        .name("control-socket".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                let mut reader = std::io::BufReader::new(stream);
                if reader.read_line(&mut line).is_ok() {
                    let cmd = line.trim().to_string();
                    if !cmd.is_empty() && tx.unbounded_send(cmd).is_err() {
                        return;
                    }
                }
            }
        })
        .expect("spawn control-socket thread");
    rx
}

fn open_settings(
    panel: gpui::WindowHandle<Switcher>,
    cx: &mut gpui::AsyncApp,
) -> Result<gpui::WindowHandle<crate::ui::settings::Settings>> {
    let panel_entity = panel.update(cx, |_, _, cx| cx.entity())?;
    let cfg = Config::load();
    let handle = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: gpui::point(px(0.), px(0.)),
                    size: size(px(620.), px(460.)),
                })),
                titlebar: None,
                focus: true,
                show: true,
                kind: WindowKind::Normal,
                is_movable: true,
                is_resizable: false,
                is_minimizable: false,
                window_background: WindowBackgroundAppearance::Opaque,
                app_id: Some("sidetab-settings".into()),
                ..Default::default()
            },
            |window, cx| {
                window.set_window_title("Sidetab Settings");
                cx.new(|_| crate::ui::settings::Settings::new(cfg, panel_entity))
            },
        )
    })??;
    Ok(handle)
}

/// Our window's Hyprland address, once it has mapped.
fn find_own_address() -> Option<String> {
    ctl::clients()
        .ok()?
        .into_iter()
        .find(|c| c.class == "sidetab")
        .map(|c| c.address)
}

pub fn run() -> Result<()> {
    // Best-effort app-menu integration for cargo-install users.
    let _ = crate::setup::install_desktop_integration();
    let listener = bind_control_socket()?;
    ctl::apply_panel_rules()?;
    let cfg = Config::load();

    let cmd_rx = spawn_control_listener(listener);
    let event_rx = events::spawn_reader();

    Application::new()
        .with_assets(crate::assets::Assets)
        .run(move |cx: &mut App| {
        let width = cfg.width;
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: gpui::point(px(0.), px(0.)),
                        size: size(px(width), px(600.)),
                    })),
                    titlebar: None,
                    focus: false,
                    show: true,
                    kind: WindowKind::Normal,
                    is_movable: false,
                    is_resizable: false,
                    is_minimizable: false,
                    window_background: WindowBackgroundAppearance::Transparent,
                    app_id: Some("sidetab".into()),
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| Switcher::new(cfg, cx)),
            )
            .expect("open panel window");

        // Discover our Hyprland address once the surface maps, then park it.
        let handle = window;
        cx.spawn(async move |cx| {
            for _ in 0..100 {
                if let Some(addr) = find_own_address() {
                    let _ = handle.update(cx, |view, _, cx| view.set_address(addr, cx));
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
            }
            eprintln!("sidetab: never found own window in hyprctl clients");
        })
        .detach();

        // Pump control commands and Hyprland events into the view.
        let handle = window;
        cx.spawn(async move |cx| {
            let mut settings_window: Option<gpui::WindowHandle<crate::ui::settings::Settings>> =
                None;
            let mut stream = futures::stream::select(
                cmd_rx.map(Msg::Cmd),
                event_rx.map(Msg::Event),
            );
            while let Some(msg) = stream.next().await {
                if let Msg::Cmd(cmd) = &msg {
                    match cmd.as_str() {
                        "quit" => {
                            let _ = cx.update(|cx| cx.quit());
                            return;
                        }
                        "settings" => {
                            // focus the existing window, or open a fresh one
                            let focused = settings_window
                                .as_ref()
                                .and_then(|w| {
                                    w.update(cx, |_, window, _| window.activate_window()).ok()
                                })
                                .is_some();
                            if !focused {
                                settings_window = open_settings(handle, cx).ok();
                            }
                            continue;
                        }
                        _ => {}
                    }
                }
                if handle
                    .update(cx, |view, window, cx| view.handle_msg(msg, window, cx))
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
    });

    let _ = std::fs::remove_file(client::socket_path());
    Ok(())
}
