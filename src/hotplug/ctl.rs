// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: 0BSD

//! The control socket: the window snapshot and the `focus` verb, plus the
//! socket's own rules (permissions, uid check, never waiting for a client).
//!
//! Requests are answered from the real `AppData`, so a snapshot here is what a
//! menu would see.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;

use super::*;
use crate::ctl;

/// One output, a seat, and `count` windows with identifiers `id-0..`; the last
/// one is focused, all on workspace 1.
fn scene(count: usize) -> (Session, MiniServer, Vec<ObjectId>) {
    let (mut s, mut sv) = build();
    s.add_seat(&mut sv);
    s.manage(&mut sv);
    s.add_output(&mut sv, "A", (0, 0), (1920, 1080));
    s.manage(&mut sv);
    let windows = (0..count)
        .map(|n| window(&mut s, &mut sv, &format!("id-{n}"), &format!("win {n}")))
        .collect();
    (s, sv, windows)
}

fn window(s: &mut Session, sv: &mut MiniServer, id: &str, title: &str) -> ObjectId {
    let win = s.add_pending_window(sv);
    for (opcode, value) in [
        (EVT_WIN_IDENTIFIER, id),
        (EVT_WIN_APP_ID, "foot"),
        (EVT_WIN_TITLE, title),
    ] {
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

fn request(s: &mut Session, line: &str) -> String {
    ctl::handle_line(&mut s.state, line)
}

/// `windows` answers one tab-separated line per window, MRU first, with the
/// output, workspace and flags a menu needs.
#[test]
fn windows_lists_them_most_recently_used_first() {
    let (mut s, _sv, _windows) = scene(3);
    let reply = request(&mut s, "windows");
    let lines: Vec<&str> = reply.lines().collect();
    assert_eq!(lines.len(), 3);

    let columns: Vec<Vec<&str>> = lines.iter().map(|l| l.split('\t').collect()).collect();
    assert_eq!(columns[0][0], "id-2", "the focused window comes first");
    assert_eq!(columns[1][0], "id-1");
    assert_eq!(columns[2][0], "id-0");
    assert_eq!(columns[0][1], "win 2");
    assert_eq!(columns[0][2], "foot");
    assert_eq!(columns[0][3], "A:1");
    assert_eq!(columns[0][4], "focused");
    assert_eq!(columns[1][4], "-", "only one window has focus");
    assert!(reply.ends_with('\n'), "a menu reads whole lines");
}

/// A floating, sticky window says so, and a window with no title keeps its
/// columns rather than collapsing them.
#[test]
fn windows_reports_flags_and_empty_fields() {
    let (mut s, mut sv, _windows) = scene(1);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleWorkspaceFloating,
    );
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::ToggleStickyWindow,
    );
    s.manage(&mut sv);

    let win = s.add_pending_window(&mut sv);
    s.send(
        &mut sv,
        win.clone(),
        EVT_WIN_IDENTIFIER,
        vec![Argument::Str(Some(Box::new(CString::new("bare").unwrap())))],
    );
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);

    let reply = request(&mut s, "windows");
    let lines: Vec<&str> = reply.lines().collect();
    assert_eq!(lines.len(), 2);
    let bare: Vec<&str> = lines
        .iter()
        .find(|line| line.starts_with("bare\t"))
        .expect("the window with no title is listed")
        .split('\t')
        .collect();
    assert_eq!(bare[1], "-");
    assert_eq!(bare[2], "-");

    let floating: Vec<&str> = lines
        .iter()
        .find(|line| line.starts_with("id-0\t"))
        .expect("the floating window is listed")
        .split('\t')
        .collect();
    assert_eq!(
        floating[4], "floating,sticky",
        "the window mapped last holds the focus"
    );
    assert_eq!(bare[4], "focused");
}

/// Tabs and newlines in a title cannot break the line format: one window is
/// always one line with five columns.
#[test]
fn a_tab_in_a_title_cannot_break_the_line_format() {
    let (mut s, mut sv, _windows) = scene(1);
    let win = s.add_pending_window(&mut sv);
    for (opcode, value) in [(EVT_WIN_IDENTIFIER, "tricky"), (EVT_WIN_TITLE, "a\tb\nc")] {
        s.send(
            &mut sv,
            win.clone(),
            opcode,
            vec![Argument::Str(Some(Box::new(CString::new(value).unwrap())))],
        );
    }
    s.map_window(&mut sv, &win);
    s.manage(&mut sv);

    let reply = request(&mut s, "windows");
    let line = reply
        .lines()
        .find(|line| line.starts_with("tricky\t"))
        .expect("listed");
    let columns: Vec<&str> = line.split('\t').collect();
    assert_eq!(columns.len(), 5, "still five columns: {line:?}");
    assert_eq!(columns[1], "a b c");
}

/// `focus <id>` moves the focus, and it is the only write verb: anything else
/// is refused, so the socket can never run a program or an arbitrary action.
#[test]
fn focus_is_the_only_write_verb() {
    let (mut s, mut sv, _windows) = scene(2);
    assert_eq!(
        s.state
            .wm
            .focused_window()
            .and_then(|w| w.identifier.clone()),
        Some("id-1".to_string())
    );

    assert_eq!(request(&mut s, "focus id-0"), "ok\n");
    s.manage(&mut sv);
    assert_eq!(
        s.state
            .wm
            .focused_window()
            .and_then(|w| w.identifier.clone()),
        Some("id-0".to_string())
    );
    let after = request(&mut s, "windows");
    assert!(
        after
            .lines()
            .next()
            .is_some_and(|line| line.starts_with("id-0\t")),
        "the newly focused window is now the most recent"
    );

    for refused in [
        "spawn alacritty",
        "close_window",
        "close",
        "exit",
        "windows extra",
        "focus",
        "focus a b",
        "",
    ] {
        let reply = request(&mut s, refused);
        assert!(
            reply.starts_with("error:"),
            "{refused:?} must be refused, got {reply:?}"
        );
    }
}

