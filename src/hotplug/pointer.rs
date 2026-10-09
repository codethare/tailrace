// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: 0BSD

//! Seat/pointer scenarios: focus-follows-pointer and the pointer binding drag
//! (move/resize) path.

use super::*;

/// Focus-follows-pointer: hover refocuses exactly like clicking (sloppy
/// focus) when `focus_follows_pointer` is enabled; with the toggle off,
/// hovering must not move focus.
#[test]
fn pointer_enter_focus_follows_toggle() {
    let (mut s, mut sv) = build();
    let seat = s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "eDP-1", (0, 0), (1360, 768));
    s.manage(&mut sv);
    let a = s.add_window(&mut sv);
    s.manage(&mut sv);
    let b = s.add_window(&mut sv);
    s.manage(&mut sv);
    s.check_consistent();

    let ws = &s.state.wm.outputs[0].workspace_list[0];
    let initial = ws.focused_window_idx.expect("window has focus");
    // Server-side id for the event payload, client-side proxy for the check.
    let (hover, hover_proxy) = if initial == 0 {
        (b, ws.window_list[1].river_window.clone())
    } else {
        (a, ws.window_list[0].river_window.clone())
    };

    // Toggle off: hover changes nothing.
    s.config_mut().focus_follows_pointer = false;
    s.send(
        &mut sv,
        seat.clone(),
        EVT_SEAT_POINTER_ENTER,
        vec![Argument::Object(hover.clone())],
    );
    s.manage(&mut sv);
    assert_eq!(
        s.state.wm.outputs[0].workspace_list[0].focused_window_idx,
        Some(initial),
        "hover must not move focus when focus_follows_pointer is off"
    );

    // Toggle on: hovering the other window refocuses it.
    s.config_mut().focus_follows_pointer = true;
    s.send(
        &mut sv,
        seat,
        EVT_SEAT_POINTER_ENTER,
        vec![Argument::Object(hover)],
    );
    s.manage(&mut sv);
    let ws = &s.state.wm.outputs[0].workspace_list[0];
    let now = ws.focused_window_idx.expect("window has focus");
    assert_ne!(now, initial, "hover must move focus when enabled");
    assert_eq!(
        ws.window_list[now].river_window, hover_proxy,
        "focused window must be the hovered one"
    );
}

/// seat.rs: a pointer binding press starts a drag, op_delta follows the
/// pointer (clamped to the output), op_release ends it. This is the
/// `move_window` path behind drag-to-move and side-button bindings.
#[test]
fn pointer_binding_drag_moves_floating_window() {
    let (mut s, mut sv) = build();
    let seat = s.add_seat(&mut sv);
    s.manage(&mut sv);
    // Bindings are only created once a focused output exists (manage returns
    // early before that), so capture them after the first output manage.
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    let binds = pointer_binding_objects(&sv);
    let move_binding = binds[0].clone();

    s.add_window(&mut sv);
    s.manage(&mut sv);
    s.check_consistent();

    // Drag only applies to floating windows (seat.rs press handler).
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleWorkspaceFloating,
    );
    s.manage(&mut sv);
    let origin = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;
    assert!(
        s.state.wm.outputs[0].workspace_list[0].window_list[0]
            .geom
            .is_floating,
        "window must be floating for the drag binding to engage"
    );

    s.send(&mut sv, move_binding, EVT_PTR_BINDING_PRESSED, vec![]);
    s.manage(&mut sv);
    assert!(
        matches!(
            s.state.wm.status,
            crate::types::Status::PointerAction(crate::actions::PointerAction::MoveWindow)
        ),
        "press must enter the move drag state, got {:?}",
        s.state.wm.status
    );

    // Compose the op delta: the window follows the pointer, clamped to the
    // output and pushed to the compositor.
    sv.clear_request_log();
    s.send(
        &mut sv,
        seat.clone(),
        EVT_SEAT_OP_DELTA,
        vec![Argument::Int(100), Argument::Int(50)],
    );
    s.manage(&mut sv);
    let moved = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;
    assert_eq!(
        (moved.x, moved.y),
        (origin.x + 100, origin.y + 50),
        "window must follow the pointer delta"
    );
    assert!(
        node_positions(&sv, moved.x + 3, moved.y + 3) > 0,
        "the dragged position must reach the compositor (border inset)"
    );

    s.send(&mut sv, seat, EVT_SEAT_OP_RELEASE, vec![]);
    assert_eq!(
        s.state.wm.status,
        crate::types::Status::None,
        "release ends the drag"
    );
    assert!(
        s.state.wm.outputs[0].workspace_list[0].window_list[0]
            .geom
            .drag_origin
            .is_none(),
        "drag origin must be cleared on release"
    );
    s.manage(&mut sv);
}

