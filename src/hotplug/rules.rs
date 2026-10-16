// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: 0BSD

//! Window rules end to end: they are read from the pending window's
//! app_id/title when it is mapped, so the events carrying them have to arrive
//! before the window is added to a workspace.

use super::*;
use crate::types::{WindowRule, WorkspaceName, WorkspaceTarget};

fn rule(app_id: Option<&str>, title: Option<&str>, floating: bool) -> WindowRule {
    WindowRule {
        app_id: app_id.map(Into::into),
        title: title.map(Into::into),
        floating,
        ..WindowRule::default()
    }
}

/// One output, one pending window, and the given rules.
fn scene(rules: Vec<WindowRule>) -> (Session, MiniServer) {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    s.config_mut().window_rules = rules;
    (s, sv)
}

fn send_str(s: &mut Session, sv: &mut MiniServer, win: &ObjectId, opcode: u16, value: &str) {
    s.send(
        sv,
        win.clone(),
        opcode,
        vec![Argument::Str(Some(Box::new(CString::new(value).unwrap())))],
    );
}

/// The single window of the scene, asserting there is exactly one.
fn only_window(s: &Session) -> &crate::types::Window {
    let ws = &s.state.wm.outputs[0].workspace_list[0];
    assert_eq!(ws.window_list.len(), 1, "one window expected");
    &ws.window_list[0]
}

#[test]
fn a_matching_app_id_floats_the_window() {
    let (mut s, mut sv) = scene(vec![rule(Some("foot"), None, true)]);
    let win = s.add_pending_window(&mut sv);
    send_str(&mut s, &mut sv, &win, EVT_WIN_APP_ID, "foot");
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);
    assert!(only_window(&s).geom.is_floating);
}

#[test]
fn a_matching_title_glob_floats_the_window() {
    let (mut s, mut sv) = scene(vec![rule(None, Some("*Picture-in-Picture*"), true)]);
    let win = s.add_pending_window(&mut sv);
    send_str(
        &mut s,
        &mut sv,
        &win,
        EVT_WIN_TITLE,
        "Picture-in-Picture (mpv)",
    );
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);
    assert!(only_window(&s).geom.is_floating);
}

#[test]
fn an_unmatched_app_id_leaves_the_window_tiled() {
    let (mut s, mut sv) = scene(vec![rule(Some("foot"), None, true)]);
    let win = s.add_pending_window(&mut sv);
    send_str(&mut s, &mut sv, &win, EVT_WIN_APP_ID, "something-else");
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);
    assert!(!only_window(&s).geom.is_floating);
}

/// `floating = false` (the default) never floats even on a match, and a rule
/// with no app_id and no title cannot match anything.
#[test]
fn rules_need_floating_and_a_matcher() {
    let (mut s, mut sv) = scene(vec![
        rule(Some("foot"), None, false),
        rule(None, None, true),
    ]);
    let win = s.add_pending_window(&mut sv);
    send_str(&mut s, &mut sv, &win, EVT_WIN_APP_ID, "foot");
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);
    assert!(!only_window(&s).geom.is_floating);
}

/// Rules are evaluated when the window is mapped: an app_id arriving after
/// that does not retroactively float the window. river sends the app_id
/// before the first manage sequence, so in practice rules see it.
#[test]
fn a_late_app_id_does_not_apply_the_rule() {
    let (mut s, mut sv) = scene(vec![rule(Some("foot"), None, true)]);
    let win = s.add_window(&mut sv); // dimensions first: mapped with no app_id
    s.manage(&mut sv);
    send_str(&mut s, &mut sv, &win, EVT_WIN_APP_ID, "foot");
    s.manage(&mut sv);
    assert!(!only_window(&s).geom.is_floating);
}

/// A rule can pin a window to a workspace number (1-based, as in
/// `focus_workspace_number`) instead of the focused one.
#[test]
fn a_rule_opens_the_window_on_its_workspace() {
    let mut pinning = rule(Some("foot"), None, false);
    pinning.open_on_workspace = Some(WorkspaceTarget::Number(3));
    let (mut s, mut sv) = scene(vec![pinning]);
    let win = s.add_pending_window(&mut sv);
    send_str(&mut s, &mut sv, &win, EVT_WIN_APP_ID, "foot");
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);

    let workspaces = &s.state.wm.outputs[0].workspace_list;
    assert_eq!(workspaces[0].window_list.len(), 0, "not on the focused one");
    assert_eq!(workspaces[2].window_list.len(), 1, "on workspace 3");
}

/// A rule can pin a window to a named output. Output B is added first so the
/// focused one is A; the window lands on B without being focused, like niri.
#[test]
fn a_rule_opens_the_window_on_its_output() {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "B", (1920, 0), (1920, 1080));
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    assert_eq!(s.state.wm.focused_output_idx, Some(1), "A is focused");

    let mut pinning = rule(Some("foot"), None, false);
    pinning.open_on_output = Some("B".into());
    s.config_mut().window_rules = vec![pinning];

    let win = s.add_pending_window(&mut sv);
    send_str(&mut s, &mut sv, &win, EVT_WIN_APP_ID, "foot");
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);

    assert_eq!(s.state.wm.outputs[0].workspace_list[0].window_list.len(), 1);
    assert_eq!(s.state.wm.outputs[1].workspace_list[0].window_list.len(), 0);
}

