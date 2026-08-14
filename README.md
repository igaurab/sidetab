# sidetab

A [Contexts](https://contexts.co/)-style window switcher for **Hyprland** — a fast
sidebar panel with windows grouped by workspace, real app icons, fuzzy search,
and hover-reveal at the screen edge. Built in Rust with
[GPUI](https://gpui.rs/) (Zed's GPU-accelerated UI framework).

## Features

- **True Alt-Tab**: hold Alt, tap Tab to cycle, release Alt to switch.
  A quick Alt-Tab tap switches to the previous window instantly without
  flashing the panel.
- **Grouped like macOS**: sections for Full Screen, each workspace (tiled
  windows), and Floating windows. Most-recently-used order within groups.
- **App icons** resolved from your icon theme (SVG icons rasterized and cached).
- **Fuzzy search** (Super+Tab): type to filter windows, Enter to switch,
  digits 1–9 to jump.
- **Hover reveal**: an invisible strip at the screen edge; hover to peek at
  your windows without touching the keyboard. Never steals focus.
- **Mouse friendly**: hover to highlight, click to switch.
- **Light/dark themes**, following your system preference by default.
- **Settings GUI** (`sidetab settings`) with live-applied changes: panel
  position (six placements), width, hover behavior, delays, theme.

## Install

```sh
cargo install sidetab
```

or from the AUR:

```sh
yay -S sidetab
```

Requires Hyprland ≥ 0.53 (new windowrule syntax), Vulkan, and Rust 1.85+
if building from source.

## Hyprland setup

sidetab applies its own window rules at runtime — you only add bindings and
autostart. In `~/.config/hypr/bindings.conf` (or your main config):

```conf
# Replace Alt-Tab with sidetab
unbind = ALT, TAB
unbind = ALT SHIFT, TAB
binde = ALT, TAB, exec, sidetab next
binde = ALT SHIFT, TAB, exec, sidetab prev
bindrt = ALT, ALT_L, exec, sidetab commit   # switch on Alt release

# Window search on Super+Tab
bindd = SUPER, TAB, Window search, exec, sidetab search
```

And in your autostart:

```conf
exec-once = sidetab daemon
```

## Commands

| Command | Effect |
|---|---|
| `sidetab daemon` | run the panel daemon (also the default with no args) |
| `sidetab next` / `prev` | cycle selection (bind with `binde`) |
| `sidetab commit` | switch to selection (bind with `bindrt` on Alt release) |
| `sidetab search` | open with keyboard focus + fuzzy filter |
| `sidetab toggle` / `show` / `hide` | control panel visibility |
| `sidetab settings` | open the settings window |
| `sidetab quit` | stop the daemon |

## Configuration

Settings are edited from the GUI (`sidetab settings`) or by hand in
`~/.config/sidetab/config.toml`:

```toml
position = "left-center"   # left-top | left-center | left-bottom
                           # right-top | right-center | right-bottom
width = 320.0
max_height_frac = 0.85
hover_reveal = true
hover_strip_px = 4.0       # width of the invisible reveal strip
show_delay_ms = 120
hide_delay_ms = 300
# font = "Inter"           # optional UI font override

[theme]
variant = "system"         # system | light | dark
# Optional color overrides ("#rrggbb" or "#rrggbbaa"):
# background = "#f2f2f2cc"
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
windows through Hyprland's event socket. The panel is `no_focus`-tagged so
hovering never steals your keyboard; search mode lifts the tag while open.

## License

MIT
