//! Which tiles the user added to the launcher and which configured ones they removed.
//! The configuration is generated and read-only, so this is kept beside it, in the
//! state directory, like the tile order (`order.rs`). Services of the catalog that were
//! added join the main row; a removed configured tile is hidden by its identity.

use crate::catalog::{self, App};
use crate::files;
use crate::order::{self, Identity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The page key of the main row, which is where added tiles go.
pub const MAIN: &str = "main";

/// What the user changed, as saved in `tiles.json`.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Shown {
    /// Ids of catalog services on the main row, in the order they were added.
    #[serde(default)]
    pub added: Vec<String>,
    /// Configured tiles taken off a page, by page key.
    #[serde(default)]
    pub hidden: BTreeMap<String, Vec<Identity>>,
}

fn file() -> PathBuf {
    files::state_file("tiles.json")
}

impl Shown {
    /// The saved state, or none if there is none or the file cannot be used.
    pub fn load() -> Shown {
        files::read_json(&file(), "the added and removed tiles").unwrap_or_default()
    }

    /// Saves the state; true if it was written. A failure is logged and the caller says
    /// so, since the change would not survive a restart.
    pub fn save(&self) -> bool {
        match files::write_json(&file(), self) {
            Ok(()) => true,
            Err(error) => {
                log::warn!("cannot save the added and removed tiles: {error}");
                false
            }
        }
    }

    /// Puts a service of the catalog on the main row. Nothing for an id the catalog does
    /// not have, or a service already there.
    pub fn add(&mut self, id: &str) {
        if catalog::find(id).is_some() && !self.added.iter().any(|added| added == id) {
            self.added.push(id.to_owned());
        }
    }

    /// Takes a catalog service off the main row.
    pub fn remove(&mut self, id: &str) {
        self.added.retain(|added| added != id);
    }

    /// Takes a configured tile off a page.
    pub fn hide(&mut self, page: &str, identity: Identity) {
        let hidden = self.hidden.entry(page.to_owned()).or_default();
        if !hidden.contains(&identity) {
            hidden.push(identity);
        }
    }

    /// Puts a hidden configured tile back on its page.
    pub fn unhide(&mut self, page: &str, identity: &Identity) {
        if let Some(hidden) = self.hidden.get_mut(page) {
            hidden.retain(|other| other != identity);
            if hidden.is_empty() {
                self.hidden.remove(page);
            }
        }
    }

    fn is_hidden(&self, page: &str, identity: &Identity) -> bool {
        self.hidden
            .get(page)
            .is_some_and(|hidden| hidden.contains(identity))
    }
}

/// A tile of a page before it is arranged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item {
    /// The configured tile at this index of the page.
    Configured(usize),
    Catalog(&'static App),
}

/// The tiles of a page before they are arranged, with their names: the configured ones,
/// then, on the main row, the catalog services that were added. A service whose name a
/// configured tile has since taken is left out, so a name stays one tile. A page's
/// tiles are known by their index in this list.
pub fn listed(page: &str, configured: &[&str], state: &Shown) -> Vec<(Item, String)> {
    let mut list: Vec<(Item, String)> = configured
        .iter()
        .enumerate()
        .map(|(index, name)| (Item::Configured(index), (*name).to_owned()))
        .collect();
    if page == MAIN {
        for id in &state.added {
            let Some(app) = catalog::find(id) else {
                continue;
            };
            let taken = list.iter().any(|(_, name)| name == app.name);
            if !taken {
                list.push((Item::Catalog(app), app.name.to_owned()));
            }
        }
    }
    list
}

/// The indexes into a page's list (`names`, as [`listed`] gives them) of the tiles to
/// show, in the order to show them: arranged by the saved order, without the configured
/// tiles that were removed. Identities are those of the whole list, so removing a tile
/// never changes another's.
pub fn visible(
    page: &str,
    names: &[&str],
    state: &Shown,
    saved: Option<&Vec<Identity>>,
) -> Vec<usize> {
    let identities = order::identities(names);
    order::arrange(names, saved)
        .into_iter()
        .filter(|&index| !state.is_hidden(page, &identities[index]))
        .collect()
}

impl Shown {
    /// The configured tiles taken off `page`.
    pub fn hidden_on(&self, page: &str) -> &[Identity] {
        self.hidden.get(page).map_or(&[], Vec::as_slice)
    }
}

