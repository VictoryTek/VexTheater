//! GNOME Shell behaviour that gets in the way of a TV: the hot corner opening the
//! overview, and notification banners over a film. Both are off while the launcher is
//! showing, and back to the values they had before whenever it leaves: desktop mode,
//! quitting, closing the window, SIGTERM or SIGINT.
//!
//! The values from before are saved to a state file before the first change, and a
//! launcher restarted after a crash keeps that file rather than reading the values it
//! had itself turned off.

use gio::prelude::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const KEYS: [(&str, &str); 2] = [
    ("org.gnome.desktop.interface", "enable-hot-corners"),
    ("org.gnome.desktop.notifications", "show-banners"),
];

/// Saved values, keyed by "schema key".
type Values = BTreeMap<String, bool>;

pub fn theater_mode() {
    let path = state_file();
    if !path.exists() {
        if let Err(error) = save(&path, &current_values()) {
            // Without the saved values there would be nothing to restore later.
            log::warn!("not changing GNOME settings: cannot save them: {error:#}");
            return;
        }
    }
    for (schema, key) in KEYS {
        if let Some(settings) = settings(schema) {
            if let Err(error) = settings.set_boolean(key, false) {
                log::warn!("cannot turn off {schema} {key}: {error}");
            }
        }
    }
}

/// Puts back the values saved by [`theater_mode`]; nothing to do if it never ran.
pub fn restore() {
    let Some(values) = take(&state_file()) else {
        return;
    };
    for (schema, key) in KEYS {
        let (Some(settings), Some(&value)) = (settings(schema), values.get(&id(schema, key)))
        else {
            continue;
        };
        if let Err(error) = settings.set_boolean(key, value) {
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
        .filter_map(|&(schema, key)| {
            settings(schema).map(|settings| (id(schema, key), settings.boolean(key)))
        })
        .collect()
}

fn id(schema: &str, key: &str) -> String {
    format!("{schema} {key}")
}

fn state_file() -> PathBuf {
    glib::user_state_dir()
        .join("vextheater")
        .join("gnome-settings.json")
}

fn save(path: &Path, values: &Values) -> anyhow::Result<()> {
    crate::files::write_atomic(path, serde_json::to_string(values)?.as_bytes())?;
    Ok(())
}

/// Reads and removes the saved values. An unreadable file is removed too, so it
/// cannot block saving fresh values next time.
fn take(path: &Path) -> Option<Values> {
    let text = std::fs::read_to_string(path).ok()?;
    let _ = std::fs::remove_file(path);
    serde_json::from_str(&text)
        .inspect_err(|error| log::warn!("ignoring {}: {error}", path.display()))
        .ok()
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
    fn saved_values_come_back_once() {
        let path = scratch("roundtrip.json");
        let values = Values::from([
            (
                "org.gnome.desktop.interface enable-hot-corners".into(),
                true,
            ),
            ("org.gnome.desktop.notifications show-banners".into(), false),
        ]);
        save(&path, &values).unwrap();
        assert_eq!(take(&path), Some(values));
        assert!(!path.exists(), "restoring must remove the file");
        assert_eq!(take(&path), None);
    }

    #[test]
    fn a_corrupt_file_is_dropped() {
        let path = scratch("corrupt.json");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(take(&path), None);
        assert!(!path.exists());
    }
}
