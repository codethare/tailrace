// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: 0BSD

//! Keybinding dispatch: every action smoke-tested for panics and index
//! consistency, plus semantic checks for the ones carrying real logic.
//! Only three actions had any end-to-end coverage before this.

use super::*;
use crate::actions::KeybindingAction;

fn every_action() -> Vec<KeybindingAction> {
    use KeybindingAction as A;
    vec![
        A::CloseWindow,
        A::ToggleFullscreen,
        A::ToggleMaximizeColumn,
        A::AdjustWindowWidth(0.1),
        A::SetWindowWidth(0.5),
        A::AdjustFloatingWindowSize(0.1),
        A::SetFloatingWindowHeight(0.5),
        A::FocusWindowLeft,
        A::FocusWindowOrOutputLeft,
        A::FocusWindowRight,
        A::FocusWindowOrOutputRight,
        A::FocusWindowPrevious,
        A::FocusWindowLast,
        A::CycleFocusNext,
        A::CycleFocusPrev,
        A::MaximizeWindowToEdges,
        A::ExpandColumnToAvailableWidth,
        A::CenterColumn,
        A::SwitchPresetColumnWidth,
        A::SwitchPresetColumnWidthBack,
        A::FocusWorkspaceName("chat".into()),
        A::MoveWindowToWorkspaceName("chat".into()),
        A::FocusOutputNext,
        A::FocusOutputPrevious,
        A::MoveWorkspaceToOutputLeft,
        A::MoveWorkspaceToOutputRight,
        A::MoveWorkspaceToOutputAbove,
        A::MoveWorkspaceToOutputBelow,
        A::MoveWindowLeft,
        A::MoveWindowRight,
        A::MoveFloatingWindowLeft,
        A::MoveFloatingWindowRight,
        A::MoveFloatingWindowUp,
        A::MoveFloatingWindowDown,
        A::MoveWindowLeftOrToOutputLeft,
        A::MoveWindowRightOrToOutputRight,
        A::ToggleWorkspaceFloating,
        A::ToggleStickyWindow,
        A::FocusWorkspaceAbove,
        A::FocusWorkspaceBelow,
        A::FocusWorkspaceOrOutputAbove,
        A::FocusWorkspaceOrOutputBelow,
        A::FocusWorkspacePrevious,
        A::FocusWorkspaceNumber(2),
        A::MoveWindowToWorkspaceAbove,
        A::MoveWindowToWorkspaceBelow,
        A::MoveWindowToWorkspaceOrOutputAbove,
        A::MoveWindowToWorkspaceOrOutputBelow,
        A::MoveWindowToWorkspaceNumber(2),
        A::FocusOutputLeft,
        A::FocusOutputRight,
        A::FocusOutputAbove,
        A::FocusOutputBelow,
        A::MoveWindowToOutputLeft,
        A::MoveWindowToOutputRight,
        A::MoveWindowToOutputAbove,
        A::MoveWindowToOutputBelow,
        A::Exit,
        A::ReloadConfig,
        A::EnterOverview,
        A::OverviewCancel,
        A::OverviewConfirm,
        A::OverviewNavUp,
        A::OverviewNavDown,
        A::OverviewNavLeft,
        A::OverviewNavRight,
        A::Spawn(vec!["/bin/true".into()]),
    ]
}

/// Output A with two windows, output B focused with one. ReloadConfig is
/// pointed at a file that cannot exist, so it behaves the same everywhere.
fn scene() -> (Session, MiniServer) {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "B", (1920, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);
    s.state.config_path = Some(std::path::PathBuf::from("/nonexistent/tailrace.toml"));
    // A declared name for the actions that refer to one.
    s.config_mut().workspace = vec![crate::types::WorkspaceName {
        name: "chat".into(),
        slot: 3,
        output: None,
    }];
    s.check_consistent();
    (s, sv)
}

fn focused_window(s: &Session) -> &crate::types::Window {
    s.state.wm.focused_window().expect("a window is focused")
}

