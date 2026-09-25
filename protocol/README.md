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

Newer additions are vendored but deliberately unused: capture-session counts,
dimension bounds, touch operations, and xkb modifier watching. `src/window.rs`,
`src/seat.rs`, and `src/output.rs` carry explicit no-op arms where the generated
event enums require them.

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
