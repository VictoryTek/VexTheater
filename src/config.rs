//! The launcher configuration: tiles, submenus and the background, read from the JSON
//! file the NixOS module writes.

use anyhow::{bail, Context};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where the NixOS module writes the configuration.
pub const DEFAULT_PATH: &str = "/etc/vextheater/config.json";

/// Unknown keys are refused, so a misspelled one is found when the configuration is
/// checked, not by a tile that quietly lacks what it was given.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Shown under the greeting on the main row, e.g. "VexOS Theater".
    #[serde(default)]
    pub title: Option<String>,
    /// Name of the key that closes the open app, shown in the key hints.
    #[serde(default, rename = "homeKey")]
    pub home_key: Option<String>,
    /// Wallpaper behind the tiles; a plain dark background when unset.
    #[serde(default)]
    pub background: Option<PathBuf>,
    /// The main row, left to right.
    #[serde(default)]
    pub tiles: Vec<Tile>,
    /// Further rows, opened by a tile whose action is `submenu`.
    #[serde(default)]
    pub submenus: BTreeMap<String, Vec<Tile>>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(try_from = "RawTile")]
pub struct Tile {
    pub name: String,
    /// An icon-theme name, or an absolute path to an image file. Only a `desktop` or a
    /// `flatpak` tile may leave it out: the first then shows its application's icon,
    /// the second the icon named like its application id.
    pub icon: Option<String>,
    /// One line shown under the name while the tile is selected.
    pub description: Option<String>,
    /// Written as one more key beside `name` and `icon`, e.g. `"command": "mpv"`.
    pub action: Action,
}

/// A tile as written: one optional key per action. `deny_unknown_fields` does not work
/// beside `flatten`, which is how an action could be read straight into [`Tile`], and
/// this also lets a tile with two actions be refused instead of one being ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTile {
    name: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    desktop: Option<String>,
    #[serde(default)]
    flatpak: Option<String>,
    #[serde(default)]
    submenu: Option<String>,
    #[serde(default)]
    builtin: Option<Builtin>,
}

impl TryFrom<RawTile> for Tile {
    type Error = String;

