// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: 0BSD

//! Seats that appear and disappear while tailrace runs. river creates them at
//! runtime for remote-desktop and VM sessions (ext-transient-seat-v1), hands
//! each one to the window manager through the normal `seat` event, and can
//! take them away again.
//!
//! The rules under test: a seat that arrives later never takes over the first
//! one's role, every seat gets its own bindings and its own keyboard focus,
//! and a seat going away leaves the others untouched.

use super::*;

/// Objects of `interface` that the client has not destroyed, in creation
/// order. Bindings are rebuilt per seat, so the raw child list keeps the dead
/// ones too.
fn live(server: &MiniServer, interface: &str) -> Vec<ObjectId> {
    children_with_interface(server, interface)
        .into_iter()
        .filter(|object| count_requests(server, object, REQ_BINDING_DESTROY) == 0)
        .collect()
}

/// One output, one window, and two seats with their own wl_seat: the first is
/// the physical seat, the second a transient one.
fn two_seats() -> (Session, MiniServer, ObjectId, ObjectId) {
    let (mut s, mut sv) = build();
    let primary = s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_wl_seat(&mut sv, primary.clone());
    s.add_window(&mut sv);
    s.manage(&mut sv);

    let transient = s.add_seat(&mut sv);
    s.manage(&mut sv);
    let global = sv.add_wl_seat_global();
    s.add_wl_seat_global(&mut sv, transient.clone(), global);
    s.manage(&mut sv);
    (s, sv, primary, transient)
}

/// Every seat gets a full set of bindings of its own: a key or a drag on the
/// remote session's seat must do the same thing as on the physical one.
#[test]
fn every_seat_gets_its_own_bindings() {
    let (mut s, sv, primary, transient) = two_seats();
    assert_eq!(s.state.seats.len(), 2);

    for seat in 0..2 {
        assert!(
            s.state.seats[seat].xkb_bindings.len() > 50,
            "seat {seat} has no keybindings"
        );
        assert_eq!(s.state.seats[seat].pointer_bindings.len(), 2);
        assert!(
            s.state.seats[seat].wl_seat.is_some(),
            "seat {seat} did not bind its wl_seat"
        );
        assert!(
            s.state.seats[seat].layer_shell_seat.is_some(),
            "seat {seat} has no layer-shell seat"
        );
    }

    // Every binding object belongs to exactly one seat.
    let bindings = live(&sv, "river_xkb_binding_v1");
    assert_eq!(
        bindings.len(),
        s.state.seats[0].xkb_bindings.len() + s.state.seats[1].xkb_bindings.len(),
        "no binding is shared between the seats"
    );
    let mut unique: Vec<ObjectId> = Vec::new();
    for binding in &bindings {
        if !unique.contains(binding) {
            unique.push(binding.clone());
        }
    }
    assert_eq!(unique.len(), bindings.len());
    assert_eq!(live(&sv, "river_pointer_binding_v1").len(), 4);
    assert_ne!(primary, transient);
    s.check_consistent();
}

/// Keyboard focus is per seat in river, so every seat has to be told which
/// window to focus. A shared cache would leave the second seat on whatever it
/// had.
#[test]
fn a_window_is_focused_on_every_seat() {
    let (mut s, mut sv, primary, transient) = two_seats();
    // Mirrors layer_shell_seat_event(FocusNone): a stale cache must force a
    // refocus pass on every seat, not only on the one that changed.
    for seat_state in &mut s.state.seats {
        seat_state.last_focused_window = None;
    }
    s.state.wm.status = crate::types::Status::Layout;
    sv.clear_request_log();
    s.manage(&mut sv);

    for seat in [&primary, &transient] {
        assert_eq!(
            count_requests(&sv, seat, REQ_SEAT_FOCUS_WINDOW),
            1,
            "each seat gets the focused window"
        );
    }
}

/// A drag from the transient seat starts the operation on that seat: a pointer
/// operation lives on the seat whose device moved.
#[test]
fn a_drag_from_a_second_seat_uses_that_seat() {
    let (mut s, mut sv, primary, transient) = two_seats();
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleWorkspaceFloating,
    );
    s.manage(&mut sv);

    // Live pointer bindings are rebuilt per seat: 0/1 primary, 2/3 transient.
    let move_binding = live(&sv, "river_pointer_binding_v1")[2].clone();
    sv.clear_request_log();
    s.send(&mut sv, move_binding, EVT_PTR_BINDING_PRESSED, vec![]);
    s.manage(&mut sv);

    assert!(matches!(
        s.state.wm.status,
        crate::types::Status::PointerAction { .. }
    ));
    assert_eq!(
        count_requests(&sv, &transient, REQ_SEAT_OP_START_POINTER),
        1,
        "the op runs on the seat that started it"
    );
    assert_eq!(
        count_requests(&sv, &primary, REQ_SEAT_OP_START_POINTER),
        0,
        "and not on the other seat"
    );

    // Releasing ends it.
    s.send(&mut sv, transient.clone(), EVT_SEAT_OP_RELEASE, vec![]);
    assert_eq!(s.state.wm.status, crate::types::Status::None);
}

