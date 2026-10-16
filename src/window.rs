// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Window lifecycle, ported from rill-ed window.zig.

use wayland_client::{Dispatch, Proxy, QueueHandle};

use crate::actions::PointerAction;
use crate::app::AppData;
use crate::layout::{self, common};
use crate::river::river_window_v1::{self, RiverWindowV1};
use crate::types::{
    ContentBounds, PendingWindow, Status, Window, WindowGeom, WindowRule, WorkspaceTarget,
};
use crate::wm::WindowManager;

/// Handle a river_window_v1 event. Returns true when a manage sequence
/// should be requested.
pub fn window_event(
    state: &mut AppData,
    river_window: &RiverWindowV1,
    event: river_window_v1::Event,
    qh: &QueueHandle<AppData>,
) -> bool {
    match &event {
        river_window_v1::Event::Title { title: Some(title) } => {
            set_pending_string(
                &mut state.wm,
                river_window,
                title.clone(),
                PendingField::Title,
            );
        }
        river_window_v1::Event::AppId {
            app_id: Some(app_id),
        } => {
            set_pending_string(
                &mut state.wm,
                river_window,
                app_id.clone(),
                PendingField::AppId,
            );
        }
        // Sent once when the window object is created, i.e. before the first
        // `dimensions` maps it, so the pending phase has to keep the count.
        river_window_v1::Event::CaptureSessions { count } => {
            for pending in &mut state.wm.pending_windows {
                if &pending.river_window == river_window {
                    pending.capture_sessions = *count;
                    break;
                }
            }
        }
        // The client's own limits arrive before `dimensions` maps it. Without
        // them the layout would hand a window with a minimum width a slot
        // narrower than it can render.
        river_window_v1::Event::DimensionsHint {
            min_width,
            min_height,
            max_width,
            max_height,
        } => {
            for pending in &mut state.wm.pending_windows {
                if &pending.river_window == river_window {
                    pending.bounds = ContentBounds {
                        min_width: *min_width,
                        max_width: *max_width,
                        min_height: *min_height,
                        max_height: *max_height,
                    };
                    break;
                }
            }
        }
        // A window with a parent is a dialog or similar and is floated, like
        // niri. River sends this with the rest of the pending state; a parent
        // set after the window is mapped is ignored (rules are map-time too).
        river_window_v1::Event::Parent { parent } => {
            for pending in &mut state.wm.pending_windows {
                if &pending.river_window == river_window {
                    pending.has_parent = parent.is_some();
                    break;
                }
            }
        }
        _ => {}
    }

    if let river_window_v1::Event::Closed = event {
        // Closed before ever getting dimensions: drop the pending window.
        if let Some(idx) = state
            .wm
            .pending_windows
            .iter()
            .position(|p| &p.river_window == river_window)
        {
            state.wm.pending_windows.swap_remove(idx);
            river_window.destroy();
            return false;
        }
    }

    if let river_window_v1::Event::Dimensions { .. } = event
        && let Some((output_idx, _ws_idx)) = state.wm.current_ws_idx()
        && let Some(idx) = state
            .wm
            .pending_windows
            .iter()
            .position(|p| &p.river_window == river_window)
    {
        let pending = state.wm.pending_windows.swap_remove(idx);
        add_window(&mut state.wm, pending, output_idx, qh);

        layout::update(&mut state.wm);
        state.wm.status = Status::Layout;
        return true;
    }

    for output_idx in 0..state.wm.outputs.len() {
        for workspace_idx in 0..10 {
            let Some(focused_idx) = state
                .wm
                .workspace(output_idx, workspace_idx)
                .and_then(|ws| ws.focused_window_idx)
            else {
                continue;
            };
            let Some(win_idx) = state
                .wm
                .workspace(output_idx, workspace_idx)
                .unwrap()
                .window_list
                .iter()
                .position(|w| &w.river_window == river_window)
            else {
                continue;
            };

            match event {
                river_window_v1::Event::Closed => {
                    let workspace = state.wm.workspace_mut(output_idx, workspace_idx).unwrap();
                    if workspace.window_list.len() == 1 {
                        workspace.focused_window_idx = None;
                    } else if win_idx <= focused_idx && focused_idx != 0 {
                        workspace.focused_window_idx = Some(focused_idx - 1);
                    }
                    workspace.window_list.remove(win_idx);
                    if state.wm.last_focused_window.as_ref() == Some(river_window) {
                        state.wm.last_focused_window = None;
                    }
                    if state.wm.lock_focus.as_ref() == Some(river_window) {
                        state.wm.lock_focus = None;
                    }
                    river_window.destroy();
                    // Keep the overview grid consistent when a window is
                    // closed while the overview is showing it (may cancel
                    // the overview and refocus the pre-overview workspace).
                    crate::overview::prune(state);
                }
                river_window_v1::Event::FullscreenRequested { .. } => {
                    state
                        .wm
                        .workspace_mut(output_idx, workspace_idx)
                        .unwrap()
                        .window_list[win_idx]
                        .geom
                        .is_fullscreen = true;
                }
                river_window_v1::Event::ExitFullscreenRequested => {
                    state
                        .wm
                        .workspace_mut(output_idx, workspace_idx)
                        .unwrap()
                        .window_list[win_idx]
                        .geom
                        .is_fullscreen = false;
                }
                // While a window is being captured (screencast, window share)
                // the layout stops proposing new dimensions so the stream does
                // not rescale; positions still follow the layout.
                river_window_v1::Event::CaptureSessions { count } => {
                    let geom = &mut state
                        .wm
                        .workspace_mut(output_idx, workspace_idx)
                        .unwrap()
                        .window_list[win_idx]
                        .geom;
                    geom.capture_sessions = count;
                    if count == 0 {
                        // The window kept its size for the whole capture, so
                        // the layout target still has to reach it.
                        geom.forget_sent();
                    }
                }
                // The window asked for an interactive move/resize with the
                // pointer or a touch point (client-side titlebar drag, an
                // xdg_toplevel.resize grip).
                river_window_v1::Event::PointerMoveRequested { seat } => {
                    crate::seat::pointer_op_requested(
                        state,
                        river_window,
                        &seat,
                        PointerAction::MoveWindow,
                    );
                    return false;
                }
                river_window_v1::Event::PointerResizeRequested { seat, .. } => {
                    crate::seat::pointer_op_requested(
                        state,
                        river_window,
                        &seat,
                        PointerAction::ResizeWindow,
                    );
                    return false;
                }
                river_window_v1::Event::TouchMoveRequested { seat, touch_point } => {
                    crate::seat::touch_op_requested(
                        state,
                        river_window,
                        &seat,
                        touch_point,
                        PointerAction::MoveWindow,
                    );
                    return false;
                }
                river_window_v1::Event::TouchResizeRequested {
                    seat, touch_point, ..
                } => {
                    crate::seat::touch_op_requested(
                        state,
                        river_window,
                        &seat,
                        touch_point,
                        PointerAction::ResizeWindow,
                    );
                    return false;
                }
                _ => return false,
            }
            layout::update(&mut state.wm);
            state.wm.status = if state.wm.overview_state.is_some() {
                Status::Overview
            } else {
                Status::Layout
            };
            return false;
        }
    }

    // The window may belong to a detached (removed) output. A closed event
    // for a detached window must still be honored, otherwise the proxy ends
    // up in a restored workspace as a dead window.
    if let river_window_v1::Event::Closed = event {
        for detached in state.wm.detached_outputs.values_mut() {
            for workspace in &mut detached.workspace_list {
                let Some(focused_idx) = workspace.focused_window_idx else {
                    continue;
                };
                let Some(win_idx) = workspace
                    .window_list
                    .iter()
                    .position(|w| &w.river_window == river_window)
                else {
                    continue;
                };

                if workspace.window_list.len() == 1 {
                    workspace.focused_window_idx = None;
                } else if win_idx <= focused_idx && focused_idx != 0 {
                    workspace.focused_window_idx = Some(focused_idx - 1);
                }
                workspace.window_list.remove(win_idx);
                if state.wm.last_focused_window.as_ref() == Some(river_window) {
                    state.wm.last_focused_window = None;
                }
                if state.wm.lock_focus.as_ref() == Some(river_window) {
                    state.wm.lock_focus = None;
                }
                river_window.destroy();
                return false;
            }
        }
    }

    false
}

