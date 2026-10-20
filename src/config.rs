// SPDX-FileCopyrightText: © 2026 Julian Andrews
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::io;
use std::path::{Path, PathBuf};

use serde::de::Error as _;

use crate::actions::KeybindingAction;
use crate::keybinding::{default_keybindings, default_pointer_bindings};
use crate::types::{Config, WorkspaceTarget};

/// Candidate config paths, in order: `$XDG_CONFIG_HOME/tailrace/config.toml`,
/// `$HOME/.config/tailrace/config.toml`.
fn config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        paths.push(PathBuf::from(xdg).join("tailrace").join("config.toml"));
    }
    if let Some(home) = std::env::var_os("HOME")
        && !home.is_empty()
    {
        paths.push(
            PathBuf::from(home)
                .join(".config")
                .join("tailrace")
                .join("config.toml"),
        );
    }
    paths
}

fn parse(content: &str) -> Result<Config, toml::de::Error> {
    let mut config: Config = toml::from_str(content)?;
    // Workspace numbers are 1-10 everywhere in the config
    // (`focus_workspace_number`), so a rule outside that range is a typo, not a
    // request to fall back to the focused workspace.
    for (idx, rule) in config.window_rules.iter().enumerate() {
        if let Some(WorkspaceTarget::Number(number)) = rule.open_on_workspace
            && !(1..=10).contains(&number)
        {
            return Err(toml::de::Error::custom(format!(
                "window_rules[{idx}].open_on_workspace = {number} is out of range (1-10)"
            )));
        }
        // A minimum wider than the maximum cannot be honoured: the layout lets
        // the minimum win, so the maximum would be silently ignored.
        if let (Some(min), Some(max)) = (rule.min_width, rule.max_width)
            && min > max
        {
            return Err(toml::de::Error::custom(format!(
                "window_rules[{idx}].min_width = {min} is greater than max_width = {max}"
            )));
        }
    }
    // A workspace name is only meaningful once `[[workspace]]` declares it, so
    // a name that nothing defines (a typo, or a renamed workspace) is an error
    // in a rule or a binding rather than a silent no-op.
    for (idx, workspace) in config.workspace.iter().enumerate() {
        if !(1..=10).contains(&workspace.slot) {
            return Err(toml::de::Error::custom(format!(
                "workspace[{idx}].slot = {} is out of range (1-10)",
                workspace.slot
            )));
        }
        if config.workspace[..idx]
            .iter()
            .any(|other| other.name == workspace.name)
        {
            return Err(toml::de::Error::custom(format!(
                "workspace[{idx}].name = \"{}\" is declared twice",
                workspace.name
            )));
        }
    }
    for (idx, rule) in config.window_rules.iter().enumerate() {
        if let Some(WorkspaceTarget::Name(name)) = &rule.open_on_workspace
            && config.workspace.iter().all(|w| &w.name != name)
        {
            return Err(toml::de::Error::custom(format!(
                "window_rules[{idx}].open_on_workspace = \"{name}\" is not a declared workspace"
            )));
        }
    }
    for (idx, binding) in config.keybindings.iter().enumerate() {
        let name = match &binding.action {
            KeybindingAction::FocusWorkspaceName(name)
            | KeybindingAction::MoveWindowToWorkspaceName(name) => name,
            _ => continue,
        };
        if config.workspace.iter().all(|w| &w.name != name) {
            return Err(toml::de::Error::custom(format!(
                "keybindings[{idx}] refers to \"{name}\", which `[[workspace]]` does not declare"
            )));
        }
    }
    // Empty binding lists fall back to the built-in defaults (rill-ed
    // behavior); window_rules and spawn_at_startup default to empty.
    if config.keybindings.is_empty() {
        config.keybindings = default_keybindings();
    }
    if config.pointer_bindings.is_empty() {
        config.pointer_bindings = default_pointer_bindings();
    }
    Ok(config)
}

