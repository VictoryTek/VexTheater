//! The launcher configuration: tiles, submenus and the background, read from the JSON
//! file the NixOS module writes.

use anyhow::{bail, Context};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where the NixOS module writes the configuration.
pub const DEFAULT_PATH: &str = "/etc/vextheater/config.json";

#[derive(Debug, Default, Deserialize, PartialEq)]
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
pub struct Tile {
    pub name: String,
    /// An icon-theme name, or an absolute path to an image file.
    pub icon: String,
    /// One line shown under the name while the tile is selected.
    #[serde(default)]
    pub description: Option<String>,
    /// Written as one more key beside `name` and `icon`, e.g. `"command": "mpv"`.
    #[serde(flatten)]
    pub action: Action,
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
    /// Leaves a confirmation page. Only the launcher's own pages use it: a
    /// configuration cannot ask for it.
    #[serde(skip_deserializing)]
    Cancel,
}

impl Tile {
    /// Whether `icon` names a file rather than an icon-theme entry.
    pub fn icon_is_file(&self) -> bool {
        self.icon.starts_with('/')
    }
}

impl Config {
    /// Reads `$VEXTHEATER_CONFIG`, or [`DEFAULT_PATH`] when that is unset.
    pub fn load_default() -> anyhow::Result<Self> {
        let path = std::env::var_os("VEXTHEATER_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_PATH));
        Self::load(&path)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid configuration in {}", path.display()))
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let config: Self = serde_json::from_str(text)?;
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
        assert!(config.tiles[0].icon_is_file());
        assert!(!config.tiles[1].icon_is_file());
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

    #[test]
    fn a_configuration_cannot_use_the_launchers_own_cancel() {
        let text = r#"{ "tiles": [ { "name": "A", "icon": "a", "builtin": "cancel" } ] }"#;
        assert!(Config::parse(text).is_err());
    }

    #[test]
    fn rejects_an_unknown_submenu() {
        let text = r#"{ "tiles": [ { "name": "A", "icon": "a", "submenu": "Games" } ] }"#;
        let error = format!("{:#}", Config::parse(text).unwrap_err());
        assert!(error.contains("unknown submenu \"Games\""), "{error}");
    }
}
