// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: 0BSD

//! Multi-output hotplug scenarios: unplug/replug, output removal and the
//! overview grid across displays.

use super::*;

/// eDP-1 and HDMI both active; HDMI is unplugged. Focus is on HDMI.
#[test]
fn unplug_hdmi_while_edp_present() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.manage(&mut sv); // bindings setup consumed one manage_start

    s.add_output(&mut sv, "eDP-1", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv); // focused: eDP-1
    s.manage(&mut sv);

    let hdmi = s.add_output(&mut sv, "HDMI-A-1", (1920, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv); // focused: HDMI-A-1
    s.manage(&mut sv);

    // Unplug HDMI.
    s.send(&mut sv, hdmi, EVT_OUT_REMOVED, vec![]);
    s.manage(&mut sv);
    s.check_consistent();

    let wm = &s.state.wm;
    assert_eq!(wm.outputs.len(), 1, "only eDP-1 should survive");
    let edp = &wm.outputs[0];
    assert_eq!(edp.name.as_deref(), Some("eDP-1"));
    assert_eq!(wm.focused_output_idx, Some(0));
    let edp_ws0 = &edp.workspace_list[0];
    assert_eq!(
        edp_ws0.window_list.len(),
        2,
        "HDMI windows migrate to eDP ws0"
    );
    assert_eq!(
        edp_ws0.window_list[1].geom.former_output_name.as_deref(),
        Some("HDMI-A-1"),
        "migrated window remembers its source output"
    );
    layout::update(&mut s.state.wm);
    s.check_consistent();
}

/// External-only setup: plugging HDMI removes eDP-1 (`detach`), unplugging
/// HDMI leaves zero outputs (`detach`), then eDP-1 reappears and windows are
/// restored. This is the reported crash: "back on eDP-1 after HDMI unplug".
#[test]
fn external_only_hdmi_unplug_returns_to_edp() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.manage(&mut sv);

    s.add_output(&mut sv, "eDP-1", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);

    // HDMI plugged, eDP-1 disabled (external-only mode).
    let edp = sv.output_id("eDP-1");
    s.send(&mut sv, edp, EVT_OUT_REMOVED, vec![]);
    s.manage(&mut sv);
    s.check_consistent();
    assert_eq!(s.state.wm.outputs.len(), 0, "no outputs while HDMI-only");

    let hdmi = s.add_output(&mut sv, "HDMI-A-1", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);
    assert!(!s.state.wm.detached_outputs.is_empty() || s.state.wm.outputs.len() == 1);

    // Unplug HDMI: zero outputs left, workspaces detached and preserved.
    s.send(&mut sv, hdmi, EVT_OUT_REMOVED, vec![]);
    s.manage(&mut sv);
    s.check_consistent();
    assert_eq!(s.state.wm.outputs.len(), 0);
    assert!(
        !s.state.wm.detached_outputs.is_empty(),
        "windows must survive the HDMI unplug"
    );

    // eDP-1 comes back: the "returning to eDP-1" moment.
    s.add_output(&mut sv, "eDP-1", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.check_consistent();

    let wm = &s.state.wm;
    assert_eq!(wm.outputs.len(), 1, "eDP-1 back");
    assert_eq!(wm.outputs[0].name.as_deref(), Some("eDP-1"));
    assert_eq!(wm.focused_output_idx, Some(0));
    let total: usize = wm.outputs[0]
        .workspace_list
        .iter()
        .map(|ws| ws.window_list.len())
        .sum();
    assert_eq!(total, 2, "both windows restored to eDP-1");
    assert!(
        wm.detached_outputs.is_empty(),
        "restored workspaces are no longer detached"
    );
}

/// Detach (last output removed) then re-add by the same name: the
/// name-keyed restore path in layout::apply.
#[test]
fn detach_then_readd_same_name_restores_windows() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.manage(&mut sv);

    s.add_output(&mut sv, "eDP-1", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);

    // eDP-1 disabled, then re-enabled.
    let edp = sv.output_id("eDP-1");
    s.send(&mut sv, edp, EVT_OUT_REMOVED, vec![]);
    s.manage(&mut sv);
    s.check_consistent();

    // eDP-1 back as the only output.
    s.add_output(&mut sv, "eDP-1", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.check_consistent();
    let wm = &s.state.wm;
    assert_eq!(wm.outputs.len(), 1);
    let total: usize = wm.outputs[0]
        .workspace_list
        .iter()
        .map(|ws| ws.window_list.len())
        .sum();
    assert!(total >= 1, "window survives the cycle");
    assert_eq!(wm.focused_output_idx, Some(0));
}

