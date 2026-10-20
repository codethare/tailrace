// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: AGPL-3.0-or-later

use serde::Deserialize;

use crate::actions::{Button, KeybindingAction, PointerAction};
use crate::river::river_layer_shell_output_v1::RiverLayerShellOutputV1;
use crate::river::river_node_v1::RiverNodeV1;
use crate::river::river_output_v1::RiverOutputV1;
use crate::river::river_seat_v1::RiverSeatV1;
use crate::river::river_window_v1::RiverWindowV1;
use crate::river::wl_output::WlOutput;

/// Window pending initialization: held in `pending_windows` until
/// `dimensions` arrives, then moved to a workspace.
pub struct PendingWindow {
    pub river_window: RiverWindowV1,
    pub initialized: bool,
    pub title: Option<String>,
    pub app_id: Option<String>,
    /// river sends capture_sessions once when the window object is created,
    /// before `dimensions` maps it, so the count has to survive the pending phase.
    pub capture_sessions: u32,
    /// The client's own size preference, sent before `dimensions` like the rest
    /// of the pending state.
    pub bounds: ContentBounds,
    /// The window has a parent: a dialog or similar, floated on open.
    pub has_parent: bool,
}

/// A window's own content size limits (river's `dimensions_hint`), possibly
/// overridden by window rules. 0 means "no bound", as in the protocol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContentBounds {
    pub min_width: i32,
    pub max_width: i32,
    pub min_height: i32,
    pub max_height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rectangle {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rectangle {
    pub fn eql(self, other: Rectangle) -> bool {
        self.x == other.x
            && self.y == other.y
            && self.width == other.width
            && self.height == other.height
    }
}

/// Per-window layout state. Kept separate from the Wayland proxies so the
/// geometry code (common/scroller/floating) is pure and unit-testable.
#[derive(Clone)]
pub struct WindowGeom {
    /// Fraction of the output's available width this column occupies.
    pub proportion: f32,
    /// Content size limits; the layout never gives the window a slot its
    /// content does not fit in.
    pub bounds: ContentBounds,
    pub is_fullscreen: bool,
    /// Fills the workspace area (niri's maximize-to-edges): the slot the window
    /// would occupy is unaffected, so its neighbours do not move.
    pub is_maximized_to_edges: bool,
    pub is_floating: bool,
    /// Follow the focused workspace of this output (i3/kwm sticky).
    pub is_sticky: bool,
    pub is_closing: bool,
    /// Geometry while floating (rest position).
    pub floating: Rectangle,
    pub current: Rectangle,
    /// Floating-rect anchor captured at pointer/touch-op start.
    pub drag_origin: Option<Rectangle>,
    /// Last geometry sent to the compositor; used to skip redundant requests.
    pub sent_current: Option<Rectangle>,
    /// Last content size sent with `propose_dimensions` and
    /// `set_dimension_bounds`; separate from `sent_current` because a captured
    /// window keeps its size while the layout still moves it.
    pub sent_dimensions: Option<(i32, i32)>,
    /// Active river_window_v1.capture_sessions count (screen capture). While
    /// it is non-zero the layout stops resizing the window.
    pub capture_sessions: u32,
    /// Layout target computed by the layout pass; the coordinator snaps
    /// `current` to it (no animation).
    pub finish: Option<Rectangle>,
    pub sent_clip: Option<Rectangle>,
    pub sent_visible: Option<bool>,
    pub sent_border_focused: Option<bool>,
    pub sent_border_width: Option<u8>,
    /// Name of the output this window was migrated from (if any). Used to
    /// return windows to their original output when it reappears.
    pub former_output_name: Option<String>,
}

impl WindowGeom {
    pub fn new(proportion: f32, current: Rectangle) -> WindowGeom {
        WindowGeom {
            proportion,
            bounds: ContentBounds::default(),
            is_fullscreen: false,
            is_maximized_to_edges: false,
            is_floating: false,
            is_sticky: false,
            is_closing: false,
            floating: current,
            current,
            drag_origin: None,
            sent_current: None,
            sent_dimensions: None,
            capture_sessions: 0,
            finish: None,
            sent_clip: None,
            sent_visible: None,
            sent_border_focused: None,
            sent_border_width: None,
            former_output_name: None,
        }
    }

    /// Forget what the compositor was told, so the next layout pass re-sends
    /// geometry, size, clip, visibility and borders.
    pub fn forget_sent(&mut self) {
        self.sent_current = None;
        self.sent_dimensions = None;
        self.sent_clip = None;
        self.sent_visible = None;
        self.sent_border_focused = None;
        self.sent_border_width = None;
    }
}

pub struct Window {
    pub river_window: RiverWindowV1,
    pub river_node: RiverNodeV1,
    pub geom: WindowGeom,
}

pub struct Workspace {
    pub window_list: Vec<Window>,
    pub focused_window_idx: Option<usize>,
    /// Focus as of the previous layout pass. `center_focused_window =
    /// "on_overflow"` needs to know which window the focus came from, and only
    /// `layout::update` maintains this.
    pub previous_focused_window_idx: Option<usize>,
    /// The window focus came from, kept by identity so `focus_window_previous`
    /// survives reordering; index-based state would point at the wrong window
    /// after a move. Only `layout::update` maintains this.
    pub last_focused_window: Option<RiverWindowV1>,
    /// The `on_overflow` decision taken when the focus last moved. Reused while
    /// the focus stays put, so an unrelated layout pass cannot un-center the
    /// window.
    pub on_overflow_center: bool,
    /// `center_column` asked for the focused window to be centered. Latched like
    /// `on_overflow_center` and cleared when the focus moves, so an unrelated
    /// pass cannot scroll the window back out of the middle.
    pub center_latch: bool,
}

impl Workspace {
    pub fn new() -> Workspace {
        Workspace {
            window_list: Vec::new(),
            focused_window_idx: None,
            previous_focused_window_idx: None,
            last_focused_window: None,
            on_overflow_center: false,
            center_latch: false,
        }
    }

    pub fn focused_window(&self) -> Option<&Window> {
        let idx = self.focused_window_idx?;
        self.window_list.get(idx)
    }

    pub fn focused_window_mut(&mut self) -> Option<&mut Window> {
        let idx = self.focused_window_idx?;
        self.window_list.get_mut(idx)
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace::new()
    }
}

pub struct Output {
    pub river_output: RiverOutputV1,
    pub river_layer_shell_output: Option<RiverLayerShellOutputV1>,
    pub wl_output: Option<WlOutput>,
    pub name: Option<String>,
    pub workspace_list: [Workspace; 10],
    pub focused_workspace_idx: usize,
    pub rectangle: Rectangle,
    /// Output rectangle minus exclusive zone (e.g. bars).
    pub non_exclusive: Rectangle,
    pub is_removed: bool,
}

/// A detached output: workspaces and windows preserved when the output is
/// removed (e.g. laptop panel off during lock / TTY switch), restored when an
/// output with the same name reappears.
pub struct DetachedOutput {
    pub workspace_list: [Workspace; 10],
    pub focused_workspace_idx: usize,
}

pub struct OverviewState {
    pub entries: Vec<OverviewEntry>,
    pub highlighted: usize,
    pub columns: usize,
    pub previous_workspace: Option<OverviewHome>,
}

#[derive(Clone)]
pub struct OverviewEntry {
    /// Stable identity: resolved by scanning live lists at use time, so
    /// closing or reordering windows mid-overview cannot corrupt entries.
    pub window: RiverWindowV1,
    /// Fullscreen state captured at enter; restored on exit.
    pub was_fullscreen: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct OverviewHome {
    pub output_idx: usize,
    pub workspace_idx: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Layout,
    /// The seat that began the operation is carried along: river keeps pointer
    /// operations per seat, and `op_start_pointer`/`op_end` are requests on the
    /// seat that owns the op.
    PointerAction {
        action: PointerAction,
        seat: RiverSeatV1,
    },
    /// The touch point is carried along so delta events for other points are ignored.
    TouchAction {
        action: PointerAction,
        touch_point: i32,
        seat: RiverSeatV1,
    },
    Overview,
    SetupBindings,
    Exit,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CenterFocused {
    Never,
    Always,
    Single,
    /// Center only when the focused window does not fit next to the window it
    /// was focused from (niri's `on-overflow`).
    #[serde(rename = "on_overflow")]
    OnOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Border {
    pub width: u8,
    pub focused_color: Color,
    pub unfocused_color: Color,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub theme: String,
    pub size: u32,
}

/// A workspace a rule or action refers to: a number (1-10, as in
/// `focus_workspace_number`) or a name from `[[workspace]]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum WorkspaceTarget {
    Number(usize),
    Name(String),
}

/// Rule matching windows by exact app_id and glob title. All set fields must
/// match. Title supports `*` (any run, including empty) and `?` (single char).
///
/// The remaining fields are applied to matching windows when they are mapped;
/// for each of them the first matching rule that sets it wins.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowRule {
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub floating: bool,
    /// Workspace to open on.
    pub open_on_workspace: Option<WorkspaceTarget>,
    /// river output name (e.g. "HDMI-A-1") to open on.
    pub open_on_output: Option<String>,
    /// Proportion of the output width, overriding `default_window_width`.
    pub default_window_width: Option<f32>,
    /// Content width limits in pixels, overriding the client's own hint.
    pub min_width: Option<i32>,
    pub max_width: Option<i32>,
}

/// A column width preset: a proportion of the output or a fixed pixel width.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnWidth {
    Proportion(f32),
    Fixed(i32),
}

/// A name for one of an output's fixed workspaces (niri's named workspaces,
/// without dynamic ones: the slot is what the workspace actually is).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceName {
    pub name: String,
    /// 1-10, as in `focus_workspace_number`.
    pub slot: usize,
    /// Output the name resolves to; the focused output when omitted.
    pub output: Option<String>,
}