/// One operation at a time: a second seat pressing a move binding while a drag
/// runs must not take over the status.
#[test]
fn a_second_drag_is_ignored_while_one_is_running() {
    let (mut s, mut sv, _primary, _transient) = two_seats();
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleWorkspaceFloating,
    );
    s.manage(&mut sv);

    let bindings = live(&sv, "river_pointer_binding_v1");
    s.send(
        &mut sv,
        bindings[0].clone(),
        EVT_PTR_BINDING_PRESSED,
        vec![],
    );
    s.manage(&mut sv);
    let started = s.state.wm.status.clone();
    assert!(matches!(
        started,
        crate::types::Status::PointerAction { .. }
    ));

    s.send(
        &mut sv,
        bindings[2].clone(),
        EVT_PTR_BINDING_PRESSED,
        vec![],
    );
    s.manage(&mut sv);
    assert_eq!(s.state.wm.status, started, "the running op owns the status");
}

/// A transient seat going away must not take the physical seat's bindings with
/// it, and the physical seat must still respond afterwards. This is the bug a
/// single-seat cleanup produced: it destroyed everything and cleared the state
/// for the seat that was still there.
#[test]
fn removing_a_transient_seat_keeps_the_primary_working() {
    let (mut s, mut sv, primary, transient) = two_seats();

    s.send(&mut sv, transient.clone(), EVT_SEAT_REMOVED, vec![]);
    s.manage(&mut sv);

    assert_eq!(s.state.seats.len(), 1, "only the transient seat went away");
    assert!(
        s.state.seats[0].xkb_bindings.len() > 50,
        "the physical seat keeps its keybindings"
    );
    assert_eq!(
        count_requests(&sv, &transient, REQ_SEAT_DESTROY),
        1,
        "the removed seat object is destroyed"
    );
    assert_eq!(
        count_requests(&sv, &primary, REQ_SEAT_DESTROY),
        0,
        "the physical seat was not destroyed"
    );
    // Only the primary seat's bindings are still live.
    let still_live =
        live(&sv, "river_xkb_binding_v1").len() + live(&sv, "river_pointer_binding_v1").len();
    assert_eq!(
        still_live,
        s.state.seats[0].xkb_bindings.len() + 2,
        "only the primary's bindings are live"
    );

    // Keybindings still work on the surviving seat.
    let close = crate::keybinding::default_keybindings()
        .iter()
        .position(|kb| kb.action == crate::actions::KeybindingAction::CloseWindow)
        .expect("close_window has a default binding");
    let binding = live(&sv, "river_xkb_binding_v1")[close].clone();
    s.send(&mut sv, binding, EVT_XKB_BINDING_PRESSED, vec![]);
    assert!(
        s.state.wm.outputs[0].workspace_list[0].window_list[0]
            .geom
            .is_closing,
        "the binding still closes a window"
    );
    s.check_consistent();
}

/// A seat arriving after the last one went away is adopted normally.
#[test]
fn the_last_seat_going_away_is_replaced_by_the_next_one() {
    let (mut s, mut sv, primary, _transient) = two_seats();
    s.send(&mut sv, primary.clone(), EVT_SEAT_REMOVED, vec![]);
    s.manage(&mut sv);
    assert_eq!(s.state.seats.len(), 1, "the transient seat survives");

    let next = s.add_seat(&mut sv);
    s.manage(&mut sv);
    assert_eq!(s.state.seats.len(), 2);
    assert!(
        s.state.seats[1].xkb_bindings.len() > 50,
        "the seat that arrives after the physical one still gets bindings"
    );

    // Removing it really removes the seat this object stands for.
    s.send(&mut sv, next, EVT_SEAT_REMOVED, vec![]);
    assert_eq!(s.state.seats.len(), 1);
    s.check_consistent();
}

