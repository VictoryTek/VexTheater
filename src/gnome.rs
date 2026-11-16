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
use std::collections::btree_map::Entry;
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

/// Where the settings are read and written: GSettings, or a stand-in in the tests.
trait Backend {
    /// The current value; None if the schema is not installed.
    fn get(&self, key: &Key) -> Option<Saved>;
    /// Writes a value; an error if the schema is missing or the write is refused.
    fn set(&self, key: &Key, value: &Saved) -> Result<(), String>;
    /// Flushes what was written, so it is stored when this returns.
    fn sync(&self);
}

struct Gio;

impl Backend for Gio {
    fn get(&self, key: &Key) -> Option<Saved> {
        let settings = settings(key.schema)?;
        Some(match key.kind {
            Kind::Bool => Saved::Bool(settings.boolean(key.key)),
            Kind::Text => Saved::Text(settings.string(key.key).to_string()),
        })
    }

    fn set(&self, key: &Key, value: &Saved) -> Result<(), String> {
        let settings = settings(key.schema).ok_or("the schema is not installed")?;
        match value {
            Saved::Bool(value) => settings.set_boolean(key.key, *value),
            Saved::Text(value) => settings.set_string(key.key, value),
        }
        .map_err(|error| error.to_string())
    }

    fn sync(&self) {
        gio::Settings::sync();
    }
}

/// What a setting is turned off to.
fn off(kind: Kind) -> Saved {
    match kind {
        Kind::Bool => Saved::Bool(false),
        Kind::Text => Saved::Text(String::new()),
    }
}

pub fn theater_mode() {
    let missing = theater_mode_with(&state_file(), &Gio);
    if first_to_warn(&WARNED_OF_MISSING, &missing) {
        log::warn!(
            "GNOME schemas not found, so their settings are left alone: {}; \
             see XDG_DATA_DIRS, or run `vextheater --diagnose`",
            missing.join(", ")
        );
    }
}

/// Turns the settings off, each only once its original value is in the snapshot file.
/// Returns the schemas that are not installed.
fn theater_mode_with(path: &Path, backend: &dyn Backend) -> Vec<&'static str> {
    let snapshot = match cover(path, backend) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            // Without the saved values there would be nothing to restore later.
            log::warn!("not changing GNOME settings: cannot save them: {error:#}");
            return Vec::new();
        }
    };
    let mut missing = Vec::new();
    for key in &KEYS {
        match snapshot.get(&id(key.schema, key.key)) {
            Some(saved) if saved.is(key.kind) => {
                if let Err(error) = backend.set(key, &off(key.kind)) {
                    log::warn!("cannot turn off {} {}: {error}", key.schema, key.key);
                }
            }
            Some(_) => {} // of another type; restoring says so
            None => {
                if backend.get(key).is_none() {
                    missing.push(key.schema);
                }
            }
        }
    }
    missing
}

/// The snapshot with every setting there is to turn off, saved. The file of an earlier
/// launcher, perhaps one that crashed in theater mode, keeps its values, which are the
/// originals; the settings it did not know of, such as the Super key for a file from
/// before it was handled, are added from their current values, still the originals
/// since nothing turned them off. Nothing is written when the file already has them all.
///
/// An error if the snapshot could not be saved, in which case nothing may be changed.
/// A file that cannot be read is set aside for a look rather than overwritten.
fn cover(path: &Path, backend: &dyn Backend) -> anyhow::Result<Values> {
    let existing = if path.exists() {
        match files::read_json::<Values>(path, "the saved GNOME settings") {
            Some(values) => values,
            None => {
                let aside = path.with_extension("json.unreadable");
                std::fs::rename(path, &aside)?;
                log::warn!(
                    "the saved GNOME settings could not be read; kept as {}",
                    aside.display()
                );
                Values::new()
            }
        }
    } else {
        Values::new()
    };
    let mut snapshot = existing.clone();
    for key in &KEYS {
        let id = id(key.schema, key.key);
        if let Entry::Vacant(vacant) = snapshot.entry(id) {
            if let Some(current) = backend.get(key) {
                vacant.insert(current);
            }
        }
    }
    if snapshot != existing || !path.exists() {
        save(path, &snapshot)?;
    }
    Ok(snapshot)
}

