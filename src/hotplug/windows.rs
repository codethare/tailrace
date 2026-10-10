// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: 0BSD

//! Window event scenarios: fullscreen requests, close handling and the
//! fullscreen geometry sent to the compositor.

use super::*;

/// Regression: a fullscreen window fills its output rect exactly, so any
/// window border (and the -border clip offset) would be drawn 3px beyond the
/// output onto a neighboring monitor's adjoining edge ("the edge of A facing
/// B shows the edge of B's fullscreen window"). Fullscreen must send border
/// width 0 and a
/// zero-origin clip box.
#[test]
fn fullscreen_window_sends_no_border_and_zero_origin_clip() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    // A at origin, B to the right (adjoining edge = A's right / B's left).
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_output(&mut sv, "B", (1920, 0), (1920, 1080));
    s.manage(&mut sv);
    s.state.wm.focused_output_idx = Some(0);
    s.manage(&mut sv);
    s.add_window(&mut sv); // window on A
    s.manage(&mut sv);
    s.state.wm.focused_output_idx = Some(1);
    s.manage(&mut sv);
    let b_win = s.add_window(&mut sv); // window to be fullscreened
    s.manage(&mut sv);
    s.state.wm.outputs[1].workspace_list[0].focused_window_idx = Some(0);
    s.manage(&mut sv);

    sv.clear_request_log();
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleFullscreen,
    );
    s.manage(&mut sv);

    for (_obj, op, args) in sv.requests_for(&b_win) {
        if op == 8 {
            // set_borders(edges, width, r, g, b, a)
            let width: i32 = args
                .split(',')
                .nth(1)
                .unwrap()
                .trim_start_matches('i')
                .parse()
                .unwrap();
            assert_eq!(
                width, 0,
                "fullscreen window must not set a border, got {args}"
            );
        }
        if op == REQ_WIN_SET_CONTENT_CLIP_BOX {
            // set_content_clip_box(x, y, w, h)
            let v: Vec<i32> = args
                .split(',')
                .map(|a| a.trim_start_matches('i').parse().unwrap())
                .collect();
            assert_eq!(
                (v[0], v[1]),
                (0, 0),
                "fullscreen clip box must be zero-origin, got {args}"
            );
        }
    }
}

/// The `fullscreen` requests (opcode 19) sent for a window, as raw logged args.
fn fullscreen_requests(server: &MiniServer, window: &ObjectId) -> Vec<String> {
    server
        .requests_for(window)
        .into_iter()
        .filter(|(_, op, _)| *op == 19)
        .map(|(_, _, args)| args)
        .collect()
}

/// window.rs: a client-side fullscreen request fills the window's output and
/// the inverse request restores the tiled layout.
#[test]
fn window_fullscreen_requests_toggle_and_fill_output() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    let out = s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    let win = s.add_window(&mut sv);
    s.manage(&mut sv);

    s.send(
        &mut sv,
        win.clone(),
        EVT_WIN_FULLSCREEN_REQUESTED,
        vec![Argument::Object(out.clone())],
    );
    s.manage(&mut sv);
    let output_rect = s.state.wm.outputs[0].rectangle;
    let geom = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;
    assert!(
        s.state.wm.outputs[0].workspace_list[0].window_list[0]
            .geom
            .is_fullscreen,
        "fullscreen_requested must set the flag"
    );
    assert!(
        geom.eql(output_rect),
        "fullscreen window must fill its output: {geom:?} vs {output_rect:?}"
    );
    // The compositor owns the geometry and clipping of a fullscreen window
    // (river-window-management-v1.fullscreen); rill-ed requests it for the
    // focused one and the port had dropped that line.
    let reqs = fullscreen_requests(&sv, &win);
    assert_eq!(
        reqs.len(),
        1,
        "the focused fullscreen window must be handed to the compositor: {reqs:?}"
    );
    assert!(
        reqs[0].contains(&format!("{out:?}")),
        "fullscreen must target the window's own output: {reqs:?}"
    );

    sv.clear_request_log();
    s.send(
        &mut sv,
        win.clone(),
        EVT_WIN_EXIT_FULLSCREEN_REQUESTED,
        vec![],
    );
    s.manage(&mut sv);
    assert!(
        !s.state.wm.outputs[0].workspace_list[0].window_list[0]
            .geom
            .is_fullscreen,
        "exit_fullscreen_requested must clear the flag"
    );
    assert!(
        fullscreen_requests(&sv, &win).is_empty(),
        "a window that left fullscreen must not be re-handed to the compositor"
    );
}