/// The reported crash: laptop display (1360x768) OFF while a 2K HDMI
/// (2560x1440) is the only active screen; HDMI is unplugged and the laptop
/// comes back. Every window must land back on the laptop AND fit within its
/// smaller bounds — a floating window centered on the 2K screen must not
/// overhang the 1360x768 screen (the "does not fit" symptom).
#[test]
fn hdmi_2k_unplug_restores_small_laptop_screen() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.manage(&mut sv);

    s.add_output(&mut sv, "eDP-1", (0, 0), (1360, 768));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);

    // HDMI plugged, laptop screen disabled: tiled window migrates to 2K.
    let hdmi = s.add_output(&mut sv, "HDMI-A-1", (0, 0), (2560, 1440));
    s.manage(&mut sv);
    let edp = sv.output_id("eDP-1");
    s.send(&mut sv, edp, EVT_OUT_REMOVED, vec![]);
    s.manage(&mut sv);
    s.check_consistent();
    assert_eq!(s.state.wm.outputs.len(), 1, "only HDMI while laptop is off");
    assert_eq!(s.state.wm.outputs[0].name.as_deref(), Some("HDMI-A-1"));

    // A second window, toggled floating on the 2K screen (centered rect,
    // mirroring ToggleWorkspaceFloating).
    s.add_window(&mut sv);
    s.manage(&mut sv);
    {
        let wm = &mut s.state.wm;
        let oi = wm.focused_output_idx.unwrap();
        let ws = wm.outputs[oi].focused_workspace_idx;
        let rect = wm.outputs[oi].rectangle;
        let window = wm.outputs[oi].workspace_list[ws]
            .window_list
            .last_mut()
            .unwrap();
        window.geom.is_floating = true;
        window.geom.floating = crate::layout::common::center_rectangle(
            rect,
            &wm.config,
            wm.config.default_window_width,
        );
    }
    s.manage(&mut sv);
    s.check_consistent();

    // Unplug HDMI: zero outputs, workspaces detached and preserved.
    s.send(&mut sv, hdmi, EVT_OUT_REMOVED, vec![]);
    s.manage(&mut sv);
    assert_eq!(s.state.wm.outputs.len(), 0, "nothing left while laptop off");
    assert!(!s.state.wm.detached_outputs.is_empty(), "windows preserved");

    // Laptop screen comes back: the "back to the laptop panel" moment.
    s.add_output(&mut sv, "eDP-1", (0, 0), (1360, 768));
    s.manage(&mut sv);
    s.check_consistent();

    let wm = &s.state.wm;
    assert_eq!(wm.outputs.len(), 1, "eDP-1 back");
    assert_eq!(wm.outputs[0].name.as_deref(), Some("eDP-1"));
    assert_eq!(wm.focused_output_idx, Some(0));
    let out = wm.outputs[0].rectangle;
    assert_eq!((out.width, out.height), (1360, 768));

    let mut total = 0;
    for ws in &wm.outputs[0].workspace_list {
        for w in &ws.window_list {
            total += 1;
            let rect = w.geom.finish.unwrap_or(w.geom.current);
            assert!(
                rect.x >= out.x
                    && rect.y >= out.y
                    && rect.x + rect.width <= out.x + out.width
                    && rect.y + rect.height <= out.y + out.height,
                "window {total} {rect:?} overhangs the laptop {out:?}"
            );
        }
    }
    assert_eq!(total, 2, "both windows transferred back to the laptop");
    assert!(
        wm.detached_outputs.is_empty(),
        "no detached workspaces left"
    );
}

/// HDMI unplug and laptop re-enable in the SAME batch, before one manage:
/// the race the auto-detect path hits. No panic; windows come back on the
/// laptop.
#[test]
fn unplug_and_readd_in_same_batch_returns_to_laptop() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.manage(&mut sv);

    s.add_output(&mut sv, "eDP-1", (0, 0), (1360, 768));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);

    // Laptop disabled while HDMI active; work on the 2K screen.
    let hdmi = s.add_output(&mut sv, "HDMI-A-1", (0, 0), (2560, 1440));
    s.manage(&mut sv);
    let edp = sv.output_id("eDP-1");
    s.send(&mut sv, edp, EVT_OUT_REMOVED, vec![]);
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);

    // Both events before the manage: HDMI disappears, laptop returns.
    s.send(&mut sv, hdmi, EVT_OUT_REMOVED, vec![]);
    s.add_output(&mut sv, "eDP-1", (0, 0), (1360, 768));
    s.manage(&mut sv);
    s.check_consistent();

    let wm = &s.state.wm;
    assert_eq!(wm.outputs.len(), 1, "eDP-1 back");
    assert_eq!(wm.outputs[0].name.as_deref(), Some("eDP-1"));
    let total: usize = wm.outputs[0]
        .workspace_list
        .iter()
        .map(|ws| ws.window_list.len())
        .sum();
    assert_eq!(total, 2, "both windows transferred back to the laptop");
    assert!(wm.detached_outputs.is_empty());
}

