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


/// How long a healthy daemon gets to answer `ping`. The reply is produced on
/// the gpui main thread after two local Hyprland round-trips, so this is
/// orders of magnitude more than it needs.
const PROBE_TIMEOUT: Duration = Duration::from_millis(400);

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

/// The running daemon's pid, but only if that pid is still a live sidetab.
/// `panic = "abort"` means the pid file can outlive its process, and pids get
/// reused.
fn daemon_pid() -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(pid_path()).ok()?.trim().parse().ok()?;
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    (comm.trim() == "sidetab").then_some(pid)
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
fn restart() -> Result<()> {
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
        // `pong lost` means the daemon is pumping but its window is gone,
        // which it cannot repair from the inside.
        Ok(reply) if reply.starts_with("pong") && reply != "pong lost" => {
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