/// window.rs/layout: a fullscreen window that is *not* focused stays on the
/// WM's own off-screen workspace stacking and is never handed to the
/// compositor — otherwise the compositor would paint it over the focused
/// workspace.
#[test]
fn unfocused_fullscreen_window_is_not_handed_to_the_compositor() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    let out_a = s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    let win_a = s.add_window(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "B", (1920, 0), (1920, 1080));
    s.manage(&mut sv);
    let win_b = s.add_window(&mut sv);
    s.manage(&mut sv);
    s.check_consistent();

    // Fullscreen A's window while it is focused: it goes to the compositor.
    s.state.wm.focused_output_idx = Some(0);
    s.manage(&mut sv);
    s.send(
        &mut sv,
        win_a.clone(),
        EVT_WIN_FULLSCREEN_REQUESTED,
        vec![Argument::Object(out_a)],
    );
    s.manage(&mut sv);
    let reqs = fullscreen_requests(&sv, &win_a);
    assert_eq!(reqs.len(), 1, "focused fullscreen goes to the compositor");

    // Focus moves to B: the fullscreen window on A must not be handed over.
    sv.clear_request_log();
    s.state.wm.focused_output_idx = Some(1);
    s.manage(&mut sv);
    let reqs = fullscreen_requests(&sv, &win_a);
    assert!(
        reqs.is_empty(),
        "unfocused fullscreen must not be handed to the compositor: {reqs:?}"
    );
    let _ = win_b;
}

/// window.rs: closing windows keeps the workspace focus index valid — the
/// off-by-one class that used to panic in window moves.
#[test]
fn closing_windows_keeps_focus_index_valid() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);
    let w1 = s.add_window(&mut sv);
    s.manage(&mut sv);
    let w2 = s.add_window(&mut sv);
    s.manage(&mut sv);
    s.check_consistent();

    // Focus the middle window, close it: focus shifts to the one before it.
    s.state.wm.outputs[0].workspace_list[0].focused_window_idx = Some(1);
    s.manage(&mut sv);
    s.send(&mut sv, w1, EVT_WIN_CLOSED, vec![]);
    s.manage(&mut sv);
    assert_eq!(
        s.state.wm.outputs[0].workspace_list[0].focused_window_idx,
        Some(0),
        "closing the focused middle window moves focus down"
    );
    assert_eq!(s.state.wm.outputs[0].workspace_list[0].window_list.len(), 2);
    s.check_consistent();

    // Closing a window after the focused one leaves focus alone.
    s.send(&mut sv, w2, EVT_WIN_CLOSED, vec![]);
    s.manage(&mut sv);
    assert_eq!(
        s.state.wm.outputs[0].workspace_list[0].focused_window_idx,
        Some(0),
        "closing a later window must not move focus"
    );
    s.check_consistent();
}

/// A floating window that goes fullscreen fills its output. The compositor
/// only owns the focused fullscreen window, so the WM's own geometry has to
/// do it for the unfocused one — rill-ed leaves such a window at its rest
/// rect here.
#[test]
fn floating_window_fullscreen_fills_the_output() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);

    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleWorkspaceFloating,
    );
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleFullscreen,
    );
    s.manage(&mut sv);

    let geom = &s.state.wm.outputs[0].workspace_list[0].window_list[0].geom;
    let output_rect = s.state.wm.outputs[0].rectangle;
    assert!(geom.is_floating && geom.is_fullscreen);
    assert!(
        geom.current.eql(output_rect),
        "fullscreen floating window must fill the output: {:?} vs {:?}",
        geom.current,
        output_rect
    );
    s.check_consistent();
}