fn read(path: &Path) -> Result<Config, io::Error> {
    let content = std::fs::read_to_string(path)?;
    parse(&content).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Load config from the first candidate path that exists. Falls back to
/// defaults when no file is found. Parse errors propagate so the caller can
/// exit loudly.
pub fn load(explicit: Option<&Path>) -> Result<Config, io::Error> {
    if let Some(path) = explicit {
        return read(path);
    }
    for path in config_paths() {
        match read(&path) {
            Ok(config) => return Ok(config),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(default_config())
}

/// Reload config. `Err` carries the read or parse error so the caller can
/// report it; `ErrorKind::NotFound` means there is no config file, which is
/// not an error worth interrupting the user for.
pub fn reload(explicit: Option<&Path>) -> Result<Config, io::Error> {
    if let Some(path) = explicit {
        return read(path);
    }
    for path in config_paths() {
        match read(&path) {
            Ok(config) => return Ok(config),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no tailrace config file found",
    ))
}

pub fn default_config() -> Config {
    Config {
        keybindings: default_keybindings(),
        pointer_bindings: default_pointer_bindings(),
        ..Config::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::KeybindingAction;

    #[test]
    fn default_config_has_bindings() {
        let config = default_config();
        assert!(config.keybindings.len() > 50);
        assert!(
            config
                .keybindings
                .iter()
                .any(|kb| kb.action == KeybindingAction::CloseWindow)
        );
        assert_eq!(config.pointer_bindings.len(), 2);
    }

    #[test]
    fn parse_empty_falls_back_to_default_bindings() {
        let config = parse("").unwrap();
        assert_eq!(config.keybindings.len(), default_keybindings().len());
        assert_eq!(config.pointer_bindings, default_pointer_bindings());
        assert!(config.window_rules.is_empty());
    }

    #[test]
    fn parse_user_bindings_override_defaults() {
        let config =
            parse("[[keybindings]]\nkey = \"x\"\nmodifiers = []\naction = \"exit\"\n").unwrap();
        assert_eq!(config.keybindings.len(), 1);
        assert_eq!(config.keybindings[0].action, KeybindingAction::Exit);
        // Pointer bindings still default when omitted.
        assert_eq!(config.pointer_bindings.len(), 2);
    }

    #[test]
    fn parse_error_is_reported() {
        assert!(parse("vertical_gap = \"nine\"").is_err());
    }

    /// Serde ignores unknown fields by default, so a typo'd toggle
    /// (`focus_folows_pointer`) would be silently dropped and the user would
    /// never know the setting didn't apply. Unknown keys must be loud.
    #[test]
    fn unknown_keys_are_rejected() {
        assert!(parse("focus_folows_pointer = true").is_err());
        assert!(parse("border = { widht = 3 }").is_err());
        assert!(parse("[[keybindings]]\nkey = \"x\"\nmodifers = []\naction = \"exit\"\n").is_err());
        assert!(parse("[[pointer_bindings]]\nbuton = \"middle\"\nmodifiers = []\naction = \"move_window\"\n").is_err());
        assert!(parse("[[window_rules]]\napp_id = \"x\"\nfloatng = true\n").is_err());
    }

    #[test]
    fn out_of_range_open_on_workspace_is_rejected() {
        let rule = |number: i32| {
            format!("[[window_rules]]\napp_id = \"x\"\nopen_on_workspace = {number}\n")
        };
        assert!(parse(&rule(0)).is_err(), "workspace numbers are 1-based");
        assert!(parse(&rule(11)).is_err());
        assert!(parse(&rule(1)).is_ok());
        assert!(parse(&rule(10)).is_ok());
    }

    /// Workspace names are declared by `[[workspace]]`; a binding or a rule
    /// that names an undeclared one is a typo (or a renamed workspace) and must
    /// be loud rather than a silent no-op.
    #[test]
    fn workspaces_are_validated() {
        let declare = "[[workspace]]\nname = \"chat\"\nslot = 3\n";
        assert!(parse(declare).is_ok());
        assert!(parse("[[workspace]]\nname = \"chat\"\nslot = 11\n").is_err());
        assert!(
            parse("[[workspace]]\nname = \"chat\"\n").is_err(),
            "slot is required"
        );
        assert!(
            parse(&format!("{declare}{declare}")).is_err(),
            "a name may only be declared once"
        );
        assert!(
            parse(&format!(
                "{declare}[[keybindings]]\nkey = \"x\"\nmodifiers = []\naction = {{ focus_workspace_name = \"chat\" }}\n"
            ))
            .is_ok()
        );
        assert!(
            parse("[[keybindings]]\nkey = \"x\"\nmodifiers = []\naction = {{ focus_workspace_name = \"cht\" }}\n")
                .is_err(),
            "an undeclared binding name"
        );
        assert!(
            parse("[[window_rules]]\napp_id = \"x\"\nopen_on_workspace = \"cht\"\n").is_err(),
            "an undeclared rule name"
        );
        assert!(
            parse(&format!(
                "{declare}[[window_rules]]\napp_id = \"x\"\nopen_on_workspace = \"chat\"\n"
            ))
            .is_ok()
        );
        // A number still means a slot number.
        assert!(parse("[[window_rules]]\napp_id = \"x\"\nopen_on_workspace = 3\n").is_ok());
    }

    /// A rule cannot ask for a minimum wider than its maximum: the layout lets
    /// the minimum win, so the pair would silently ignore the maximum.
    #[test]
    fn contradictory_content_width_bounds_are_rejected() {
        let rule = |min: i32, max: i32| {
            format!("[[window_rules]]\napp_id = \"x\"\nmin_width = {min}\nmax_width = {max}\n")
        };
        assert!(parse(&rule(300, 900)).is_ok());
        assert!(
            parse(&rule(900, 900)).is_ok(),
            "equal bounds are satisfiable"
        );
        assert!(parse(&rule(900, 300)).is_err());

        // Either bound on its own is fine.
        assert!(parse("[[window_rules]]\napp_id = \"x\"\nmin_width = 900\n").is_ok());
        assert!(parse("[[window_rules]]\napp_id = \"x\"\nmax_width = 300\n").is_ok());
    }

    /// The shipped example must parse against the schema and must not change
    /// behaviour when copied verbatim: its binding sections are commented out,
    /// so it falls back to the built-in defaults. A hand-maintained copy of
    /// the defaults lived here before and had already drifted (46 of 64
    /// bindings), silently dropping keybindings for anyone who copied it.
    #[test]
    fn example_config_parses() {
        let content = include_str!("../config.example.toml");
        let config = parse(content).expect("config.example.toml must parse");
        assert_eq!(config.keybindings, default_keybindings());
        assert_eq!(config.pointer_bindings, default_pointer_bindings());
    }

    /// `-c/--config <path>`: the explicit-path branch of load/reload had no
    /// coverage (only the argument parser and `parse` were tested).
    #[test]
    fn explicit_path_loads_and_reloads() {
        let dir = std::env::temp_dir().join(format!("tailrace-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        std::fs::write(&path, "vertical_gap = 42\n").unwrap();
        assert_eq!(load(Some(&path)).unwrap().vertical_gap, 42);

        // A broken file is an error for load, and reload hands the error back
        // so the caller can show which line is wrong.
        std::fs::write(&path, "vertical_gap = \"nine\"\n").unwrap();
        assert!(load(Some(&path)).is_err());
        let err = reload(Some(&path)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("vertical_gap"), "{err}");

        // An explicitly named file that does not exist is reported, not
        // silently replaced by the built-in defaults.
        let missing = dir.join("nope.toml");
        assert_eq!(
            load(Some(&missing)).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        assert_eq!(
            reload(Some(&missing)).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
