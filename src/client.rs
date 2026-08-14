//! `sidetab <command>` fast path: write one line to the daemon socket and
//! exit. Must never touch gpui — this runs on every Alt+Tab press.

use anyhow::{bail, Context as _, Result};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

pub fn socket_path() -> PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(dir).join("sidetab.sock")
}

pub fn send(command: &str) -> Result<()> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path).with_context(|| {
        format!(
            "sidetab daemon is not running (no socket at {}). Start it with: sidetab daemon",
            path.display()
        )
    })?;
    stream.write_all(command.as_bytes())?;
    stream.write_all(b"\n")?;
    Ok(())
}

pub const COMMANDS: &[&str] = &[
    "next", "prev", "commit", "toggle", "show", "hide", "search", "settings",
];

pub fn validate(command: &str) -> Result<()> {
    if !COMMANDS.contains(&command) {
        bail!(
            "unknown command '{command}'. Available: daemon, {}",
            COMMANDS.join(", ")
        );
    }
    Ok(())
}
