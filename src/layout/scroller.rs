// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: 0BSD

//! Scrolling column layout, ported from rill-ed layout/scroller.zig.
//!
//! Operates on the pure geometry view (`WindowGeom`) of a workspace's
//! windows; the caller maps to/from the real windows and snaps `current`
//! to `finish`.

use crate::layout::common;
use crate::types::{CenterFocused, Config, Rectangle, WindowGeom};

pub fn apply(
    windows: &mut [WindowGeom],
    focused_window_idx: Option<usize>,
    on_overflow_center: bool,
    output_rect: Rectangle,
    non_exclusive: Rectangle,
    config: &Config,
    y_offset: i32,
) {
    for window in windows.iter_mut() {
        if window.is_floating {
            // A floating window that goes fullscreen still fills the output.
            // rill-ed leaves it at its rest rect here, which only looks right
            // for the focused window (the compositor owns that one).
            let mut finish = if window.is_fullscreen {
                output_rect
            } else {
                window.floating
            };
            finish.y += y_offset;
            window.finish = Some(finish);
        }
    }

    if focused_window_idx.is_some_and(|idx| idx >= windows.len()) {
        return;
    }
    // Nothing tiled to focus: the focused window is floating, or the workspace
    // has no focus at all (a sticky window pulled onto an empty one). Either
    // way the strip is anchored on the first tiled window instead of following
    // the focus, so every window still gets a layout.
    let focused_window_idx = focused_window_idx.filter(|idx| !windows[*idx].is_floating);
    let window_count = windows.len();

    // Floating windows take no space in the strip; only tiled windows count
    // for the single-window fill and .single centering below.
    let tiled_count = windows.iter().filter(|w| !w.is_floating).count();

    let should_center = match config.center_focused_window {
        CenterFocused::Never => false,
        CenterFocused::Always => true,
        CenterFocused::Single => tiled_count == 1,
        CenterFocused::OnOverflow => on_overflow_center,
    };

    let mut rectangle;

    if let Some(focused_window_idx) = focused_window_idx {
        rectangle = focused_window_layout(
            &windows[focused_window_idx],
            output_rect,
            non_exclusive,
            config,
            y_offset,
            should_center,
        );
        windows[focused_window_idx].finish = Some(rectangle);

        // Unfocused windows to the right of focused
        rectangle.x += rectangle.width + config.horizontal_gap;
        for window in windows[focused_window_idx + 1..].iter_mut() {
            if window.is_floating {
                continue;
            }
            unfocused_window_layout(
                window,
                &mut rectangle,
                output_rect,
                non_exclusive,
                config,
                y_offset,
            );
            window.finish = Some(rectangle);
            rectangle.x += rectangle.width + config.horizontal_gap;
        }

        // Unfocused windows to the left of focused
        rectangle.x = windows[focused_window_idx].finish.unwrap().x;
        let mut window_idx = focused_window_idx;
        while window_idx > 0 {
            window_idx -= 1;
            let window = &mut windows[window_idx];
            if window.is_floating {
                continue;
            }
            unfocused_window_layout(
                window,
                &mut rectangle,
                output_rect,
                non_exclusive,
                config,
                y_offset,
            );
            rectangle.x -= config.horizontal_gap + rectangle.width;
            window.finish = Some(rectangle);
        }
    } else {
        // No focused tile: lay out every tiled window from the first one.
        // Keep anchor at its current.x (no left-edge clamp) so the strip
        // preserves its scroll position instead of jumping back to the
        // leftmost window.
        let Some(anchor_idx) = windows.iter().position(|w| !w.is_floating) else {
            return;
        };

        let base_width = (non_exclusive.width - config.horizontal_gap) as f32;
        let width_with_gap = (base_width * windows[anchor_idx].proportion) as i32;

        rectangle = Rectangle {
            width: width_with_gap - config.horizontal_gap,
            height: non_exclusive.height - 2 * config.vertical_gap,
            x: windows[anchor_idx].current.x,
            y: non_exclusive.y + config.vertical_gap + y_offset,
        };

        if should_center {
            rectangle.x = non_exclusive.x + non_exclusive.width / 2 - rectangle.width / 2;
        }

        if windows[anchor_idx].is_fullscreen {
            rectangle = output_rect;
            rectangle.y += y_offset;
        }
        windows[anchor_idx].finish = Some(rectangle);

        let mut i = (anchor_idx + 1) % window_count;
        while i != anchor_idx {
            let window = &mut windows[i];
            if !window.is_floating {
                rectangle.x += rectangle.width + config.horizontal_gap;
                unfocused_window_layout(
                    window,
                    &mut rectangle,
                    output_rect,
                    non_exclusive,
                    config,
                    y_offset,
                );
                window.finish = Some(rectangle);
            }
            i = (i + 1) % window_count;
        }
    }

    if !should_center {
        snap_to_edge(windows, non_exclusive, config.horizontal_gap);
    }

    for window in windows.iter_mut() {
        common::skip_if_at_rest(window);
    }
}

