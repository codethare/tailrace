<!--
SPDX-FileCopyrightText: © 2026 Julian Andrews
SPDX-License-Identifier: AGPL-3.0-or-later
-->

# tailrace

Tiny scrolling window manager for [river](https://isaacfreund.com/software/river/),
implemented in Rust against the
[river-window-management-v1](https://isaacfreund.com/docs/wayland/river-window-management-v1/)
protocol. A port of [rill-ed](https://github.com/codethare/rill-ed) (Zig) with
**no animations** — layout changes commit instantly.

## Features

* Per-workspace scrolling column layout; individual windows can float
* Floating windows center on screen; drag with mouse or touch to move, resize with mouse, touch or keyboard
* Interactive move/resize requests from the window itself (a client-side titlebar drag, an `xdg_toplevel` resize grip) work on floating windows
* Windows under screen capture keep their size; the layout still moves them
* A window scrolled off the output is clipped at the edge with its borders drawn there
* Clients are told tailrace only supports fullscreen, so no dead maximize/minimize buttons
* 10 workspaces per output
* Overview mode: grid of all windows across outputs/workspaces, vi-key navigation
* Most-recently-used window switching (`Alt+Tab`), across outputs and workspaces
* Control socket for menus and scripts: `tailrace ctl windows` lists windows
  most recently used first, `tailrace ctl focus <id>` switches to one
* Live-reloading TOML config
* Multi-output with window migration (windows return to their output when it reappears)
* Multiple seats: a seat created at runtime for a remote-desktop or VM session
  (river's ext-transient-seat-v1) gets its own keybindings and keyboard focus,
  and going away leaves the physical seat working
* TTY switch resilience — workspaces survive output removal (laptop panel off, lock)
* Window rules by exact `app_id` / glob `title`: float, open on a given workspace or output, or open at a given width
* Dialogs (windows with a parent) float instead of taking a column
* A window's own minimum/maximum width is honoured, so a client is never squeezed into a slot it cannot render in
* Preset column widths, cycled with one key; center, expand and maximize-to-edges on demand
* Named workspaces, and moving a workspace to another output
* Focus follows the mouse pointer (opt-in, sloppy focus)
* Session-lock focus save/restore

## Building

Dependencies: Rust (stable), `libxkbcommon`, `wayland`, `wayland-protocols` (runtime).

```sh
cargo build --release
```

Contributors: [docs/architecture.md](docs/architecture.md) covers the manage
cycle, the layout pipeline and the test harness.

## Running

```
river -c ./target/release/tailrace
```

Requires a River build advertising `river_window_manager_v1` **v6** (currently
River `main`; the v0.4.8 release only provides v5). tailrace also binds
`river_xkb_bindings_v1` v3.

tailrace itself takes `-c/--config <path>` to read a specific config file,
`--check-config` to parse the config, report any error and exit without
starting, `--help` and `--version`.

## Control socket

A running tailrace listens on `$XDG_RUNTIME_DIR/tailrace/ctl.sock` — directory
0700, socket 0600, and the peer's uid is checked — so a menu or script can read
the window list and switch to one:

```sh
tailrace ctl windows     # one line per window, most recently used first
tailrace ctl focus <id>  # focus that window, wherever it lives
```

`windows` writes one tab-separated line per window:

```
id <TAB> title <TAB> app_id <TAB> output:workspace <TAB> flags
```

`-` stands for an empty field; flags are `focused`, `floating`, `fullscreen`,
`maximized`, `sticky`, `closing`, comma separated. `id` is river's window
identifier: unique and never reused, so a menu that holds an id cannot hit a
different window once the first one closes. Tabs and newlines inside a title are
replaced by spaces, so one window is always one line.

`focus` is the only write verb: there is no way to run a program or an arbitrary
action over the socket. An unknown id is an error, and focusing the window that
already has focus is not one.

A request is one line and is answered once; then the connection closes. tailrace
never waits for a client: an unfinished or oversized request is answered as far
as it goes and the connection is dropped.

## Configuration

Config is searched at, in order:

1. `$XDG_CONFIG_HOME/tailrace/config.toml`
2. `$HOME/.config/tailrace/config.toml`

If no file is found, the built-in defaults (identical to rill-ed's defaults,
minus animations) are used. A `[keybindings]` or `[[pointer_bindings]]` section
**replaces the built-in list as a whole** — bindings do not merge — so an empty
(or absent) section is how you keep the defaults. See
[config.example.toml](config.example.toml) for an annotated example and the
default key table below.

Unknown keys are rejected at parse time: a typo is reported rather than
silently ignored.

`Super+r` reloads the config; on parse errors the current config is kept and
the error is printed.

### Top-level options

| Option | Default | Meaning |
|---|---|---|
| `vertical_gap` | `9` | Gap between windows and the output's top/bottom edge |
| `horizontal_gap` | `9` | Gap between adjacent windows |
| `default_window_width` | `0.5` | Starting width of a new window, as a proportion of the output |
| `center_focused_window` | `"never"` | Center the focused window: `never`, `always`, `single`, `on_overflow` (when it does not fit next to the window the focus came from) |
| `always_center_single_column` | `false` | Center a workspace that holds a single window, whatever `center_focused_window` says |
| `preset_column_widths` | 1/3, 1/2, 2/3 | Widths `switch_preset_column_width` cycles through: `{ proportion = 0.5 }` of the output or `{ fixed = 1280 }` pixels |
| `focus_follows_pointer` | `false` | Keyboard focus follows the mouse pointer (sloppy focus) |
| `no_csd` | `true` | Disable client side decorations |
| `border.width` | `3` | Border width in pixels |
| `border.focused_color`, `border.unfocused_color` | | `{ r, g, b, a }` with 0–255 channels and 0.0–1.0 alpha |
| `cursor.theme`, `cursor.size` | | XCursor theme and size (omit for the default cursor) |
| `spawn_at_startup` | `[]` | Commands spawned detached at startup |
| `[[workspace]]` | `[]` | Names for workspaces, so bindings and rules can refer to them: `name`, `slot` (1-10), optional `output` |
| `window_rules` | `[]` | Match by `app_id` and/or glob `title`; matched windows float, open on a workspace or output, or open at a width |

Rule fields: `floating`, `open_on_workspace` (1-10 as in
`focus_workspace_number`, or a `[[workspace]]` name), `open_on_output` (a river
output name such as `HDMI-A-1`; one that is not attached falls back to the
focused output), `default_window_width` (a proportion of the output, as at the
top level) and `min_width` / `max_width` (content pixels, overriding the
client's own size hint). For each field the first matching rule that sets it
wins; `floating` is an any-match. A value outside 1-10 for a number
`open_on_workspace`, a workspace name nothing declares, or a `min_width`
greater than the `max_width` beside it is a parse error.

Rules are evaluated when a window is first mapped, so `Super+r` does not
re-evaluate them for already-open windows (matching rill-ed) — a window you
floated by hand stays floated.

### Default keybindings

| Keybinding | Action |
|---|---|
| `Super` `q` | Close window |
| `Super` `f` | Toggle fullscreen |
| `Super` `Shift` `f` | Maximize the focused window to the workspace edges (no gaps, no borders, no wider slot) |
| `Super` `w` / `Super` `Shift` `w` | Cycle the focused window's width through `preset_column_widths`, forwards / backwards |
| `Super` `Shift` `e` | Expand the focused window to the available width |
| `Super` `Shift` `c` | Center the focused window |
| `Super` `minus` / `equal` | Decrease / increase window width by 0.1 |
| `Super` `BackSpace` | Set window width to 0.5 |
| `Super` `Ctrl` `minus` / `equal` | Shrink / grow floating window |
| `Super` `Ctrl` `BackSpace` | Set floating window height to 0.5 |
| `Super` `Left` / `Right` | Focus window left / right (falls through to output focus at edges) |
| `Super` `Shift` `Left` / `Right` | Move window left / right |
| `Super` `Ctrl` `Left`/`Right`/`Up`/`Down` | Move floating window |
| `Super` `v` | Toggle window floating |
| `Super` `s` | Toggle sticky window |
| `Super` `Up` / `Down` | Focus workspace above / below |
| `Super` `` ` `` | Previous workspace |
| `Super` `1`–`0` | Focus workspace 1–10 |
| `Super` `Shift` `Up` / `Down` | Move window to workspace above / below |
| `Super` `Shift` `1`–`0` | Move window to workspace 1–10 |
| `Super` `h`/`j`/`k`/`l` | Focus output left/below/above/right |
| `Super` `comma` / `period` | Focus previous / next output |
| `Super` `Tab` | Focus the previously focused window |
| `Super` `Shift` `Tab` | Focus the most recently used window (any output, any workspace) |
| `Alt` `Tab` / `Alt` `Shift` `Tab` | Cycle through the most recently used windows |
| `Super` `Shift` `h`/`j`/`k`/`l` | Move window to output left/below/above/right |
| `Super` `Escape` | Exit river |
| `Super` `Space` | Enter overview |
| `Escape` / `Return` / `hjkl` | Overview: cancel / confirm / navigate (overview only) |
| `Super` `r` | Reload config |
| `Super` `t` | Spawn alacritty |
| `XF86Audio*` | Volume via `wpctl` |

`spawn` takes an argv list, not a shell command line —
`{ spawn = ["sh", "-c", "foo | bar"] }` is how you get pipes, variables or
quoting. `toggle_workspace_floating` (historical name, kept for compatibility
with rill-ed) toggles the focused *window*'s floating state.

`toggle_sticky_window` marks the focused window sticky: it follows the focused
workspace of its output, i3/kwm style. A sticky window keeps the focus when it
had it before the switch, and never takes the focus from a window already on
the target workspace. Un-marking leaves it where it is.

### Default pointer bindings

| Pointer binding | Action |
|---|---|
| `Super` `Left Click` | Move floating window |
| `Super` `Right Click` | Resize floating window |

`[[pointer_bindings]]` takes `button = "left" | "right" | "middle" | "side" | "extra"`
and any modifier list, including none. A bare button (empty `modifiers`) is how a
touchpad gesture that a remapper turns into a mouse button — or an extra mouse
button — drives `move_window` / `resize_window`.

## Differences from rill-ed

* No animations (no spring physics, no frame interpolation) — by design
* No kwim hotplug integration (Zig-specific input method)
* TOML config instead of ZON

## License

AGPL-3.0-or-later — see [LICENSE](LICENSE). Ported from rill-ed.
