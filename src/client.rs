//! `sidetab <command>` fast path: write one line to the daemon socket and
//! exit. Must never touch gpui — this runs on every Alt+Tab press.

use anyhow::{bail, Context as _, Result};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn socket_path() -> PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(dir).join("sidetab.sock")
}

/// Pid of the daemon that owns the socket, written at bind time.
pub fn pid_path() -> PathBuf {
    socket_path().with_extension("pid")
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


/// How long a healthy daemon gets to answer `ping`.
///
/// A healthy answer takes ~2ms, but the handler checks the panel window on the
/// gpui main thread, and each Hyprland round-trip it makes is itself bounded
/// by `ctl::READ_TIMEOUT` — so a slow-but-alive compositor (exactly the
/// post-suspend case this feature exists for) can legitimately take seconds.
/// This budget has to exceed the daemon's, or a healthy daemon gets killed.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Round-trip the daemon's health check. `Err` means it is not pumping.
pub fn probe(timeout: Duration) -> Result<String> {
    let mut stream = UnixStream::connect(socket_path()).with_context(|| {
        format!(
            "sidetab daemon is not running (no socket at {})",
            socket_path().display()
        )
    })?;
    stream.set_write_timeout(Some(timeout))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.write_all(b"ping\n")?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = String::new();
    stream
        .read_to_string(&mut reply)
        .context("the sidetab daemon accepted the connection but never answered")?;
    Ok(reply.trim().to_string())
}

fn is_live_daemon(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "sidetab")
}

/// The running daemon's pid.
///
/// The pid file is the fast path, but it cannot be the only one: `panic =
/// "abort"` means it can outlive its process, and a daemon from before it
/// existed has none at all. Falling back to a `/proc` scan matters because the
/// caller spawns a replacement — without a pid to stop, an upgrade would leave
/// two daemons fighting over one panel.
fn daemon_pid() -> Option<u32> {
    let me = std::process::id();
    if let Some(pid) = std::fs::read_to_string(pid_path())
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&p| p != me && is_live_daemon(p))
    {
        return Some(pid);
    }
    std::fs::read_dir("/proc")
        .ok()?
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .find(|&pid| pid != me && is_live_daemon(pid))
}

fn wait_until(deadline: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    done()
}

/// Replace a daemon that has stopped answering.
///
/// Two clients racing here would both kill and both spawn, and the loser's
/// daemon would exit on bind ("already running") *after* having killed the
/// winner's — leaving none. The lock file makes the loser wait instead.
///
/// Must run in a *client* process: see [`spawn_restart`] for why the daemon
/// cannot do this to itself.
pub fn restart() -> Result<()> {
    let lock = socket_path().with_extension("restart");
    // A holder that crashed leaves the lock behind; treat a stale one as free.
    if let Ok(meta) = std::fs::metadata(&lock) {
        let stale = meta
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_none_or(|age| age > Duration::from_secs(10));
        if stale {
            let _ = std::fs::remove_file(&lock);
        }
    }
    let held = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)
        .is_ok();
    if !held {
        // Someone else is already doing this; just wait for the result.
        wait_until(Duration::from_secs(5), || probe(PROBE_TIMEOUT).is_ok());
        return Ok(());
    }
    let result = do_restart();
    let _ = std::fs::remove_file(&lock);
    result
}

fn do_restart() -> Result<()> {
    if let Some(pid) = daemon_pid() {
        let alive = || std::path::Path::new(&format!("/proc/{pid}")).exists();
        let _ = Command::new("kill").arg(pid.to_string()).status();
        if !wait_until(Duration::from_millis(1500), || !alive()) {
            // A SIGSTOPped daemon never acts on SIGTERM, but SIGKILL still
            // lands on it.
            let _ = Command::new("kill")
                .arg("-KILL")
                .arg(pid.to_string())
                .status();
            wait_until(Duration::from_secs(1), || !alive());
        }
    }
    // Spawning on top of a daemon we failed to stop is worse than doing
    // nothing: the survivor keeps its panel window but loses its socket, so it
    // is unreachable and un-killable by every later command.
    if let Some(pid) = daemon_pid() {
        bail!("a sidetab daemon (pid {pid}) is still running and would not stop; kill it and retry");
    }
    // The dead daemon left these bound/behind; a fresh bind needs them gone.
    let _ = std::fs::remove_file(socket_path());
    let _ = std::fs::remove_file(pid_path());

    Command::new(std::env::current_exe()?)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("could not start a replacement sidetab daemon")?;

    // The socket is bound before gpui opens the window, so waiting for the
    // socket is not enough — probe until the loop actually answers.
    if wait_until(Duration::from_secs(5), || probe(Duration::from_secs(1)).is_ok()) {
        Ok(())
    } else {
        bail!("the replacement daemon did not come up")
    }
}

/// Ask a detached helper process to restart the daemon.
///
/// This is the entry point for a restart requested from inside the daemon
/// (the Restart button in settings). The daemon cannot run [`restart`]
/// itself: `daemon_pid` deliberately skips the calling process, so the kill
/// step would be a no-op and the spawn step would leave a second daemon
/// bound to a fresh socket while the first one keeps its panel window.
/// `sidetab restart` in a separate process sees us as the daemon to replace.
pub fn spawn_restart() -> Result<()> {
    Command::new(std::env::current_exe()?)
        .arg("restart")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("could not spawn the sidetab restart helper")?;
    Ok(())
}

/// For user-initiated commands: check the daemon is really pumping before
/// sending, and replace it if it is not.
///
/// Never used on the Alt-Tab hot path — see this module's header. In the
/// healthy case this costs one sub-millisecond round-trip and spawns nothing.
pub fn send_checked(command: &str) -> Result<()> {
    // No socket at all is the plain "not running" case, whose error text
    // already tells the user what to do.
    if !socket_path().exists() {
        return send(command);
    }
    match probe(PROBE_TIMEOUT) {
        // The daemon is pumping but has no panel window; only a restart can
        // give it one back.
        Ok(reply) if reply == "pong lost" || reply == "pong window-gone" => {
            eprintln!("sidetab: the panel window is gone — restarting the daemon");
            restart()?;
            send(command)
        }
        // Hyprland is the one not answering. Restarting sidetab would not fix
        // that and would throw away the session's state for nothing.
        Ok(reply) if reply == "pong hypr-unreachable" => {
            eprintln!("sidetab: hyprland is not answering; leaving the daemon alone");
            send(command)
        }
        Ok(reply) if reply.starts_with("pong") => {
            if reply != "pong ok" {
                eprintln!("sidetab: daemon reports '{reply}'");
            }
            send(command)
        }
        Ok(reply) => {
            eprintln!("sidetab: daemon is unhealthy ('{reply}') — restarting it");
            restart()?;
            send(command)
        }
        Err(_) => {
            eprintln!("sidetab: daemon is not responding — restarting it");
            restart()?;
            send(command)
        }
    }
}

pub const COMMANDS: &[&str] = &[
    "next", "prev", "next-ws", "prev-ws", "commit", "toggle", "show", "hide", "search",
    "settings", "ping",
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
