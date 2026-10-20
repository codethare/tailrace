// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Entry point: connection, initial setup, event loop. The manage cycle
//! lives in `app::manage` (dispatched from river_window_manager_v1 events).

mod actions;
mod app;
mod config;
mod ctl;
mod keybinding;
mod layout;
mod output;
mod overview;
mod river;
mod seat;
mod spawn;
mod types;
mod window;
mod wm;

#[cfg(test)]
mod hotplug;

use wayland_client::{Connection, QueueHandle};

use crate::app::AppData;
use crate::wm::WindowManager;

const USAGE: &str = "\
tailrace - tiny scrolling window manager for river

usage: tailrace [-c <path>] [--check-config]
       tailrace ctl windows
       tailrace ctl focus <id>

options:
  -c, --config <path>  read this file instead of searching
                       $XDG_CONFIG_HOME/tailrace/config.toml and ~/.config/tailrace/config.toml
  --check-config       parse the config, report errors and exit
  -h, --help           print this help and exit
  -V, --version        print the version and exit

ctl talks to the running window manager over $XDG_RUNTIME_DIR/tailrace/ctl.sock:
  windows              one line per window, most recently used first
  focus <id>           focus the window with that id
";

struct Args {
    config: Option<std::path::PathBuf>,
    check_config: bool,
    /// Words after `ctl`: a client request instead of starting the window
    /// manager.
    ctl: Option<Vec<String>>,
}

