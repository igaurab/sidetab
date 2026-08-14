# sidetab


<img width="1056" height="778" alt="image" src="https://github.com/user-attachments/assets/f695a6c1-dbc5-43be-aed1-e1b081ca13be" />


A [Contexts](https://contexts.co/)-style window switcher for **Hyprland** — a fast
sidebar panel with windows grouped by workspace, real app icons, fuzzy search,
and hover-reveal at the screen edge. Built in Rust with
[GPUI](https://gpui.rs/) (Zed's GPU-accelerated UI framework).


## Features

- **True Alt-Tab**: hold Alt, tap Tab to cycle through all windows in a
  centered overlay, release Alt to switch. A quick Alt-Tab tap switches to
  the previous window instantly without flashing the panel.
- **Super+Tab** cycles only the current workspace's windows, same mechanics.
- **Grouped like macOS**: sections for Pinned apps, Full Screen, each
  workspace (tiled windows), and Floating windows. Most-recently-used order
  within groups. The panel always sizes itself to show every window — no
  scrolling.
- **App icons** resolved from your icon theme.
- **Fuzzy search** (`sidetab search`): type to filter windows, Enter to
  switch, digits 1–9 to jump.
- **Hover reveal**: the sidebar stays hidden until your cursor touches the
  screen edge, then slides in; it never steals your keyboard focus.
- **Configurable switching**: choose what Alt+Tab and Super+Tab each cycle
  through (all workspaces or current workspace) from settings — no Hyprland
  config edits needed. Disabling a shortcut restores the stock behavior
  (plain Hyprland window cycling / next-workspace switching).
- **Mouse friendly**: hover to highlight, click to switch.
- **Follows your Omarchy theme**: colors come from the current theme's
  `colors.toml` (background, foreground, accent, light/dark), re-read on
  every reveal, so switching themes or wallpapers restyles the panel with
  no restart. Falls back to your system light/dark preference elsewhere.
- **Settings GUI** (`sidetab settings`), laid out like the Contexts
  preferences: panel width (down to 170px), Alt-Tab overlay width, six
  position presets plus a fine-tune slider with a mini screen preview (the
  real panel moves live while you drag), hover behavior, delays, theme, and
  pinned apps.

## Install

```sh
cargo install sidetab
```

Requires Hyprland ≥ 0.53 (new windowrule syntax), Vulkan, and Rust 1.85+
if building from source.

### App menu entry

sidetab installs its icon and a `.desktop` launcher into `~/.local/share`
automatically the first time the daemon starts, so it shows up in app
launchers even when installed with `cargo install`. To (re)install it
manually, run `sidetab setup`. The AUR package ships system-wide copies
instead.

## Hyprland setup

sidetab applies its own window rules at runtime — you only add bindings and
autostart. In `~/.config/hypr/bindings.conf` (or your main config):

```conf
# Replace Alt-Tab with sidetab (all workspaces, centered overlay)
unbind = ALT, TAB
unbind = ALT SHIFT, TAB
binde = ALT, TAB, exec, sidetab next
binde = ALT SHIFT, TAB, exec, sidetab prev
bindrt = ALT, ALT_L, exec, sidetab commit   # switch on Alt release

# Super+Tab cycles the current workspace only
binde = SUPER, TAB, exec, sidetab next-ws
bindrt = SUPER, SUPER_L, exec, sidetab commit

# Optional: fuzzy window search
# bindd = SUPER, SLASH, Window search, exec, sidetab search
```

And in your autostart:

```conf
exec-once = sidetab daemon
```

## Commands

| Command | Effect |
|---|---|
| `sidetab daemon` | run the panel daemon (also the default with no args) |
| `sidetab next` / `prev` | cycle selection across all workspaces (bind with `binde`) |
| `sidetab next-ws` / `prev-ws` | cycle selection within the active workspace |
| `sidetab commit` | switch to selection (bind with `bindrt` on modifier release) |
| `sidetab search` | open with keyboard focus + fuzzy filter |
| `sidetab toggle` / `show` / `hide` | control panel visibility |
| `sidetab settings` | open the settings window |
| `sidetab quit` | stop the daemon |

## Configuration

Settings are edited from the GUI (`sidetab settings`) or by hand in
`~/.config/sidetab/config.toml`:

```toml
edge = "left"              # left | right
width = 320.0              # docked sidebar (hover reveal, show, search)
overlay_width = 640.0      # centered Alt-Tab / Super+Tab overlay
v_pos = 0.5                # placement along the edge, 0.0 top .. 1.0 bottom
                           # (set it visually from the settings slider/preview)
alt_tab = "all-workspaces"      # all-workspaces | current-workspace | disabled
super_tab = "current-workspace" # what each cycling shortcut shows
hover_strip_px = 4.0       # width of the invisible hover trigger zone at the
                           # screen edge (nothing is shown while hidden)
show_delay_ms = 120
hide_delay_ms = 300
# font = "Inter"           # optional UI font override
pinned = ["Spotify"]       # window classes shown in the Pinned section
                           # (easier: toggle apps in `sidetab settings`)

[theme]
variant = "omarchy"        # omarchy | system | light | dark
# Optional color overrides ("#rrggbb" or "#rrggbbaa"):
# background = "#f2f2f2"
# accent = "#2c6fef"
# text = "#2b2b2b"
# dim_text = "#8a8a8a"
```

In search mode: type to filter, Up/Down/Tab to move, Enter to switch,
Esc to dismiss, digits 1–9 to jump when the query is empty.

## How it works

A resident daemon owns a GPUI window that Hyprland treats as a floating,
pinned, chromeless panel (rules applied via `hyprctl` at runtime and
re-applied on config reload). Hiding parks the window off-screen leaving a
few pixels as a hover target. Key bindings talk to the daemon over a unix
socket at `$XDG_RUNTIME_DIR/sidetab.sock`, and the daemon tracks your
windows through Hyprland's event socket.

Two window tags carry the rules that have to change on a live window, since
Hyprland only re-evaluates rules when a window's tags change: the panel is
`no_focus`-tagged so hovering never steals your keyboard (search mode lifts
the tag while open), and `sidetab-chromeless` carries `border_size 0`,
`no_shadow on` and the panel's corner rounding — the last one matters
because Hyprland clips a window's blur region to the window's rounding.

## License

MIT
