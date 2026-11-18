//! The tiles of a page as plain data, with no GTK: their names, their identities, and
//! which are shown in what order. The launcher, the command line (`--print-order`,
//! `--list-tiles`, `--launch`) and the layout bundle all ask this, instead of each
//! combining `tiles` and `order` its own way. It holds no state of its own beyond a
//! page's list of names; what is added, removed and arranged is passed in.

use crate::config::Config;
use crate::order::{self, Identity, Saved};
use crate::tiles::{self, Shown};

/// The prefix of a submenu's page key. Saved files hold these keys, so it must not change.
pub const SUBMENU_PREFIX: &str = "submenu:";

/// One page's list of tiles, which every index of the page refers to.
#[derive(Clone, Debug, PartialEq)]
pub struct PageModel {
    key: String,
    names: Vec<String>,
    identities: Vec<Identity>,
}

impl PageModel {
    /// A page whose list is exactly `names`.
    pub fn from_names(key: &str, names: Vec<String>) -> PageModel {
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let identities = order::identities(&refs);
        PageModel {
            key: key.to_owned(),
            names,
            identities,
        }
    }

    /// The page `key` as the launcher lists it: its `configured` tiles, then on the main
    /// row the services added in `state` (`tiles::listed`).
    pub fn listed(key: &str, configured: &[String], state: &Shown) -> PageModel {
        let refs: Vec<&str> = configured.iter().map(String::as_str).collect();
        let names = tiles::listed(key, &refs, state)
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        PageModel::from_names(key, names)
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn identities(&self) -> &[Identity] {
        &self.identities
    }

    pub fn identity(&self, index: usize) -> Option<&Identity> {
        self.identities.get(index)
    }

    /// The index of the tile with `identity`, if the page has it.
    pub fn index_of(&self, identity: &Identity) -> Option<usize> {
        self.identities.iter().position(|other| other == identity)
    }

    /// The indexes of the tiles to show, in the order to show them: arranged by `saved`,
    /// without the tiles removed in `state`.
    pub fn shown(&self, state: &Shown, saved: &Saved) -> Vec<usize> {
        let names: Vec<&str> = self.names.iter().map(String::as_str).collect();
        tiles::visible(&self.key, &names, state, saved.get(&self.key))
    }
}

/// The key of the page of the submenu `name`.
pub fn submenu_key(name: &str) -> String {
    format!("{SUBMENU_PREFIX}{name}")
}

/// The page key and configured tile names of every page of `config`, the main row first.
pub fn configured_names(config: &Config) -> Vec<(String, Vec<String>)> {
    let names =
        |tiles: &[crate::config::Tile]| tiles.iter().map(|tile| tile.name.clone()).collect();
    std::iter::once((tiles::MAIN.to_owned(), names(&config.tiles)))
        .chain(
            config
                .submenus
                .iter()
                .map(|(name, tiles)| (submenu_key(name), names(tiles))),
        )
        .collect()
}

/// Every page of `config` as the launcher lists it, the main row first.
pub fn pages(config: &Config, state: &Shown) -> Vec<PageModel> {
    configured_names(config)
        .into_iter()
        .map(|(key, names)| PageModel::listed(&key, &names, state))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| (*n).to_owned()).collect()
    }

    fn id(name: &str, n: usize) -> Identity {
        (name.to_owned(), n)
    }

    #[test]
    fn tiles_sharing_a_name_have_identities_by_place() {
        let page = PageModel::from_names("main", names(&["Browser", "Files", "Browser"]));
        assert_eq!(page.identity(0), Some(&id("Browser", 0)));
        assert_eq!(page.identity(2), Some(&id("Browser", 1)));
        assert_eq!(page.identity(3), None);
        assert_eq!(page.index_of(&id("Browser", 1)), Some(2));
        assert_eq!(page.index_of(&id("Browser", 2)), None);
    }

    #[test]
    fn the_shown_tiles_follow_the_saved_order_and_leave_out_the_removed() {
        let page = PageModel::from_names("main", names(&["A", "B", "A", "C"]));
        let state = Shown {
            added: vec![],
            hidden: [("main".to_owned(), vec![id("A", 0)])].into(),
        };
        let saved = Saved::from([("main".to_owned(), vec![id("C", 0), id("A", 1), id("A", 0)])]);
        // C, then the second A; the first A is removed; B was not saved, so it is last.
        assert_eq!(page.shown(&state, &saved), [3, 2, 1]);
        // With nothing saved: configured order, minus the removed.
        assert_eq!(page.shown(&state, &Saved::new()), [1, 2, 3]);
        // An order naming a tile that is not there is ignored.
        let stray = Saved::from([("main".to_owned(), vec![id("Gone", 0), id("B", 0)])]);
        assert_eq!(page.shown(&Shown::default(), &stray), [1, 0, 2, 3]);
    }

    #[test]
    fn an_added_service_is_listed_after_the_configured_tiles_on_the_main_row_only() {
        let service = &crate::catalog::APPS[0];
        let state = Shown {
            added: vec![service.id.to_owned()],
            hidden: Default::default(),
        };
        let configured = names(&["Files"]);
        let main = PageModel::listed("main", &configured, &state);
        assert_eq!(main.names(), ["Files", service.name]);
        let submenu = PageModel::listed("submenu:Games", &configured, &state);
        assert_eq!(submenu.names(), ["Files"]);
        // A configured tile that has the service's name keeps the name one tile.
        let taken = PageModel::listed("main", &names(&[service.name]), &state);
        assert_eq!(taken.names(), [service.name]);
    }

    #[test]
    fn every_page_of_a_configuration_is_modelled_with_its_key() {
        let config = Config::parse(
            r#"{ "tiles": [ { "name": "A", "icon": "a", "command": "a" },
                            { "name": "Games", "icon": "g", "submenu": "Games" } ],
                 "submenus": { "Games": [ { "name": "X", "icon": "x", "command": "x" } ] } }"#,
        )
        .unwrap();
        let pages = pages(&config, &Shown::default());
        let keys: Vec<&str> = pages.iter().map(PageModel::key).collect();
        assert_eq!(keys, ["main", "submenu:Games"]);
        assert_eq!(pages[0].names(), ["A", "Games"]);
        assert_eq!(pages[1].names(), ["X"]);
        assert_eq!(submenu_key("Games"), "submenu:Games");
    }
}