/// floating windows are raised above tiled ones: apply_focus_and_borders
/// raises the focused window, then raise_floating_windows raises every float,
/// so the last place_top must belong to the floating window.
#[test]
fn floating_windows_raise_above_tiled_ones() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv); // window 0, node 0
    s.manage(&mut sv);
    s.add_window(&mut sv); // window 1, node 1, focused
    s.manage(&mut sv);

    // Float the focused window 1, then focus the tiled window 0.
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleWorkspaceFloating,
    );
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::FocusWindowLeft,
    );
    s.manage(&mut sv);

    let nodes = children_with_interface(&sv, "river_node_v1");
    assert_eq!(nodes.len(), 2, "one node per window");
    assert!(
        !s.state.wm.outputs[0].workspace_list[0].window_list[0]
            .geom
            .is_floating,
        "window 0 is the tiled one"
    );
    assert!(
        s.state.wm.outputs[0].workspace_list[0].window_list[1]
            .geom
            .is_floating,
        "window 1 is the floating one"
    );

    let raised: Vec<ObjectId> = sv
        .request_log
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, op, _)| *op == REQ_NODE_PLACE_TOP)
        .filter(|(object, _, _)| nodes.contains(object))
        .map(|(object, _, _)| object.clone())
        .collect();
    assert_eq!(
        raised.last(),
        Some(&nodes[1]),
        "the floating window must be the last one raised, got {raised:?}"
    );
}

/// Ordering requests (place_top / place_above) seen for the given nodes.
fn ordering_requests(server: &MiniServer, nodes: &[ObjectId]) -> Vec<(ObjectId, u16, String)> {
    server
        .request_log
        .lock()
        .unwrap()
        .iter()
        .filter(|(object, op, _)| {
            nodes.contains(object) && (*op == REQ_NODE_PLACE_TOP || *op == REQ_NODE_PLACE_ABOVE)
        })
        .cloned()
        .collect()
}

/// Windows are told the capabilities tailrace honours: fullscreen yes,
/// maximize/minimize/window menu no (river tells clients everything is
/// supported when the request is never made, which produces dead buttons).
#[test]
fn new_windows_are_told_the_supported_capabilities() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    // Capabilities are sent while the window is still pending (the "tell the
    // client what we are" pass), so drive the pending flow explicitly.
    let win = s.add_pending_window(&mut sv);
    s.manage(&mut sv);
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);

    let caps: Vec<String> = sv
        .requests_for(&win)
        .into_iter()
        .filter(|(_, op, _)| *op == REQ_WIN_SET_CAPABILITIES)
        .map(|(_, _, args)| args)
        .collect();
    // capabilities::fullscreen = 4
    assert_eq!(caps, vec!["u4".to_string()], "fullscreen only");
}

/// The bounds logged for a window, in order.
fn dimension_bounds(server: &MiniServer, window: &ObjectId) -> Vec<String> {
    server
        .requests_for(window)
        .into_iter()
        .filter(|(_, op, _)| *op == REQ_WIN_SET_DIMENSION_BOUNDS)
        .map(|(_, _, args)| args)
        .collect()
}

/// A window is bounded to its slot so that a client which ignores
/// propose_dimensions still does not grow past it.
#[test]
fn windows_are_bounded_to_their_slot() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    let win = s.add_window(&mut sv);
    s.manage(&mut sv);

    let geom = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;
    assert_eq!(
        dimension_bounds(&sv, &win),
        vec![format!("i{},i{}", geom.width - 6, geom.height - 6)],
        "content size of the slot, borders excluded"
    );

    // A manage pass without a layout change must not repeat it.
    sv.clear_request_log();
    s.manage(&mut sv);
    assert!(dimension_bounds(&sv, &win).is_empty());

    // Shrinking the column re-bounds the window.
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::AdjustWindowWidth(-0.2),
    );
    s.manage(&mut sv);
    let geom = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;
    assert_eq!(
        dimension_bounds(&sv, &win),
        vec![format!("i{},i{}", geom.width - 6, geom.height - 6)]
    );
}

