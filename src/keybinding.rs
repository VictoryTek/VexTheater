//! The GNOME shortcut that runs `vextheater --home`, so the home key closes the open app
//! and brings the launcher back from any app and from desktop mode.
//!
//! The shortcut belongs to the configuration: with `homeKey` set, the launcher makes
//! sure the shortcut exists and is right; without it, the launcher removes its own. It
//! is done here, when the launcher starts and when the configuration is reloaded, and
//! not by a NixOS dconf database, because dconf does not merge lists: the user's own
//! `custom-keybindings` list hides a system one entirely, so a declared shortcut would be
//! missing for anyone who already has shortcuts of their own. Here the list is read as
//! the user sees it, ours is added to it, and nothing else in it is touched.
//!
//! A shortcut that is already right is not written again, and one edited by hand in
//! GNOME Settings is put back at the next start.

use gio::prelude::*;
use std::path::Path;

const LIST_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
const LIST_KEY: &str = "custom-keybindings";
/// A relocatable schema: one instance per shortcut, at a path of its own.
const ENTRY_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
/// The path the shortcut has always had in the vexos-nix module, so the two agree.
const ENTRY_PATH: &str =
    "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/vextheater-home/";
const NAME: &str = "VexTheater home";
/// Where NixOS puts the program of the running system. It does not change with a
/// rebuild, and a garbage collection does not remove it, unlike a store path.
const SYSTEM_BIN: &str = "/run/current-system/sw/bin/vextheater";

/// What a shortcut holds.
#[derive(Clone, Debug, Default, PartialEq)]
struct Entry {
    name: String,
    command: String,
    binding: String,
}

/// Where the shortcuts are kept. GSettings in the launcher; a fake in the tests, which
/// count the writes.
trait Store {
    /// The list of shortcut paths as the user sees it.
    fn list(&self) -> Vec<String>;
    /// The writes say whether they worked, so only what happened is logged.
    fn set_list(&self, paths: &[String]) -> bool;
    /// What the shortcut at [`ENTRY_PATH`] holds.
    fn entry(&self) -> Entry;
    fn set_entry(&self, entry: &Entry) -> bool;
    fn reset_entry(&self);
}

/// Makes the shortcut what `home_key` says, or removes it if there is none.
pub fn sync(home_key: Option<&str>) {
    let Some(store) = GSettingsStore::open() else {
        // Info, not warn: outside GNOME this is normal. Inside it, a service that
        // cannot see the GNOME schemas (see XDG_DATA_DIRS) ends up here too.
        log::info!(
            "the GNOME shortcut schemas are not available: the home key shortcut is left alone"
        );
        return;
    };
    sync_with(&store, home_key, &command());
}

/// An empty `home_key` is no home key.
fn sync_with(store: &impl Store, home_key: Option<&str>, command: &str) {
    let home_key = home_key.filter(|key| !key.is_empty());
    let list = store.list();
    let listed = list.iter().any(|path| path == ENTRY_PATH);
    match home_key {
        Some(binding) => {
            let wanted = Entry {
                name: NAME.to_owned(),
                command: command.to_owned(),
                binding: binding.to_owned(),
            };
            // The entry first, so GNOME never sees the path of one that is not whole.
            if store.entry() != wanted && store.set_entry(&wanted) {
                log::info!("home key shortcut set to {binding}: {command}");
            }
            if !listed {
                let mut list = list;
                list.push(ENTRY_PATH.to_owned());
                if store.set_list(&list) {
                    log::info!("home key shortcut added to the custom shortcuts");
                }
            }
        }
        None if listed => {
            let kept: Vec<String> = list.into_iter().filter(|path| path != ENTRY_PATH).collect();
            let removed = store.set_list(&kept);
            store.reset_entry();
            if removed {
                log::info!("home key shortcut removed");
            }
        }
        None => {}
    }
}

/// The command the shortcut runs.
fn command() -> String {
    command_for(Path::new(SYSTEM_BIN))
}

fn command_for(system_bin: &Path) -> String {
    if system_bin.exists() {
        format!("{} --home", system_bin.display())
    } else {
        "vextheater --home".to_owned()
    }
}

struct GSettingsStore {
    list: gio::Settings,
    entry: gio::Settings,
}