/// niri's `on-overflow` test: do the focused window and the window the focus
/// came from *not* fit on the output together? niri takes the neighbour of the
/// focused window toward the previous one, so the two can be several columns
/// apart. Floating windows take no space in the strip and are skipped the same
/// way the layout skips them.
pub fn overflows(
    windows: &[WindowGeom],
    focused_idx: Option<usize>,
    previous_idx: Option<usize>,
    non_exclusive: Rectangle,
    config: &Config,
) -> bool {
    // First pass for this workspace, or the focus never moved: niri fits.
    let (Some(focused_idx), Some(previous_idx)) = (focused_idx, previous_idx) else {
        return false;
    };
    if previous_idx == focused_idx || focused_idx >= windows.len() {
        return false;
    }

    let step: isize = if previous_idx > focused_idx { 1 } else { -1 };
    let mut idx = focused_idx as isize + step;
    let neighbour = loop {
        if idx < 0 || idx as usize >= windows.len() {
            return false;
        }
        let window = &windows[idx as usize];
        if !window.is_floating {
            break window;
        }
        idx += step;
    };

    // Each width is computed with a gap folded in, so the pair plus the gaps
    // around and between them come to a single `horizontal_gap`.
    let base_width = (non_exclusive.width - config.horizontal_gap) as f32;
    let neighbour_width = (base_width * neighbour.proportion) as i32;
    let focused_width = (base_width * windows[focused_idx].proportion) as i32;
    neighbour_width + focused_width + config.horizontal_gap > non_exclusive.width
}

fn focused_window_layout(
    window: &WindowGeom,
    output_rect: Rectangle,
    non_exclusive: Rectangle,
    config: &Config,
    y_offset: i32,
    should_center: bool,
) -> Rectangle {
    let base_width = (non_exclusive.width - config.horizontal_gap) as f32;
    let width_with_gap = (base_width * window.proportion) as i32;

    let mut rectangle = Rectangle {
        width: width_with_gap - config.horizontal_gap,
        height: non_exclusive.height - 2 * config.vertical_gap,
        x: window.current.x,
        y: non_exclusive.y + config.vertical_gap + y_offset,
    };

    if should_center {
        rectangle.x = non_exclusive.x + non_exclusive.width / 2 - rectangle.width / 2;
    } else if rectangle.x < non_exclusive.x + config.horizontal_gap {
        rectangle.x = non_exclusive.x + config.horizontal_gap;
    } else if rectangle.x + width_with_gap > non_exclusive.x + non_exclusive.width {
        rectangle.x = (non_exclusive.x + non_exclusive.width - width_with_gap)
            .max(non_exclusive.x + config.horizontal_gap);
    }

    if window.is_fullscreen {
        rectangle = output_rect;
        rectangle.y += y_offset;
    }
    rectangle
}

fn unfocused_window_layout(
    window: &WindowGeom,
    rectangle: &mut Rectangle,
    output_rect: Rectangle,
    non_exclusive: Rectangle,
    config: &Config,
    y_offset: i32,
) {
    if window.is_fullscreen {
        rectangle.width = output_rect.width;
        rectangle.height = output_rect.height;
        rectangle.y = output_rect.y + y_offset;
    } else {
        let base_width = (non_exclusive.width - config.horizontal_gap) as f32;
        let width_with_gap = (base_width * window.proportion) as i32;

        rectangle.width = width_with_gap - config.horizontal_gap;
        rectangle.height = non_exclusive.height - 2 * config.vertical_gap;
        rectangle.y = non_exclusive.y + config.vertical_gap + y_offset;
    }
}

