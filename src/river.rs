// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: AGPL-3.0-or-later

pub extern crate wayland_client;
pub use wayland_client::protocol::*;

mod interfaces {
    pub(super) mod tailrace {
        pub use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocol/river-window-management-v1.xml");
    }

    pub(super) mod rxkb {
        use super::tailrace::*;
        wayland_scanner::generate_interfaces!("./protocol/river-xkb-bindings-v1.xml");
    }

    pub(super) mod rls {
        use super::tailrace::*;
        wayland_scanner::generate_interfaces!("./protocol/river-layer-shell-v1.xml");
    }

    pub(super) mod wlr_layer {
        pub use wayland_client::protocol::__interfaces::*;
        // `get_popup` takes an xdg_popup, so the interface and its C
        // descriptor have to be in scope even though tailrace never calls it.
        pub use wayland_protocols::xdg::shell::client::__interfaces::{
            XDG_POPUP_INTERFACE, xdg_popup_interface,
        };
        wayland_scanner::generate_interfaces!("./protocol/wlr-layer-shell-unstable-v1.xml");
    }
}

use wayland_protocols::xdg::shell::client::xdg_popup;

use self::interfaces::rls::*;
use self::interfaces::rxkb::*;
use self::interfaces::tailrace::*;
use self::interfaces::wlr_layer::*;
wayland_scanner::generate_client_code!("./protocol/river-window-management-v1.xml");
wayland_scanner::generate_client_code!("./protocol/river-xkb-bindings-v1.xml");
wayland_scanner::generate_client_code!("./protocol/river-layer-shell-v1.xml");
wayland_scanner::generate_client_code!("./protocol/wlr-layer-shell-unstable-v1.xml");
