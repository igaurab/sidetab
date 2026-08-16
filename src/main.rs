mod apps;
mod assets;
mod bindings;
mod client;
mod config;
mod daemon;
mod hypr;
mod icons;
mod setup;
mod ui;
mod windows;

const HELP: &str = "\
sidetab — a Contexts-style window switcher for Hyprland

USAGE:
    sidetab [daemon]     run the panel daemon (default)
    sidetab <command>    send a command to the running daemon

COMMANDS:
    next      select the next window, all workspaces (bind to ALT, TAB with binde)
    prev      select the previous window
    next-ws   like next, but only windows on the active workspace (Super+Tab)
    prev-ws   like prev, current workspace only
    commit    switch to the selection (bind to modifier release with bindrt)
    toggle    show or hide the panel
    show      show the panel
    hide      hide the panel
    search    open the panel with keyboard focus and fuzzy search
    settings  open the settings window
    setup     install the app-menu entry and icon (also done on daemon start)
    install-bindings
              add the Alt-Tab / Super+Tab shortcuts and the daemon autostart
              to your Hyprland config (detects Lua vs .conf; safe to re-run)
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
        Some("setup") => {
            setup::install_desktop_integration()?;
            for path in [setup::icon_path(), setup::desktop_entry_path()]
                .into_iter()
                .flatten()
            {
                println!("installed {}", path.display());
            }
            Ok(())
        }
        Some("install-bindings") => {
            match bindings::install()? {
                bindings::Outcome::AlreadyInstalled { file } => {
                    println!("bindings already installed in {}", file.display());
                    println!("nothing to do");
                }
                bindings::Outcome::Installed {
                    file,
                    backup,
                    reloaded,
                } => {
                    println!("detected: {}", bindings::plan()?.style.label());
                    if let Some(backup) = backup {
                        println!("backup:   {}", backup.display());
                    }
                    println!("wrote:    {}", file.display());
                    if reloaded {
                        println!("reloaded Hyprland — Alt+Tab is live");
                    } else {
                        println!("run 'hyprctl reload' (or log back in) to apply");
                    }
                }
            }
            Ok(())
        }
        Some("quit") => client::send("quit"),
        Some(cmd) => {
            client::validate(cmd)?;
            client::send(cmd)
        }
    }
}