static WARNED_OF_MISSING: AtomicBool = AtomicBool::new(false);

/// Whether to warn of `missing` schemas now: when there are some, and not again at
/// every showing of the launcher.
fn first_to_warn(warned: &AtomicBool, missing: &[&str]) -> bool {
    !missing.is_empty() && !warned.swap(true, Ordering::Relaxed)
}

/// Puts back the values saved by [`theater_mode`]; nothing to do if it never ran.
pub fn restore() {
    restore_with(&state_file(), &Gio);
}

/// Puts the saved values back and forgets them once they are all back: the file is the
/// only copy of the originals. A value that could not be put back stays in it for the
/// next try (at desktop mode, quitting, or the next start), and a file that cannot be
/// read is left where it is.
fn restore_with(path: &Path, backend: &dyn Backend) {
    if !path.exists() {
        return;
    }
    let Some(values) = files::read_json::<Values>(path, "the saved GNOME settings") else {
        log::warn!(
            "not restoring GNOME settings: {} cannot be read, and is kept",
            path.display()
        );
        return;
    };
    let mut left = Values::new();
    for key in &KEYS {
        let id = id(key.schema, key.key);
        let Some(value) = values.get(&id) else {
            continue;
        };
        if !value.is(key.kind) {
            log::warn!(
                "not restoring {} {}: the saved value is of another type",
                key.schema,
                key.key
            );
            continue;
        }
        if let Err(error) = backend.set(key, value) {
            log::warn!("cannot restore {} {}: {error}", key.schema, key.key);
            left.insert(id, value.clone());
        }
    }
    backend.sync();
    let saved = if left.is_empty() {
        files::remove(path)
    } else {
        files::write_json(path, &left)
    };
    if let Err(error) = saved {
        log::warn!("cannot update {}: {error}", path.display());
    }
}

