//! GNOME Shell behaviour that gets in the way of a TV: the hot corner and the Super key
//! opening the overview, and notification banners over a film. All are off while the
//! launcher is showing, and back to the values they had before whenever it leaves:
//! desktop mode, quitting, closing the window, SIGTERM or SIGINT.
//!
//! The values from before are saved to a state file before the first change, and a
//! launcher restarted after a crash keeps that file rather than reading the values it
//! had itself turned off.

use crate::files;
use gio::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// The type of a setting the launcher turns off.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// Off is `false`.
    Bool,
    /// Off is the empty string.
    Text,
}

struct Key {
    schema: &'static str,
    key: &'static str,
    kind: Kind,
}

/// The schemas of [`KEYS`], for [`crate::diagnose`].
pub const SCHEMAS: [&str; 3] = [
    "org.gnome.desktop.interface",
    "org.gnome.desktop.notifications",
    "org.gnome.mutter",
];

const KEYS: [Key; 3] = [
    Key {
        schema: "org.gnome.desktop.interface",
        key: "enable-hot-corners",
        kind: Kind::Bool,
    },
    Key {
        schema: "org.gnome.desktop.notifications",
        key: "show-banners",
        kind: Kind::Bool,
    },
    // The Super key opens the overview; with no key set, it does nothing.
    Key {
        schema: "org.gnome.mutter",
        key: "overlay-key",
        kind: Kind::Text,
    },
];

/// A saved value. A boolean is written as a JSON boolean, as before text values existed,
/// so a file from an older launcher still loads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum Saved {
    Bool(bool),
    Text(String),
}

impl Saved {
    /// Whether this is a value of `kind`.
    fn is(&self, kind: Kind) -> bool {
        matches!(
            (self, kind),
            (Saved::Bool(_), Kind::Bool) | (Saved::Text(_), Kind::Text)
        )
    }
}

/// Saved values, keyed by "schema key".
type Values = BTreeMap<String, Saved>;

pub fn theater_mode() {
    let path = state_file();
    if !path.exists() {
        if let Err(error) = save(&path, &current_values()) {
            // Without the saved values there would be nothing to restore later.
            log::warn!("not changing GNOME settings: cannot save them: {error:#}");
            return;
        }
    }
    let mut missing = Vec::new();
    for Key { schema, key, kind } in KEYS {
        let Some(settings) = settings(schema) else {
            missing.push(schema);
            continue;
        };
        let off = match kind {
            Kind::Bool => settings.set_boolean(key, false),
            Kind::Text => settings.set_string(key, ""),
        };
        if let Err(error) = off {
            log::warn!("cannot turn off {schema} {key}: {error}");
        }
    }
    if first_to_warn(&WARNED_OF_MISSING, &missing) {
        log::warn!(
            "GNOME schemas not found, so their settings are left alone: {}; \
             see XDG_DATA_DIRS, or run `vextheater --diagnose`",
            missing.join(", ")
        );
    }
}

static WARNED_OF_MISSING: AtomicBool = AtomicBool::new(false);

/// Whether to warn of `missing` schemas now: when there are some, and not again at
/// every showing of the launcher.
fn first_to_warn(warned: &AtomicBool, missing: &[&str]) -> bool {
    !missing.is_empty() && !warned.swap(true, Ordering::Relaxed)
}

/// Puts back the values saved by [`theater_mode`]; nothing to do if it never ran.
pub fn restore() {
    let Some(values) = take(&state_file()) else {
        return;
    };
    for Key { schema, key, kind } in KEYS {
        let (Some(settings), Some(value)) = (settings(schema), values.get(&id(schema, key))) else {
            continue;
        };
        if !value.is(kind) {
            log::warn!("not restoring {schema} {key}: the saved value is of another type");
            continue;
        }
        let restored = match value {
            Saved::Bool(value) => settings.set_boolean(key, *value),
            Saved::Text(value) => settings.set_string(key, value),
        };
        if let Err(error) = restored {
            log::warn!("cannot restore {schema} {key}: {error}");
        }
    }
    gio::Settings::sync();
}

/// None when the schema is not installed (a bare dev shell, not a GNOME session).
pub fn settings(schema: &str) -> Option<gio::Settings> {
    gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(schema, true))
        .map(|_| gio::Settings::new(schema))
}