/// No action may panic or leave an out-of-bounds focus index behind.
#[test]
fn every_action_keeps_the_state_consistent() {
    for action in every_action() {
        let (mut s, mut sv) = scene();
        crate::keybinding::dispatch_action(&mut s.state, &action);
        s.manage(&mut sv);
        s.check_consistent();
        // Some actions only finish on the next cycle (status retries).
        s.manage(&mut sv);
        s.check_consistent();
    }
}

#[test]
fn focus_workspace_number_and_previous() {
    let (mut s, mut sv) = scene();
    let output = s.state.wm.focused_output_idx.unwrap();
    assert_eq!(s.state.wm.outputs[output].focused_workspace_idx, 0);

    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::FocusWorkspaceNumber(3));
    s.manage(&mut sv);
    assert_eq!(s.state.wm.outputs[output].focused_workspace_idx, 2);
    assert_eq!(
        s.state.wm.previous_workspace.map(|p| p.workspace_idx),
        Some(0)
    );

    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::FocusWorkspacePrevious);
    s.manage(&mut sv);
    assert_eq!(
        s.state.wm.outputs[output].focused_workspace_idx, 0,
        "FocusWorkspacePrevious returns to the pre-switch workspace"
    );
    s.check_consistent();
}

#[test]
fn move_window_to_workspace_number_moves_it() {
    let (mut s, mut sv) = scene();
    let output = s.state.wm.focused_output_idx.unwrap();
    assert_eq!(
        s.state.wm.outputs[output].workspace_list[0]
            .window_list
            .len(),
        1
    );

    crate::keybinding::dispatch_action(
        &mut s.state,
        &KeybindingAction::MoveWindowToWorkspaceNumber(2),
    );
    s.manage(&mut sv);
    let workspaces = &s.state.wm.outputs[output].workspace_list;
    assert_eq!(workspaces[0].window_list.len(), 0, "left the old workspace");
    assert_eq!(workspaces[1].window_list.len(), 1, "landed on workspace 2");
    s.check_consistent();
}

#[test]
fn focus_and_move_across_outputs() {
    let (mut s, mut sv) = scene();
    assert_eq!(s.state.wm.focused_output_idx, Some(1), "focus starts on B");

    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::FocusOutputLeft);
    s.manage(&mut sv);
    assert_eq!(s.state.wm.focused_output_idx, Some(0), "moved to A");

    // Move the focused window from A to the output on its right (B).
    let before_a = s.state.wm.outputs[0].workspace_list[0].window_list.len();
    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::MoveWindowToOutputRight);
    s.manage(&mut sv);
    assert_eq!(
        s.state.wm.outputs[0].workspace_list[0].window_list.len(),
        before_a - 1,
        "the window left output A"
    );
    assert!(
        s.state.wm.outputs[1]
            .workspace_list
            .iter()
            .any(|ws| !ws.window_list.is_empty()),
        "the window arrived on output B"
    );
    s.check_consistent();
}

#[test]
fn toggle_maximize_column_switches_proportion() {
    let (mut s, mut sv) = scene();
    let output = s.state.wm.focused_output_idx.unwrap();
    assert_eq!(focused_window(&s).geom.proportion, 0.5);

    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::ToggleMaximizeColumn);
    s.manage(&mut sv);
    assert_eq!(
        s.state.wm.outputs[output].workspace_list[0].window_list[0]
            .geom
            .proportion,
        1.0
    );

    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::ToggleMaximizeColumn);
    s.manage(&mut sv);
    assert_eq!(
        s.state.wm.outputs[output].workspace_list[0].window_list[0]
            .geom
            .proportion,
        0.5
    );
}

#[test]
fn window_width_actions_change_the_proportion() {
    let (mut s, mut sv) = scene();
    let output = s.state.wm.focused_output_idx.unwrap();
    let before = focused_window(&s).geom.proportion;

    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::AdjustWindowWidth(0.1));
    assert!(focused_window(&s).geom.proportion > before);

    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::SetWindowWidth(0.5));
    s.manage(&mut sv);
    assert_eq!(
        s.state.wm.outputs[output].workspace_list[0].window_list[0]
            .geom
            .proportion,
        0.5
    );
}

