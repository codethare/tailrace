// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Core window-manager state, ported from rill-ed types.zig `WindowManager`.
//!
//! Protocol globals (river_window_manager, seat, xkb/pointer binding lists)
//! live in the dispatch layer (`AppData`); this struct holds the WM state and
//! config only, so pure logic here is testable without Wayland.

use std::collections::HashMap;

use crate::river::river_window_v1::RiverWindowV1;
use crate::types::{
    Config, DetachedOutput, Output, OverviewHome, OverviewState, PendingWindow, Status, Window,
    Workspace,
};

/// How many windows the MRU list remembers. A window closes rarely enough that
/// the list never grows past the number of open windows, but a cap keeps a
/// long session bounded.
const MRU_LIMIT: usize = 64;

/// A `cycle_focus_*` session: the MRU order frozen when the cycle started, so
/// every press moves one step in the same order even though focusing a window
/// reorders the live list.
pub struct MruCycle {
    pub order: Vec<String>,
    pub index: usize,
}

/// Layer-shell keyboard focus state of one seat. Per seat, because a layer
/// surface grabs keyboard focus on the seat it is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayerShellFocus {
    #[default]
    None,
    NonExclusive,
    Exclusive,
}

pub struct WindowManager {
    /// Shared so layout passes take a refcount bump instead of cloning the
    /// whole config (bindings, rules) on every manage cycle.
    pub config: std::rc::Rc<Config>,
    pub outputs: Vec<Output>,
    pub focused_output_idx: Option<usize>,
    pub previous_workspace: Option<OverviewHome>,
    pub pending_windows: Vec<PendingWindow>,
    /// Workspaces preserved when their output was removed, keyed by output
    /// name; restored when an output with the same name reappears.
    pub detached_outputs: HashMap<String, DetachedOutput>,
    pub overview_state: Option<OverviewState>,
    pub status: Status,
    pub needs_refocus: bool,
    pub needs_setup_bindings: bool,
    pub should_exit_loop: bool,
    /// Window that had focus when every output went away (suspend, TTY
    /// switch): the window to return to when the outputs come back, whatever
    /// workspace the output that reappears was showing.
    pub resume_focus: Option<RiverWindowV1>,
    /// True when the compositor has sent session_locked.
    pub session_locked: bool,
    /// Window focused when the session was locked, restored on unlock.
    pub lock_focus: Option<RiverWindowV1>,
    /// Set when the focused output changed because an output was
    /// added/removed; the next layout pass warps the pointer.
    pub needs_pointer_warp: bool,
    /// Window identifiers in most-recently-used order; the front is the focused
    /// window. Maintained by `layout::apply_focus_and_borders`, the one place a
    /// focus change is committed.
    pub mru: Vec<String>,
    /// Set while `cycle_focus_next`/`_prev` walks the list.
    pub mru_cycle: Option<MruCycle>,
}

impl WindowManager {
    pub fn new(config: Config) -> WindowManager {
        WindowManager {
            config: std::rc::Rc::new(config),
            outputs: Vec::new(),
            focused_output_idx: None,
            previous_workspace: None,
            pending_windows: Vec::new(),
            detached_outputs: HashMap::new(),
            overview_state: None,
            status: Status::None,
            needs_refocus: false,
            needs_setup_bindings: false,
            should_exit_loop: false,
            resume_focus: None,
            session_locked: false,
            lock_focus: None,
            needs_pointer_warp: false,
            mru: Vec::new(),
            mru_cycle: None,
        }
    }

    pub fn focused_output(&self) -> Option<&Output> {
        self.outputs.get(self.focused_output_idx?)
    }

    /// (output_idx, workspace_idx) of the focused workspace.
    pub fn current_ws_idx(&self) -> Option<(usize, usize)> {
        let output = self.focused_output()?;
        Some((self.focused_output_idx?, output.focused_workspace_idx))
    }

    pub fn workspace(&self, output_idx: usize, workspace_idx: usize) -> Option<&Workspace> {
        self.outputs
            .get(output_idx)?
            .workspace_list
            .get(workspace_idx)
    }

    pub fn workspace_mut(
        &mut self,
        output_idx: usize,
        workspace_idx: usize,
    ) -> Option<&mut Workspace> {
        self.outputs
            .get_mut(output_idx)?
            .workspace_list
            .get_mut(workspace_idx)
    }

    pub fn focused_window(&self) -> Option<&Window> {
        let (oi, wi) = self.current_ws_idx()?;
        self.workspace(oi, wi)?.focused_window()
    }

