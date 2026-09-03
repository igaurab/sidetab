# Changelog

## 0.2.4

The panel placement is rolled back to the 0.2.1 model. 0.2.2 rewrote how the
panel follows workspaces and it made things worse in daily use; this release
reverts that rewrite and keeps only the parts of 0.2.2 that proved sound.

### Issues fixed

- **Issue: the panel flickered constantly** (regression introduced in 0.2.2).
  0.2.2 removed the `pin` window rule and instead moved the panel between
  workspaces itself with `movetoworkspacesilent`, batched with the resize and
  move that park it offscreen. On Hyprland 0.56 that move re-homes the
  floating window and can reposition it *after* the batched park lands, so on
  workspace switches the panel appeared on screen for a few frames before the
  next event shoved it back off. The same churn hit the centered Alt-Tab
  overlay, which re-placed itself on every window event while cycling
  (`Mode::Cycling => place(true)`), making the overlay flicker too.
- **Issue: the panel randomly ended up sitting in the middle of the screen**
  (regression introduced in 0.2.2, worsened by 0.2.2's own repair logic).
  Two paths led there. (1) If the daemon never discovered its own window's
  Hyprland address — a startup race — nothing ever placed the window, so it
  stayed exactly where Hyprland puts a new floating window: dead center,
  fully visible. (2) The 0.2.2 re-anchoring machinery (`ensure_anchored`)
  moved the panel across workspaces outside the placement path; when its
  cached workspace went stale or a dispatch was rejected, the panel was left
  wherever Hyprland had dropped it, on screen, until some later event
  happened to re-park it.

### How they were fixed

- **Reverted to the `pin` rule** for workspace-following (the 0.2.1 model):
  Hyprland itself carries the panel to every workspace, and sidetab only ever
  resizes and positions it. The known cosmetic cost of `pin` — Hyprland can
  clamp the parked panel on screen for a frame when switching to an *empty*
  workspace — is handled the way 0.2.1 handled it, with an immediate park
  plus a delayed re-park 200ms later.
- **Removed the re-anchoring machinery** (`ensure_anchored`, the monitor
  added/removed and focused-monitor event handlers, `movetoworkspacesilent`).
  The panel's placement now has exactly one writer, `place`, and it never
  moves the window between workspaces.
- **The never-learned-address failure now heals itself.** A 15-second
  watchdog adopts the window if the 5-second startup discovery missed it, and
  `sidetab ping` does the same repair on demand — adoption pins, de-chromes
  and parks the window, which is precisely what a center-of-screen stray
  needs.

### Added

- **A Restart button** in Settings → About (and a `sidetab restart` command).
  Kills the running daemon and starts a fresh one — settings are kept. The
  escape hatch for any state no automatic repair covers.

### Known trade-offs

- 0.2.2's monitor-unplug recovery is gone with the revert: after a screen
  lock, suspend, or monitor swap, a panel stranded on an orphaned workspace
  no longer auto-migrates. `settings`, `search`, `toggle` and `show` still
  health-check the daemon and replace one that stopped answering, and the new
  Restart button / `sidetab restart` recovers everything else in one click.
  If stranding turns out to matter in practice, it should come back as a
  *re-pin* (unpin + move + pin) confined to the actual unplug event, not as a
  parallel placement path.

## 0.2.3

Install without compiling.

- **Prebuilt binaries.** `cargo install sidetab` builds around 1450 crates —
  gpui's dependency tree is enormous — for a binary that is identical for
  everyone. There are now two ways to skip that: `yay -S sidetab-bin` on Arch
  and Omarchy, or `cargo binstall sidetab` anywhere else. Both fetch the
  binary attached to this release. Building from source still works.
- The binaries are built against glibc 2.35 so they run on anything newer, and
  are tested on Omarchy 4 (Hyprland 0.56).

No changes to sidetab itself — 0.2.2's fixes are the current behaviour.

## 0.2.2

> **Retrospective (written for 0.2.4):** the two placement changes below — the
> self-managed workspace-following and the monitor re-anchoring — turned out
> to be a net regression. They traded 0.2.1's one cosmetic frame-glitch for
> constant flicker and a panel that could stray to the middle of the screen.
> Both were reverted in 0.2.4. The health check, daemon replacement, and
> socket timeouts from this release survived and are still in place.

### Fixed

- **The panel no longer disappears for good after a screen lock, suspend, or
  monitor power-cycle.** Removing an output makes Hyprland orphan that
  monitor's workspaces — it reports them as `monitor: -1` — and any window
  sitting on one goes with it. `pin` is no rescue: it only makes a window
  follow the active workspace of its *own* monitor, so a pinned panel on an
  orphan is pinned to nothing and stays invisible and unfocusable forever.
  Nothing looked broken from the outside — the daemon, its event reader and
  its cursor poll all kept running, quietly moving a window nobody could see —
  and only restarting the daemon brought it back. sidetab now watches the
  monitor events (it dropped them entirely before) and moves its window back
  onto a live workspace, so this recovers on its own.
- **No more flicker when switching workspaces.** The panel used to be pinned so
  that it followed you between workspaces, but Hyprland re-homes a pinned
  window onto the new workspace and clamps it back inside the monitor — so the
  parked panel snapped fully on screen for a frame or two on every switch,
  before sidetab could shove it off again. It now follows workspaces by moving
  itself, batched with the placement so it lands in one frame and never appears
  on screen. (A `pin` left over from an older sidetab is cleared automatically.)
- The settings window failing to open is reported instead of silently doing
  nothing, and a stale handle from a closed window no longer suppresses
  re-opening it.

### Added

- **`sidetab ping`** health-checks the daemon: `pong ok`, `pong repaired` if it
  found and fixed a stranded panel, or `pong lost`. The reply is produced on
  the UI thread rather than the socket thread, so it reflects whether sidetab
  is actually running — not merely that the process exists.
- `settings`, `search`, `toggle` and `show` run that check first and replace a
  daemon that has genuinely stopped responding. The Alt-Tab commands
  deliberately skip it: they run on every keypress. A forced replacement drops
  the session-only "Pinned" window list.

### Internal

- The Hyprland command socket has read/write timeouts. It is called inline from
  the UI thread, so a half-open socket across a suspend used to park the whole
  loop indefinitely.
- The control socket has a read timeout too — a client that connected without
  writing previously wedged every later command, and with it every shortcut.

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