/// A removed seat takes its bindings with it: river asserts that the previous
/// bindings are gone before it hands out a seat object again, and the next
/// seat must get a fresh, working set.
#[test]
fn removed_seat_destroys_its_bindings_and_a_new_seat_gets_new_ones() {
    let (mut s, mut sv) = build();
    let seat = s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_wl_seat(&mut sv, seat.clone());
    s.add_window(&mut sv);
    s.manage(&mut sv);
    assert_eq!(s.state.pointer_bindings.len(), 2);
    assert!(s.state.river_seat.is_some());
    assert!(s.state.wl_seat.is_some());
    assert!(s.state.layer_shell_seat.is_some());

    let bindings: Vec<ObjectId> = children_with_interface(&sv, "river_xkb_binding_v1")
        .into_iter()
        .chain(children_with_interface(&sv, "river_pointer_binding_v1"))
        .collect();
    assert!(!bindings.is_empty());

    sv.clear_request_log();
    s.send(&mut sv, seat.clone(), EVT_SEAT_REMOVED, vec![]);
    assert!(
        s.state.river_seat.is_none(),
        "the stale seat proxy is dropped"
    );
    assert!(s.state.wl_seat.is_none());
    assert!(s.state.layer_shell_seat.is_none());
    assert!(s.state.xkb_bindings.is_empty());
    assert!(s.state.pointer_bindings.is_empty());
    assert_eq!(
        count_requests(&sv, &seat, REQ_SEAT_DESTROY),
        1,
        "the removed seat object is destroyed"
    );
    let destroyed = bindings
        .iter()
        .filter(|b| count_requests(&sv, b, REQ_BINDING_DESTROY) == 1)
        .count();
    assert_eq!(
        destroyed,
        bindings.len(),
        "every xkb and pointer binding is destroyed"
    );

    // A new seat re-establishes bindings.
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    assert!(s.state.river_seat.is_some());
    assert_eq!(s.state.pointer_bindings.len(), 2);
    assert!(s.state.xkb_bindings.len() > 50);
    s.check_consistent();
}

/// Focusing another output warps the pointer to its centre (rill-ed/niri
/// behaviour), so the cursor does not stay on a display the user just left.
#[test]
fn focusing_another_output_warps_the_pointer_to_its_centre() {
    let (mut s, mut sv) = build();
    let seat = s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "B", (1920, 0), (1280, 720));
    s.manage(&mut sv);
    assert_eq!(s.state.wm.focused_output_idx, Some(1), "focus starts on B");

    sv.clear_request_log();
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::FocusOutputLeft,
    );
    s.manage(&mut sv);
    assert_eq!(s.state.wm.focused_output_idx, Some(0));
    let warps: Vec<String> = sv
        .requests_for(&seat)
        .into_iter()
        .filter(|(_, op, _)| *op == REQ_SEAT_POINTER_WARP)
        .map(|(_, _, args)| args)
        .collect();
    assert_eq!(
        warps,
        vec!["i960,i540".to_string()],
        "one warp to the centre of output A"
    );

    // No further warp while the focus stays put.
    sv.clear_request_log();
    s.manage(&mut sv);
    assert!(
        !sv.requests_for(&seat)
            .iter()
            .any(|(_, op, _)| *op == REQ_SEAT_POINTER_WARP),
        "the warp must not repeat every manage cycle"
    );
}