enum PendingField {
    Title,
    AppId,
}

fn set_pending_string(
    wm: &mut WindowManager,
    river_window: &RiverWindowV1,
    value: String,
    field: PendingField,
) {
    for pending in &mut wm.pending_windows {
        if &pending.river_window != river_window {
            continue;
        }
        match field {
            PendingField::Title => pending.title = Some(value),
            PendingField::AppId => pending.app_id = Some(value),
        }
        return;
    }
}

/// Move a pending window into the workspace its rules select, or the focused
/// one (rill-ed `add`). A rule naming an output that is not attached opens on
/// the focused output instead.
fn add_window(
    wm: &mut WindowManager,
    pending: PendingWindow,
    focused_output_idx: usize,
    qh: &QueueHandle<AppData>,
) {
    let config = wm.config.clone();
    // For each rule field the first matching rule that sets it wins; `floating`
    // is an any-match so a later rule cannot un-float a window.
    let matched: Vec<&WindowRule> = config
        .window_rules
        .iter()
        .filter(|rule| rule.matches(pending.app_id.as_deref(), pending.title.as_deref()))
        .collect();
    // A named workspace also pins its own output, below an explicit
    // `open_on_output` rule.
    let (name_output, target_slot) = match matched
        .iter()
        .find_map(|rule| rule.open_on_workspace.as_ref())
    {
        Some(WorkspaceTarget::Number(number)) => (None, Some(number - 1)),
        Some(WorkspaceTarget::Name(name)) => config
            .workspace_slot(name)
            .map_or((None, None), |(output, slot)| (output, Some(slot))),
        None => (None, None),
    };
    let output_name = matched
        .iter()
        .find_map(|rule| rule.open_on_output.as_deref())
        .or(name_output);
    let output_idx = output_name
        .and_then(|name| {
            wm.outputs
                .iter()
                .position(|o| o.name.as_deref() == Some(name))
        })
        .unwrap_or(focused_output_idx);
    let ws_idx = target_slot.unwrap_or(wm.outputs[output_idx].focused_workspace_idx);
    let width = matched
        .iter()
        .find_map(|rule| rule.default_window_width)
        .unwrap_or(config.default_window_width);

    let output = &mut wm.outputs[output_idx];
    let workspace = &mut output.workspace_list[ws_idx];

    // A new window landing here means the user is actively using this
    // workspace; clear former_output_name so previously-migrated windows
    // stay on this output instead of jumping back on reconnection.
    for w in &mut workspace.window_list {
        w.geom.former_output_name = None;
    }

    let is_floating = matched.iter().any(|rule| rule.floating) || pending.has_parent;

    // New windows start just off the right edge (or at the centered floating
    // rect) so the first manage pass moves them into place.
    let rightmost = common::initial_rectangle(output.non_exclusive, &config);
    let floating_rect = if is_floating {
        common::center_rectangle(output.non_exclusive, &config, width)
    } else {
        rightmost
    };

    let mut start_rect = rightmost;
    start_rect.x = output.non_exclusive.x + output.non_exclusive.width;

    let river_node = pending.river_window.get_node(qh, ());
    let mut geom = WindowGeom::new(width, start_rect);
    geom.is_floating = is_floating;
    geom.floating = floating_rect;
    geom.capture_sessions = pending.capture_sessions;
    // The client's hint, overridden by rules where they say so.
    geom.bounds = pending.bounds;
    if let Some(min) = matched.iter().find_map(|rule| rule.min_width) {
        geom.bounds.min_width = min;
    }
    if let Some(max) = matched.iter().find_map(|rule| rule.max_width) {
        geom.bounds.max_width = max;
    }
    let window = Window {
        river_window: pending.river_window,
        river_node,
        geom,
    };

    let window_idx = workspace.focused_window_idx.map_or(0, |idx| idx + 1);
    workspace.window_list.insert(window_idx, window);
    workspace.focused_window_idx = Some(window_idx);
}

impl Dispatch<RiverWindowV1, ()> for AppData {
    fn event(
        state: &mut Self,
        proxy: &RiverWindowV1,
        event: <RiverWindowV1 as Proxy>::Event,
        _data: &(),
        _conn: &wayland_client::Connection,
        qh: &QueueHandle<Self>,
    ) {
        let needs_manage = window_event(state, proxy, event, qh);
        if needs_manage {
            state.manage_dirty();
        }
    }
}