    pub fn focused_window_mut(&mut self) -> Option<&mut Window> {
        let (oi, wi) = self.current_ws_idx()?;
        self.workspace_mut(oi, wi)?.focused_window_mut()
    }

    /// Locate a window by its river proxy across all outputs/workspaces.
    /// Returns (output_idx, workspace_idx, window_idx).
    pub fn locate_window(&self, river_window: &RiverWindowV1) -> Option<(usize, usize, usize)> {
        for (output_idx, output) in self.outputs.iter().enumerate() {
            for (workspace_idx, workspace) in output.workspace_list.iter().enumerate() {
                if let Some(window_idx) = workspace
                    .window_list
                    .iter()
                    .position(|w| &w.river_window == river_window)
                {
                    return Some((output_idx, workspace_idx, window_idx));
                }
            }
        }
        None
    }

    /// Move a window between workspaces, adjusting both focus indices
    /// (rill-ed `moveWindowToWorkspace`). The moved window lands right after
    /// the target's focused window.
    pub fn move_window_to_workspace(
        &mut self,
        source: (usize, usize),
        target: (usize, usize),
        window_idx: usize,
    ) {
        // A move onto the window's own workspace would take the list out and
        // assign it back over itself, dropping the window: nothing to do.
        if source == target {
            return;
        }
        // mem::take dance: two &mut Workspace from the same [Workspace; 10]
        // cannot coexist, so the source list is taken out first.
        let (mut source_list, mut source_focus) = {
            let ws = self.workspace_mut(source.0, source.1);
            match ws {
                Some(ws) => (
                    std::mem::take(&mut ws.window_list),
                    std::mem::take(&mut ws.focused_window_idx),
                ),
                None => return,
            }
        };
        let Some(target_ws) = self.workspace_mut(target.0, target.1) else {
            // Put the source back untouched.
            let ws = self.workspace_mut(source.0, source.1).unwrap();
            ws.window_list = source_list;
            ws.focused_window_idx = source_focus;
            return;
        };
        move_window(
            &mut source_list,
            &mut source_focus,
            &mut target_ws.window_list,
            &mut target_ws.focused_window_idx,
            window_idx,
        );
        let ws = self.workspace_mut(source.0, source.1).unwrap();
        ws.window_list = source_list;
        ws.focused_window_idx = source_focus;
    }

    /// Locate a window by river's stable identifier.
    pub fn locate_window_id(&self, id: &str) -> Option<(usize, usize, usize)> {
        for (output_idx, output) in self.outputs.iter().enumerate() {
            for (workspace_idx, workspace) in output.workspace_list.iter().enumerate() {
                if let Some(window_idx) = workspace
                    .window_list
                    .iter()
                    .position(|w| w.identifier.as_deref() == Some(id))
                {
                    return Some((output_idx, workspace_idx, window_idx));
                }
            }
        }
        None
    }

    pub fn window_by_id(&self, id: &str) -> Option<&Window> {
        let (output_idx, workspace_idx, window_idx) = self.locate_window_id(id)?;
        self.outputs[output_idx].workspace_list[workspace_idx]
            .window_list
            .get(window_idx)
    }

    /// Make `id` the most recently used window. Called for the window a focus
    /// pass commits, which is the only way focus changes.
    pub fn mru_promote(&mut self, id: &str) {
        if self.mru.first().is_some_and(|front| front == id) {
            return;
        }
        self.mru.retain(|other| other != id);
        self.mru.insert(0, id.to_string());
        self.mru.truncate(MRU_LIMIT);
    }

    /// Forget a window that closed. A cycle in progress is dropped rather than
    /// patched: its frozen order would otherwise point at a window that is
    /// gone, and the next cycle is one keypress away.
    pub fn mru_remove(&mut self, id: &str) {
        self.mru.retain(|other| other != id);
        if let Some(cycle) = &self.mru_cycle
            && cycle.order.iter().any(|other| other == id)
        {
            self.mru_cycle = None;
        }
    }

