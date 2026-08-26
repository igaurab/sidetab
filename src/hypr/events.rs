//! Event socket (.socket2.sock) reader thread. Emits typed events into a
//! channel; reconnects with backoff and rediscovers the instance directory
//! if Hyprland restarts.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum HyprEvent {
    /// Window set changed (open/close/move/workspace change) — refresh list.
    WindowsChanged,
    ActiveWindowChanged,
    WindowTitle { address: String },
    FullscreenChanged(bool),
    /// A monitor was added or removed. Rare, and the one moment at which the
    /// panel can be orphaned onto a workspace that no longer has a monitor.
    MonitorsChanged,
    /// Focus moved to another monitor. Fires on ordinary pointer motion across
    /// a monitor edge, so it is kept separate from [`Self::MonitorsChanged`] —
    /// it must never force a `clients()` round-trip.
    FocusedMonitorChanged,
    ConfigReloaded,
}

fn parse_line(line: &str) -> Option<HyprEvent> {
    let (event, data) = line.split_once(">>")?;
    match event {
        "openwindow" | "closewindow" | "movewindow" | "workspace" | "changefloatingmode" => {
            Some(HyprEvent::WindowsChanged)
        }
        "activewindow" => Some(HyprEvent::ActiveWindowChanged),
        "windowtitlev2" => {
            let address = data.split(',').next()?.to_string();
            Some(HyprEvent::WindowTitle {
                address: format!("0x{}", address.trim_start_matches("0x")),
            })
        }
        "fullscreen" => Some(HyprEvent::FullscreenChanged(data.trim() == "1")),
        "monitoradded" | "monitoraddedv2" | "monitorremoved" | "monitorremovedv2" => {
            Some(HyprEvent::MonitorsChanged)
        }
        "focusedmon" | "focusedmonv2" => Some(HyprEvent::FocusedMonitorChanged),
        "configreloaded" => Some(HyprEvent::ConfigReloaded),
        _ => None,
    }
}

/// Spawns the reader thread; events land in the returned receiver.
pub fn spawn_reader() -> futures::channel::mpsc::UnboundedReceiver<HyprEvent> {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    std::thread::Builder::new()
        .name("hypr-events".into())
        .spawn(move || {
            let mut backoff = Duration::from_millis(200);
            loop {
                let Some(dir) = super::instance_dir() else {
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                };
                match UnixStream::connect(dir.join(".socket2.sock")) {
                    Ok(stream) => {
                        backoff = Duration::from_millis(200);
                        let reader = BufReader::new(stream);
                        for line in reader.lines() {
                            let Ok(line) = line else { break };
                            if let Some(ev) = parse_line(&line) {
                                if tx.unbounded_send(ev).is_err() {
                                    return; // daemon gone
                                }
                            }
                        }
                        // EOF: Hyprland restarted; signal a refresh once we reconnect.
                        let _ = tx.unbounded_send(HyprEvent::ConfigReloaded);
                    }
                    Err(_) => {
                        std::thread::sleep(backoff);
                        backoff = (backoff * 2).min(Duration::from_secs(5));
                    }
                }
            }
        })
        .expect("spawn hypr-events thread");
    rx
}
