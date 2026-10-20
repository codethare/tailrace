// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Control socket: a read-only window snapshot and a whitelist of write verbs.
//!
//! The protocol is one request line in, a reply out, then the connection
//! closes: no framing, no JSON, no persistent session. That is enough for a
//! menu (`tailrace ctl windows`, pick a line, `tailrace ctl focus <id>`) and it
//! keeps the server free of a client state machine.
//!
//! Everything here is deliberately small and one-shot because the listener
//! lives in the window manager's event loop: a slow or hostile client must
//! never be able to stall input. See `docs/architecture.md`.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use crate::app::AppData;
use crate::types::Status;
use crate::wm::WindowManager;

/// Longest request line accepted. The longest legal request is
/// `focus <32-byte identifier>`, so this is generous.
const MAX_REQUEST: usize = 256;
/// Connections served per wakeup, so a connect flood cannot keep us in the
/// accept loop instead of reading compositor input.
const MAX_ACCEPTS: usize = 8;

/// `$XDG_RUNTIME_DIR/tailrace/ctl.sock`, or None when the variable is unset.
pub fn socket_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty())?;
    Some(PathBuf::from(dir).join("tailrace").join("ctl.sock"))
}

/// Create the socket: directory 0700, socket 0600, stale file removed. Both
/// permissions are what makes the uid check meaningful to a local attacker who
/// can open the path.
pub fn bind_listener(path: &Path) -> std::io::Result<UnixListener> {
    let Some(dir) = path.parent() else {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "control socket path has no directory",
        ));
    };
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    // Only this session's window manager owns the path, and a leftover socket
    // from a previous run would make bind fail.
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != ErrorKind::NotFound
    {
        return Err(e);
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// The peer must be us. The socket permissions already keep other users out;
/// this is the check that does not depend on the filesystem.
pub fn peer_uid_ok(stream: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;

    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` is a `ucred` of the size passed in, and the fd is a live
    // unix socket for the duration of the call.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    result == 0 && cred.uid == unsafe { libc::geteuid() }
}

/// Serve whatever is waiting, without blocking. Called from the event loop when
/// the listener reads readable.
pub fn serve(listener: &UnixListener, state: &mut AppData) {
    for _ in 0..MAX_ACCEPTS {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        handle_connection(stream, state);
    }
}

/// `handle_connection` for the harness: the event loop is the only production
/// caller, and the scenarios need to drive one connection without a listener.
#[cfg(test)]
pub(crate) fn handle_connection_for_test(stream: UnixStream, state: &mut AppData) {
    handle_connection(stream, state);
}

/// Read one request line, answer it, close. The stream is non-blocking, so a
/// client that has not sent its line yet is dropped instead of waited for.
fn handle_connection(mut stream: UnixStream, state: &mut AppData) {
    if !peer_uid_ok(&stream) {
        return;
    }
    if stream.set_nonblocking(true).is_err() {
        return;
    }
    let mut buf = [0u8; MAX_REQUEST];
    let mut len = 0;
    while len < buf.len() {
        match stream.read(&mut buf[len..]) {
            Ok(0) => break,
            Ok(n) => {
                len += n;
                if buf[..len].contains(&b'\n') {
                    break;
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let request = String::from_utf8_lossy(&buf[..len]);
    let reply = handle_line(state, request.trim_end_matches(['\n', '\r']));
    // shortcut: the reply is written in one non-blocking pass, so it must fit
    // the socket buffer (64 KiB, i.e. hundreds of windows); upgrade to a queued
    // writer if that ever stops being true.
    let _ = stream.write_all(reply.as_bytes());
}

/// Serve one request. This is the whole verb whitelist: anything else is
/// refused, so the socket can never become a way to run an arbitrary action or
/// a program.
pub fn handle_line(state: &mut AppData, request: &str) -> String {
    let (verb, argument) = match request.split_once(' ') {
        // Identifiers are opaque: the rest of the line is the id, spaces and all.
        Some((verb, argument)) => (verb, argument.trim()),
        None => (request, ""),
    };
    match verb {
        "windows" if argument.is_empty() => snapshot(&state.wm),
        "windows" => "error: usage: windows\n".to_string(),
        "focus" => {
            if argument.is_empty() {
                return "error: usage: focus <id>\n".to_string();
            }
            focus(state, argument)
        }
        "" => "error: empty request\n".to_string(),
        other => format!("error: unknown verb {other}\n"),
    }
}

/// `focus <id>`: the only write verb. It moves the focus through the same state
/// a keybinding would set, so the next manage pass commits it the usual way.
fn focus(state: &mut AppData, id: &str) -> String {
    if state.wm.window_by_id(id).is_none() {
        return format!("error: no window with id {id}\n");
    }
    if !crate::keybinding::focus_window_id(state, id) {
        // The window exists but is already focused: not an error.
        return "ok\n".to_string();
    }
    crate::layout::update(&mut state.wm);
    state.wm.status = Status::Layout;
    state.manage_dirty();
    "ok\n".to_string()
}

/// One line per window, most recently used first, columns separated by tabs:
///
/// ```text
/// id <TAB> title <TAB> app_id <TAB> output:workspace <TAB> flags
/// ```
///
/// `-` stands for an empty field. Flags are `focused`, `floating`,
/// `fullscreen`, `maximized`, `sticky`, `closing`, comma separated.
pub fn snapshot(wm: &WindowManager) -> String {
    let mut out = String::new();
    let mut written: Vec<&str> = Vec::new();
    // MRU first: that order is what makes this useful to a switcher.
    let ordered = wm
        .mru
        .iter()
        .map(String::as_str)
        .chain(windows_in_layout_order(wm));
    for id in ordered {
        if written.contains(&id) {
            continue;
        }
        let Some((output_idx, workspace_idx, window_idx)) = wm.locate_window_id(id) else {
            continue;
        };
        written.push(id);
        push_row(&mut out, wm, output_idx, workspace_idx, window_idx);
    }
    out
}

/// Every identifier in layout order, for windows the MRU list has not seen yet
/// (a window mapped between two manage passes is not in it).
fn windows_in_layout_order(wm: &WindowManager) -> Vec<&str> {
    let mut ids = Vec::new();
    for output in &wm.outputs {
        for workspace in &output.workspace_list {
            for window in &workspace.window_list {
                if let Some(id) = window.identifier.as_deref() {
                    ids.push(id);
                }
            }
        }
    }
    ids
}

fn push_row(
    out: &mut String,
    wm: &WindowManager,
    output_idx: usize,
    workspace_idx: usize,
    window_idx: usize,
) {
    let output = &wm.outputs[output_idx];
    let workspace = &output.workspace_list[workspace_idx];
    let window = &workspace.window_list[window_idx];

    let id = window.identifier.as_deref().unwrap_or("-");
    let title = window.title.as_deref().unwrap_or("-");
    let app_id = window.app_id.as_deref().unwrap_or("-");
    let output_name = output
        .name
        .clone()
        .unwrap_or_else(|| format!("#{output_idx}"));
    let workspace_label = format!("{output_name}:{}", workspace_idx + 1);

    let mut flags = Vec::new();
    let focused = wm.focused_output_idx == Some(output_idx)
        && output.focused_workspace_idx == workspace_idx
        && workspace.focused_window_idx == Some(window_idx);
    if focused {
        flags.push("focused");
    }
    if window.geom.is_floating {
        flags.push("floating");
    }
    if window.geom.is_fullscreen {
        flags.push("fullscreen");
    }
    if window.geom.is_maximized_to_edges {
        flags.push("maximized");
    }
    if window.geom.is_sticky {
        flags.push("sticky");
    }
    if window.geom.is_closing {
        flags.push("closing");
    }
    let flags = if flags.is_empty() {
        "-".to_string()
    } else {
        flags.join(",")
    };

    // A title may contain tabs or newlines; one line per window must stay true.
    out.push_str(&sanitize(id));
    out.push('\t');
    out.push_str(&sanitize(title));
    out.push('\t');
    out.push_str(&sanitize(app_id));
    out.push('\t');
    out.push_str(&sanitize(&workspace_label));
    out.push('\t');
    out.push_str(&sanitize(&flags));
    out.push('\n');
}

/// Replace control characters so a value cannot break the line format.
fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c == '\t' || c == '\n' || c == '\r' {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// Run the client half: connect, send the request, print the reply.
pub fn run_client(request: &str) -> Result<String, String> {
    let path = socket_path().ok_or("XDG_RUNTIME_DIR is not set")?;
    let mut stream = UnixStream::connect(&path)
        .map_err(|e| format!("cannot connect to {}: {e}", path.display()))?;
    stream
        .write_all(format!("{request}\n").as_bytes())
        .map_err(|e| format!("cannot send the request: {e}"))?;
    let mut reply = String::new();
    stream
        .read_to_string(&mut reply)
        .map_err(|e| format!("cannot read the reply: {e}"))?;
    Ok(reply)
}
