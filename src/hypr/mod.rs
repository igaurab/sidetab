pub mod ctl;
pub mod events;

use std::path::PathBuf;

/// Directory holding Hyprland's IPC sockets for the running instance.
/// Falls back to scanning $XDG_RUNTIME_DIR/hypr for the newest instance
/// (handles Hyprland restarts making $HYPRLAND_INSTANCE_SIGNATURE stale).
pub fn instance_dir() -> Option<PathBuf> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let base = PathBuf::from(runtime).join("hypr");
    if let Ok(sig) = std::env::var("HYPRLAND_INSTANCE_SIGNATURE") {
        let dir = base.join(&sig);
        if dir.join(".socket.sock").exists() {
            return Some(dir);
        }
    }
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&base).ok()?.flatten() {
        let dir = entry.path();
        if !dir.join(".socket.sock").exists() {
            continue;
        }
        let mtime = entry.metadata().and_then(|m| m.modified()).ok()?;
        if newest.as_ref().is_none_or(|(t, _)| mtime > *t) {
            newest = Some((mtime, dir));
        }
    }
    newest.map(|(_, d)| d)
}