/// None when the schema is not installed (a bare dev shell, not a GNOME session).
pub fn settings(schema: &str) -> Option<gio::Settings> {
    gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(schema, true))
        .map(|_| gio::Settings::new(schema))
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

    const HOT: &str = "org.gnome.desktop.interface enable-hot-corners";
    const BANNERS: &str = "org.gnome.desktop.notifications show-banners";
    const OVERLAY: &str = "org.gnome.mutter overlay-key";

    /// A stand-in for GSettings.
    struct Fake {
        values: std::cell::RefCell<Values>,
        /// Schemas that are not installed.
        missing: Vec<&'static str>,
        /// Keys whose writes are refused.
        refuse: std::cell::RefCell<Vec<&'static str>>,
        /// Panics in `sync`, as a crash part way through restoring would.
        crash_in_sync: bool,
    }

    impl Fake {
        fn new(hot: bool, banners: bool, overlay: &str) -> Fake {
            Fake {
                values: std::cell::RefCell::new(Values::from([
                    (HOT.into(), Saved::Bool(hot)),
                    (BANNERS.into(), Saved::Bool(banners)),
                    (OVERLAY.into(), Saved::Text(overlay.into())),
                ])),
                missing: Vec::new(),
                refuse: Default::default(),
                crash_in_sync: false,
            }
        }

        fn value(&self, id: &str) -> Saved {
            self.values.borrow()[id].clone()
        }

        fn all(&self) -> [Saved; 3] {
            [HOT, BANNERS, OVERLAY].map(|id| self.value(id))
        }
    }

    impl Backend for Fake {
        fn get(&self, key: &Key) -> Option<Saved> {
            if self.missing.contains(&key.schema) {
                return None;
            }
            self.values.borrow().get(&id(key.schema, key.key)).cloned()
        }

        fn set(&self, key: &Key, value: &Saved) -> Result<(), String> {
            if self.missing.contains(&key.schema) {
                return Err("no such schema".into());
            }
            if self.refuse.borrow().contains(&key.key) {
                return Err("refused".into());
            }
            self.values
                .borrow_mut()
                .insert(id(key.schema, key.key), value.clone());
            Ok(())
        }

        fn sync(&self) {
            assert!(!self.crash_in_sync, "simulated crash");
        }
    }

    fn saved_file(path: &Path) -> Values {
        files::read_json(path, "the test snapshot").expect("a readable snapshot")
    }

    fn all_off() -> [Saved; 3] {
        [
            Saved::Bool(false),
            Saved::Bool(false),
            Saved::Text("".into()),
        ]
    }

    #[test]
    fn saved_values_come_back_and_only_then_is_the_file_removed() {
        let path = scratch("roundtrip.json");
        let _ = std::fs::remove_file(&path);
        let settings = Fake::new(true, false, "Super_L");
        let original = settings.all();
        assert!(theater_mode_with(&path, &settings).is_empty());
        assert_eq!(settings.all(), all_off());
        assert_eq!(saved_file(&path)[OVERLAY], Saved::Text("Super_L".into()));

        restore_with(&path, &settings);
        assert_eq!(settings.all(), original);
        assert!(!path.exists(), "restoring must remove the file");
        restore_with(&path, &settings); // nothing to do, and no harm
    }

    // A file written before the Super key was handled holds only booleans. It survives
    // a whole enter-and-restore cycle with the Super key's original value recorded.
    #[test]
    fn a_file_from_an_older_launcher_gets_the_super_key_added_before_it_is_turned_off() {
        let path = scratch("old.json");
        std::fs::write(&path, format!(r#"{{"{HOT}":true,"{BANNERS}":false}}"#)).unwrap();
        // The older launcher turned off the first two; the Super key is still the user's.
        let settings = Fake::new(false, false, "Super_L");

        theater_mode_with(&path, &settings);
        let snapshot = saved_file(&path);
        assert_eq!(snapshot[HOT], Saved::Bool(true), "the original is kept");
        assert_eq!(snapshot[OVERLAY], Saved::Text("Super_L".into()));
        assert_eq!(settings.all(), all_off());

        restore_with(&path, &settings);
        assert_eq!(
            settings.all(),
            [
                Saved::Bool(true),
                Saved::Bool(false),
                Saved::Text("Super_L".into())
            ]
        );
        assert!(!path.exists());
    }

    // An existing snapshot is not rewritten with the values the launcher turned off.
    #[test]
    fn a_launcher_restarted_after_a_crash_keeps_the_originals() {
        let path = scratch("crash.json");
        let _ = std::fs::remove_file(&path);
        let settings = Fake::new(true, true, "Super_L");
        theater_mode_with(&path, &settings);
        assert_eq!(settings.all(), all_off());

        theater_mode_with(&path, &settings); // restarted: the settings are already off
        restore_with(&path, &settings);
        assert_eq!(
            settings.all(),
            [
                Saved::Bool(true),
                Saved::Bool(true),
                Saved::Text("Super_L".into())
            ]
        );
    }

    // If the completed snapshot cannot be saved, nothing is turned off.
    #[test]
    fn nothing_is_turned_off_when_the_snapshot_cannot_be_saved() {
        let path = scratch("unsaveable.json");
        std::fs::write(&path, format!(r#"{{"{HOT}":true}}"#)).unwrap();
        // A directory in the way of the temporary file makes the save fail.
        std::fs::create_dir_all(path.with_extension("json.tmp")).unwrap();
        let settings = Fake::new(true, true, "Super_L");
        let before = settings.all();

        theater_mode_with(&path, &settings);
        assert_eq!(settings.all(), before);
        assert_eq!(saved_file(&path).len(), 1, "the old snapshot is intact");

        // And with no file yet.
        let blocker = scratch("blocker");
        std::fs::write(&blocker, "x").unwrap();
        theater_mode_with(&blocker.join("snapshot.json"), &settings);
        assert_eq!(settings.all(), before);
    }

    // A value that cannot be put back stays saved, and a retry puts it back; the file
    // goes only when everything is back.
    #[test]
    fn a_refused_restore_is_kept_for_a_retry() {
        let path = scratch("refused.json");
        let _ = std::fs::remove_file(&path);
        let settings = Fake::new(true, true, "Super_L");
        theater_mode_with(&path, &settings);

        settings.refuse.borrow_mut().push("overlay-key");
        restore_with(&path, &settings);
        assert_eq!(settings.value(HOT), Saved::Bool(true));
        assert_eq!(settings.value(OVERLAY), Saved::Text("".into()));
        let left = saved_file(&path);
        assert_eq!(left.len(), 1, "{left:?}");
        assert_eq!(left[OVERLAY], Saved::Text("Super_L".into()));

        settings.refuse.borrow_mut().clear();
        restore_with(&path, &settings);
        assert_eq!(settings.value(OVERLAY), Saved::Text("Super_L".into()));
        assert!(!path.exists());
    }

    #[test]
    fn a_schema_that_is_missing_at_restore_keeps_its_value() {
        let path = scratch("missing-schema.json");
        let _ = std::fs::remove_file(&path);
        let mut settings = Fake::new(true, true, "Super_L");
        theater_mode_with(&path, &settings);

        settings.missing.push("org.gnome.mutter");
        restore_with(&path, &settings);
        assert_eq!(saved_file(&path).len(), 1);

        settings.missing.clear();
        restore_with(&path, &settings);
        assert_eq!(settings.value(OVERLAY), Saved::Text("Super_L".into()));
        assert!(!path.exists());
    }

    #[test]
    fn a_schema_that_is_not_installed_is_reported_and_left_alone() {
        let path = scratch("no-mutter.json");
        let _ = std::fs::remove_file(&path);
        let mut settings = Fake::new(true, true, "Super_L");
        settings.missing.push("org.gnome.mutter");
        assert_eq!(theater_mode_with(&path, &settings), ["org.gnome.mutter"]);
        assert_eq!(settings.value(OVERLAY), Saved::Text("Super_L".into()));
        assert!(!saved_file(&path).contains_key(OVERLAY));
    }

    // A crash while restoring, before the file is dealt with, leaves the whole snapshot.
    #[test]
    fn an_interrupted_restore_leaves_the_snapshot_for_the_next_one() {
        let path = scratch("interrupted.json");
        let _ = std::fs::remove_file(&path);
        let mut settings = Fake::new(true, false, "Super_L");
        let original = settings.all();
        theater_mode_with(&path, &settings);
        let snapshot = saved_file(&path);

        settings.crash_in_sync = true;
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            restore_with(&path, &settings)
        }));
        assert!(crashed.is_err());
        assert_eq!(saved_file(&path), snapshot);

        settings.crash_in_sync = false;
        restore_with(&path, &settings);
        assert_eq!(settings.all(), original);
        assert!(!path.exists());
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

    // A file that cannot be read is never deleted by a restore, and a launcher that
    // finds one sets it aside, to be looked at, before saving fresh values.
    #[test]
    fn an_unreadable_file_is_kept_not_deleted() {
        let path = scratch("corrupt.json");
        let aside = path.with_extension("json.unreadable");
        let _ = std::fs::remove_file(&aside);
        std::fs::write(&path, "not json").unwrap();
        let settings = Fake::new(true, true, "Super_L");
        let before = settings.all();

        restore_with(&path, &settings);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
        assert_eq!(settings.all(), before);

        theater_mode_with(&path, &settings);
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "not json");
        assert_eq!(saved_file(&path)[HOT], Saved::Bool(true));
        assert_eq!(settings.all(), all_off());
    }
}