/// `focus <id>` reaches a window on another workspace of the same output: the
/// menu lists every workspace, so picking one has to move there. The workspace
/// left behind becomes the previous workspace, as with a keybinding switch.
#[test]
fn focus_reaches_a_window_on_another_workspace() {
    let (mut s, mut sv, _windows) = scene(2);
    // id-1 onto workspace 2, then back to workspace 1, where only id-0 lives.
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::MoveWindowToWorkspaceNumber(2),
    );
    s.manage(&mut sv);
    crate::keybinding::dispatch_action(
        &mut s.state,
        &crate::actions::KeybindingAction::FocusWorkspaceNumber(1),
    );
    s.manage(&mut sv);
    assert_eq!(s.state.wm.current_ws_idx(), Some((0, 0)));
    assert!(
        request(&mut s, "windows")
            .lines()
            .any(|line| line.starts_with("id-1\t") && line.contains("\tA:2\t")),
        "the other workspace is listed"
    );

    assert_eq!(request(&mut s, "focus id-1"), "ok\n");
    s.manage(&mut sv);

    assert_eq!(
        s.state.wm.current_ws_idx(),
        Some((0, 1)),
        "focus follows the window to its workspace"
    );
    assert_eq!(
        s.state
            .wm
            .focused_window()
            .and_then(|w| w.identifier.as_deref()),
        Some("id-1")
    );
    assert_eq!(
        s.state
            .wm
            .previous_workspace
            .map(|p| (p.output_idx, p.workspace_idx)),
        Some((0, 0))
    );
    s.check_consistent();
}

/// An identifier that never existed, and one whose window has closed, are both
/// errors: a menu must not be able to hit a different window by mistake.
#[test]
fn focus_refuses_ids_it_cannot_resolve() {
    let (mut s, mut sv, windows) = scene(2);
    let reply = request(&mut s, "focus nope");
    assert!(
        reply.starts_with("error: no window with id nope"),
        "{reply:?}"
    );

    // Close "id-1" and ask for it again: a closed id must never resolve.
    assert_eq!(
        s.state.wm.outputs[0].workspace_list[0].window_list[1]
            .identifier
            .as_deref(),
        Some("id-1")
    );
    s.send(&mut sv, windows[1].clone(), EVT_WIN_CLOSED, vec![]);
    s.manage(&mut sv);

    let reply = request(&mut s, "focus id-1");
    assert!(
        reply.starts_with("error: no window with id id-1"),
        "a closed id must not resolve: {reply:?}"
    );
    s.check_consistent();
}

/// The socket file is 0600 in a 0700 directory, a stale file does not stop the
/// next start, and a local peer is accepted by the uid check.
#[test]
fn the_socket_is_private_and_replaceable() {
    let dir = std::env::temp_dir().join(format!("tailrace-ctl-{}", std::process::id()));
    let path = dir.join("tailrace").join("ctl.sock");

    let listener = ctl::bind_listener(&path).expect("bind");
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(path.parent().unwrap()), 0o700, "directory");
    assert_eq!(mode(&path), 0o600, "socket");
    drop(listener);

    // A leftover socket from a dead session must not block the next one.
    let listener = ctl::bind_listener(&path).expect("rebind over a stale socket");
    drop(listener);

    let (a, b) = UnixStream::pair().unwrap();
    assert!(ctl::peer_uid_ok(&a), "our own uid is accepted");
    drop(b);

    std::fs::remove_dir_all(&dir).unwrap();
}

/// A client that has not finished its request line is answered and dropped, not
/// waited for: the event loop must never sit in `accept`.
#[test]
fn a_partial_request_is_answered_and_dropped() {
    let (mut s, _sv, _windows) = scene(1);
    let (server_side, mut client_side) = UnixStream::pair().unwrap();
    client_side.write_all(b"windows").unwrap();
    // No trailing newline: the request still parses, and `handle_connection`
    // must return without waiting for more.
    ctl::handle_connection_for_test(server_side, &mut s.state);
    let mut reply = String::new();
    use std::io::Read;
    client_side.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("id-0\t"), "got {reply:?}");
}

/// The accept loop serves a real connection and then returns instead of
/// sitting in `accept`: that is what keeps the event loop responsive.
#[test]
fn the_accept_loop_serves_a_real_connection() {
    let (mut s, _sv, _windows) = scene(2);
    let dir = std::env::temp_dir().join(format!("tailrace-ctl-accept-{}", std::process::id()));
    let path = dir.join("tailrace").join("ctl.sock");
    let listener = ctl::bind_listener(&path).expect("bind");

    let mut client = UnixStream::connect(&path).expect("connect");
    client.write_all(b"windows\n").expect("write");
    ctl::serve(&listener, &mut s.state);

    let mut reply = String::new();
    use std::io::Read;
    client.read_to_string(&mut reply).expect("read");
    let lines: Vec<&str> = reply.lines().collect();
    assert_eq!(lines.len(), 2, "both windows: {reply:?}");
    assert!(lines[0].starts_with("id-1\t"));

    std::fs::remove_dir_all(&dir).unwrap();
}