    /// The messages here are only a fallback: [`locate`] words a refused tile with its
    /// place in the document.
    fn try_from(raw: RawTile) -> Result<Self, String> {
        let mut actions = Vec::new();
        if let Some(command) = raw.command {
            actions.push(("command", Action::Command(command)));
        }
        if let Some(id) = raw.desktop {
            actions.push(("desktop", Action::Desktop(id)));
        }
        if let Some(id) = raw.flatpak {
            actions.push(("flatpak", Action::Flatpak(id)));
        }
        if let Some(name) = raw.submenu {
            actions.push(("submenu", Action::Submenu(name)));
        }
        if let Some(builtin) = raw.builtin {
            actions.push(("builtin", Action::Builtin(builtin)));
        }
        let name = raw.name;
        let (kind, action) = match actions.len() {
            0 => return Err(format!("tile \"{name}\" has no action")),
            1 => actions.remove(0),
            _ => {
                let keys: Vec<_> = actions.iter().map(|(key, _)| *key).collect();
                return Err(format!(
                    "tile \"{name}\" has more than one action ({})",
                    keys.join(", ")
                ));
            }
        };
        if raw.icon.is_none() && !matches!(kind, "desktop" | "flatpak") {
            return Err(format!("tile \"{name}\" has no icon"));
        }
        Ok(Tile {
            name,
            icon: raw.icon,
            description: raw.description,
            action,
        })
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// A shell command line.
    Command(String),
    /// A desktop file id, e.g. `org.gnome.Nautilus.desktop`.
    Desktop(String),
    /// A Flatpak application id.
    Flatpak(String),
    /// The name of an entry in `submenus`.
    Submenu(String),
    Builtin(Builtin),
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Builtin {
    /// Hide the launcher and show the GNOME desktop.
    DesktopMode,
    Shutdown,
    Restart,
}

impl Config {
    /// `$VEXTHEATER_CONFIG`, or [`DEFAULT_PATH`] when that is unset.
    pub fn default_path() -> PathBuf {
        std::env::var_os("VEXTHEATER_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_PATH))
    }

    pub fn load_default() -> anyhow::Result<Self> {
        Self::load(&Self::default_path())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let mut config = Self::parse(&text)
            .with_context(|| format!("invalid configuration in {}", path.display()))?;
        config.drop_power_tiles();
        Ok(config)
    }

    /// The power button beside the clock replaces the Desktop Mode, Restart and Shutdown
    /// tiles, and a tile opening a submenu of only those (the old "Power" tile). A
    /// configuration that still sends them, as the vexos-nix HTPC module used to, would
    /// show them twice.
    fn drop_power_tiles(&mut self) {
        let submenus = &self.submenus;
        self.tiles.retain(|tile| !is_power_tile(tile, submenus));
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let config: Self = serde_json::from_str(text).map_err(|error| match locate(text) {
            Some(problem) => anyhow::anyhow!(problem),
            None => error.into(),
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Every `submenu` action must name a submenu that exists.
    fn validate(&self) -> anyhow::Result<()> {
        let tiles = self.tiles.iter().chain(self.submenus.values().flatten());
        for tile in tiles {
            if let Action::Submenu(name) = &tile.action {
                if !self.submenus.contains_key(name) {
                    bail!("tile \"{}\" opens unknown submenu \"{name}\"", tile.name);
                }
            }
        }
        Ok(())
    }
}

/// The keys that give a tile its action; one of them is required.
const ACTIONS: [&str; 5] = ["command", "desktop", "flatpak", "submenu", "builtin"];

/// The `builtin` values a configuration may use; see [`Builtin`].
const BUILTINS: [&str; 3] = ["desktop-mode", "shutdown", "restart"];

/// The keys of the configuration.
const KEYS: [&str; 5] = ["title", "homeKey", "background", "tiles", "submenus"];

/// The keys of a tile besides its action.
const TILE_KEYS: [&str; 3] = ["name", "icon", "description"];

/// Finds what serde rejected in `text` and says where, as in `tiles[2].icon: missing`.
/// serde's own message gives a line and column, or for a tile with no action no place
/// at all, and a failed build log is read without the file at hand. None when the text
/// is not JSON or nothing specific is found; serde's message stands then. It only
/// words the error: what loads is decided by serde alone.
fn locate(text: &str) -> Option<String> {
    let document: serde_json::Value = serde_json::from_str(text).ok()?;
    let root = document.as_object()?;
    if let Some(key) = root.keys().find(|key| !KEYS.contains(&key.as_str())) {
        return Some(format!(
            "{key}: unknown key, the configuration has {}",
            KEYS.join(", ")
        ));
    }
    for key in ["title", "homeKey", "background"] {
        if root
            .get(key)
            .is_some_and(|v| !v.is_null() && !v.is_string())
        {
            return Some(format!("{key}: must be a string"));
        }
    }
    if let Some(tiles) = root.get("tiles") {
        if let Some(problem) = locate_in_tiles("tiles", tiles) {
            return Some(problem);
        }
    }
    match root.get("submenus") {
        None => None,
        Some(serde_json::Value::Object(submenus)) => submenus
            .iter()
            .find_map(|(name, tiles)| locate_in_tiles(&format!("submenus.{name}"), tiles)),
        Some(_) => Some("submenus: must be an object of tile lists".into()),
    }
}

fn locate_in_tiles(path: &str, tiles: &serde_json::Value) -> Option<String> {
    let Some(tiles) = tiles.as_array() else {
        return Some(format!("{path}: must be a list of tiles"));
    };
    tiles
        .iter()
        .enumerate()
        .find_map(|(index, tile)| locate_in_tile(&format!("{path}[{index}]"), tile))
}

fn locate_in_tile(path: &str, tile: &serde_json::Value) -> Option<String> {
    let Some(fields) = tile.as_object() else {
        return Some(format!("{path}: must be an object"));
    };
    let of = match fields.get("name").and_then(serde_json::Value::as_str) {
        Some(name) => format!(" (tile \"{name}\")"),
        None => String::new(),
    };
    let known = |key: &str| TILE_KEYS.contains(&key) || ACTIONS.contains(&key);
    if let Some(key) = fields.keys().find(|key| !known(key)) {
        return Some(format!(
            "{path}.{key}: unknown key{of}, a tile has {} and one of {}",
            TILE_KEYS.join(", "),
            ACTIONS.join(", ")
        ));
    }
    // A key set to null is as good as left out.
    let given = |key: &str| fields.get(key).filter(|value| !value.is_null());
    match fields.get("name") {
        None => return Some(format!("{path}.name: missing")),
        Some(value) if !value.is_string() => return Some(format!("{path}.name: must be a string")),
        Some(_) => {}
    }
    for key in ["icon", "description"] {
        if given(key).is_some_and(|value| !value.is_string()) {
            return Some(format!("{path}.{key}: must be a string{of}"));
        }
    }
    let actions: Vec<_> = ACTIONS
        .into_iter()
        .filter(|key| given(key).is_some())
        .collect();
    let [action] = actions.as_slice() else {
        return Some(if actions.is_empty() {
            format!(
                "{path}: has no action, add one of {}{of}",
                ACTIONS.join(", ")
            )
        } else {
            format!(
                "{path}: has more than one action ({}){of}",
                actions.join(", ")
            )
        });
    };
    match given(action)?.as_str() {
        None => return Some(format!("{path}.{action}: must be a string{of}")),
        Some(value) if *action == "builtin" && !BUILTINS.contains(&value) => {
            return Some(format!(
                "{path}.builtin: \"{value}\" is not one of {}{of}",
                BUILTINS.join(", ")
            ))
        }
        Some(_) => {}
    }
    // Only these two can do without: the launcher finds the application's own icon.
    if given("icon").is_none() && !matches!(*action, "desktop" | "flatpak") {
        return Some(format!("{path}.icon: missing{of}"));
    }
    None
}

fn is_power_tile(tile: &Tile, submenus: &BTreeMap<String, Vec<Tile>>) -> bool {
    let is_power_builtin = |tile: &Tile| {
        matches!(
            tile.action,
            Action::Builtin(Builtin::DesktopMode | Builtin::Shutdown | Builtin::Restart)
        )
    };
    match &tile.action {
        Action::Submenu(name) => submenus
            .get(name)
            .is_some_and(|tiles| !tiles.is_empty() && tiles.iter().all(is_power_builtin)),
        _ => is_power_builtin(tile),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_action_kind() {
        let config = Config::parse(
            r#"{
                "title": "VexOS Theater",
                "homeKey": "F10",
                "background": "/wall.jpg",
                "tiles": [
                    { "name": "YouTube", "icon": "/icons/youtube.svg", "description": "Full screen", "command": "brave --kiosk" },
                    { "name": "Files", "icon": "org.gnome.Nautilus", "desktop": "org.gnome.Nautilus.desktop" },
                    { "name": "FreeTube", "icon": "io.freetubeapp.FreeTube", "flatpak": "io.freetubeapp.FreeTube" },
                    { "name": "Desktop", "icon": "user-desktop", "builtin": "desktop-mode" },
                    { "name": "Power", "icon": "system-shutdown-symbolic", "submenu": "Power" }
                ],
                "submenus": {
                    "Power": [
                        { "name": "Shutdown", "icon": "system-shutdown-symbolic", "builtin": "shutdown" },
                        { "name": "Restart", "icon": "system-reboot-symbolic", "builtin": "restart" }
                    ]
                }
            }"#,
        )
        .unwrap();

        assert_eq!(config.title.as_deref(), Some("VexOS Theater"));
        assert_eq!(config.home_key.as_deref(), Some("F10"));
        assert_eq!(config.background, Some(PathBuf::from("/wall.jpg")));
        assert_eq!(config.tiles[0].description.as_deref(), Some("Full screen"));
        assert_eq!(config.tiles[1].description, None);
        let actions: Vec<_> = config.tiles.iter().map(|t| &t.action).collect();
        assert_eq!(
            actions,
            [
                &Action::Command("brave --kiosk".into()),
                &Action::Desktop("org.gnome.Nautilus.desktop".into()),
                &Action::Flatpak("io.freetubeapp.FreeTube".into()),
                &Action::Builtin(Builtin::DesktopMode),
                &Action::Submenu("Power".into()),
            ]
        );
        assert_eq!(
            config.submenus["Power"][1].action,
            Action::Builtin(Builtin::Restart)
        );
        assert_eq!(config.tiles[0].icon.as_deref(), Some("/icons/youtube.svg"));
        assert_eq!(config.tiles[1].icon.as_deref(), Some("org.gnome.Nautilus"));
    }

    #[test]
    fn empty_object_is_an_empty_launcher() {
        assert_eq!(Config::parse("{}").unwrap(), Config::default());
    }

    #[test]
    fn rejects_a_tile_without_an_action() {
        assert!(Config::parse(r#"{ "tiles": [ { "name": "A", "icon": "a" } ] }"#).is_err());
    }

    #[test]
    fn rejects_an_unknown_builtin() {
        let text = r#"{ "tiles": [ { "name": "A", "icon": "a", "builtin": "sleep" } ] }"#;
        assert!(Config::parse(text).is_err());
    }

    // The launcher's own commands are not builtins: the type has no such values, so a
    // configuration cannot name them, and the JSON for them is refused.
    #[test]
    fn a_configuration_cannot_use_the_launchers_own_cancel() {
        let text = r#"{ "tiles": [ { "name": "A", "icon": "a", "builtin": "cancel" } ] }"#;
        assert!(Config::parse(text).is_err());
    }

    #[test]
    fn a_configuration_cannot_use_the_launchers_own_arrange_or_reset() {
        for builtin in ["arrange", "reset-order"] {
            let text = format!(
                r#"{{ "tiles": [ {{ "name": "A", "icon": "a", "builtin": "{builtin}" }} ] }}"#
            );
            assert!(Config::parse(&text).is_err(), "{builtin}");
        }
    }

    #[test]
    fn the_power_button_replaces_configured_power_tiles() {
        let mut config = Config::parse(
            r#"{
                "tiles": [
                    { "name": "Files", "icon": "f", "desktop": "org.gnome.Nautilus.desktop" },
                    { "name": "Desktop", "icon": "d", "builtin": "desktop-mode" },
                    { "name": "Restart", "icon": "r", "builtin": "restart" },
                    { "name": "Power", "icon": "p", "submenu": "Power" },
                    { "name": "System", "icon": "s", "submenu": "System" },
                    { "name": "Empty", "icon": "e", "submenu": "Empty" }
                ],
                "submenus": {
                    "Power": [
                        { "name": "Shutdown", "icon": "s", "builtin": "shutdown" },
                        { "name": "Restart", "icon": "r", "builtin": "restart" }
                    ],
                    "System": [
                        { "name": "Shutdown", "icon": "s", "builtin": "shutdown" },
                        { "name": "Settings", "icon": "g", "desktop": "org.gnome.Settings.desktop" }
                    ],
                    "Empty": []
                }
            }"#,
        )
        .unwrap();
        config.drop_power_tiles();

        let names: Vec<_> = config.tiles.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Files", "System", "Empty"]);
    }

    /// The message `parse` gives for `text`, which must be rejected.
    fn problem(text: &str) -> String {
        format!("{:#}", Config::parse(text).unwrap_err())
    }

    #[test]
    fn names_the_tile_field_that_is_wrong() {
        let missing = r#"{ "tiles": [
            { "name": "A", "icon": "a", "command": "a" },
            { "name": "B", "icon": "b", "command": "b" },
            { "name": "Files", "command": "files" } ] }"#;
        assert_eq!(problem(missing), "tiles[2].icon: missing (tile \"Files\")");

        let wrong_type = r#"{ "tiles": [ { "name": "A", "icon": 5, "command": "a" } ] }"#;
        assert_eq!(
            problem(wrong_type),
            "tiles[0].icon: must be a string (tile \"A\")"
        );

        let nameless = r#"{ "tiles": [ { "icon": "a", "command": "a" } ] }"#;
        assert_eq!(problem(nameless), "tiles[0].name: missing");
    }

    #[test]
    fn names_the_action_problem() {
        let none = r#"{ "tiles": [ { "name": "A", "icon": "a" } ] }"#;
        assert_eq!(
            problem(none),
            "tiles[0]: has no action, add one of command, desktop, flatpak, submenu, builtin \
             (tile \"A\")"
        );

        let wrong_type = r#"{ "tiles": [ { "name": "A", "icon": "a", "command": ["a"] } ] }"#;
        assert_eq!(
            problem(wrong_type),
            "tiles[0].command: must be a string (tile \"A\")"
        );

        let builtin = r#"{ "tiles": [ { "name": "A", "icon": "a", "builtin": "sleep" } ] }"#;
        assert_eq!(
            problem(builtin),
            "tiles[0].builtin: \"sleep\" is not one of desktop-mode, shutdown, restart (tile \"A\")"
        );
    }

    #[test]
    fn a_tile_with_two_actions_is_refused() {
        let text = r#"{ "tiles": [
            { "name": "A", "icon": "a", "command": "x", "builtin": "restart" } ] }"#;
        assert_eq!(
            problem(text),
            "tiles[0]: has more than one action (command, builtin) (tile \"A\")"
        );
        // A key set to null counts as left out.
        let null = r#"{ "tiles": [
            { "name": "A", "icon": "a", "command": "x", "builtin": null } ] }"#;
        assert!(Config::parse(null).is_ok());
    }

    #[test]
    fn a_tile_with_an_unknown_key_is_refused() {
        let text = r#"{ "tiles": [ { "name": "A", "icon": "a", "command": "x", "exec": "y" } ] }"#;
        let message = problem(text);
        assert!(
            message.starts_with("tiles[0].exec: unknown key (tile \"A\")"),
            "{message}"
        );
    }

    // Strict parsing reaches the tiles of the submenus too.
    #[test]
    fn a_submenu_tile_is_held_to_the_same_rules() {
        let unknown = r#"{ "submenus": { "Games": [
            { "name": "A", "icon": "a", "command": "a" },
            { "name": "B", "icon": "b", "command": "b", "exec": "c" } ] } }"#;
        let message = problem(unknown);
        assert!(
            message.starts_with("submenus.Games[1].exec: unknown key (tile \"B\")"),
            "{message}"
        );

        let two = r#"{ "submenus": { "Games": [
            { "name": "A", "icon": "a", "command": "a", "flatpak": "b" } ] } }"#;
        assert_eq!(
            problem(two),
            "submenus.Games[0]: has more than one action (command, flatpak) (tile \"A\")"
        );
        assert_eq!(
            problem(r#"{ "submenus": { "Games": { "name": "A" } } }"#),
            "submenus.Games: must be a list of tiles"
        );
    }

    #[test]
    fn an_unknown_top_level_key_is_refused() {
        let message = problem(r#"{ "tilez": [] }"#);
        assert!(message.starts_with("tilez: unknown key"), "{message}");
        assert!(Config::parse(r#"{ "title": "x", "homeKey": "F10" }"#).is_ok());
    }

    // Only a desktop or a flatpak tile can do without an icon: the launcher finds one.
    #[test]
    fn only_desktop_and_flatpak_tiles_may_omit_the_icon() {
        let config = Config::parse(
            r#"{ "tiles": [
                { "name": "Files", "desktop": "org.gnome.Nautilus.desktop" },
                { "name": "FreeTube", "flatpak": "io.freetubeapp.FreeTube" } ] }"#,
        )
        .unwrap();
        assert!(config.tiles.iter().all(|tile| tile.icon.is_none()));

        for (key, value) in [
            ("command", "\"x\""),
            ("submenu", "\"Games\""),
            ("builtin", "\"restart\""),
        ] {
            let text = format!(
                r#"{{ "tiles": [ {{ "name": "A", "{key}": {value} }} ],
                     "submenus": {{ "Games": [] }} }}"#
            );
            assert_eq!(
                problem(&text),
                "tiles[0].icon: missing (tile \"A\")",
                "{key}"
            );
        }
    }

    #[test]
    fn names_the_submenu_and_top_level_fields() {
        let submenu = r#"{ "submenus": { "Games": [
            { "name": "A", "icon": "a", "command": "a" },
            { "name": "B", "command": "b" } ] } }"#;
        assert_eq!(
            problem(submenu),
            "submenus.Games[1].icon: missing (tile \"B\")"
        );
        assert_eq!(problem(r#"{ "title": 5 }"#), "title: must be a string");
        assert_eq!(
            problem(r#"{ "tiles": {} }"#),
            "tiles: must be a list of tiles"
        );
    }

    #[test]
    fn invalid_json_keeps_the_line_and_column() {
        assert!(problem("{ \"tiles\": [").contains("line 1"));
    }

    // `locate` words errors for the builtins it lists, so they must be accepted.
    #[test]
    fn every_listed_builtin_is_accepted() {
        for builtin in BUILTINS {
            let text = format!(
                r#"{{ "tiles": [ {{ "name": "A", "icon": "a", "builtin": "{builtin}" }} ] }}"#
            );
            assert!(Config::parse(&text).is_ok(), "{builtin}");
        }
    }

    #[test]
    fn rejects_an_unknown_submenu() {
        let text = r#"{ "tiles": [ { "name": "A", "icon": "a", "submenu": "Games" } ] }"#;
        let error = format!("{:#}", Config::parse(text).unwrap_err());
        assert!(error.contains("unknown submenu \"Games\""), "{error}");
    }
}
