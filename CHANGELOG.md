# Changelog

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