#[test]
fn floating_actions_move_and_resize_the_rest_rect() {
    let (mut s, mut sv) = scene();
    let output = s.state.wm.focused_output_idx.unwrap();
    let geom = |s: &Session| {
        s.state.wm.outputs[output].workspace_list[0].window_list[0]
            .geom
            .clone()
    };

    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::ToggleWorkspaceFloating);
    s.manage(&mut sv);
    assert!(geom(&s).is_floating);

    let before = geom(&s);
    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::MoveFloatingWindowRight);
    assert!(
        geom(&s).floating.x > before.floating.x,
        "MoveFloatingWindowRight shifts the rest rect right"
    );

    let before = geom(&s);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &KeybindingAction::AdjustFloatingWindowSize(0.1),
    );
    assert!(
        geom(&s).floating.width > before.floating.width,
        "AdjustFloatingWindowSize grows the rest rect"
    );

    crate::keybinding::dispatch_action(
        &mut s.state,
        &KeybindingAction::SetFloatingWindowHeight(0.5),
    );
    assert!(geom(&s).floating.height > 0);
    s.check_consistent();
}

/// `center_focused_window = "on_overflow"` compares against the focus of the
/// previous layout pass; the unit test in `scroller` only proves the
/// comparison itself. Two 0.5 windows fit on a 1920px output, so focusing to
/// one of them must not center; widening the other to 1.0 makes the pair
/// overflow, so focusing away from it must.
#[test]
fn on_overflow_centers_only_a_window_that_does_not_fit() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.config_mut().center_focused_window = crate::types::CenterFocused::OnOverflow;

    s.add_window(&mut sv);
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);

    let rect = |s: &Session, idx: usize| {
        let geom = &s.state.wm.outputs[0].workspace_list[0].window_list[idx].geom;
        geom.current
    };
    let centered_x = |s: &Session, idx: usize| (1920 - rect(s, idx).width) / 2;

    // 0.5 + 0.5 fits: moving the focus left must not center.
    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::FocusWindowLeft);
    s.manage(&mut sv);
    assert_ne!(rect(&s, 0).x, centered_x(&s, 0), "the pair fits");

    // Widen the focused (left) window to 1.0, then focus right again: the pair
    // no longer fits, so the newly focused window is centered.
    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::SetWindowWidth(1.0));
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::FocusWindowRight);
    s.manage(&mut sv);
    assert_eq!(rect(&s, 1).x, centered_x(&s, 1), "the pair overflows");

    // A later pass that does not move the focus must leave it there: the
    // decision is latched, not re-taken.
    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::OverviewCancel);
    s.manage(&mut sv);
    assert_eq!(rect(&s, 1).x, centered_x(&s, 1), "still centered");
    s.check_consistent();
}

/// Sticky windows follow the focused workspace of their output (i3/kwm
/// style), so every switch path works without knowing about stickiness.
#[test]
fn sticky_window_follows_the_focused_workspace() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);

    fn ws(s: &Session, idx: usize) -> &crate::types::Workspace {
        &s.state.wm.outputs[0].workspace_list[idx]
    }

    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::ToggleStickyWindow);
    s.manage(&mut sv);
    assert!(focused_window(&s).geom.is_sticky);

    // The focused window is sticky: switching to the empty workspace 2 brings
    // it along and keeps the focus on it.
    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::FocusWorkspaceNumber(2));
    s.manage(&mut sv);
    assert!(ws(&s, 0).window_list.is_empty(), "left workspace 1");
    assert_eq!(ws(&s, 1).window_list.len(), 1, "arrived on workspace 2");
    assert_eq!(ws(&s, 1).focused_window_idx, Some(0), "kept the focus");
    s.check_consistent();

    // Switching back to the now empty workspace 1 brings it home again.
    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::FocusWorkspaceNumber(1));
    s.manage(&mut sv);
    assert_eq!(ws(&s, 0).window_list.len(), 1, "returned to workspace 1");
    assert_eq!(ws(&s, 0).focused_window_idx, Some(0), "kept the focus");
    s.check_consistent();

    // A non-sticky window on workspace 1 takes the focus; switching to the
    // empty workspace 3 brings the sticky one along but leaves no focus there.
    s.add_window(&mut sv);
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::FocusWorkspaceNumber(3));
    s.manage(&mut sv);
    assert_eq!(ws(&s, 2).window_list.len(), 1, "the sticky window followed");
    assert_eq!(ws(&s, 2).focused_window_idx, None, "must not steal focus");
    assert_eq!(ws(&s, 0).window_list.len(), 1, "the other window stayed");
    s.check_consistent();
}