/// The window itself asks for an interactive move (a client-side titlebar
/// drag). The request only takes effect for floating windows, and then runs
/// through the same op machinery as the `move_window` pointer binding.
#[test]
fn client_move_request_drags_a_floating_window() {
    let (mut s, mut sv) = build();
    let seat = s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    let window = s.add_window(&mut sv);
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleWorkspaceFloating,
    );
    s.manage(&mut sv);
    let origin = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;

    sv.clear_request_log();
    s.send(
        &mut sv,
        window,
        EVT_WIN_POINTER_MOVE_REQUESTED,
        vec![Argument::Object(seat.clone())],
    );
    s.manage(&mut sv);
    assert!(
        matches!(
            s.state.wm.status,
            crate::types::Status::PointerAction(crate::actions::PointerAction::MoveWindow)
        ),
        "a pointer move request must start the move drag, got {:?}",
        s.state.wm.status
    );
    assert_eq!(
        count_requests(&sv, &seat, REQ_SEAT_OP_START_POINTER),
        1,
        "the drag must ask river for the pointer operation"
    );

    s.send(
        &mut sv,
        seat.clone(),
        EVT_SEAT_OP_DELTA,
        vec![Argument::Int(30), Argument::Int(-20)],
    );
    s.manage(&mut sv);
    let moved = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;
    assert_eq!(
        (moved.x, moved.y),
        (origin.x + 30, origin.y - 20),
        "the window must follow the delta"
    );

    s.send(&mut sv, seat, EVT_SEAT_OP_RELEASE, vec![]);
    assert_eq!(s.state.wm.status, crate::types::Status::None);
    s.manage(&mut sv);
}

/// The window asks to be resized (an xdg_toplevel.resize edge grip).
/// `shortcut:` the requested edges are ignored, exactly like the
/// `resize_window` binding: the window grows to the bottom right.
#[test]
fn client_resize_request_resizes_a_floating_window() {
    let (mut s, mut sv) = build();
    let seat = s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    let window = s.add_window(&mut sv);
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleWorkspaceFloating,
    );
    s.manage(&mut sv);
    let origin = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;

    s.send(
        &mut sv,
        window,
        EVT_WIN_POINTER_RESIZE_REQUESTED,
        vec![Argument::Object(seat.clone()), Argument::Uint(5)],
    );
    s.manage(&mut sv);
    assert!(
        matches!(
            s.state.wm.status,
            crate::types::Status::PointerAction(crate::actions::PointerAction::ResizeWindow)
        ),
        "a pointer resize request must start the resize drag, got {:?}",
        s.state.wm.status
    );

    s.send(
        &mut sv,
        seat.clone(),
        EVT_SEAT_OP_DELTA,
        vec![Argument::Int(40), Argument::Int(20)],
    );
    s.manage(&mut sv);
    let resized = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;
    assert_eq!(
        (resized.width, resized.height),
        (origin.width + 40, origin.height + 20)
    );

    s.send(&mut sv, seat, EVT_SEAT_OP_RELEASE, vec![]);
    s.manage(&mut sv);
}

/// A touch point on a floating window moves it (river v6 interactive touch
/// operations); deltas for other touch points are ignored, and releasing the
/// point ends the operation.
#[test]
fn touch_move_request_drags_a_floating_window() {
    let (mut s, mut sv) = build();
    let seat = s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    let window = s.add_window(&mut sv);
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleWorkspaceFloating,
    );
    s.manage(&mut sv);
    let origin = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;

    s.send(
        &mut sv,
        window,
        EVT_WIN_TOUCH_MOVE_REQUESTED,
        vec![Argument::Object(seat.clone()), Argument::Int(7)],
    );
    s.manage(&mut sv);
    assert!(
        matches!(
            s.state.wm.status,
            crate::types::Status::TouchAction {
                action: crate::actions::PointerAction::MoveWindow,
                touch_point: 7
            }
        ),
        "a touch move request must start a touch op, got {:?}",
        s.state.wm.status
    );
    assert!(
        count_requests(&sv, &seat, REQ_SEAT_OP_START_TOUCH) > 0,
        "the op must be started with op_start_touch"
    );

    s.send(
        &mut sv,
        seat.clone(),
        EVT_SEAT_OP_DELTA_TOUCH,
        vec![Argument::Int(7), Argument::Int(25), Argument::Int(15)],
    );
    s.manage(&mut sv);
    let moved = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;
    assert_eq!(
        (moved.x, moved.y),
        (origin.x + 25, origin.y + 15),
        "the window must follow the touch point"
    );

    // A delta for a different touch point belongs to another operation.
    s.send(
        &mut sv,
        seat.clone(),
        EVT_SEAT_OP_DELTA_TOUCH,
        vec![Argument::Int(8), Argument::Int(400), Argument::Int(400)],
    );
    s.manage(&mut sv);
    let unchanged = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;
    assert!(unchanged.eql(moved), "foreign touch deltas are ignored");

    s.send(
        &mut sv,
        seat,
        EVT_SEAT_OP_RELEASE_TOUCH,
        vec![Argument::Int(7)],
    );
    assert_eq!(s.state.wm.status, crate::types::Status::None);
    assert!(
        s.state.wm.outputs[0].workspace_list[0].window_list[0]
            .geom
            .drag_origin
            .is_none()
    );
    s.manage(&mut sv);
}

