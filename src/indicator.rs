// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Overflow card: a short-lived layer-shell surface that shows the focused
//! workspace's horizontal strip after the focus moves while windows are
//! scrolled off-screen.
//!
//! The card is drawn by tailrace itself (wl_shm + ARGB8888), so it needs
//! `wl_compositor`, `wl_shm` and `zwlr_layer_shell_v1`. They are optional: a
//! compositor that does not expose them simply never shows the card.
//!
//! Only the focused output gets a surface. It is destroyed when the focus has
//! no overflow, when the card belongs to another output, when overview opens,
//! when the feature is disabled, or after [`SHOW_FOR`]. Layout itself stays in
//! the geometry pipeline; this module only reads the committed geometry
//! (`WindowGeom::current`, after `snap_to_finish`).

use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use wayland_client::QueueHandle;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_surface::WlSurface;

use crate::app::AppData;
use crate::river::river_output_v1::RiverOutputV1;
use crate::river::river_window_v1::RiverWindowV1;
use crate::river::zwlr_layer_shell_v1::Layer;
use crate::river::zwlr_layer_surface_v1::{Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1};
use crate::types::{Color, Config, Rectangle};
use crate::wm::WindowManager;

/// How long the card stays after a focus move. No animations elsewhere, and
/// the card is informational only; this is long enough to read, short enough
/// not to sit in the way.
pub const SHOW_FOR: Duration = Duration::from_millis(1500);

const CARD_HEIGHT: i32 = 16;
const CARD_PADDING: f32 = 10.0;
const SIDE_MARGIN: i32 = 8;
const TOP_MARGIN: i32 = 8;
const MIN_CARD_WIDTH: i32 = 180;
const MAX_CARD_WIDTH: i32 = 360;
/// The workspace is one thin line; the viewport is a separate border around
/// the part of it that is on screen.
const TRACK_HEIGHT: f32 = 2.0;
const TRACK_Y: f32 = 7.0;
const VIEWPORT_HEIGHT: f32 = 12.0;
const VIEWPORT_Y: f32 = 2.0;
const VIEWPORT_INSET: f32 = 2.0;

pub struct Indicator {
    /// Focus the card reflects. Stored even while the feature is off, so
    /// enabling it later waits for the next focus move instead of popping up.
    focus: Option<FocusKey>,
    surface: Option<CardSurface>,
    hide_at: Option<Instant>,
    /// The first focus a session gets is not a focus *move*: record it without
    /// showing a card, so startup does not flash one.
    primed: bool,
}

struct FocusKey {
    output: usize,
    workspace: usize,
    window: Option<RiverWindowV1>,
    /// Windows in the focused workspace. A change here is a window opening or
    /// closing, not a focus move: the card must not pop up like a titlebar
    /// merely because a fourth window was mapped. Equality ignores this field.
    count: usize,
}

impl PartialEq for FocusKey {
    fn eq(&self, other: &Self) -> bool {
        self.output == other.output
            && self.workspace == other.workspace
            && self.window == other.window
    }
}

impl FocusKey {
    /// The focused workspace gained or lost a window, so the focus change came
    /// from mapping/unmapping rather than from a focus action.
    fn structurally_changed(&self, new: &FocusKey) -> bool {
        self.output == new.output && self.workspace == new.workspace && self.count != new.count
    }
}

struct CardSurface {
    layer_surface: ZwlrLayerSurfaceV1,
    surface: WlSurface,
    river_output: RiverOutputV1,
    configured: bool,
    size: (u32, u32),
    plan: Plan,
}

impl Indicator {
    pub fn new() -> Indicator {
        Indicator {
            focus: None,
            surface: None,
            hide_at: None,
            primed: false,
        }
    }

    /// Time until the next poll timeout, or -1 while no card is waiting to be
    /// hidden.
    pub fn poll_timeout_ms(&self) -> i32 {
        match self.hide_at {
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                i32::try_from(left.as_millis()).unwrap_or(i32::MAX)
            }
            None => -1,
        }
    }

    /// Called from the event loop after dispatch; hides an expired card.
    pub fn tick(&mut self) {
        if self
            .hide_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.destroy_surface();
        }
    }

    fn destroy_surface(&mut self) {
        if let Some(surface) = self.surface.take() {
            surface.layer_surface.destroy();
            surface.surface.destroy();
        }
        self.hide_at = None;
    }
}