/// Un-sticking leaves the window where it is (i3 behavior): it only follows
/// the focus while the mark is set.
#[test]
fn unsticking_leaves_the_window_in_place() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);

    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::ToggleStickyWindow);
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::FocusWorkspaceNumber(2));
    s.manage(&mut sv);
    assert_eq!(s.state.wm.outputs[0].workspace_list[1].window_list.len(), 1);

    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::ToggleStickyWindow);
    s.manage(&mut sv);
    assert!(!focused_window(&s).geom.is_sticky);

    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::FocusWorkspaceNumber(3));
    s.manage(&mut sv);
    assert_eq!(
        s.state.wm.outputs[0].workspace_list[1].window_list.len(),
        1,
        "the unmarked window stays on workspace 2"
    );
    assert!(
        s.state.wm.outputs[0].workspace_list[2]
            .window_list
            .is_empty()
    );
    s.check_consistent();
}

/// Two sticky windows on workspace 1 plus a non-sticky one that holds the
/// focus; switching to the empty workspace 2 pulls the two sticky windows over
/// without moving the focus with them. That leaves a workspace with windows
/// and no focused window — the state the direction keys must still handle.
fn no_focus_workspace() -> (Session, MiniServer) {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    for _ in 0..3 {
        s.add_window(&mut sv);
        s.manage(&mut sv);
    }
    // The first two are sticky, the third keeps the focus.
    for idx in 0..2 {
        s.state.wm.outputs[0].workspace_list[0].focused_window_idx = Some(idx);
        crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::ToggleStickyWindow);
        s.manage(&mut sv);
    }
    s.state.wm.outputs[0].workspace_list[0].focused_window_idx = Some(2);
    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::FocusWorkspaceNumber(2));
    s.manage(&mut sv);

    assert_eq!(s.state.wm.outputs[0].workspace_list[1].window_list.len(), 2);
    assert_eq!(
        s.state.wm.outputs[0].workspace_list[1].focused_window_idx, None,
        "test setup must leave the workspace with no focus"
    );
    (s, sv)
}

/// A workspace with windows but no focus must be reachable from the keyboard:
/// each direction key enters the strip from the end it points at.
#[test]
fn direction_keys_enter_a_workspace_with_no_focus() {
    let focus = |s: &Session| s.state.wm.outputs[0].workspace_list[1].focused_window_idx;

    let (mut s, mut sv) = no_focus_workspace();
    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::FocusWindowLeft);
    s.manage(&mut sv);
    assert_eq!(focus(&s), Some(1), "Left enters from the right end");

    let (mut s, mut sv) = no_focus_workspace();
    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::FocusWindowRight);
    s.manage(&mut sv);
    assert_eq!(focus(&s), Some(0), "Right enters from the left end");

    // The output-aware variants enter the strip too, instead of looking for an
    // output to move to from a focus they do not have.
    let (mut s, mut sv) = no_focus_workspace();
    crate::keybinding::dispatch_action(&mut s.state, &KeybindingAction::FocusWindowOrOutputLeft);
    s.manage(&mut sv);
    assert_eq!(focus(&s), Some(1));
    s.check_consistent();
}

/// One output with two 0.5 columns, the second window focused.
fn two_windows() -> (Session, MiniServer) {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);
    (s, sv)
}