/// Regression: overview on a multi-display setup with different resolutions.
/// The grid is drawn on the focused output (global coordinates), but
/// place_window measured visibility/clip against each window's HOME output,
/// so every window from another display was hidden or clipped off the grid
/// (the "misplaced / cannot adapt" report). Both windows must land visibly
/// inside the focused output.
#[test]
fn overview_keeps_foreign_display_windows_in_grid() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.manage(&mut sv);

    // Two displays, different resolutions: eDP-1 1360x768 at origin,
    // HDMI-A-1 2560x1440 to its right. Focus lands on HDMI (last added).
    s.add_output(&mut sv, "eDP-1", (0, 0), (1360, 768));
    s.manage(&mut sv);
    s.add_window(&mut sv); // window homed on eDP-1
    s.manage(&mut sv);

    s.add_output(&mut sv, "HDMI-A-1", (1360, 0), (2560, 1440));
    s.manage(&mut sv);
    s.add_window(&mut sv); // window homed on HDMI
    s.manage(&mut sv);
    s.check_consistent();
    assert_eq!(s.state.wm.focused_output_idx, Some(1));

    // Enter overview: grid on the focused output (HDMI), global coords.
    crate::overview::enter(&mut s.state);
    s.state.wm.status = crate::types::Status::Overview;
    s.manage(&mut sv);
    s.check_consistent();

    let wm = &s.state.wm;
    let grid = wm.outputs[1].rectangle;
    for (oi, label) in [(0usize, "eDP-1"), (1, "HDMI-A-1")] {
        let w = &wm.outputs[oi].workspace_list[0].window_list[0];
        let r = w.geom.current;
        assert!(
            r.x >= grid.x
                && r.y >= grid.y
                && r.x + r.width <= grid.x + grid.width
                && r.y + r.height <= grid.y + grid.height,
            "{label} window {r:?} outside the focused grid {grid:?}"
        );
        assert_eq!(
            w.geom.sent_visible,
            Some(true),
            "{label} window was hidden from the overview grid"
        );
    }
}

/// The reported bug: the laptop panel is turned OFF (river keeps exposing a
/// soft-disabled output, so the window manager never sees a `removed` for it)
/// while an external screen is the only visible one. Then suspend (every
/// output is destroyed) and resume on the panel, with the external screen
/// unplugged meanwhile.
///
/// The panel comes back on the workspace it was left on, while the external
/// screen's window lands on the panel's *matching* workspace index. If those
/// indices differ, the window is not on the focused workspace: it is stacked
/// off-screen and focus has no window to move from.
#[test]
fn panel_off_then_suspend_returns_windows_to_the_focused_workspace() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.manage(&mut sv);

    // The panel, left on workspace 3 before the external screen was plugged.
    s.add_output(&mut sv, "eDP-1", (0, 0), (1360, 768));
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::FocusWorkspaceNumber(3),
    );
    s.manage(&mut sv);
    assert_eq!(s.state.wm.outputs[0].focused_workspace_idx, 2);

    // External screen: the panel is off but still exposed, so HDMI takes focus
    // and two windows are opened on HDMI's workspace 1.
    let hdmi = s.add_output(&mut sv, "HDMI-A-1", (1360, 0), (2560, 1440));
    s.manage(&mut sv);
    assert_eq!(s.state.wm.focused_output_idx, Some(1));
    s.add_window(&mut sv);
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);

    // Suspend destroys every output; the external screen was unplugged, so the
    // panel is the one that comes back.
    let edp = sv.output_id("eDP-1");
    s.send(&mut sv, hdmi, EVT_OUT_REMOVED, vec![]);
    s.send(&mut sv, edp, EVT_OUT_REMOVED, vec![]);
    s.manage(&mut sv);
    s.check_consistent();
    assert_eq!(
        s.state.wm.outputs.len(),
        0,
        "all outputs gone while suspended"
    );

    s.add_output(&mut sv, "eDP-1", (0, 0), (1360, 768));
    s.manage(&mut sv);
    s.check_consistent();

    let wm = &s.state.wm;
    assert_eq!(wm.outputs.len(), 1, "the panel is back");
    let output = &wm.outputs[0];
    let total: usize = output
        .workspace_list
        .iter()
        .map(|ws| ws.window_list.len())
        .sum();
    assert_eq!(
        total, 2,
        "the external screen's windows come back to the panel"
    );
    let focused_ws = &output.workspace_list[output.focused_workspace_idx];
    assert!(
        focused_ws.focused_window_idx.is_some(),
        "the focused workspace ({}) must hold the windows that came back; it holds {} \
         window(s) while the panel's {} windows sit elsewhere",
        output.focused_workspace_idx,
        focused_ws.window_list.len(),
        total
    );

    // The reported symptoms: the windows are laid out inside the panel (not
    // hanging off it), and focus can move from one to the other.
    for ws in &output.workspace_list {
        for window in &ws.window_list {
            let g = &window.geom.current;
            assert!(
                g.x >= 0 && g.x + g.width <= 1360 && g.y >= 0 && g.y + g.height <= 768,
                "the returned window must fit the panel, got {g:?}"
            );
        }
    }
    let before = s
        .state
        .wm
        .focused_window()
        .expect("a window is focused")
        .river_window
        .clone();
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::FocusWindowRight,
    );
    s.manage(&mut sv);
    s.check_consistent();
    let after = &s
        .state
        .wm
        .focused_window()
        .expect("a window is focused")
        .river_window;
    assert_ne!(*after, before, "focus moves to the other window");
}

