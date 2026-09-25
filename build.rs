// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: AGPL-3.0-or-later

fn main() {
    for protocol in [
        "river-window-management-v1.xml",
        "river-xkb-bindings-v1.xml",
        "river-layer-shell-v1.xml",
    ] {
        println!("cargo:rerun-if-changed=protocol/{protocol}");
    }
}