fn dispatch(s: &mut Session, sv: &mut MiniServer, action: KeybindingAction) {
    crate::keybinding::dispatch_action(&mut s.state, &action);
    s.manage(sv);
}

#[test]
fn focus_window_previous_toggles_between_the_last_two() {
    let (mut s, mut sv) = two_windows();
    let focus = |s: &Session| s.state.wm.outputs[0].workspace_list[0].focused_window_idx;
    assert_eq!(focus(&s), Some(1));

    dispatch(&mut s, &mut sv, KeybindingAction::FocusWindowLeft);
    assert_eq!(focus(&s), Some(0));

    dispatch(&mut s, &mut sv, KeybindingAction::FocusWindowPrevious);
    assert_eq!(focus(&s), Some(1), "back to the window we came from");

    dispatch(&mut s, &mut sv, KeybindingAction::FocusWindowPrevious);
    assert_eq!(focus(&s), Some(0), "and back again");
    s.check_consistent();
}

/// The presets cycle through the defaults (1/3, 1/2, 2/3) and wrap around; the
/// back action walks the same list in reverse.
#[test]
fn switch_preset_column_width_cycles_the_presets() {
    let (mut s, mut sv) = two_windows();
    let proportion = |s: &Session| focused_window(s).geom.proportion;
    let close = |a: f32, b: f32| (a - b).abs() < 0.01;
    assert!(close(proportion(&s), 0.5), "starts on the 1/2 preset");

    dispatch(&mut s, &mut sv, KeybindingAction::SwitchPresetColumnWidth);
    assert!(close(proportion(&s), 2.0 / 3.0));

    dispatch(&mut s, &mut sv, KeybindingAction::SwitchPresetColumnWidth);
    assert!(
        close(proportion(&s), 1.0 / 3.0),
        "wraps to the first preset"
    );

    dispatch(
        &mut s,
        &mut sv,
        KeybindingAction::SwitchPresetColumnWidthBack,
    );
    assert!(
        close(proportion(&s), 2.0 / 3.0),
        "back walks the list in reverse"
    );
}

/// A column that is on no preset goes to the next larger one, not to the first.
#[test]
fn switch_preset_column_width_from_an_off_preset_width() {
    let (mut s, mut sv) = two_windows();
    dispatch(&mut s, &mut sv, KeybindingAction::SetWindowWidth(0.4));

    dispatch(&mut s, &mut sv, KeybindingAction::SwitchPresetColumnWidth);
    let proportion = focused_window(&s).geom.proportion;
    assert!((proportion - 0.5).abs() < 0.01, "0.4 → the 1/2 preset");

    dispatch(&mut s, &mut sv, KeybindingAction::SetWindowWidth(0.4));
    dispatch(
        &mut s,
        &mut sv,
        KeybindingAction::SwitchPresetColumnWidthBack,
    );
    let proportion = focused_window(&s).geom.proportion;
    assert!(
        (proportion - 1.0 / 3.0).abs() < 0.01,
        "0.4 ← the 1/3 preset"
    );
}

#[test]
fn center_column_centers_the_focused_window() {
    let (mut s, mut sv) = two_windows();
    let geom = |s: &Session| focused_window(s).geom.current;
    assert_ne!(geom(&s).x, (1920 - geom(&s).width) / 2);

    dispatch(&mut s, &mut sv, KeybindingAction::CenterColumn);
    let g = geom(&s);
    assert_eq!(g.x, (1920 - g.width) / 2);
    assert_eq!(g.width, 946, "centering does not resize the column");

    // Moving the focus away drops the latch: the new window is not centered.
    dispatch(&mut s, &mut sv, KeybindingAction::FocusWindowLeft);
    let other = geom(&s);
    assert_ne!(other.x, (1920 - other.width) / 2);
}

