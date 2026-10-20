<!--
SPDX-FileCopyrightText: © 2026 Julian Andrews
SPDX-License-Identifier: AGPL-3.0-or-later
-->

# Architecture

tailrace is a window manager *client*: river owns input, rendering and the
protocol, and tailrace decides where windows go and which one has focus. This
document is the map for changing it; `README.md` covers user-facing behaviour.

## Where things live

| File | Contents |
|---|---|
| `main.rs` | Arguments, the Wayland connection and the event loop (a `poll` on the compositor fd and the control socket) |
| `ctl.rs` | Control socket: window snapshot, the `focus` verb, socket permissions and the uid check |
| `app.rs` | `AppData` (protocol globals + binding lists), registry binds, `manage()` |
| `wm.rs`, `types.rs` | Derived state: outputs, workspaces, windows, status, config |
| `window.rs`, `output.rs`, `seat.rs` | Event handlers for the three river global objects |
| `layout/` | Geometry: `update` (targets), `apply`/`snap_to_finish` (commit), `focus.rs` (borders, focus, raise), `migrate.rs` (output hotplug) |
| `keybinding/` | Default tables (`defaults.rs`), parsing/setup (`keybinding.rs`), action dispatch (`dispatch.rs`) |
| `overview.rs` | Grid overlay |
| `hotplug/` | The test harness and all scenario tests (see below) |
| `protocol/` | Vendored river protocol XMLs; provenance in `protocol/README.md` |

## The manage cycle

river drives everything: it sends events, then a `manage_start`, and tailrace
answers with requests and finally `manage_finish`. `app.rs::manage()` runs the
state machine once per sequence:

| `Status` | What the sequence does |
|---|---|
| `SetupBindings` | Create xkb and pointer bindings from the config, then go to `Layout` |
| `Layout` | `layout::apply` (init pending windows, hotplug, then `update` + focus/borders) and `layout::snap_to_finish` (commit geometry), then `None` |
| `Overview` | `overview::apply_borders` + `snap_to_finish`, then `None` |
| `PointerAction` | `op_start_pointer` and follow the drag in `seat::op_action`; ends on `OpRelease` |
| `TouchAction` | `op_start_touch` for the requesting touch point, then `seat::op_action`; ends on `op_release_touch` (or `op_cancel_touch`, which restores the geometry) |
| `Exit` | `river_window_manager_v1.exit_session` |
| `None` | `seat.op_end()` |

Anything that changes state sets `status = Layout` (or calls
`manage_dirty()`) so the next sequence commits it. `render_start` is answered
with `render_finish`.

## Layout pipeline

1. `layout::update` recomputes every window's `geom.finish` from the workspace
   layout (`layout::scroller`), clamping floats into the output. The slot width
   comes from `proportion`, then clamps to the window's content bounds
   (`dimensions_hint`, overridden by `min_width`/`max_width` rules), so a client
   that cannot shrink keeps a wider column and its neighbours move over.
   Fullscreen wins over everything: the focused one is handed to the compositor
   (`river_window_v1.fullscreen`) which then owns its geometry and clipping.
   Maximize-to-edges covers `non_exclusive` without widening the slot, so its
   neighbours stay put underneath it.
2. `layout::apply` handles pending windows, removed outputs (migrate or
   detach) and restored outputs, then `apply_focus_and_borders`.
3. `layout::snap_to_finish` sets `current = finish` and issues the protocol
   requests through `layout::common::place_window`: `propose_dimensions` with
   `set_dimension_bounds` (skipped while `geom.capture_sessions > 0`, so a
   screencast does not rescale), `set_position`, `show`/`hide` and
   `set_clip_box`, each deduplicated against the `sent_*` caches on
   `WindowGeom`. `raise_floating_windows` then raises every float above the
   tiles, the focused float last, using `place_above` for all but the first.

Focus is derived state: `focused_output_idx` → `focused_workspace_idx` →
`focused_window_idx`. `apply_focus_and_borders` is the only place that talks
to river about focus, and it only does so for a seat whose
`last_focused_window` differs from the target — every seat keeps its own cache
because river tracks keyboard focus per seat. Focus commands are suppressed for
a seat while a layer-shell surface holds exclusive focus on it, and for all
seats while the session is locked; `needs_refocus` makes the next sequence
retry.

Interactive move/resize ops (pointer bindings, and the window's own
`pointer_*_requested` / `touch_*_requested` events in `window.rs`) only target
floating, non-fullscreen windows: `seat::begin_op` is the single place that
starts one, and `seat::op_action` re-sends the op window's geometry on every
manage sequence of the operation.

## The test harness (`src/hotplug/`)

`hotplug.rs` is an in-process fake compositor built on `wayland-backend`'s
server side, wired to the real client code over a socketpair:

- `MiniServer` advertises the river globals (and `wl_seat`, and
  `wp_cursor_shape_manager_v1`), answers requests, and records every request
  as `(object, opcode, args)` — that log is what most tests assert on.
- `Session` owns the same `AppData` the binary uses, so scenarios drive the
  real `Dispatch` implementations, the real `manage()` and the real layout.

A scenario is: `build()`, add a seat and outputs, add windows, send the events
under test, call `manage()`, then assert on `Session.state` or on the request
log. Helpers: `add_output`, `add_pending_window` + `map_window`,
`add_window`, `add_wl_seat`, `manage`, `config_mut`, plus `children_with_interface`,
`count_requests` and `requests_for` for assertions. Each scenario module
(`outputs.rs`, `seats.rs`, `pointer.rs`, `windows.rs`, `manage.rs`,
`overview.rs`, `keybindings.rs`, `cursor.rs`, `rules.rs`, `mru.rs`, `ctl.rs`)
covers one area.