impl GSettingsStore {
    /// None where the schemas are not installed (not a GNOME session).
    fn open() -> Option<GSettingsStore> {
        // The entry's schema is relocatable, which `gnome::settings` cannot open.
        gio::SettingsSchemaSource::default()?.lookup(ENTRY_SCHEMA, true)?;
        Some(GSettingsStore {
            list: crate::gnome::settings(LIST_SCHEMA)?,
            entry: gio::Settings::with_path(ENTRY_SCHEMA, ENTRY_PATH),
        })
    }

    fn flush() {
        gio::Settings::sync();
    }
}

impl Store for GSettingsStore {
    fn list(&self) -> Vec<String> {
        self.list
            .strv(LIST_KEY)
            .iter()
            .map(|path| path.to_string())
            .collect()
    }

    fn set_list(&self, paths: &[String]) -> bool {
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let result = self.list.set_strv(LIST_KEY, paths.as_slice());
        if let Err(error) = &result {
            log::warn!("cannot update the list of custom shortcuts: {error}");
        }
        Self::flush();
        result.is_ok()
    }

    fn entry(&self) -> Entry {
        Entry {
            name: self.entry.string("name").to_string(),
            command: self.entry.string("command").to_string(),
            binding: self.entry.string("binding").to_string(),
        }
    }

    fn set_entry(&self, entry: &Entry) -> bool {
        let mut worked = true;
        for (key, value) in [
            ("name", &entry.name),
            ("command", &entry.command),
            ("binding", &entry.binding),
        ] {
            if let Err(error) = self.entry.set_string(key, value) {
                log::warn!("cannot set the home key shortcut's {key}: {error}");
                worked = false;
            }
        }
        Self::flush();
        worked
    }