fn current_values() -> Values {
    KEYS.iter()
        .filter_map(|&Key { schema, key, kind }| {
            let settings = settings(schema)?;
            let value = match kind {
                Kind::Bool => Saved::Bool(settings.boolean(key)),
                Kind::Text => Saved::Text(settings.string(key).to_string()),
            };
            Some((id(schema, key), value))
        })
        .collect()
}

fn id(schema: &str, key: &str) -> String {
    format!("{schema} {key}")
}

fn state_file() -> PathBuf {
    files::state_file("gnome-settings.json")
}

fn save(path: &Path, values: &Values) -> anyhow::Result<()> {
    files::write_json(path, values)?;
    Ok(())
}

/// Reads and removes the saved values. An unreadable file is removed too, so it
/// cannot block saving fresh values next time.
fn take(path: &Path) -> Option<Values> {
    let values = files::read_json(path, "the saved GNOME settings");
    if let Err(error) = files::remove(path) {
        log::warn!("cannot remove {}: {error}", path.display());
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vextheater-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn missing_schemas_are_warned_of_once() {
        let warned = AtomicBool::new(false);
        assert!(
            !first_to_warn(&warned, &[]),
            "nothing missing, nothing to say"
        );
        assert!(first_to_warn(&warned, &["org.gnome.mutter"]));
        assert!(
            !first_to_warn(&warned, &["org.gnome.mutter"]),
            "not at every showing"
        );
        assert!(!first_to_warn(&warned, &["org.gnome.desktop.interface"]));
    }

    // `SCHEMAS` is what the diagnosis checks, so it must name the schema of every key.
    #[test]
    fn the_schema_list_covers_every_key() {
        for key in KEYS {
            assert!(SCHEMAS.contains(&key.schema), "{}", key.schema);
        }
        assert_eq!(SCHEMAS.len(), KEYS.len());
    }

    #[test]
    fn saved_values_come_back_once() {
        let path = scratch("roundtrip.json");
        let values = Values::from([
            (
                "org.gnome.desktop.interface enable-hot-corners".into(),
                Saved::Bool(true),
            ),
            (
                "org.gnome.desktop.notifications show-banners".into(),
                Saved::Bool(false),
            ),
            (
                "org.gnome.mutter overlay-key".into(),
                Saved::Text("Super_L".into()),
            ),
        ]);
        save(&path, &values).unwrap();
        assert_eq!(take(&path), Some(values));
        assert!(!path.exists(), "restoring must remove the file");
        assert_eq!(take(&path), None);
    }

    // A file written before the Super key was handled holds only booleans.
    #[test]
    fn a_file_from_an_older_launcher_still_loads() {
        let path = scratch("old.json");
        std::fs::write(
            &path,
            r#"{"org.gnome.desktop.interface enable-hot-corners":true,"org.gnome.desktop.notifications show-banners":false}"#,
        )
        .unwrap();
        let values = take(&path).expect("it loads");
        assert_eq!(
            values["org.gnome.desktop.interface enable-hot-corners"],
            Saved::Bool(true)
        );
        assert_eq!(
            values["org.gnome.desktop.notifications show-banners"],
            Saved::Bool(false)
        );
        assert!(!values.contains_key("org.gnome.mutter overlay-key"));
    }

    // Booleans are still written as JSON booleans, and text as a JSON string.
    #[test]
    fn values_are_written_as_json_booleans_and_strings() {
        let values = Values::from([
            ("a b".into(), Saved::Bool(true)),
            ("c d".into(), Saved::Text("Super_L".into())),
            ("e f".into(), Saved::Text(String::new())),
        ]);
        assert_eq!(
            serde_json::to_string(&values).unwrap(),
            r#"{"a b":true,"c d":"Super_L","e f":""}"#
        );
    }

    // A saved value that is not of the setting's type is not written back.
    #[test]
    fn a_value_of_another_type_does_not_fit_a_setting() {
        assert!(Saved::Bool(true).is(Kind::Bool));
        assert!(Saved::Text("x".into()).is(Kind::Text));
        assert!(!Saved::Bool(true).is(Kind::Text));
        assert!(!Saved::Text("true".into()).is(Kind::Bool));
    }

    #[test]
    fn a_corrupt_file_is_dropped() {
        let path = scratch("corrupt.json");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(take(&path), None);
        assert!(!path.exists());
    }
}