    /// Sticky windows follow the focused workspace of their output. An
    /// invariant, not a hook on each switch path: it is idempotent, and the
    /// paths that change `focused_workspace_idx` stay unaware of stickiness.
    pub fn pull_sticky_windows(&mut self) {
        for output in self.outputs.iter_mut() {
            if output.is_removed {
                continue;
            }
            let target_idx = output.focused_workspace_idx;
            // Taken out first: two &mut Workspace from the same [Workspace;
            // 10] cannot coexist (see move_window_to_workspace).
            let mut target_list =
                std::mem::take(&mut output.workspace_list[target_idx].window_list);
            // Where a sticky window that was focused before the switch lands
            // in the target; only used when the target has no focus of its own,
            // so a sticky window never steals it from a window already there.
            let mut moved_focus = None;
            for (workspace_idx, workspace) in output.workspace_list.iter_mut().enumerate() {
                if workspace_idx == target_idx
                    || !workspace.window_list.iter().any(|w| w.geom.is_sticky)
                {
                    continue;
                }
                let was_focused = workspace.focused_window_idx;
                let mut kept = Vec::with_capacity(workspace.window_list.len());
                for (window_idx, window) in std::mem::take(&mut workspace.window_list)
                    .into_iter()
                    .enumerate()
                {
                    if window.geom.is_sticky {
                        if was_focused == Some(window_idx) {
                            moved_focus = Some(target_list.len());
                        }
                        target_list.push(window);
                    } else {
                        kept.push(window);
                    }
                }
                workspace.window_list = kept;
                if workspace.window_list.is_empty() {
                    workspace.focused_window_idx = None;
                } else if workspace
                    .focused_window_idx
                    .is_none_or(|f| f >= workspace.window_list.len())
                {
                    // The focused window itself was pulled out: keep the index
                    // at the same slot (or the last window).
                    workspace.focused_window_idx = Some(workspace.window_list.len() - 1);
                }
            }
            let target = &mut output.workspace_list[target_idx];
            target.window_list = target_list;
            if target.focused_window_idx.is_none() {
                target.focused_window_idx = moved_focus;
            }
        }
    }
}

/// Move `window_idx` from `source` to `target`, fixing up both focus indices.
/// Generic so the index semantics are testable without Wayland.
pub fn move_window<T>(
    source: &mut Vec<T>,
    source_focus: &mut Option<usize>,
    target: &mut Vec<T>,
    target_focus: &mut Option<usize>,
    window_idx: usize,
) {
    if window_idx >= source.len() {
        return;
    }
    let window = source.remove(window_idx);

    if source.is_empty() {
        *source_focus = None;
    } else if window_idx != 0 {
        *source_focus = Some(window_idx - 1);
    }

    let target_idx = target_focus.map_or(0, |i| i + 1);
    target.insert(target_idx, window);
    *target_focus = Some(target_idx);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn move_in_list(
        source: &mut Vec<i32>,
        source_focus: &mut Option<usize>,
        target: &mut Vec<i32>,
        target_focus: &mut Option<usize>,
        window_idx: usize,
    ) {
        move_window(source, source_focus, target, target_focus, window_idx);
    }

    #[test]
    fn move_window_adjusts_source_focus() {
        let mut src = vec![1, 2, 3];
        let mut src_focus = Some(2);
        let mut dst = vec![];
        let mut dst_focus: Option<usize> = None;

        move_in_list(&mut src, &mut src_focus, &mut dst, &mut dst_focus, 2);
        assert_eq!(src, vec![1, 2]);
        assert_eq!(src_focus, Some(1));
        assert_eq!(dst, vec![3]);
        assert_eq!(dst_focus, Some(0));
    }

    #[test]
    fn move_window_lands_after_target_focus() {
        let mut src = vec![1];
        let mut src_focus = Some(0);
        let mut dst = vec![7, 8, 9];
        let mut dst_focus = Some(1);

        move_in_list(&mut src, &mut src_focus, &mut dst, &mut dst_focus, 0);
        assert_eq!(src, vec![]);
        assert_eq!(src_focus, None);
        assert_eq!(dst, vec![7, 8, 1, 9]);
        assert_eq!(dst_focus, Some(2));
    }

    #[test]
    fn move_window_index_zero_keeps_focus_at_zero() {
        // rill-ed behavior: focus stays at position 0 (the next window
        // slides into focus) when the removed window was at index 0.
        let mut src = vec![1, 2];
        let mut src_focus = Some(0);
        let mut dst = vec![];
        let mut dst_focus: Option<usize> = None;

        move_in_list(&mut src, &mut src_focus, &mut dst, &mut dst_focus, 0);
        assert_eq!(src, vec![2]);
        assert_eq!(src_focus, Some(0));
    }

    #[test]
    fn move_window_out_of_bounds_is_noop() {
        let mut src = vec![1];
        let mut src_focus = Some(0);
        let mut dst: Vec<i32> = vec![];
        let mut dst_focus: Option<usize> = None;

        move_in_list(&mut src, &mut src_focus, &mut dst, &mut dst_focus, 5);
        assert_eq!(src, vec![1]);
        assert!(dst.is_empty());
    }
}