impl Default for Indicator {
    fn default() -> Self {
        Indicator::new()
    }
}

/// Reconcile the card with the window manager state. Called once per manage
/// pass, after layout has run.
pub fn sync(state: &mut AppData) {
    let enabled = state.wm.config.overflow_indicator && state.wm.overview_state.is_none();

    // A surface is bound to one output: drop it when the focus moved away, so
    // the new output gets one with that output's geometry.
    if let Some(surface) = &state.indicator.surface {
        let current = focused_output(&state.wm).map(|output| output.river_output.clone());
        if current.as_ref() != Some(&surface.river_output) {
            state.indicator.destroy_surface();
        }
    }

    if !enabled {
        state.indicator.focus = focus_key(&state.wm);
        state.indicator.destroy_surface();
        return;
    }

    let plan = plan_for(&state.wm);
    let focus = focus_key(&state.wm);
    let changed = state.indicator.primed && state.indicator.focus != focus;
    let structural = match (&state.indicator.focus, &focus) {
        (Some(old), Some(new)) => old.structurally_changed(new),
        _ => false,
    };
    state.indicator.primed = true;
    state.indicator.focus = focus;

    match plan {
        Some(plan) => {
            // Only a focus move brings the card up. Mapping or unmapping a
            // window changes the focus target too, but popping a bar above the
            // new window is mistakable for its titlebar, so that case only
            // clears an already visible card.
            if structural {
                state.indicator.destroy_surface();
            } else if changed {
                show(state, plan);
            } else if state
                .indicator
                .surface
                .as_ref()
                .is_some_and(|surface| surface.plan != plan)
            {
                // The workspace changed without a focus move (a window closed
                // or opened). Keep a visible card honest.
                show(state, plan);
            }
        }
        None => {
            if state.indicator.surface.is_some() {
                state.indicator.destroy_surface();
            }
        }
    }
}

fn focused_output(wm: &WindowManager) -> Option<&crate::types::Output> {
    wm.outputs.get(wm.focused_output_idx?)
}

fn focus_key(wm: &WindowManager) -> Option<FocusKey> {
    let output = wm.focused_output_idx?;
    let out = wm.outputs.get(output)?;
    let workspace = out.focused_workspace_idx;
    let focused = out.workspace_list.get(workspace);
    let window = focused.and_then(|workspace| {
        workspace
            .focused_window_idx
            .and_then(|idx| workspace.window_list.get(idx))
            .map(|window| window.river_window.clone())
    });
    Some(FocusKey {
        output,
        workspace,
        window,
        count: focused.map_or(0, |workspace| workspace.window_list.len()),
    })
}

/// Create the surface if needed, or redraw an already configured one, and
/// restart the hide timer.
fn show(state: &mut AppData, plan: Plan) {
    let Some(output) = state
        .wm
        .focused_output_idx
        .and_then(|i| state.wm.outputs.get(i))
    else {
        return;
    };
    let river_output = output.river_output.clone();
    let wl_output = output.wl_output.clone();
    let top_margin = (output.non_exclusive.y - output.rectangle.y).max(0) + TOP_MARGIN;
    // Center on the usable area (a top bar is already subtracted above; side
    // bars shift the left edge) instead of the raw output.
    let left_margin = (output.non_exclusive.x - output.rectangle.x).max(0)
        + (output.non_exclusive.width - plan.width as i32).max(0) / 2;

    let recreate = state.indicator.surface.as_ref().is_none_or(|surface| {
        surface.river_output != river_output || surface.size != (plan.width, plan.height)
    });
    if recreate {
        state.indicator.destroy_surface();
        let (Some(compositor), Some(layer_shell), Some(_shm)) =
            (&state.compositor, &state.wlr_layer_shell, &state.shm)
        else {
            return;
        };
        let surface = compositor.create_surface(&state.qh, ());
        // Click-through: the card is informational. Without this the whole
        // rectangle is an input region, so it swallows clicks (including a
        // window's titlebar or close button) for as long as it is visible.
        let empty = compositor.create_region(&state.qh, ());
        surface.set_input_region(Some(&empty));
        empty.destroy();
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            wl_output.as_ref(),
            Layer::Overlay,
            "tailrace-overflow".to_string(),
            &state.qh,
            (),
        );
        layer_surface.set_anchor(Anchor::Top | Anchor::Left);
        layer_surface.set_size(plan.width, plan.height);
        layer_surface.set_exclusive_zone(0);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.set_margin(top_margin, 0, 0, left_margin);
        // Initial commit without a buffer: the compositor answers with
        // configure, and only then may a buffer be attached.
        surface.commit();
        state.indicator.surface = Some(CardSurface {
            layer_surface,
            surface,
            river_output,
            configured: false,
            size: (plan.width, plan.height),
            plan: plan.clone(),
        });
    }

    if let Some(surface) = &mut state.indicator.surface {
        surface.plan = plan;
        let configured = surface.configured;
        if configured {
            draw(state);
        }
    }
    state.indicator.hide_at = Some(Instant::now() + SHOW_FOR);
}