/// The order to save for a page: `shown` as it is now, with the hidden tiles the old
/// order had kept where they were (each right after the tile that preceded it), so a
/// tile put back returns to its place. `old` is the order saved before, or, if none was,
/// the page's order in the configuration.
pub fn order_to_save(old: &[Identity], shown: Vec<Identity>, hidden: &[Identity]) -> Vec<Identity> {
    let mut result = shown;
    for (position, identity) in old.iter().enumerate() {
        if !hidden.contains(identity) || result.contains(identity) {
            continue;
        }
        // After the nearest earlier tile of the old order that is in the new one.
        let after = old[..position]
            .iter()
            .rev()
            .find_map(|earlier| result.iter().position(|kept| kept == earlier));
        result.insert(after.map_or(0, |at| at + 1), identity.clone());
    }
    result
}

/// Something the Add page offers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Addable {
    /// A removed configured tile: the page, and its index in that page's configuration.
    Hidden {
        page: usize,
        index: usize,
    },
    Catalog(&'static App),
}

/// What can be put on the launcher: removed configured tiles first, then the catalog
/// services that are not shown. `pages` are the configured tile names of each page, the
/// main row first, with the page's key. A hidden identity that matches no tile is
/// ignored, as is a catalog service whose name a main-row tile has (hidden or not): it
/// would be a second tile of the same name.
pub fn addable(pages: &[(&str, Vec<&str>)], state: &Shown) -> Vec<Addable> {
    let mut offers = Vec::new();
    for (page, (key, names)) in pages.iter().enumerate() {
        let identities = order::identities(names);
        for (index, identity) in identities.iter().enumerate() {
            if state.is_hidden(key, identity) {
                offers.push(Addable::Hidden { page, index });
            }
        }
    }
    let main_names: &[&str] = pages
        .iter()
        .find(|(key, _)| *key == MAIN)
        .map_or(&[], |(_, names)| names.as_slice());
    offers.extend(
        catalog::APPS
            .iter()
            .filter(|app| !state.added.iter().any(|added| added == app.id))
            .filter(|app| !main_names.contains(&app.name))
            .map(Addable::Catalog),
    );
    offers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str) -> Identity {
        (name.to_owned(), 0)
    }

    fn ids(names: &[&str]) -> Vec<Identity> {
        names.iter().map(|name| id(name)).collect()
    }

    fn state(added: &[&str], hidden: &[(&str, &[&str])]) -> Shown {
        Shown {
            added: added.iter().map(|s| (*s).to_owned()).collect(),
            hidden: hidden
                .iter()
                .map(|(page, names)| ((*page).to_owned(), ids(names)))
                .collect(),
        }
    }

    /// What a page shows: the item and name of each tile, in order.
    fn show(
        page: &str,
        configured: &[&str],
        state: &Shown,
        saved: Option<&Vec<Identity>>,
    ) -> Vec<(Item, String)> {
        let list = listed(page, configured, state);
        let names: Vec<&str> = list.iter().map(|(_, name)| name.as_str()).collect();
        visible(page, &names, state, saved)
            .into_iter()
            .map(|index| list[index].clone())
            .collect()
    }

    fn names(tiles: &[(Item, String)]) -> Vec<&str> {
        tiles.iter().map(|(_, name)| name.as_str()).collect()
    }

    #[test]
    fn nothing_changed_shows_the_configured_tiles() {
        let configured = ["A", "B", "C"];
        let tiles = show(MAIN, &configured, &Shown::default(), None);
        assert_eq!(names(&tiles), ["A", "B", "C"]);
        assert_eq!(
            tiles.iter().map(|t| t.0).collect::<Vec<_>>(),
            [
                Item::Configured(0),
                Item::Configured(1),
                Item::Configured(2)
            ]
        );
    }

    #[test]
    fn an_added_service_goes_last_and_can_be_arranged_like_the_others() {
        let configured = ["A", "B"];
        let added = state(&["netflix"], &[]);
        let tiles = show(MAIN, &configured, &added, None);
        assert_eq!(names(&tiles), ["A", "B", "Netflix"]);
        let moved = ids(&["Netflix", "A", "B"]);
        let tiles = show(MAIN, &configured, &added, Some(&moved));
        assert_eq!(names(&tiles), ["Netflix", "A", "B"]);
    }

    #[test]
    fn added_services_go_in_the_main_row_only() {
        let configured = ["Doom"];
        let added = state(&["netflix"], &[]);
        let tiles = show("submenu:Games", &configured, &added, None);
        assert_eq!(names(&tiles), ["Doom"]);
    }

    #[test]
    fn a_removed_configured_tile_is_not_shown() {
        let configured = ["A", "B", "C"];
        let removed = state(&[], &[(MAIN, &["B"])]);
        let tiles = show(MAIN, &configured, &removed, None);
        assert_eq!(names(&tiles), ["A", "C"]);
        // On the page it was removed from only.
        let tiles = show("submenu:X", &configured, &removed, None);
        assert_eq!(names(&tiles), ["A", "B", "C"]);
    }

    #[test]
    fn a_put_back_tile_returns_to_its_saved_place() {
        let configured = ["A", "B", "C"];
        let saved = ids(&["C", "B", "A"]);
        let mut now = state(&[], &[(MAIN, &["B"])]);
        let tiles = show(MAIN, &configured, &now, Some(&saved));
        assert_eq!(names(&tiles), ["C", "A"]);
        now.unhide(MAIN, &id("B"));
        let tiles = show(MAIN, &configured, &now, Some(&saved));
        assert_eq!(names(&tiles), ["C", "B", "A"]);
        assert!(now.hidden.is_empty());
    }

    #[test]
    fn a_stale_id_or_hidden_identity_is_ignored() {
        let configured = ["A", "B"];
        let stale = state(
            &["gone-service"],
            &[(MAIN, &["Gone"]), ("submenu:Gone", &["A"])],
        );
        let tiles = show(MAIN, &configured, &stale, None);
        assert_eq!(names(&tiles), ["A", "B"]);
    }

    #[test]
    fn a_service_the_configuration_later_gains_is_not_shown_twice() {
        let configured = ["Plex", "YouTube"];
        let added = state(&["youtube", "netflix"], &[]);
        let tiles = show(MAIN, &configured, &added, None);
        assert_eq!(names(&tiles), ["Plex", "YouTube", "Netflix"]);
        assert_eq!(tiles[1].0, Item::Configured(1));
    }

    #[test]
    fn removing_one_of_two_same_named_tiles_keeps_the_others_identity() {
        let configured = ["Browser", "Files", "Browser"];
        let removed = Shown {
            hidden: BTreeMap::from([(MAIN.to_owned(), vec![("Browser".to_owned(), 0)])]),
            ..Shown::default()
        };
        let tiles = show(MAIN, &configured, &removed, None);
        assert_eq!(
            tiles.iter().map(|t| t.0).collect::<Vec<_>>(),
            [Item::Configured(1), Item::Configured(2)]
        );
        // The second "Browser" is still the second one: it is not hidden because the
        // first was.
        let again = state(&[], &[(MAIN, &["Files"])]);
        let tiles = show(MAIN, &configured, &again, None);
        assert_eq!(names(&tiles), ["Browser", "Browser"]);
    }

    #[test]
    fn hidden_identities_are_listed_per_page() {
        let now = state(&[], &[(MAIN, &["A"])]);
        assert_eq!(now.hidden_on(MAIN), ids(&["A"]));
        assert!(now.hidden_on("submenu:X").is_empty());
    }

    #[test]
    fn adding_is_idempotent_and_ignores_unknown_ids() {
        let mut now = Shown::default();
        now.add("netflix");
        now.add("netflix");
        now.add("no-such-service");
        assert_eq!(now.added, ["netflix"]);
        now.remove("netflix");
        assert!(now.added.is_empty());
    }

    #[test]
    fn hiding_twice_keeps_one_entry() {
        let mut now = Shown::default();
        now.hide(MAIN, id("A"));
        now.hide(MAIN, id("A"));
        assert_eq!(now.hidden[MAIN], ids(&["A"]));
    }

    #[test]
    fn the_saved_order_keeps_where_a_hidden_tile_was() {
        let old = ids(&["C", "B", "A", "D"]);
        let hidden = ids(&["B"]);
        // The user moved D to the front.
        let result = order_to_save(&old, ids(&["D", "C", "A"]), &hidden);
        assert_eq!(result, ids(&["D", "C", "B", "A"]));
        // Put back, it is after C, where it was.
    }

    // With no order saved yet, the configured order is what the hidden tile keeps.
    #[test]
    fn a_hidden_tile_keeps_its_configured_place_when_nothing_was_saved() {
        let configured = ids(&["A", "B", "C", "D"]);
        // B was removed, then D moved to the front.
        let result = order_to_save(&configured, ids(&["D", "A", "C"]), &ids(&["B"]));
        assert_eq!(result, ids(&["D", "A", "B", "C"]));
    }

    #[test]
    fn a_hidden_tile_that_led_the_old_order_still_does() {
        let old = ids(&["B", "A"]);
        let result = order_to_save(&old, ids(&["A"]), &ids(&["B"]));
        assert_eq!(result, ids(&["B", "A"]));
        // A hidden tile the old order lacked is not invented.
        assert_eq!(
            order_to_save(&ids(&["A"]), ids(&["A"]), &ids(&["Z"])),
            ids(&["A"])
        );
    }

    #[test]
    fn hidden_configured_tiles_are_offered_first_then_the_catalog() {
        let pages = [
            (MAIN, vec!["Plex", "YouTube"]),
            ("submenu:Games", vec!["Doom"]),
        ];
        let removed = state(
            &["netflix"],
            &[(MAIN, &["Plex"]), ("submenu:Games", &["Doom"])],
        );
        let offers = addable(&pages, &removed);
        assert_eq!(offers[0], Addable::Hidden { page: 0, index: 0 });
        assert_eq!(offers[1], Addable::Hidden { page: 1, index: 0 });
        let catalog_offers: Vec<_> = offers[2..]
            .iter()
            .map(|offer| match offer {
                Addable::Catalog(app) => app.id,
                other => panic!("{other:?}"),
            })
            .collect();
        // Netflix is already added; YouTube is a configured tile's name.
        assert!(!catalog_offers.contains(&"netflix"));
        assert!(!catalog_offers.contains(&"youtube"));
        assert!(catalog_offers.contains(&"hulu"));
    }

    #[test]
    fn a_catalog_service_named_like_a_removed_main_tile_is_not_offered() {
        let pages = [(MAIN, vec!["YouTube"])];
        let removed = state(&[], &[(MAIN, &["YouTube"])]);
        let offers = addable(&pages, &removed);
        assert_eq!(offers[0], Addable::Hidden { page: 0, index: 0 });
        assert!(!offers
            .iter()
            .any(|o| matches!(o, Addable::Catalog(a) if a.id == "youtube")));
    }

    #[test]
    fn a_hidden_identity_with_no_tile_is_not_offered() {
        let pages = [(MAIN, vec!["A"])];
        let removed = state(&[], &[(MAIN, &["Gone"])]);
        assert!(addable(&pages, &removed)
            .iter()
            .all(|offer| matches!(offer, Addable::Catalog(_))));
    }

    #[test]
    fn everything_added_leaves_nothing_to_offer() {
        let pages = [(MAIN, vec![])];
        let all: Vec<&str> = catalog::APPS.iter().map(|app| app.id).collect();
        assert!(addable(&pages, &state(&all, &[])).is_empty());
    }

    /// The state read from a file holding `text`.
    fn read(name: &str, text: &str) -> Shown {
        let dir = std::env::temp_dir().join(format!("vextheater-tiles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        files::read_json(&path, "the test tiles").unwrap_or_default()
    }

    #[test]
    fn reads_the_state_it_writes() {
        let now = state(&["netflix", "hulu"], &[(MAIN, &["Plex"])]);
        let text = serde_json::to_string(&now).unwrap();
        assert_eq!(read("good.json", &text), now);
        assert_eq!(
            read(
                "plan.json",
                r#"{ "added": ["netflix"], "hidden": { "main": [["Plex", 0]] } }"#
            ),
            state(&["netflix"], &[(MAIN, &["Plex"])])
        );
    }

    #[test]
    fn a_corrupt_file_is_no_state() {
        assert_eq!(read("empty.json", ""), Shown::default());
        assert_eq!(read("garbage.json", "{ not json"), Shown::default());
        assert_eq!(
            read("string.json", r#"{ "added": "netflix" }"#),
            Shown::default()
        );
        assert_eq!(
            read("short.json", r#"{ "hidden": { "main": ["A"] } }"#),
            Shown::default()
        );
    }

    #[test]
    fn a_file_with_only_one_key_is_fine() {
        assert_eq!(
            read("added.json", r#"{ "added": ["hulu"] }"#),
            state(&["hulu"], &[])
        );
        assert_eq!(read("none.json", "{}"), Shown::default());
    }
}
