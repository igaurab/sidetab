# Changelog

## 0.2.1

- **`sidetab install-bindings`** writes the Alt-Tab / Super+Tab shortcuts and
  the daemon autostart into your Hyprland config, so a fresh install no longer
  means hand-copying a snippet out of the README. It detects whether Hyprland
  reads the Lua config or the classic `.conf` one, picks the file that is
  actually loaded (a `bindings.*` file is only used when the main config really
  sources it), backs it up, and reloads Hyprland so the shortcuts work straight
  away. Guarded by a marker comment, so re-running is a no-op.
- The same action is in the settings window under **Window Switching**, which
  now states whether the shortcuts are installed and where they live — the one
  piece of setup the GUI couldn't do for you.
- Only stock `hl.*` Lua API is emitted, never Omarchy's `o.*` helpers, so the
  generated block works on any Lua config.

## 0.2.0

Omarchy 4 support. Omarchy 4 moves its theme state and switches Hyprland to
the new Lua config parser; sidetab now detects what's live at runtime and
speaks to it accordingly, so the same binary works on Omarchy 3 and 4.

### Fixed

- **Window rules no longer silently fail on Omarchy 4.** Hyprland 0.56's Lua
  parser rejects `keyword` outright and reinterprets `dispatch <args>` as a
  Lua expression, so none of the panel's rules landed: the panel and the
  settings window opened as ordinary tiled windows, unpinned, with a full
  border and square corners. Both are applied through `hl.window_rule` /
  `hl.dsp.*` when the Lua parser is in use, and through the original
  `keyword` / `dispatch` strings otherwise.
- **Theming follows Omarchy 4 again.** The current-theme pointer moved from
  `~/.config/omarchy/current/theme` to `~/.local/state/omarchy/current/theme`,
  which left the panel falling back to the system light/dark palette. Both
  locations are checked, newest first.
- Light themes are detected from `colors.toml`'s `mode` key (Omarchy 4),
  falling back to the `light.mode` marker file (Omarchy 3).
- `cursor:no_warps` is read correctly on Hyprland 0.56, which reports the
  option as `bool: true` where earlier versions printed `int: 1`.

### Changed

- Dispatchers are now a closed set rather than free-form command strings, so
  each one carries both a legacy and a Lua spelling.
- README documents binding setup for both the Lua config (Omarchy 4) and the
  classic `.conf` config, and notes that Omarchy 4 stops reading `.conf` —
  bindings and `exec-once` lines left there go silently inactive.

## 0.1.2

- **Omarchy theming**: the new `theme.variant = "omarchy"` (now the default)
  reads background, foreground, accent and light/dark from the current
  Omarchy theme's `colors.toml` on every reveal, so theme switches restyle
  the panel live. Falls back to `system` when Omarchy isn't installed. The
  settings window picks up the same colors.
- The centered Alt-Tab / Super+Tab overlay has its own width
  (`overlay_width`, default 640px, in settings under Switching, on a
  320–1200px slider with the same live preview as the sidebar width). The
  panel width now only sizes the docked sidebar, so a narrow sidebar no
  longer cramps window titles in the overlay.
- The sidebar can be narrowed to 170px (was 240px).
- Settings speaks plain language: the width controls are sliders labelled
  Small / Medium / Large rather than pixel counts, "Switching" is now
  "Window Switching", and the overlay is "Alt-Tab window size". The window
  itself is 60px wider so the navigation labels fit.
- Dragging the Alt-Tab window size previews it centered, where the overlay
  really appears, and steps around the settings window so the slider stays
  in view.
- The search header shows the settings gear like every other mode, and drops
  the "type to filter" placeholder.

### Fixed

- No more dark squares in the panel's corners. Hyprland clips a window's blur
  region to the *window's* rounding, so a square window painted blur into the
  corners the rounded card leaves transparent; the window now rounds to the
  same radius as the card (`CARD_ROUNDING`, 12px, shared by the rule and the
  card so they can't drift).
- The panel no longer picks up the theme's focus border after a Hyprland
  config reload (an Omarchy theme switch triggers one). `border_size 0`,
  `no_shadow on` and the rounding moved from `hyprctl setprop` — which no
  longer exists in Hyprland 0.56, so it had silently stopped working — to
  rules scoped to a `sidetab-chromeless` tag, re-tagged on every reveal.
  Re-tagging is what forces Hyprland to re-evaluate rules on a live window;
  re-adding the rule alone does not. Reloads also re-read theme colors,
  restyling a revealed panel in place.

## 0.1.1

- The panel no longer hides after **Close Window** from the right-click menu,
  so you can close several windows in a row. It also stays put while a
  context menu is open.
- Terminal rows show the foreground command running inside them.
- The panel never flashes the theme's focus border (per-window prop instead
  of a config-reload-sensitive window rule).
- Smaller binary: 20.9 MiB → 15.8 MiB, from `panic = "abort"` and a single
  codegen unit. (Fat LTO would save another 1.4 MiB but quadruples build
  time, so it's left off.)
- Faster builds: line-tables-only debuginfo cuts the debug binary from 531 MB
  to 147 MB. A new `quick` profile gives an optimized binary in 13s per edit
  instead of the release profile's 63s (`cargo build --profile quick`).

## 0.1.0

First release — Alt-Tab overlay, Super+Tab for the current workspace, windows
grouped by Pinned / Full Screen / workspace / Floating in MRU order, app icons
from your icon theme, fuzzy search, hover reveal at the screen edge, and a
settings GUI with a live panel preview.