/// The same bug through the other path: an output removed while another one
/// survives. The survivor keeps showing its own focused workspace, so the
/// windows that migrate into it have to pull the view along.
#[test]
fn unplug_hdmi_pulls_the_survivors_view_to_the_migrated_windows() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.manage(&mut sv);

    // The panel is left on workspace 3 while an external screen is used.
    s.add_output(&mut sv, "eDP-1", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::FocusWorkspaceNumber(3),
    );
    s.manage(&mut sv);

    let hdmi = s.add_output(&mut sv, "HDMI-A-1", (1920, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);

    s.send(&mut sv, hdmi, EVT_OUT_REMOVED, vec![]);
    s.manage(&mut sv);
    s.check_consistent();

    let wm = &s.state.wm;
    assert_eq!(wm.outputs.len(), 1);
    let output = &wm.outputs[0];
    let focused_ws = &output.workspace_list[output.focused_workspace_idx];
    assert!(
        focused_ws.focused_window_idx.is_some(),
        "the survivor ({}) shows the migrated window",
        output.focused_workspace_idx
    );
}

/// The window in use before the outputs went away wins over whatever the
/// output that comes back was showing: the panel is left on a workspace with
/// its own window, the external screen holds the focused window, and after
/// suspend/resume the focused window is the one in view.
#[test]
fn resume_shows_the_window_that_was_focused() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.manage(&mut sv);

    // The panel, left on workspace 3 with a window of its own.
    s.add_output(&mut sv, "eDP-1", (0, 0), (1360, 768));
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::FocusWorkspaceNumber(3),
    );
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);
    assert_eq!(s.state.wm.outputs[0].focused_workspace_idx, 2);

    // The external screen with the window the user is using.
    let hdmi = s.add_output(&mut sv, "HDMI-A-1", (1360, 0), (2560, 1440));
    s.manage(&mut sv);
    s.add_window(&mut sv);
    s.manage(&mut sv);

    // Suspend with the external screen unplugged.
    let edp = sv.output_id("eDP-1");
    s.send(&mut sv, hdmi, EVT_OUT_REMOVED, vec![]);
    s.send(&mut sv, edp, EVT_OUT_REMOVED, vec![]);
    s.manage(&mut sv);
    s.check_consistent();

    s.add_output(&mut sv, "eDP-1", (0, 0), (1360, 768));
    s.manage(&mut sv);
    s.check_consistent();

    let wm = &s.state.wm;
    assert_eq!(wm.outputs.len(), 1);
    let output = &wm.outputs[0];
    let total: usize = output
        .workspace_list
        .iter()
        .map(|ws| ws.window_list.len())
        .sum();
    assert_eq!(total, 2, "both windows are back on the panel");

    // The window that had focus is the one in view and the one focused.
    let was_focused = output
        .workspace_list
        .iter()
        .enumerate()
        .find_map(|(ws_idx, ws)| {
            ws.window_list.iter().find_map(|w| {
                (w.geom.former_output_name.as_deref() == Some("HDMI-A-1")).then_some(ws_idx)
            })
        })
        .expect("the external screen's window is back");
    assert_eq!(
        output.focused_workspace_idx, was_focused,
        "the view must be on the workspace of the window that was focused ({}), \
         not on the panel's own workspace ({})",
        was_focused, output.focused_workspace_idx
    );
    assert_eq!(
        wm.focused_window()
            .and_then(|w| w.geom.former_output_name.as_deref()),
        Some("HDMI-A-1"),
        "keyboard focus is on the window that was focused before suspend"
    );
}