pub fn on_configure(state: &mut AppData, proxy: &ZwlrLayerSurfaceV1, serial: u32) {
    let matches = state
        .indicator
        .surface
        .as_ref()
        .is_some_and(|surface| surface.layer_surface == *proxy);
    if !matches {
        return;
    }
    proxy.ack_configure(serial);
    if let Some(surface) = &mut state.indicator.surface {
        surface.configured = true;
    }
    if state.indicator.hide_at.is_none() {
        state.indicator.hide_at = Some(Instant::now() + SHOW_FOR);
    }
    draw(state);
}

pub fn on_closed(state: &mut AppData, proxy: &ZwlrLayerSurfaceV1) {
    let matches = state
        .indicator
        .surface
        .as_ref()
        .is_some_and(|surface| surface.layer_surface == *proxy);
    if matches {
        state.indicator.destroy_surface();
        // Recreate on the next manage pass if the workspace still overflows.
        state.indicator.focus = None;
    }
}

/// Paint the current plan into a new shm buffer and commit it.
fn draw(state: &mut AppData) {
    let Some(surface) = state.indicator.surface.as_ref() else {
        return;
    };
    if !surface.configured {
        return;
    }
    let Some(plan) = plan_for(&state.wm) else {
        state.indicator.destroy_surface();
        return;
    };
    if plan.width != surface.size.0 || plan.height != surface.size.1 {
        return;
    }
    let pixels = render(&plan, &state.wm.config);
    let Some(shm) = &state.shm else {
        return;
    };
    let surface = surface.surface.clone();
    attach_and_commit(shm, &state.qh, &surface, &pixels, plan.width, plan.height);
}

// ---------------------------------------------------------------------------
// Pure geometry: what to draw, independent of Wayland.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub width: u32,
    pub height: u32,
    pub windows: Vec<MiniWindow>,
    pub viewport_x: f32,
    pub viewport_width: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MiniWindow {
    pub x: f32,
    pub width: f32,
    pub focused: bool,
}

/// Plan the card for the focused workspace, or `None` when every tiled window
/// already fits on screen (or there is nothing to draw).
fn plan_for(wm: &WindowManager) -> Option<Plan> {
    let output = focused_output(wm)?;
    if output.is_removed || output.rectangle.width <= 0 {
        return None;
    }
    let workspace = output.workspace_list.get(output.focused_workspace_idx)?;
    let mut rects = Vec::new();
    let mut focused = None;
    for (idx, window) in workspace.window_list.iter().enumerate() {
        if window.geom.is_floating {
            continue;
        }
        let current = window.geom.current;
        if Some(idx) == workspace.focused_window_idx {
            focused = Some(rects.len());
        }
        rects.push(current);
    }
    build_plan(
        output.non_exclusive,
        &rects,
        focused,
        output.non_exclusive.width,
    )
}