/// An output name that is not attached falls back to the focused output.
#[test]
fn an_unknown_output_falls_back_to_the_focused_one() {
    let mut pinning = rule(Some("foot"), None, false);
    pinning.open_on_output = Some("HDMI-A-1".into());
    let (mut s, mut sv) = scene(vec![pinning]);
    let win = s.add_pending_window(&mut sv);
    send_str(&mut s, &mut sv, &win, EVT_WIN_APP_ID, "foot");
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);

    assert_eq!(s.state.wm.outputs[0].workspace_list[0].window_list.len(), 1);
}

/// A rule can set the width of the new window: the tiled proportion and the
/// floating rest rect are both derived from it.
#[test]
fn a_rule_sets_the_window_width() {
    let mut wide = rule(Some("foot"), None, true);
    wide.default_window_width = Some(0.25);
    let (mut s, mut sv) = scene(vec![wide]);
    let win = s.add_pending_window(&mut sv);
    send_str(&mut s, &mut sv, &win, EVT_WIN_APP_ID, "foot");
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);

    let window = only_window(&s);
    assert_eq!(window.geom.proportion, 0.25);
    assert_eq!(window.geom.floating.width, 468); // trunc(1911 * 0.25) - 9
}

/// A title change after the window is mapped does not re-evaluate rules
/// (titles of live windows are not tracked at all).
#[test]
fn a_late_title_does_not_apply_the_rule() {
    let (mut s, mut sv) = scene(vec![rule(None, Some("*Picture-in-Picture*"), true)]);
    let win = s.add_window(&mut sv); // mapped with no title
    s.manage(&mut sv);
    send_str(&mut s, &mut sv, &win, EVT_WIN_TITLE, "Picture-in-Picture");
    s.manage(&mut sv);
    assert!(!only_window(&s).geom.is_floating);
}

/// `open_on_workspace` also takes a `[[workspace]]` name, which resolves to
/// that name's slot.
#[test]
fn a_rule_opens_the_window_on_a_named_workspace() {
    let mut pinning = rule(Some("foot"), None, false);
    pinning.open_on_workspace = Some(WorkspaceTarget::Name("chat".into()));
    let (mut s, mut sv) = scene(vec![pinning]);
    s.config_mut().workspace = vec![WorkspaceName {
        name: "chat".into(),
        slot: 4,
        output: None,
    }];

    let win = s.add_pending_window(&mut sv);
    send_str(&mut s, &mut sv, &win, EVT_WIN_APP_ID, "foot");
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);

    let workspaces = &s.state.wm.outputs[0].workspace_list;
    assert_eq!(workspaces[3].window_list.len(), 1, "on slot 4");
    assert_eq!(workspaces[0].window_list.len(), 0, "not the focused one");
}

/// A named workspace carries its output, so a rule that only names a workspace
/// still opens the window on the right monitor.
#[test]
fn a_named_workspace_pins_its_output() {
    let mut pinning = rule(Some("foot"), None, false);
    pinning.open_on_workspace = Some(WorkspaceTarget::Name("chat".into()));
    let (mut s, mut sv) = scene(vec![pinning]);
    s.add_output(&mut sv, "B", (1920, 0), (1920, 1080));
    s.manage(&mut sv);
    s.config_mut().workspace = vec![WorkspaceName {
        name: "chat".into(),
        slot: 2,
        output: Some("B".into()),
    }];

    let win = s.add_pending_window(&mut sv);
    send_str(&mut s, &mut sv, &win, EVT_WIN_APP_ID, "foot");
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);

    assert_eq!(s.state.wm.outputs[1].workspace_list[1].window_list.len(), 1);
    assert_eq!(s.state.wm.outputs[0].workspace_list[0].window_list.len(), 0);
}

/// Window rules can override the client's own size hints.
#[test]
fn a_rule_sets_content_width_bounds() {
    let mut narrow = rule(Some("foot"), None, false);
    narrow.default_window_width = Some(0.25);
    narrow.min_width = Some(1000);
    let (mut s, mut sv) = scene(vec![narrow]);
    let win = s.add_pending_window(&mut sv);
    send_str(&mut s, &mut sv, &win, EVT_WIN_APP_ID, "foot");
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);
    // 1000 of content plus a 3px border on each side.
    assert_eq!(only_window(&s).geom.current.width, 1006);

    let mut wide = rule(Some("foot"), None, false);
    wide.default_window_width = Some(0.75);
    wide.max_width = Some(300);
    let (mut s, mut sv) = scene(vec![wide]);
    let win = s.add_pending_window(&mut sv);
    send_str(&mut s, &mut sv, &win, EVT_WIN_APP_ID, "foot");
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);
    assert_eq!(only_window(&s).geom.current.width, 306);
}
