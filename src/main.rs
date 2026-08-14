mod client;
mod config;
mod daemon;
mod hypr;
mod icons;
mod ui;
mod windows;

const HELP: &str = "\
sidetab — a Contexts-style window switcher for Hyprland

USAGE:
    sidetab [daemon]     run the panel daemon (default)
    sidetab <command>    send a command to the running daemon

COMMANDS:
    next      select the next window (bind to ALT, TAB with binde)
    prev      select the previous window
    commit    switch to the selection (bind to ALT release with bindrt)
    toggle    show or hide the panel
    show      show the panel
    hide      hide the panel
    search    open the panel with keyboard focus and fuzzy search
    settings  open the settings window
    quit      stop the daemon

CONFIG:
    ~/.config/sidetab/config.toml (created by the settings window)
";

fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        None | Some("daemon") => daemon::run(),
        Some("-h" | "--help" | "help") => {
            print!("{HELP}");
            Ok(())
        }
        Some("-V" | "--version" | "version") => {
            println!("sidetab {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("quit") => client::send("quit"),
        Some(cmd) => {
            client::validate(cmd)?;
            client::send(cmd)
        }
    }
}