#[test]
fn expand_column_to_available_width_fills_the_strip() {
    let (mut s, mut sv) = two_windows();
    // Two windows: the focused one already spans half the output.
    dispatch(
        &mut s,
        &mut sv,
        KeybindingAction::ExpandColumnToAvailableWidth,
    );
    let g = focused_window(&s).geom.current;
    assert_eq!(g.x + g.width, 1911, "reaches the output's right gap");
    s.check_consistent();
}

#[test]
fn maximize_window_to_edges_covers_the_workspace() {
    let (mut s, mut sv) = two_windows();
    let neighbour = s.state.wm.outputs[0].workspace_list[0].window_list[0]
        .geom
        .current;

    dispatch(&mut s, &mut sv, KeybindingAction::MaximizeWindowToEdges);
    let windows = &s.state.wm.outputs[0].workspace_list[0].window_list;
    assert_eq!(windows[1].geom.current.width, 1920, "fills the width");
    assert_eq!(windows[1].geom.current.height, 1080, "and the height");
    assert_eq!(
        windows[0].geom.current, neighbour,
        "the neighbour keeps its column"
    );
    assert_eq!(windows[1].geom.current.x, 0, "covers the workspace area");
}

#[test]
fn focus_output_next_and_previous_cycle_the_monitors() {
    let (mut s, mut sv) = scene();
    assert_eq!(s.state.wm.focused_output_idx, Some(1));

    dispatch(&mut s, &mut sv, KeybindingAction::FocusOutputNext);
    assert_eq!(s.state.wm.focused_output_idx, Some(0));
    dispatch(&mut s, &mut sv, KeybindingAction::FocusOutputNext);
    assert_eq!(s.state.wm.focused_output_idx, Some(1), "wraps around");
    dispatch(&mut s, &mut sv, KeybindingAction::FocusOutputPrevious);
    assert_eq!(s.state.wm.focused_output_idx, Some(0));
}

/// Moving a workspace to another output exchanges the two slots and follows
/// the workspace, so the windows travel with it.
#[test]
fn move_workspace_to_output_swaps_with_the_target() {
    let (mut s, mut sv) = scene();
    let a_windows = s.state.wm.outputs[0].workspace_list[0].window_list.len();
    let b_windows = s.state.wm.outputs[1].workspace_list[0].window_list.len();
    assert_eq!(a_windows, 2);
    assert_eq!(b_windows, 1);
    dispatch(&mut s, &mut sv, KeybindingAction::FocusOutputLeft);
    assert_eq!(s.state.wm.focused_output_idx, Some(0));

    dispatch(
        &mut s,
        &mut sv,
        KeybindingAction::MoveWorkspaceToOutputRight,
    );
    assert_eq!(
        s.state.wm.focused_output_idx,
        Some(1),
        "the workspace is followed to its new output"
    );
    assert_eq!(s.state.wm.outputs[0].workspace_list[0].window_list.len(), 1);
    assert_eq!(s.state.wm.outputs[1].workspace_list[0].window_list.len(), 2);
    s.check_consistent();
}

/// `focus_workspace_name` and `move_window_to_workspace_name` resolve through
/// `[[workspace]]`; the scene declares "chat" on slot 3.
#[test]
fn named_workspace_actions_resolve_the_slot() {
    let (mut s, mut sv) = scene();
    dispatch(&mut s, &mut sv, KeybindingAction::FocusOutputLeft);
    let before = s.state.wm.outputs[0].workspace_list[0].window_list.len();

    dispatch(
        &mut s,
        &mut sv,
        KeybindingAction::MoveWindowToWorkspaceName("chat".into()),
    );
    assert_eq!(
        s.state.wm.outputs[0].workspace_list[2].window_list.len(),
        1,
        "the window landed on the named workspace"
    );
    assert_eq!(
        s.state.wm.outputs[0].workspace_list[0].window_list.len(),
        before - 1,
        "and left the focused one"
    );

    dispatch(
        &mut s,
        &mut sv,
        KeybindingAction::FocusWorkspaceName("chat".into()),
    );
    assert_eq!(s.state.wm.outputs[0].focused_workspace_idx, 2);
    s.check_consistent();
}
