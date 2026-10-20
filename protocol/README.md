<!--
SPDX-FileCopyrightText: © 2026 Julian Andrews
SPDX-License-Identifier: AGPL-3.0-or-later
-->

# Vendored river protocols

These XML files are vendored unmodified from the river compositor
(<https://codeberg.org/river/river>, MIT, © Isaac Freund). They are consumed at
build time by `wayland-scanner` (see `src/river.rs`), so changing them changes
the generated client API and can require handler updates in `src/`.

The vendored state matches river `main` commit
[`fd5ea7f`](https://codeberg.org/river/river/commit/fd5ea7fe823cda6db7411871cac600a0ebd3b154):

| File | Interface versions | Upstream drift |
|---|---|---|
| `river-window-management-v1.xml` | v6 | none |
| `river-xkb-bindings-v1.xml` | v3 | none |
| `river-layer-shell-v1.xml` | v1 | none |

Only the window management protocol changed: v6 adds interactive touch move and
resize events/requests. The xkb bindings and layer shell protocols still match
the v0.4.8 release byte-for-byte. Because v6 is newer than the v0.4.8 release,
tailrace requires River `main` or a later release advertising
`river_window_manager_v1` v6.

Newer additions are vendored and used: window capture-session counts, dimension
bounds, v6 touch operations, client-requested interactive move/resize,
capabilities and the clip box. Still deliberately unused: the content clip box
(river positions the border frame at the node origin, so a clip that cuts the
left or top edge offsets it; `set_clip_box` is used instead), xkb modifier
watching (`river_xkb_bindings_seat_v1.modifiers_watch`), output capture-session
counts, `set_presentation_mode`, and the xdg-shell requests tailrace ignores
(maximize, minimize, window menu, `use_csd`, decoration surfaces).
`src/window.rs`, `src/seat.rs`, and `src/output.rs` carry explicit no-op arms
where the generated event enums require them.

Checked against river `main` on 2025-10-20 (0.6.0-dev, commit `13752403`):
`protocol/update.sh main` is a no-op, all three files byte-identical. The
protocol-relevant work since `fd5ea7f` is compositor-side only —
ext-transient-seat-v1 (`b2c2b6a7`, `49b1e7de`, `3a7ac1c6`),
river-virtual-keyboard-v1 (`bfda604a`), and the virtual-pointer seat hint and
output mapping fixes (`869c64a9`, `831ad21d`) — none of which changes
`river-window-management-v1` (still v6) or `river-xkb-bindings-v1` (still v3).
What a window manager has to do about them is handle seats that appear and
disappear at runtime; see `docs/architecture.md`.

Verified by blob equality rather than by date, e.g. for the window management
protocol:

```sh
git hash-object protocol/river-window-management-v1.xml
# 2194b09a9fb1bb9421b7a5aa369406c49150dc0a
git -C <river clone> log --all --find-object=2194b09a9fb1bb9421b7a5aa369406c49150dc0a
# the main commit carrying the v6 protocol
```

## Refreshing

```sh
protocol/update.sh          # upstream main
protocol/update.sh <ref>    # any river branch or commit
```

Then build and test: a protocol change is an API change for the code
implementing it, so expect `src/` follow-up work (and check whether the new
revision is still understood by the river build being run).
