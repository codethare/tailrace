// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: 0BSD

//! The most-recently-used window list and the actions that read it: the MRU
//! order is what `Alt+Tab` cycling and the control socket's window list mean.
//!
//! The list has exactly one writer, `layout::apply_focus_and_borders`, so these
//! drive real focus changes through the harness rather than poking the list.

use super::*;
use crate::actions::KeybindingAction;

fn dispatch(s: &mut Session, sv: &mut MiniServer, action: KeybindingAction) {
    crate::keybinding::dispatch_action(&mut s.state, &action);
    s.manage(sv);
}

/// One output, a seat, and `count` mapped windows with stable identifiers; the
/// last one is focused. Returns the server-side objects of those windows.
fn scene(count: usize) -> (Session, MiniServer, Vec<ObjectId>) {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    let windows = (0..count)
        .map(|n| window(&mut s, &mut sv, &format!("id-{n}")))
        .collect();
    (s, sv, windows)
}

/// Map one window with identifier `id` and app_id `app`.
fn window(s: &mut Session, sv: &mut MiniServer, id: &str) -> ObjectId {
    let win = s.add_pending_window(sv);
    for (opcode, value) in [(EVT_WIN_IDENTIFIER, id), (EVT_WIN_APP_ID, "foot")] {
        s.send(
            sv,
            win.clone(),
            opcode,
            vec![Argument::Str(Some(Box::new(CString::new(value).unwrap())))],
        );
    }
    s.map_window(sv, &win);
    s.manage(sv);
    win
}

fn mru(s: &Session) -> Vec<String> {
    s.state.wm.mru.clone()
}

fn focused_id(s: &Session) -> Option<String> {
    s.state
        .wm
        .focused_window()
        .and_then(|w| w.identifier.clone())
}

/// The list is focus order: the front is the window that has focus now.
#[test]
fn the_mru_list_is_focus_order() {
    let (s, _sv, _windows) = scene(3);
    assert_eq!(mru(&s), vec!["id-2", "id-1", "id-0"]);
    assert_eq!(focused_id(&s), Some("id-2".to_string()));
}

/// `focus_window_last` returns to the window used before this one, even when it
/// lives on another workspace: that is what the per-workspace
/// `focus_window_previous` cannot do.
#[test]
fn focus_window_last_crosses_workspaces() {
    let (mut s, mut sv, _windows) = scene(1);
    dispatch(
        &mut s,
        &mut sv,
        KeybindingAction::MoveWindowToWorkspaceNumber(2),
    );
    assert_eq!(s.state.wm.outputs[0].focused_workspace_idx, 1);

    // A second window on workspace 1 takes the focus, and the MRU remembers
    // the window on workspace 2.
    window(&mut s, &mut sv, "second");
    assert_eq!(mru(&s), vec!["second", "id-0"]);

    dispatch(&mut s, &mut sv, KeybindingAction::FocusWindowLast);
    assert_eq!(focused_id(&s), Some("id-0".to_string()));
    assert_eq!(
        s.state.wm.outputs[0].focused_workspace_idx, 1,
        "focus follows the window to its workspace"
    );
    assert_eq!(mru(&s), vec!["id-0", "second"], "and it becomes the front");
}

/// Cycling walks the frozen order once per press, wrapping, instead of
/// bouncing between the two most recent windows.
#[test]
fn cycle_focus_walks_the_mru_order() {
    let (mut s, mut sv, _windows) = scene(3);
    assert_eq!(mru(&s), vec!["id-2", "id-1", "id-0"]);

    dispatch(&mut s, &mut sv, KeybindingAction::CycleFocusNext);
    assert_eq!(focused_id(&s), Some("id-1".to_string()));
    dispatch(&mut s, &mut sv, KeybindingAction::CycleFocusNext);
    assert_eq!(focused_id(&s), Some("id-0".to_string()));
    dispatch(&mut s, &mut sv, KeybindingAction::CycleFocusNext);
    assert_eq!(focused_id(&s), Some("id-2".to_string()), "wraps around");

    // A fresh cycle going backwards starts at the oldest window.
    let (mut s, mut sv, _windows) = scene(3);
    dispatch(&mut s, &mut sv, KeybindingAction::CycleFocusPrev);
    assert_eq!(focused_id(&s), Some("id-0".to_string()));
}

/// Any other action ends the cycle, so the next press starts from the current
/// order instead of resuming a snapshot taken before the user did something
/// else.
#[test]
fn another_action_ends_the_cycle() {
    let (mut s, mut sv, _windows) = scene(3);
    dispatch(&mut s, &mut sv, KeybindingAction::CycleFocusNext);
    assert!(s.state.wm.mru_cycle.is_some());

    dispatch(&mut s, &mut sv, KeybindingAction::FocusWindowRight);
    assert!(s.state.wm.mru_cycle.is_none());
}

/// A closed window leaves the list, and it cannot be reached afterwards.
#[test]
fn closing_a_window_prunes_the_mru() {
    let (mut s, mut sv, windows) = scene(2);
    assert_eq!(mru(&s), vec!["id-1", "id-0"]);

    s.send(&mut sv, windows[1].clone(), EVT_WIN_CLOSED, vec![]);
    s.manage(&mut sv);
    assert_eq!(mru(&s), vec!["id-0"]);

    dispatch(&mut s, &mut sv, KeybindingAction::FocusWindowLast);
    assert_eq!(
        focused_id(&s),
        Some("id-0".to_string()),
        "the only window left keeps the focus"
    );
}