    fn reset_entry(&self) {
        for key in ["name", "command", "binding"] {
            self.entry.reset(key);
        }
        Self::flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A store in memory that counts what is written to it.
    #[derive(Default)]
    struct Fake {
        list: RefCell<Vec<String>>,
        entry: RefCell<Entry>,
        /// What was written, in order.
        writes: RefCell<Vec<&'static str>>,
    }

    impl Fake {
        fn with_list(paths: &[&str]) -> Fake {
            let fake = Fake::default();
            *fake.list.borrow_mut() = paths.iter().map(|p| (*p).to_owned()).collect();
            fake
        }

        fn writes(&self) -> usize {
            self.writes.borrow().len()
        }
    }

    impl Store for Fake {
        fn list(&self) -> Vec<String> {
            self.list.borrow().clone()
        }
        fn set_list(&self, paths: &[String]) -> bool {
            *self.list.borrow_mut() = paths.to_vec();
            self.writes.borrow_mut().push("list");
            true
        }
        fn entry(&self) -> Entry {
            self.entry.borrow().clone()
        }
        fn set_entry(&self, entry: &Entry) -> bool {
            *self.entry.borrow_mut() = entry.clone();
            self.writes.borrow_mut().push("entry");
            true
        }
        fn reset_entry(&self) {
            *self.entry.borrow_mut() = Entry::default();
            self.writes.borrow_mut().push("reset");
        }
    }

    const HOME: &str = "vextheater --home";
    const THEIRS: &str =
        "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/mute-mic/";

    #[test]
    fn the_shortcut_is_added_to_an_empty_list() {
        let store = Fake::default();
        sync_with(&store, Some("F10"), HOME);
        assert_eq!(store.list(), [ENTRY_PATH]);
        assert_eq!(
            store.entry(),
            Entry {
                name: "VexTheater home".into(),
                command: HOME.into(),
                binding: "F10".into()
            }
        );
    }

    // The user's own shortcuts stay, in their order, with ours after them.
    #[test]
    fn the_users_own_shortcuts_are_kept() {
        let store = Fake::with_list(&[THEIRS, "/other/"]);
        sync_with(&store, Some("F10"), HOME);
        assert_eq!(store.list(), [THEIRS, "/other/", ENTRY_PATH]);
    }

    // GNOME must never see the path of an entry that is not whole: the entry is written
    // before it is listed, and unlisted before it is reset.
    #[test]
    fn the_entry_is_written_before_it_is_listed_and_unlisted_before_it_is_reset() {
        let store = Fake::default();
        sync_with(&store, Some("F10"), HOME);
        assert_eq!(*store.writes.borrow(), ["entry", "list"]);

        let store = Fake::with_list(&[ENTRY_PATH]);
        *store.entry.borrow_mut() = Entry {
            name: "VexTheater home".into(),
            command: HOME.into(),
            binding: "F10".into(),
        };
        sync_with(&store, None, HOME);
        assert_eq!(*store.writes.borrow(), ["list", "reset"]);
    }

    // An empty home key is no home key, so it removes ours and adds nothing.
    #[test]
    fn an_empty_home_key_is_no_home_key() {
        let store = Fake::with_list(&[THEIRS, ENTRY_PATH]);
        sync_with(&store, Some(""), HOME);
        assert_eq!(store.list(), [THEIRS]);
        let empty = Fake::default();
        sync_with(&empty, Some(""), HOME);
        assert_eq!(empty.writes(), 0);
    }

    // The second start finds everything right and writes nothing.
    #[test]
    fn a_second_start_writes_nothing() {
        let store = Fake::with_list(&[THEIRS]);
        sync_with(&store, Some("F10"), HOME);
        let after_first = store.writes();
        assert!(after_first > 0);
        sync_with(&store, Some("F10"), HOME);
        sync_with(&store, Some("F10"), HOME);
        assert_eq!(store.writes(), after_first, "nothing was written again");
        assert_eq!(
            store.list(),
            [THEIRS, ENTRY_PATH],
            "and it is not listed twice"
        );
    }

    // A shortcut already listed and right, e.g. declared by the NixOS module of an
    // older setup, is left as it is.
    #[test]
    fn a_shortcut_that_is_already_right_is_not_written() {
        let store = Fake::with_list(&[ENTRY_PATH]);
        *store.entry.borrow_mut() = Entry {
            name: "VexTheater home".into(),
            command: HOME.into(),
            binding: "F10".into(),
        };
        sync_with(&store, Some("F10"), HOME);
        assert_eq!(store.writes(), 0);
    }

    // What was edited by hand, or declared differently, is put back; the list is not
    // touched for it.
    #[test]
    fn a_changed_key_or_command_is_written_back_without_touching_the_list() {
        let store = Fake::with_list(&[ENTRY_PATH]);
        *store.entry.borrow_mut() = Entry {
            name: "VexTheater home".into(),
            command: "/old/store/path --home".into(),
            binding: "F9".into(),
        };
        sync_with(&store, Some("F10"), HOME);
        assert_eq!(store.entry().binding, "F10");
        assert_eq!(store.entry().command, HOME);
        assert_eq!(store.writes(), 1, "one write, for the entry");
        assert_eq!(store.list(), [ENTRY_PATH]);
    }

    // With no home key the launcher removes its own shortcut and nobody else's.
    #[test]
    fn without_a_home_key_only_our_shortcut_is_removed() {
        let store = Fake::with_list(&[THEIRS, ENTRY_PATH, "/other/"]);
        *store.entry.borrow_mut() = Entry {
            name: "VexTheater home".into(),
            command: HOME.into(),
            binding: "F10".into(),
        };
        sync_with(&store, None, HOME);
        assert_eq!(store.list(), [THEIRS, "/other/"]);
        assert_eq!(store.entry(), Entry::default());
    }

    #[test]
    fn without_a_home_key_and_no_shortcut_nothing_is_written() {
        let store = Fake::with_list(&[THEIRS]);
        sync_with(&store, None, HOME);
        sync_with(&store, None, HOME);
        assert_eq!(store.writes(), 0);
    }

    // The command names the program of the running system when that exists, never a
    // store path, which changes with every rebuild.
    #[test]
    fn the_command_prefers_the_system_path_and_falls_back_to_the_name() {
        let dir = std::env::temp_dir().join(format!("vextheater-bin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let present = dir.join("vextheater");
        std::fs::write(&present, "").unwrap();
        assert_eq!(
            command_for(&present),
            format!("{} --home", present.display())
        );
        assert_eq!(command_for(&dir.join("missing")), "vextheater --home");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