fn snap_to_edge(windows: &mut [WindowGeom], non_exclusive: Rectangle, gap: i32) {
    if windows.is_empty() {
        return;
    }

    let Some(head_idx) = windows.iter().position(|w| !w.is_floating) else {
        return;
    };
    let Some(head_finish) = windows[head_idx].finish else {
        return;
    };

    let Some(tail_idx) = windows.iter().rposition(|w| !w.is_floating) else {
        return;
    };
    let Some(tail_finish) = windows[tail_idx].finish else {
        return;
    };

    let left = non_exclusive.x + gap;
    let head_distance = (head_finish.x > left).then(|| head_finish.x - left);

    let right = non_exclusive.x + non_exclusive.width - gap;
    let tail_end = tail_finish.x + tail_finish.width;
    let tail_distance = (tail_end < right).then(|| (right - tail_end).min(left - head_finish.x));

    for window in windows.iter_mut() {
        if window.is_floating {
            continue;
        }
        if let Some(finish) = &mut window.finish {
            if let Some(distance) = head_distance {
                finish.x -= distance;
            } else if let Some(distance) = tail_distance {
                finish.x += distance;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WindowGeom;

    const OUTPUT: Rectangle = Rectangle {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    const NON_EXCLUSIVE: Rectangle = Rectangle {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };

    fn apply(windows: &mut [WindowGeom], focused: Option<usize>, config: &Config) {
        super::apply(windows, focused, false, OUTPUT, NON_EXCLUSIVE, config, 0);
    }

    /// Mirrors the coordinator's snap: settle windows at their finish.
    fn settle(windows: &mut [WindowGeom]) {
        for w in windows.iter_mut() {
            if let Some(finish) = w.finish {
                w.current = finish;
                w.sent_current = Some(finish);
                w.finish = None;
            }
        }
    }

    fn tiled(proportion: f32) -> WindowGeom {
        WindowGeom::new(proportion, Rectangle::default())
    }

    #[test]
    fn floating_window_frees_slot_last_tiled_keeps_proportion() {
        let config = Config::default();
        let mut windows = vec![tiled(0.5), tiled(0.5)];
        let focused = Some(1);

        apply(&mut windows, focused, &config);
        settle(&mut windows);

        // Toggle the focused window floating (mirrors the keybinding handler:
        // current is NOT snapped to floating).
        windows[1].is_floating = true;
        windows[1].floating = Rectangle {
            x: 460,
            y: 190,
            width: 1000,
            height: 700,
        };

        apply(&mut windows, focused, &config);

        // The remaining tiled window keeps its stored proportion; new windows
        // do not change an existing column's width.
        let a = &windows[0];
        let base_width = (NON_EXCLUSIVE.width - config.horizontal_gap) as f32;
        let expected_width_with_gap = (base_width * a.proportion) as i32;
        let a_rect = a.finish.unwrap_or(a.current);
        assert_eq!(
            a_rect.width,
            expected_width_with_gap - config.horizontal_gap
        );
        // The floating window keeps its floating rectangle.
        assert!(windows[1].finish.unwrap().eql(windows[1].floating));
        // Stored proportions are untouched so un-floating restores the split.
        assert_eq!(windows[0].proportion, 0.5);
    }

    #[test]
    fn spawn_floating_while_focus_scrolled_right_preserves_strip_position() {
        let config = Config::default();
        let mut windows = vec![tiled(0.34), tiled(0.34), tiled(0.34)];

        apply(&mut windows, Some(2), &config);
        settle(&mut windows);

        let before: Vec<i32> = windows.iter().map(|w| w.current.x).collect();

        windows.push(WindowGeom {
            proportion: 0.5,
            is_floating: true,
            floating: Rectangle {
                x: 460,
                y: 190,
                width: 1000,
                height: 700,
            },
            current: Rectangle {
                x: 460,
                y: 190,
                width: 1000,
                height: 700,
            },
            ..WindowGeom::new(0.5, Rectangle::default())
        });

        apply(&mut windows, Some(3), &config);

        for (i, w) in windows.iter().take(3).enumerate() {
            assert_eq!(w.finish.unwrap_or(w.current).x, before[i], "window {i}");
        }
    }

    #[test]
    fn new_tiled_window_snaps_in_from_offscreen_right_edge() {
        let config = Config::default();

        // Mirrors window.add(): current starts just past the right edge.
        let mut start = common::initial_rectangle(NON_EXCLUSIVE, &config);
        start.x = NON_EXCLUSIVE.x + NON_EXCLUSIVE.width;

        let mut windows = vec![tiled(0.5)];
        windows[0].current = start;
        windows[0].floating = start;

        apply(&mut windows, Some(0), &config);

        let w = &windows[0];
        let finish = w.finish.expect("finish computed");
        // Finish is the on-screen rightmost slot, not the off-screen start.
        assert!(finish.x < NON_EXCLUSIVE.x + NON_EXCLUSIVE.width);
        assert!(finish.x >= NON_EXCLUSIVE.x);
    }

    /// A workspace with windows but no focus still gets a layout. Sticky
    /// windows are pulled onto the focused workspace even when it has no
    /// focused window, and that must not freeze their geometry: left over from
    /// a larger output, the window still follows a smaller one.
    #[test]
    fn windows_without_focus_are_still_laid_out() {
        let config = Config::default();
        let stale = Rectangle {
            x: 9,
            y: 9,
            width: 946,
            height: 1062,
        };
        let mut windows = vec![tiled(0.5)];
        windows[0].current = stale;
        windows[0].floating = stale;

        let small = Rectangle {
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
        };
        super::apply(&mut windows, None, false, small, small, &config, 0);

        let finish = windows[0]
            .finish
            .expect("an unfocused workspace is still laid out");
        assert_eq!(finish.height, small.height - 2 * config.vertical_gap);
    }

    /// niri's `on-overflow`: center only when the newly focused window does
    /// not fit next to the one the focus came from. Two 0.5 windows exactly
    /// fill a 1920px output (three 9px gaps), so that pair must not overflow;
    /// 0.6 + 0.5 must. The comparison is taken toward the previous window, and
    /// floats do not take up space in the strip.
    #[test]
    fn overflows_only_when_the_pair_does_not_fit() {
        let config = Config::default();
        let pair = |a: f32, b: f32| vec![tiled(a), tiled(b)];

        // 0.5 + 0.5 fits exactly: 955 + 955 + 9 == 1919 <= 1920.
        let windows = pair(0.5, 0.5);
        assert!(!overflows(
            &windows,
            Some(1),
            Some(0),
            NON_EXCLUSIVE,
            &config
        ));
        assert!(!overflows(
            &windows,
            Some(0),
            Some(1),
            NON_EXCLUSIVE,
            &config
        ));

        let windows = pair(0.6, 0.5);
        assert!(overflows(
            &windows,
            Some(1),
            Some(0),
            NON_EXCLUSIVE,
            &config
        ));
        assert!(overflows(
            &windows,
            Some(0),
            Some(1),
            NON_EXCLUSIVE,
            &config
        ));

        // No previous focus, or none at all, never centers.
        let windows = pair(1.0, 1.0);
        assert!(!overflows(&windows, Some(1), None, NON_EXCLUSIVE, &config));
        assert!(!overflows(&windows, None, None, NON_EXCLUSIVE, &config));
        assert!(!overflows(
            &windows,
            Some(0),
            Some(0),
            NON_EXCLUSIVE,
            &config
        ));

        // The neighbour toward the previous window is the one that counts:
        // 0.4 + 0.5 fits, 0.6 + 0.5 does not.
        let windows = vec![tiled(0.4), tiled(0.5), tiled(0.6)];
        assert!(!overflows(
            &windows,
            Some(1),
            Some(0),
            NON_EXCLUSIVE,
            &config
        ));
        assert!(overflows(
            &windows,
            Some(1),
            Some(2),
            NON_EXCLUSIVE,
            &config
        ));

        // A float in between is skipped: 0.5 + 0.5 either side of it.
        let mut windows = vec![tiled(0.5), tiled(0.5), tiled(0.5)];
        windows[1].is_floating = true;
        assert!(!overflows(
            &windows,
            Some(2),
            Some(0),
            NON_EXCLUSIVE,
            &config
        ));
    }

    #[test]
    fn center_single_centers_lone_window() {
        let config = Config {
            center_focused_window: CenterFocused::Single,
            ..Config::default()
        };
        let mut windows = vec![tiled(0.5)];
        apply(&mut windows, Some(0), &config);
        let finish = windows[0].finish.unwrap();
        assert_eq!(finish.x, (1920 - finish.width) / 2);

        // With a second tiled window, no centering.
        windows.push(tiled(0.5));
        apply(&mut windows, Some(0), &config);
        assert_ne!(
            windows[0].finish.unwrap().x,
            (1920 - windows[0].finish.unwrap().width) / 2
        );
    }
}