/// A cancelled touch point may have been a palm all along, so the geometry
/// from before the operation is restored.
#[test]
fn touch_cancel_restores_the_geometry_from_before_the_op() {
    let (mut s, mut sv) = build();
    let seat = s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    let window = s.add_window(&mut sv);
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleWorkspaceFloating,
    );
    s.manage(&mut sv);
    let origin = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;

    s.send(
        &mut sv,
        window,
        EVT_WIN_TOUCH_MOVE_REQUESTED,
        vec![Argument::Object(seat.clone()), Argument::Int(3)],
    );
    s.manage(&mut sv);
    s.send(
        &mut sv,
        seat.clone(),
        EVT_SEAT_OP_DELTA_TOUCH,
        vec![Argument::Int(3), Argument::Int(60), Argument::Int(60)],
    );
    s.manage(&mut sv);
    assert!(
        !s.state.wm.outputs[0].workspace_list[0].window_list[0]
            .geom
            .current
            .eql(origin),
        "sanity: the touch moved the window"
    );

    s.send(
        &mut sv,
        seat,
        EVT_SEAT_OP_CANCEL_TOUCH,
        vec![Argument::Int(3)],
    );
    let restored = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;
    assert!(
        restored.eql(origin),
        "cancel must undo the move: {restored:?}"
    );
    assert_eq!(s.state.wm.status, crate::types::Status::None);
    s.manage(&mut sv);
}

/// Tiled windows are positioned by the layout, so interactive requests from
/// them are ignored instead of starting a drag that the layout would undo.
#[test]
fn tiled_windows_ignore_interactive_move_resize_and_touch_requests() {
    let (mut s, mut sv) = build();
    let seat = s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    let window = s.add_window(&mut sv);
    s.manage(&mut sv);
    s.check_consistent();

    let status = s.state.wm.status.clone();
    for (opcode, args) in [
        (
            EVT_WIN_POINTER_MOVE_REQUESTED,
            vec![Argument::Object(seat.clone())],
        ),
        (
            EVT_WIN_POINTER_RESIZE_REQUESTED,
            vec![Argument::Object(seat.clone()), Argument::Uint(1)],
        ),
        (
            EVT_WIN_TOUCH_MOVE_REQUESTED,
            vec![Argument::Object(seat.clone()), Argument::Int(1)],
        ),
        (
            EVT_WIN_TOUCH_RESIZE_REQUESTED,
            vec![
                Argument::Object(seat.clone()),
                Argument::Int(1),
                Argument::Uint(1),
            ],
        ),
    ] {
        s.send(&mut sv, window.clone(), opcode, args);
        s.manage(&mut sv);
        assert_eq!(
            s.state.wm.status, status,
            "a tiled window must not enter an op (opcode {opcode})"
        );
    }
    s.send(
        &mut sv,
        seat.clone(),
        EVT_SEAT_OP_DELTA_TOUCH,
        vec![Argument::Int(1), Argument::Int(20), Argument::Int(10)],
    );
    s.send(
        &mut sv,
        seat.clone(),
        EVT_SEAT_OP_RELEASE_TOUCH,
        vec![Argument::Int(1)],
    );
    s.send(
        &mut sv,
        seat.clone(),
        EVT_SEAT_OP_CANCEL_TOUCH,
        vec![Argument::Int(2)],
    );
    assert_eq!(s.state.wm.status, status);
    assert_eq!(count_requests(&sv, &seat, REQ_SEAT_OP_START_POINTER), 0);
    assert_eq!(count_requests(&sv, &seat, REQ_SEAT_OP_START_TOUCH), 0);
    assert!(
        s.state.wm.outputs[0].workspace_list[0].window_list[0]
            .geom
            .drag_origin
            .is_none()
    );
    s.manage(&mut sv);
    s.check_consistent();
}