/// Map the whole horizontal strip (viewport plus every off-screen window) onto
/// a card. `rects` are the tiled windows' committed rectangles, `focused` indexes
/// the focused one in that slice.
pub fn build_plan(
    viewport: Rectangle,
    rects: &[Rectangle],
    focused: Option<usize>,
    output_width: i32,
) -> Option<Plan> {
    if viewport.width <= 0 || rects.is_empty() || output_width <= 0 {
        return None;
    }
    let view_left = viewport.x as f32;
    let view_right = (viewport.x + viewport.width) as f32;
    let mut left = view_left;
    let mut right = view_right;
    let mut offscreen = false;
    for rect in rects {
        let rect_left = rect.x as f32;
        let rect_right = (rect.x + rect.width) as f32;
        left = left.min(rect_left);
        right = right.max(rect_right);
        offscreen |= rect_left < view_left || rect_right > view_right;
    }
    // A window that extends past the screen edge is what makes the card
    // useful; a workspace that fits needs no card at all.
    if !offscreen || right - left <= viewport.width as f32 {
        return None;
    }

    // Two fifths of the usable width, capped: a compact line, not a panel.
    let target = (output_width * 2 / 5).clamp(MIN_CARD_WIDTH, MAX_CARD_WIDTH);
    let available = output_width - 2 * SIDE_MARGIN;
    let width = target.min(available).max(80);
    let content = width as f32 - 2.0 * CARD_PADDING;
    if content <= 0.0 {
        return None;
    }
    let scale = content / (right - left);
    let width_u32 = u32::try_from(width).ok()?;

    let windows = rects
        .iter()
        .enumerate()
        .map(|(idx, rect)| MiniWindow {
            x: CARD_PADDING + (rect.x as f32 - left) * scale,
            width: ((rect.width as f32) * scale).max(1.5),
            focused: focused == Some(idx),
        })
        .collect();

    Some(Plan {
        width: width_u32,
        height: CARD_HEIGHT as u32,
        windows,
        viewport_x: CARD_PADDING + (view_left - left) * scale,
        viewport_width: (view_right - view_left) * scale,
    })
}

// ---------------------------------------------------------------------------
// Software rendering into ARGB8888, premultiplied as wl_shm requires.
// ---------------------------------------------------------------------------

/// Render the indicator into premultiplied ARGB8888 pixels.
pub fn render(plan: &Plan, config: &Config) -> Vec<u32> {
    let mut canvas = Canvas::new(plan.width, plan.height);

    // One continuous line for the whole workspace strip. The focused window
    // is a brighter run on it; window boundaries stay implicit in the line.
    if let (Some(first), Some(last)) = (plan.windows.first(), plan.windows.last()) {
        let start = first.x;
        let mut track = config.border.unfocused_color;
        track.a = (track.a * 0.50).min(0.55);
        canvas.rounded_rect(
            Shape::new(start, TRACK_Y, last.x + last.width - start, TRACK_HEIGHT),
            TRACK_HEIGHT * 0.5,
            track,
        );
    }
    if let Some(window) = plan.windows.iter().find(|window| window.focused) {
        let mut color = config.border.focused_color;
        color.a = (color.a * 0.95).min(0.95);
        canvas.rounded_rect(
            Shape::new(window.x, TRACK_Y, window.width, TRACK_HEIGHT),
            TRACK_HEIGHT * 0.5,
            color,
        );
    }

    // The current screen is a separate rounded border wrapped around the
    // matching run of the line, not another row.
    let mut edge = config.border.focused_color;
    edge.a = (edge.a * 0.90).min(0.90);
    let viewport = Shape::new(
        plan.viewport_x - VIEWPORT_INSET,
        VIEWPORT_Y,
        plan.viewport_width + 2.0 * VIEWPORT_INSET,
        VIEWPORT_HEIGHT,
    );
    canvas.rounded_stroke(viewport, VIEWPORT_HEIGHT * 0.5, 1.0, edge);

    canvas.pixels
}

struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Canvas {
        Canvas {
            width,
            height,
            pixels: vec![0; (width as usize) * (height as usize)],
        }
    }

    fn blend(&mut self, x: i32, y: i32, color: Color, coverage: f32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let alpha = (color.a * coverage).clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return;
        }
        let index = y as usize * self.width as usize + x as usize;
        let dst = self.pixels[index];
        let inv = 1.0 - alpha;
        let blend = |src: u8, shift: u32| {
            let src = src as f32 / 255.0 * alpha;
            let dst = ((dst >> shift) & 0xff) as f32 / 255.0;
            ((src + dst * inv) * 255.0).round().clamp(0.0, 255.0) as u32
        };
        // Destination is already premultiplied, so its alpha composites too;
        // using only the source alpha would punch holes where translucent
        // layers overlap (the viewport over the card, for instance).
        let dst_alpha = ((dst >> 24) & 0xff) as f32 / 255.0;
        let out_alpha = (alpha + dst_alpha * inv).clamp(0.0, 1.0);
        let a = (out_alpha * 255.0).round() as u32;
        let r = blend(color.r, 16);
        let g = blend(color.g, 8);
        let b = blend(color.b, 0);
        self.pixels[index] = (a << 24) | (r << 16) | (g << 8) | b;
    }

    fn rounded_rect(&mut self, shape: Shape, radius: f32, color: Color) {
        self.shape(shape, radius, color, None);
    }

    fn rounded_stroke(&mut self, shape: Shape, radius: f32, width: f32, color: Color) {
        self.shape(shape, radius, color, Some(width.max(0.25) * 0.5));
    }

    fn shape(&mut self, shape: Shape, radius: f32, color: Color, stroke_half: Option<f32>) {
        let Shape { x, y, w, h } = shape;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let x0 = x.floor().max(0.0) as i32;
        let y0 = y.floor().max(0.0) as i32;
        let x1 = (x + w).ceil().min(self.width as f32) as i32;
        let y1 = (y + h).ceil().min(self.height as f32) as i32;
        for py in y0..y1 {
            for px in x0..x1 {
                let mut coverage = 0.0;
                for (ox, oy) in [(-0.25, -0.25), (0.25, -0.25), (-0.25, 0.25), (0.25, 0.25)] {
                    let distance =
                        rounded_sd(px as f32 + 0.5 + ox, py as f32 + 0.5 + oy, shape, radius);
                    let distance = match stroke_half {
                        Some(half) => distance.abs() - half,
                        None => distance,
                    };
                    coverage += (0.5 - distance).clamp(0.0, 1.0);
                }
                coverage *= 0.25;
                if coverage > 0.0 {
                    self.blend(px, py, color, coverage);
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Shape {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Shape {
    fn new(x: f32, y: f32, w: f32, h: f32) -> Shape {
        Shape { x, y, w, h }
    }
}

/// Signed distance to a rounded rectangle with radius r.
fn rounded_sd(px: f32, py: f32, shape: Shape, r: f32) -> f32 {
    let Shape { x, y, w, h } = shape;
    let r = r.min(w * 0.5).min(h * 0.5).max(0.0);
    let qx = (px - (x + w * 0.5)).abs() - (w * 0.5 - r);
    let qy = (py - (y + h * 0.5)).abs() - (h * 0.5 - r);
    let ax = qx.max(0.0);
    let ay = qy.max(0.0);
    ax.hypot(ay) + qx.max(qy).min(0.0) - r
}

// ---------------------------------------------------------------------------
// wl_shm plumbing
// ---------------------------------------------------------------------------

fn attach_and_commit(
    shm: &WlShm,
    qh: &QueueHandle<AppData>,
    surface: &WlSurface,
    pixels: &[u32],
    width: u32,
    height: u32,
) {
    let len = (width as usize) * (height as usize) * 4;
    let Ok(len) = i32::try_from(len) else {
        return;
    };
    let name = c"tailrace-overflow";
    // SAFETY: valid NUL-terminated name, no arguments used from it afterwards.
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return;
    }
    // SAFETY: fd was just returned by memfd_create and is owned from here on.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: the fd is a valid memfd.
    if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } < 0 {
        return;
    }
    // SAFETY: mapping a fresh anonymous memfd of `len` bytes.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return;
    }
    // SAFETY: the mmap succeeded with room for len bytes; pixels is len/4 u32s.
    unsafe {
        std::ptr::copy_nonoverlapping(pixels.as_ptr() as *const u8, ptr as *mut u8, len as usize);
    }
    let pool = shm.create_pool(fd.as_fd(), len, qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        (width * 4) as i32,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
    surface.attach(Some(&buffer), 0, 0);
    surface.damage(0, 0, width as i32, height as i32);
    surface.commit();
    buffer.destroy();
    pool.destroy();
    // SAFETY: ptr and len come from the successful mmap above.
    unsafe {
        libc::munmap(ptr, len as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, width: i32) -> Rectangle {
        Rectangle {
            x,
            y: 0,
            width,
            height: 1000,
        }
    }

    fn viewport() -> Rectangle {
        rect(0, 1000)
    }

    #[test]
    fn no_card_when_everything_fits() {
        let windows = [rect(0, 400), rect(400, 600)];
        assert!(build_plan(viewport(), &windows, Some(1), 1000).is_none());
    }

    #[test]
    fn card_maps_windows_and_viewport_onto_the_card() {
        let windows = [rect(0, 500), rect(500, 500), rect(1000, 500)];
        let plan = build_plan(viewport(), &windows, Some(1), 1920).unwrap();
        assert_eq!(plan.windows.len(), 3);
        assert!(plan.windows[1].focused);
        assert!(!plan.windows[0].focused);
        // The strip spans 0..1500, the viewport 0..1000: the overlay marks the
        // left two thirds of the content, on top of the window rectangles.
        let content = plan.width as f32 - 2.0 * CARD_PADDING;
        assert!((plan.viewport_x - CARD_PADDING).abs() < 0.01);
        assert!((plan.viewport_width - content * 2.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn viewport_can_be_in_the_middle() {
        let windows = [
            rect(-1000, 500),
            rect(-500, 500),
            rect(0, 500),
            rect(500, 500),
        ];
        let plan = build_plan(viewport(), &windows, Some(2), 1000).unwrap();
        assert!(plan.viewport_x > CARD_PADDING);
        assert!(plan.viewport_x + plan.viewport_width < plan.width as f32 - CARD_PADDING + 0.01);
    }

    #[test]
    fn render_fills_the_card_and_keeps_corners_transparent() {
        let windows = [rect(0, 500), rect(1000, 500)];
        let plan = build_plan(viewport(), &windows, Some(0), 1920).unwrap();
        let pixels = render(&plan, &Config::default());
        assert_eq!(pixels.len(), plan.width as usize * plan.height as usize);
        // A point on the line (inside the first window) is painted.
        let idx = (TRACK_Y as usize + 1) * plan.width as usize + plan.width as usize / 4;
        assert!(pixels[idx] >> 24 > 0);
        // The viewport border is painted above the line, not a filled card.
        let top = VIEWPORT_Y as usize * plan.width as usize
            + (plan.viewport_x + plan.viewport_width / 2.0) as usize;
        assert!(pixels[top] >> 24 > 0, "viewport border missing");
        // Nothing else is: the surface is transparent outside the line and
        // the border.
        assert_eq!(pixels[0] >> 24, 0);
        assert_eq!(pixels[pixels.len() - 1] >> 24, 0);
    }

    fn focus(output: usize, workspace: usize, count: usize) -> FocusKey {
        FocusKey {
            output,
            workspace,
            window: None,
            count,
        }
    }

    #[test]
    fn mapping_a_window_is_not_a_focus_move() {
        let before = focus(0, 0, 3);
        // The fourth window mapped and took focus: no card.
        assert!(before.structurally_changed(&focus(0, 0, 4)));
        // A real focus move inside the same window list is a card.
        assert!(!before.structurally_changed(&focus(0, 0, 3)));
        // Switching workspace or output is a focus move even if the counts
        // happen to differ.
        assert!(!before.structurally_changed(&focus(0, 1, 4)));
        assert!(!before.structurally_changed(&focus(1, 0, 4)));
    }

    #[test]
    fn timeout_is_a_poll_deadline_while_shown() {
        let mut indicator = Indicator::new();
        assert_eq!(indicator.poll_timeout_ms(), -1);
        indicator.hide_at = Some(Instant::now());
        assert_eq!(indicator.poll_timeout_ms(), 0);
        indicator.tick();
        assert!(indicator.surface.is_none());
        assert_eq!(indicator.poll_timeout_ms(), -1);
    }
}