/// A window under screen capture keeps its size while the layout still moves
/// it: resizing a captured window rescales the stream.
#[test]
fn a_captured_window_keeps_its_size_while_the_layout_moves_it() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    let win = s.add_window(&mut sv);
    s.manage(&mut sv);

    s.send(
        &mut sv,
        win.clone(),
        EVT_WIN_CAPTURE_SESSIONS,
        vec![Argument::Uint(1)],
    );
    s.manage(&mut sv);
    assert_eq!(
        s.state.wm.outputs[0].workspace_list[0].window_list[0]
            .geom
            .capture_sessions,
        1
    );

    sv.clear_request_log();
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::AdjustWindowWidth(-0.2),
    );
    s.manage(&mut sv);
    assert_eq!(
        count_requests(&sv, &win, REQ_WIN_PROPOSE_DIMENSIONS),
        0,
        "a captured window must not be resized"
    );
    assert!(dimension_bounds(&sv, &win).is_empty());
    let geom = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;
    assert!(
        node_positions(&sv, geom.x + 3, geom.y + 3) > 0,
        "the captured window must still follow the layout"
    );

    // Once the capture ends, the pending size is proposed again.
    sv.clear_request_log();
    s.send(
        &mut sv,
        win.clone(),
        EVT_WIN_CAPTURE_SESSIONS,
        vec![Argument::Uint(0)],
    );
    s.manage(&mut sv);
    assert_eq!(count_requests(&sv, &win, REQ_WIN_PROPOSE_DIMENSIONS), 1);
    assert_eq!(dimension_bounds(&sv, &win).len(), 1);
}

/// Overlapping floats stack in focus order: focusing a float raises it above
/// the other floats (place_above moves only the one node that changes).
#[test]
fn the_focused_float_stacks_above_the_other_floats() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleWorkspaceFloating,
    );
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleWorkspaceFloating,
    );
    s.manage(&mut sv);

    // Both are floating; focus the first one directly (the layout, not the
    // keybinding, decides whether a float can be focused by direction) and run
    // a layout pass, which is what reorders the nodes.
    let nodes = children_with_interface(&sv, "river_node_v1");
    assert_eq!(nodes.len(), 2, "one node per window");
    assert!(
        s.state.wm.outputs[0].workspace_list[0]
            .window_list
            .iter()
            .all(|w| w.geom.is_floating)
    );
    s.state.wm.outputs[0].workspace_list[0].focused_window_idx = Some(0);
    s.state.wm.status = crate::types::Status::Layout;

    sv.clear_request_log();
    s.manage(&mut sv);
    let ordering = ordering_requests(&sv, &nodes);
    let last = ordering.last().expect("floats must be ordered");
    assert_eq!(
        last.0, nodes[0],
        "the focused float must be ordered last, got {ordering:?}"
    );
    assert_eq!(
        last.1, REQ_NODE_PLACE_ABOVE,
        "only the focused float moves, above the other one: {ordering:?}"
    );
    assert!(
        last.2.contains(&format!("{:?}", nodes[1])),
        "it must be placed above the other float: {last:?}"
    );
}

/// Regression: river draws the compositor borders around the intersection of
/// the window *content* (the slot minus the border inset) and the content clip
/// box, disables the content when that intersection is empty, and in that case
/// also leaves the border frame it computed last on screen. tailrace used to
/// decide visibility from the whole slot, so a window peeking out by less than
/// its border width was left shown: its content went away and the stale frame
/// stayed behind as a border outline with no window in it ("a border line like
/// the first window detached"). Such a window has to be hidden instead.
#[test]
fn window_showing_only_its_border_band_is_hidden() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);

    // A maximized window (focused while it is the only one) plus two half-width
    // ones.
    let w0 = s.add_window(&mut sv);
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::SetWindowWidth(1.0),
    );
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);

    sv.clear_request_log();
    s.add_window(&mut sv); // focus lands on the new last window
    s.manage(&mut sv);

    // The strip is … | -1901..1 | 10..956 | 965..1911 |: window 0 still
    // overlaps the output by one pixel, all of it border band (3px), so none
    // of its content is on screen.
    let geom = &s.state.wm.outputs[0].workspace_list[0].window_list[0].geom;
    assert_eq!((geom.current.x, geom.current.width), (-1901, 1902));

    let log = sv.requests_for(&w0);
    assert!(
        log.iter().any(|(_, op, _)| *op == REQ_WIN_HIDE),
        "a window with no content on screen must be hidden, not left as a frame"
    );
    assert!(
        !log.iter().any(|(_, op, _)| *op == REQ_WIN_SHOW),
        "it must not be shown again while only its border band is on screen"
    );
}