impl WindowRule {
    pub fn matches(&self, app_id: Option<&str>, title: Option<&str>) -> bool {
        if self.app_id.is_none() && self.title.is_none() {
            return false;
        }
        if let Some(a) = &self.app_id
            && app_id != Some(a.as_str())
        {
            return false;
        }
        if let Some(t) = &self.title {
            match title {
                Some(title) if glob_match(t, title) => {}
                _ => return false,
            }
        }
        true
    }
}

pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut matched) = (None::<usize>, 0usize);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            matched = t;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            matched += 1;
            t = matched;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keybinding {
    pub key: String,
    /// Modifier names: shift, ctrl, mod1, mod3, mod4, mod5.
    pub modifiers: Vec<String>,
    pub action: KeybindingAction,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointerBinding {
    pub button: Button,
    pub modifiers: Vec<String>,
    pub action: PointerAction,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Gap between windows and the output's top/bottom edge.
    pub vertical_gap: i32,
    /// Gap between adjacent windows.
    pub horizontal_gap: i32,
    /// Default proportion of the output's available width for a new window.
    pub default_window_width: f32,
    pub center_focused_window: CenterFocused,
    /// Center a workspace that holds a single window, whatever
    /// `center_focused_window` says.
    pub always_center_single_column: bool,
    /// Widths `switch_preset_column_width` cycles through.
    pub preset_column_widths: Vec<ColumnWidth>,
    pub no_csd: bool,
    pub border: Border,
    pub cursor: Option<Cursor>,
    /// Focus the window under the pointer (sloppy focus).
    pub focus_follows_pointer: bool,
    pub spawn_at_startup: Vec<Vec<String>>,
    pub keybindings: Vec<Keybinding>,
    pub pointer_bindings: Vec<PointerBinding>,
    pub window_rules: Vec<WindowRule>,
    pub workspace: Vec<WorkspaceName>,
}

impl Config {
    /// Resolve a `[[workspace]]` name to its (output name, 0-based slot).
    pub fn workspace_slot(&self, name: &str) -> Option<(Option<&str>, usize)> {
        self.workspace
            .iter()
            .find(|w| w.name == name)
            .map(|w| (w.output.as_deref(), w.slot - 1))
    }
}

impl Default for Border {
    fn default() -> Self {
        Border {
            width: 3,
            focused_color: Color {
                r: 141,
                g: 214,
                b: 0,
                a: 1.0,
            },
            unfocused_color: Color {
                r: 160,
                g: 160,
                b: 160,
                a: 1.0,
            },
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            vertical_gap: 9,
            horizontal_gap: 9,
            default_window_width: 0.5,
            center_focused_window: CenterFocused::Never,
            always_center_single_column: false,
            preset_column_widths: vec![
                ColumnWidth::Proportion(0.33333),
                ColumnWidth::Proportion(0.5),
                ColumnWidth::Proportion(0.66667),
            ],
            no_csd: true,
            border: Border::default(),
            cursor: None,
            focus_follows_pointer: false,
            spawn_at_startup: Vec::new(),
            keybindings: Vec::new(),
            pointer_bindings: Vec::new(),
            window_rules: Vec::new(),
            workspace: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_eql() {
        let r = Rectangle {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert!(r.eql(r));
        assert!(!r.eql(Rectangle { width: 1919, ..r }));
    }

    #[test]
    fn window_rule_matches() {
        let r = WindowRule {
            app_id: Some("footclient".into()),
            title: None,
            floating: true,
            ..WindowRule::default()
        };
        assert!(r.matches(Some("footclient"), None));
        assert!(!r.matches(Some("foot"), None));
        assert!(!r.matches(None, None));

        let both = WindowRule {
            app_id: Some("a".into()),
            title: Some("t".into()),
            floating: false,
            ..WindowRule::default()
        };
        assert!(both.matches(Some("a"), Some("t")));
        assert!(!both.matches(Some("a"), Some("x")));

        let empty = WindowRule::default();
        assert!(!empty.matches(Some("a"), Some("t")));

        let glob = WindowRule {
            app_id: None,
            title: Some("file-*".into()),
            floating: false,
            ..WindowRule::default()
        };
        assert!(glob.matches(None, Some("file-a.txt")));
        assert!(glob.matches(None, Some("file-")));
        assert!(!glob.matches(None, Some("dir/file-a.txt")));

        let q = WindowRule {
            app_id: None,
            title: Some("file-?.txt".into()),
            floating: false,
            ..WindowRule::default()
        };
        assert!(q.matches(None, Some("file-a.txt")));
        assert!(!q.matches(None, Some("file-ab.txt")));

        let star_only = WindowRule {
            app_id: None,
            title: Some("*".into()),
            floating: false,
            ..WindowRule::default()
        };
        assert!(star_only.matches(None, Some("")));
        assert!(star_only.matches(None, Some("anything")));

        let backtrack = WindowRule {
            app_id: None,
            title: Some("a*b".into()),
            floating: false,
            ..WindowRule::default()
        };
        assert!(backtrack.matches(None, Some("aXbYb")));
        assert!(!backtrack.matches(None, Some("aXbY")));
    }

    #[test]
    fn config_defaults_deserialize_from_empty() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.vertical_gap, 9);
        assert_eq!(config.horizontal_gap, 9);
        assert_eq!(config.default_window_width, 0.5);
        assert_eq!(config.center_focused_window, CenterFocused::Never);
        assert!(config.no_csd);
        assert_eq!(config.border.width, 3);
        assert!(config.cursor.is_none());
        assert!(config.keybindings.is_empty());
    }

    #[test]
    fn config_parses_keybindings_and_rules() {
        let config: Config = toml::from_str(
            r#"
            [[keybindings]]
            key = "q"
            modifiers = ["mod4"]
            action = "close_window"

            [[keybindings]]
            key = "minus"
            modifiers = ["mod4", "ctrl"]
            action = { adjust_window_width = -0.1 }

            [[keybindings]]
            key = "t"
            modifiers = ["mod4"]
            action = { spawn = ["alacritty"] }

            [[pointer_bindings]]
            button = "left"
            modifiers = ["mod4"]
            action = "move_window"

            [[window_rules]]
            app_id = "footclient"
            floating = true

            [[window_rules]]
            title = "file-*"
            floating = true
            "#,
        )
        .unwrap();
        assert_eq!(config.keybindings.len(), 3);
        assert_eq!(
            config.keybindings[0].action,
            crate::actions::KeybindingAction::CloseWindow
        );
        assert_eq!(
            config.keybindings[1].action,
            crate::actions::KeybindingAction::AdjustWindowWidth(-0.1)
        );
        assert_eq!(
            config.keybindings[2].action,
            crate::actions::KeybindingAction::Spawn(vec!["alacritty".into()])
        );
        assert_eq!(config.pointer_bindings.len(), 1);
        assert_eq!(config.pointer_bindings[0].button, Button::Left);
        assert_eq!(config.window_rules.len(), 2);
        assert!(config.window_rules[0].matches(Some("footclient"), None));
        assert!(config.window_rules[1].matches(None, Some("file-a.txt")));
    }

    #[test]
    fn config_rejects_invalid_values() {
        assert!(toml::from_str::<Config>("vertical_gap = \"nine\"").is_err());
        assert!(toml::from_str::<Config>("center_focused_window = \"sometimes\"").is_err());
        assert!(
            toml::from_str::<Config>(
                "[[keybindings]]\nkey = \"q\"\nmodifiers = [\"mod4\"]\naction = \"nonexistent\""
            )
            .is_err()
        );
    }
}