/// Hovering a window on any seat follows focus when sloppy focus is on: the
/// pointer may be the virtual one a remote session drives.
#[test]
fn pointer_enter_from_a_second_seat_follows_focus() {
    let (mut s, mut sv) = build();
    let primary = s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_wl_seat(&mut sv, primary);
    let first = s.add_window(&mut sv);
    s.manage(&mut sv);
    let transient = s.add_seat(&mut sv);
    s.manage(&mut sv);
    let global = sv.add_wl_seat_global();
    s.add_wl_seat_global(&mut sv, transient.clone(), global);
    s.manage(&mut sv);

    s.config_mut().focus_follows_pointer = true;
    s.add_window(&mut sv);
    s.manage(&mut sv);
    assert_eq!(
        s.state.wm.outputs[0].workspace_list[0].focused_window_idx,
        Some(1),
        "the window added last is focused"
    );

    s.send(
        &mut sv,
        transient,
        EVT_SEAT_POINTER_ENTER,
        vec![Argument::Object(first)],
    );
    s.manage(&mut sv);
    assert_eq!(
        s.state.wm.outputs[0].workspace_list[0].focused_window_idx,
        Some(0),
        "hovering the other window on the second seat moves focus to it"
    );
}

/// A seat removed mid-drag takes the operation with it: river stops sending
/// deltas, and the next manage pass must not ask a destroyed seat to start
/// one.
#[test]
fn removing_a_seat_mid_operation_ends_the_operation() {
    let (mut s, mut sv, _primary, transient) = two_seats();
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleWorkspaceFloating,
    );
    s.manage(&mut sv);

    let move_binding = live(&sv, "river_pointer_binding_v1")[2].clone();
    s.send(&mut sv, move_binding, EVT_PTR_BINDING_PRESSED, vec![]);
    s.manage(&mut sv);
    assert!(matches!(
        s.state.wm.status,
        crate::types::Status::PointerAction { .. }
    ));

    s.send(&mut sv, transient, EVT_SEAT_REMOVED, vec![]);
    assert_eq!(
        s.state.wm.status,
        crate::types::Status::Layout,
        "the drag died with its seat"
    );
    // The removal also forces a refocus pass, which keeps one more sequence in
    // Layout before it settles.
    s.manage(&mut sv);
    s.manage(&mut sv);
    assert_eq!(s.state.wm.status, crate::types::Status::None);
    s.check_consistent();
}

/// A seat arriving mid-drag must not abort the drag. Rebuilding bindings takes
/// the manage sequence, and taking it while an operation owns the status ends
/// the operation: the window stopped following the pointer.
#[test]
fn a_seat_arriving_mid_drag_does_not_abort_the_drag() {
    let (mut s, mut sv, primary, _transient) = two_seats();
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleWorkspaceFloating,
    );
    s.manage(&mut sv);

    let move_binding = live(&sv, "river_pointer_binding_v1")[0].clone();
    s.send(&mut sv, move_binding, EVT_PTR_BINDING_PRESSED, vec![]);
    s.manage(&mut sv);
    let dragging = s.state.wm.status.clone();
    assert!(matches!(
        dragging,
        crate::types::Status::PointerAction { .. }
    ));

    // A remote session connects while the button is still down.
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    assert_eq!(
        s.state.wm.status, dragging,
        "the drag keeps the sequence it owns"
    );

    // Releasing ends the drag as usual, and the deferred rebuild then happens.
    s.send(&mut sv, primary, EVT_SEAT_OP_RELEASE, vec![]);
    assert_eq!(s.state.wm.status, crate::types::Status::None);
    s.manage(&mut sv);
    assert!(
        s.state.seats[1].xkb_bindings.len() > 50,
        "the seat that arrived during the drag gets its bindings afterwards"
    );
    s.check_consistent();
}

/// The overview keeps working when bindings are rebuilt while it is open: its
/// bare keys must not be disabled under it.
#[test]
fn an_open_overview_survives_a_seat_arrival() {
    let (mut s, mut sv, _primary, _transient) = two_seats();
    s.add_window(&mut sv);
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::EnterOverview,
    );
    s.manage(&mut sv);
    assert!(s.state.wm.overview_state.is_some(), "overview is open");

    s.add_seat(&mut sv);
    s.manage(&mut sv);
    assert!(
        s.state.wm.overview_state.is_some(),
        "the overview is still open"
    );
    let disabled = live(&sv, "river_xkb_binding_v1")
        .into_iter()
        .filter(|binding| count_requests(&sv, binding, REQ_XKB_DISABLE) > 0)
        .count();
    assert_eq!(
        disabled, 0,
        "no overview key may be disabled while it is open"
    );

    // And the bare keys really do something: nav-right moves the highlight to
    // the second of the two windows (they share one grid row).
    let index = crate::keybinding::default_keybindings()
        .iter()
        .position(|kb| kb.action == crate::actions::KeybindingAction::OverviewNavRight)
        .expect("nav-right has a default binding");
    let binding = live(&sv, "river_xkb_binding_v1")[index].clone();
    s.send(&mut sv, binding, EVT_XKB_BINDING_PRESSED, vec![]);
    s.manage(&mut sv);
    assert_eq!(
        s.state
            .wm
            .overview_state
            .as_ref()
            .expect("overview open")
            .highlighted,
        1,
        "the key still navigates the overview"
    );
}