Limits: there is no compositor, so requests are recorded rather than executed,
nothing is rendered, and protocol errors only surface as panics inside the
backend. Passing tests prove what tailrace *asks for*, not what river does.

## Traps worth knowing

- **The control socket is the second fd in the loop.** `main` polls the
  compositor and the listener together; the listener is non-blocking and every
  connection is one-shot (read a bounded line, write once, close), because a
  slow client must never stall input. Verbs are a whitelist in
  `ctl::handle_line` — `focus` reuses `keybinding::focus_window_id`, so the
  socket cannot reach `dispatch_action` and therefore cannot spawn anything.
  MRU order is maintained by `layout::apply_focus_and_borders` alone; the
  socket and the `cycle_focus_*` actions only read it.
- **Seats are per seat, not global.** river creates seats at runtime
  (ext-transient-seat-v1: a remote-desktop or VM session), so `AppData.seats`
  holds one `SeatState` per `river_seat_v1` — its bindings, `wl_seat`,
  `layer_shell_seat`, layer-shell focus and focus cache. Seat state is looked
  up by proxy identity, never by river's seat name (names are assigned at
  creation and can be handed out again). A pointer operation carries its seat
  in `Status`, because `op_start_pointer`/`op_end` are requests on that seat;
  `seat_removed` must destroy only that seat's objects, or a transient seat
  going away takes the physical seat's bindings with it.
- **State pokes need a status.** Setting fields and calling `manage()` does
  nothing unless `status == Status::Layout`: the focus gates and the layout
  pass only run from there. A test asserting "no request was sent" after a
  bare poke proves nothing — that bug was already fixed once in
  `manage.rs::exclusive_layer_shell_focus_skips_focus_commands`.
- **Raising needs a Layout pass too.** `apply_focus_and_borders` (focus,
  borders, `place_top`) and `raise_floating_windows` are only called from
  `layout::apply`, so changing `focused_window_idx` directly and managing with
  `Status::None` sends no ordering requests at all.
- **Windows are mapped on `Dimensions`.** `window.rs` moves a pending window
  into a workspace when its `dimensions` event arrives, which is also where
  `window_rules` are evaluated; river sends `app_id`/`title` before
  `dimensions`, and the harness mirrors that with `add_pending_window` +
  `map_window`.
- **Some requests must not be assigned as status.** A seat event that wants
  binding setup sets `needs_setup_bindings` instead of `status`, because any
  output or window event in the same batch would clobber the status and the
  bindings would never be created.
- **`on_overflow` is latched, not recomputed.** `layout::update` retakes the
  centering decision only when `focused_window_idx` moves, keeping the result
  in `Workspace::on_overflow_center` and the window it came from in
  `previous_focused_window_idx`. Re-deciding on a later pass (an added window,
  an output event, a no-op action) reads "no previous focus" and drags the
  window back out of center.
- **Centering and previous-focus state are maintained by `layout::update`.**
  `center_latch` (niri's `center_column`) and `last_focused_window`
  (`focus_window_previous`) are cleared or retaken only in the pass that sees
  `focused_window_idx` differ from `previous_focused_window_idx`. A dispatch
  arm that changes the focus must not touch them: the state means "where the
  focus came from", and the arm does not know that.
- **rill-ed is the reference implementation.** When behaviour looks wrong,
  compare with `src/` in rill-ed (`animation.zig` ↔ `snap_to_finish`,
  `layout.zig` ↔ `layout/`); two bugs in tailrace were single dropped lines from
  it, and one deliberate divergence (fullscreen floats) is documented in the
  code.
- **Show/hide is decided on the content rect, not the slot.** River clips the
  window and its borders to the clip box, so a slot that peeks out by less
  than the border width still renders its border band at the output edge with
  no content under it ("a border line like the first window detached"). A
  window whose content is entirely off the output must therefore be hidden
  (`layout/common.rs::place_window`).
- **Clip with `set_clip_box`, not `set_content_clip_box`.** River sizes the
  compositor border frame from the content-clip intersection but positions it
  from the node origin, so a content clip that cuts the left or top edge draws
  the frame shifted into the window. `set_clip_box` clips the border boxes
  themselves instead, which stays aligned.
- **Sticky is an invariant, not a switch hook.** `layout::update` starts with
  `WindowManager::pull_sticky_windows`, which moves every `geom.is_sticky`
  window onto its output's focused workspace. It runs after the overview early
  return (the overview grid must not be rearranged) and appends, so the target
  workspace keeps its focus; only a target with no focus takes the sticky
  window that was focused before the switch. Every path that changes
  `focused_workspace_idx` (keybindings, overview select/cancel, previous
  workspace, lock restore) stays unaware of stickiness.
- **A workspace with windows can have no focused window.** That is what a
  sticky switch onto an empty workspace produces (the window follows, the
  focus does not), and it must not freeze the layout: `scroller::apply`
  anchors the strip on the first tiled window when there is nothing to focus
  — the same path a focused *floating* window already took — so every window
  still gets a `finish`. The direction keys enter that strip from the end they
  point at (`FocusWindowLeft` → last window, `FocusWindowRight` → first),
  including the `…OrOutput` variants, which do not fall through to another
  output from a workspace that has no focus.
- **Protocol updates** are a `protocol/update.sh <ref>` plus a bind version
  bump in `app.rs`; see `protocol/README.md` for the current revision and what
  is still unused.