/// Prints help/version and returns None when the process should exit early.
fn parse_args() -> Result<Option<Args>, String> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from<I: Iterator<Item = String>>(mut args: I) -> Result<Option<Args>, String> {
    let mut config = None;
    let mut check_config = false;
    let mut ctl = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            // Everything after `ctl` is the request, so the config flags cannot
            // be mixed in with it.
            "ctl" => {
                ctl = Some(args.by_ref().collect());
                break;
            }
            "-c" | "--config" => {
                let Some(path) = args.next() else {
                    return Err(format!("{arg} needs a path"));
                };
                config = Some(std::path::PathBuf::from(path));
            }
            "--check-config" => check_config = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("tailrace {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(Some(Args {
        config,
        check_config,
        ctl,
    }))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => return Ok(()),
        Err(message) => {
            eprintln!("tailrace: {message}\n{USAGE}");
            std::process::exit(2);
        }
    };
    // `ctl` talks to a running window manager: no Wayland connection, no
    // config.
    if let Some(request) = &args.ctl {
        if request.is_empty() {
            eprintln!("tailrace: usage: tailrace ctl <windows|focus <id>>");
            std::process::exit(2);
        }
        match ctl::run_client(&request.join(" ")) {
            Ok(reply) => {
                print!("{reply}");
                if reply.starts_with("error:") {
                    std::process::exit(1);
                }
                return Ok(());
            }
            Err(message) => {
                eprintln!("tailrace: {message}");
                std::process::exit(1);
            }
        }
    }
    // Loaded before the Wayland connection so `--check-config` works without
    // a compositor. Config errors are reported through `Display` (the toml
    // error names the line), not as a nested io::Error Debug.
    let config = match config::load(args.config.as_deref()) {
        Ok(config) => config,
        Err(e) if args.check_config => {
            eprintln!("tailrace: {e}");
            std::process::exit(1);
        }
        Err(e) => return Err(e.into()),
    };
    if args.check_config {
        println!("tailrace: config ok");
        return Ok(());
    }
    // Auto-reap children without breaking waitpid() in spawned programs.
    // SA_NOCLDWAIT alone prevents zombies while preserving waitpid()
    // semantics for children that use fork()+waitpid() internally
    // (wmenu, shells, etc.). Using SIG_IGN would be inherited by children
    // and break their waitpid().
    unsafe {
        let sa: libc::sigaction = libc::sigaction {
            sa_sigaction: libc::SIG_DFL,
            sa_mask: std::mem::zeroed(),
            sa_flags: libc::SA_NOCLDWAIT | libc::SA_RESTART,
            sa_restorer: None,
        };
        libc::sigaction(libc::SIGCHLD, &sa, std::ptr::null_mut());
    }
    // Die when our parent (typically river -c tailrace) dies, so we don't get
    // reparented to init and outlive the session if river crashes or is
    // killed.
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
    }

    let conn = Connection::connect_to_env()?;
    let display = conn.display();
    let mut event_queue = conn.new_event_queue();
    let qh: QueueHandle<AppData> = event_queue.handle();
    let registry = display.get_registry(&qh, ());

    let mut state = AppData {
        registry,
        river_wm: None,
        river_xkb: None,
        river_layer_shell: None,
        seats: Vec::new(),
        config_path: args.config,
        warned_missing_seat: false,
        wl_seat_version: 0,
        cursor_shape_manager: None,
        wm: WindowManager::new(config),
        qh: qh.clone(),
    };

    // Roundtrip to process the registry globals and bind river interfaces.
    event_queue.roundtrip(&mut state)?;
    // Required: without them tailrace cannot manage windows or bind keys. Exit
    // non-zero so a failed session start is visible to whoever launched it.
    if state.river_wm.is_none() {
        return Err("river_window_manager_v1 global not found".into());
    }
    if state.river_xkb.is_none() {
        return Err("river_xkb_bindings_v1 global not found".into());
    }
    if state.river_layer_shell.is_none() {
        // Optional (rill-ed behaves the same): without it layer-shell focus is
        // not tracked, so exclusive keyboard focus from a bar is not honored.
        eprintln!("tailrace: river_layer_shell_v1 missing, layer-shell focus tracking disabled");
    }

    // Listen before the loop: a menu started by `spawn_at_startup` must already
    // find the socket.
    let listener = match ctl::socket_path() {
        Some(path) => match ctl::bind_listener(&path) {
            Ok(listener) => Some(listener),
            Err(e) => {
                // Not fatal: the window manager still works without a menu.
                eprintln!("tailrace: cannot open the control socket: {e}");
                None
            }
        },
        None => {
            eprintln!("tailrace: XDG_RUNTIME_DIR is not set, control socket disabled");
            None
        }
    };

    for command in state.wm.config.spawn_at_startup.clone() {
        if let Err(e) = spawn::spawn_detached(&command) {
            eprintln!("tailrace: cannot run spawn_at_startup {command:?}: {e}");
        }
    }

    // Two fds: the compositor and the control socket. Polling both is what lets
    // a menu talk to us without blocking input; a poll on one fd is the old
    // `blocking_dispatch` shape.
    let ctl_fd = listener.as_ref().map(|listener| {
        use std::os::fd::AsRawFd;
        listener.as_raw_fd()
    });
    loop {
        // Commit anything we queued (requests, redraws), then wait.
        event_queue.dispatch_pending(&mut state)?;
        conn.flush()?;
        if state.wm.should_exit_loop {
            break;
        }

        let mut fds = [
            libc::pollfd {
                fd: {
                    use std::os::fd::{AsFd, AsRawFd};
                    event_queue.as_fd().as_raw_fd()
                },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: ctl_fd.unwrap_or(-1),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let count = if ctl_fd.is_some() { 2 } else { 1 };
        // SAFETY: `fds` has `count` initialised entries and poll only writes to
        // their `revents` fields.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), count, -1) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(std::io::Error::last_os_error().into());
        }

        if fds[0].revents != 0 {
            // A hangup or error means river is gone: keep it loud instead of
            // polling a dead fd in a loop.
            if fds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                return Err("the compositor connection was closed".into());
            }
            // The compositor has data: read it into the queue, then deliver it.
            let Some(guard) = conn.prepare_read() else {
                return Err("the compositor connection is already being read".into());
            };
            guard.read()?;
            event_queue.dispatch_pending(&mut state)?;
        }
        if fds[1].revents != 0
            && let Some(listener) = &listener
        {
            ctl::serve(listener, &mut state);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Option<Args>, String> {
        parse_args_from(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn config_path_is_parsed() {
        let parsed = args(&["-c", "/tmp/x.toml"]).unwrap().unwrap();
        assert_eq!(parsed.config, Some(std::path::PathBuf::from("/tmp/x.toml")));
        let parsed = args(&["--config", "/tmp/y.toml"]).unwrap().unwrap();
        assert_eq!(parsed.config, Some(std::path::PathBuf::from("/tmp/y.toml")));
        assert!(args(&[]).unwrap().unwrap().config.is_none());
    }

    #[test]
    fn help_and_version_exit_early() {
        assert!(args(&["--help"]).unwrap().is_none());
        assert!(args(&["-V"]).unwrap().is_none());
    }

    #[test]
    fn check_config_flag_is_parsed() {
        assert!(args(&["--check-config"]).unwrap().unwrap().check_config);
        assert!(!args(&[]).unwrap().unwrap().check_config);
        assert!(args(&["-V"]).unwrap().is_none());
    }

    #[test]
    fn bad_arguments_are_rejected() {
        assert!(args(&["--config"]).is_err(), "missing path");
        assert!(args(&["--nope"]).is_err(), "unknown flag");
    }
}
