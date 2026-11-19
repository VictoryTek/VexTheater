//! Browse all, as plain data with no GTK: the app tiles a user can reach from the main
//! row, and the filter over them. Nothing here reads or writes saved state; what is
//! removed and arranged is passed in, and the result is only a list to show.

use crate::model::{PageModel, SUBMENU_PREFIX};
use crate::order::{Identity, Saved};
use crate::tiles::{self, Shown};
use std::collections::{HashMap, HashSet};

/// What a tile does, as far as Browse all is concerned.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Starts an app: listed.
    App,
    /// Opens the submenu of that name: followed, not listed.
    Opens(String),
    /// Anything else (power, desktop mode): not listed.
    Other,
}

/// A tile of a page, by the index the page's [`PageModel`] gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub description: Option<String>,
    pub kind: Kind,
}

/// One app tile that can be reached, with where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The page key (`main` or `submenu:NAME`) and the tile's index in that page's list.
    pub page: String,
    pub index: usize,
    /// The main-row tile the app is reached through: the tile itself, or the shown one that
    /// opens the submenu it is in (directly or through other shown openers).
    pub via: usize,
    pub identity: Identity,
    pub name: String,
    /// Where it is, for the line under its name: "in Games", and for a name that more
    /// than one reachable tile has, which of them it is.
    pub context: String,
    description: Option<String>,
}

impl Found {
    /// An installed application to pick, number `index` of the list the picker holds.
    /// `status` says what is already so for it (on the launcher, say), or is empty.
    pub fn installed(index: usize, name: &str, status: &str, description: Option<String>) -> Found {
        Found {
            page: "apps".to_owned(),
            index,
            via: 0,
            identity: (name.to_owned(), 0),
            name: name.to_owned(),
            context: status.to_owned(),
            description,
        }
    }

    /// The text under the tile's name: where it is, then its own description.
    pub fn detail(&self) -> String {
        match (&self.description, self.context.is_empty()) {
            (Some(description), false) => format!("{} · {description}", self.context),
            (Some(description), true) => description.clone(),
            (None, _) => self.context.clone(),
        }
    }
}

/// What a page is called to the user.
fn title(key: &str) -> &str {
    key.strip_prefix(SUBMENU_PREFIX).unwrap_or("the main row")
}

/// The app tiles shown on the main row, and on each submenu it leads to (directly or
/// through other submenus), in the order the launcher shows them. A removed tile is not
/// listed, nor is anything in a submenu whose every opener is removed; a submenu is
/// visited once, so openers that lead back to each other end.
pub fn reachable(pages: &[(PageModel, Vec<Item>)], state: &Shown, saved: &Saved) -> Vec<Found> {
    let by_key: HashMap<&str, &(PageModel, Vec<Item>)> =
        pages.iter().map(|page| (page.0.key(), page)).collect();
    let mut found = Vec::new();
    visit(
        tiles::MAIN,
        0,
        &by_key,
        state,
        saved,
        &mut HashSet::new(),
        &mut found,
    );
    // A name two reachable tiles share says which one this is.
    let mut seen: HashMap<String, usize> = HashMap::new();
    let totals: HashMap<String, usize> = found.iter().fold(HashMap::new(), |mut totals, tile| {
        *totals.entry(tile.name.clone()).or_default() += 1;
        totals
    });
    for tile in &mut found {
        let total = totals[&tile.name];
        let nth = seen.entry(tile.name.clone()).or_default();
        *nth += 1;
        if total > 1 {
            tile.context = format!("{} ({nth} of {total})", tile.context);
        }
    }
    found
}

fn visit(
    key: &str,
    via: usize,
    pages: &HashMap<&str, &(PageModel, Vec<Item>)>,
    state: &Shown,
    saved: &Saved,
    visited: &mut HashSet<String>,
    found: &mut Vec<Found>,
) {
    let Some((page, items)) = pages.get(key) else {
        return;
    };
    if !visited.insert(key.to_owned()) {
        return;
    }
    for index in page.shown(state, saved) {
        let from = if key == tiles::MAIN { index } else { via };
        let Some(item) = items.get(index) else {
            continue;
        };
        match &item.kind {
            Kind::App => found.push(Found {
                page: key.to_owned(),
                index,
                via: from,
                identity: page.identities()[index].clone(),
                name: page.names()[index].clone(),
                context: format!("in {}", title(key)),
                description: item.description.clone(),
            }),
            Kind::Opens(name) => visit(
                &crate::model::submenu_key(name),
                from,
                pages,
                state,
                saved,
                visited,
                found,
            ),
            Kind::Other => {}
        }
    }
}

/// The letter a name is filed under: its first letter or digit, in capitals; `#` for a
/// name that starts with neither.
pub fn initial(name: &str) -> char {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .and_then(|c| c.to_uppercase().next())
        .filter(|c| c.is_alphabetic())
        .unwrap_or('#')
}

fn contains_all(tile: &Found, query: &str) -> bool {
    let haystack = format!("{} {}", tile.name, tile.detail()).to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| haystack.contains(word))
}

/// The indexes into `found` of the tiles that match `query` (every word of it appears in
/// the name, description or place, whatever the case) and, if given, start with `letter`.
/// Order is that of `found`.
pub fn matches(found: &[Found], query: &str, letter: Option<char>) -> Vec<usize> {
    found
        .iter()
        .enumerate()
        .filter(|(_, tile)| letter.is_none_or(|letter| initial(&tile.name) == letter))
        .filter(|(_, tile)| contains_all(tile, query))
        .map(|(index, _)| index)
        .collect()
}

/// The letters the tiles matching `query` start with, in order, for the remote's letter
/// groups.
pub fn initials(found: &[Found], query: &str) -> Vec<char> {
    let mut letters: Vec<char> = matches(found, query, None)
        .into_iter()
        .map(|index| initial(&found[index].name))
        .collect();
    letters.sort_unstable();
    letters.dedup();
    letters
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(description: Option<&str>) -> Item {
        Item {
            description: description.map(str::to_owned),
            kind: Kind::App,
        }
    }

    fn opens(name: &str) -> Item {
        Item {
            description: None,
            kind: Kind::Opens(name.to_owned()),
        }
    }

    fn page(key: &str, tiles: Vec<(&str, Item)>) -> (PageModel, Vec<Item>) {
        let names = tiles.iter().map(|(name, _)| (*name).to_owned()).collect();
        (
            PageModel::from_names(key, names),
            tiles.into_iter().map(|(_, item)| item).collect(),
        )
    }

    fn names(found: &[Found]) -> Vec<&str> {
        found.iter().map(|tile| tile.name.as_str()).collect()
    }

    fn hide(page: &str, name: &str, nth: usize) -> Shown {
        Shown {
            added: vec![],
            apps: vec![],
            hidden: [(page.to_owned(), vec![(name.to_owned(), nth)])].into(),
        }
    }

    #[test]
    fn apps_are_listed_where_the_launcher_shows_them_and_submenus_are_followed() {
        let pages = [
            page(
                "main",
                vec![
                    ("Browser", app(None)),
                    ("Games", opens("Games")),
                    ("Files", app(Some("Browse files"))),
                ],
            ),
            page(
                "submenu:Games",
                vec![
                    ("Chess", app(None)),
                    (
                        "Shutdown",
                        Item {
                            description: None,
                            kind: Kind::Other,
                        },
                    ),
                ],
            ),
        ];
        let found = reachable(&pages, &Shown::default(), &Saved::new());
        assert_eq!(names(&found), ["Browser", "Chess", "Files"]);
        assert_eq!(found[1].page, "submenu:Games");
        assert_eq!(found[1].context, "in Games");
        assert_eq!(found[0].context, "in the main row");
        assert_eq!(found[2].detail(), "in the main row · Browse files");
    }

    #[test]
    fn each_app_knows_the_shown_main_row_tile_it_is_reached_through() {
        // Games holds a removed opener to Tools; Tools is also on the main row.
        let pages = [
            page(
                "main",
                vec![
                    ("Games", opens("Games")),
                    ("Tools", opens("Tools")),
                    ("Files", app(None)),
                ],
            ),
            page(
                "submenu:Games",
                vec![("Chess", app(None)), ("More", opens("Tools"))],
            ),
            page("submenu:Tools", vec![("Calc", app(None))]),
        ];
        let state = hide("submenu:Games", "More", 0);
        let found = reachable(&pages, &state, &Saved::new());
        let via = |name: &str| found.iter().find(|tile| tile.name == name).unwrap().via;
        assert_eq!((via("Chess"), via("Calc"), via("Files")), (0, 1, 2));
    }

    #[test]
    fn the_saved_order_decides_the_order_of_the_results() {
        let pages = [page("main", vec![("A", app(None)), ("B", app(None))])];
        let saved = Saved::from([("main".to_owned(), vec![("B".to_owned(), 0)])]);
        let found = reachable(&pages, &Shown::default(), &saved);
        assert_eq!(names(&found), ["B", "A"]);
    }

    #[test]
    fn a_removed_tile_is_not_listed_and_a_removed_opener_hides_its_submenu() {
        let pages = [
            page(
                "main",
                vec![("Games", opens("Games")), ("Files", app(None))],
            ),
            page("submenu:Games", vec![("Chess", app(None))]),
        ];
        let found = reachable(&pages, &hide("main", "Games", 0), &Saved::new());
        assert_eq!(names(&found), ["Files"]);
        // Removed from inside the submenu, only that tile goes.
        let found = reachable(&pages, &hide("submenu:Games", "Chess", 0), &Saved::new());
        assert_eq!(names(&found), ["Files"]);
        // A submenu nothing opens is not reachable.
        let lonely = [
            page("main", vec![("Files", app(None))]),
            page("submenu:Games", vec![("Chess", app(None))]),
        ];
        let found = reachable(&lonely, &Shown::default(), &Saved::new());
        assert_eq!(names(&found), ["Files"]);
    }

    #[test]
    fn a_submenu_that_another_still_opens_stays_reachable_when_one_opener_is_removed() {
        let pages = [
            page(
                "main",
                vec![("Fun", opens("Games")), ("More", opens("Games"))],
            ),
            page("submenu:Games", vec![("Chess", app(None))]),
        ];
        let found = reachable(&pages, &hide("main", "Fun", 0), &Saved::new());
        assert_eq!(names(&found), ["Chess"]);
    }

    #[test]
    fn submenus_that_open_each_other_end_and_list_each_app_once() {
        let pages = [
            page(
                "main",
                vec![("Games", opens("Games")), ("Files", app(None))],
            ),
            page(
                "submenu:Games",
                vec![
                    ("Chess", app(None)),
                    ("Back", opens("Games")),
                    ("Tools", opens("Tools")),
                ],
            ),
            page(
                "submenu:Tools",
                vec![
                    ("Calc", app(None)),
                    ("Games", opens("Games")),
                    ("Self", opens("Tools")),
                ],
            ),
        ];
        let found = reachable(&pages, &Shown::default(), &Saved::new());
        assert_eq!(names(&found), ["Chess", "Calc", "Files"]);
    }

    #[test]
    fn an_opener_of_a_submenu_that_does_not_exist_is_ignored() {
        let pages = [page(
            "main",
            vec![("Games", opens("Gone")), ("Files", app(None))],
        )];
        let found = reachable(&pages, &Shown::default(), &Saved::new());
        assert_eq!(names(&found), ["Files"]);
    }

    #[test]
    fn tiles_sharing_a_name_say_which_one_they_are_and_keep_their_identity() {
        let pages = [
            page(
                "main",
                vec![
                    ("Browser", app(None)),
                    ("Games", opens("Games")),
                    ("Browser", app(None)),
                ],
            ),
            page("submenu:Games", vec![("Browser", app(None))]),
        ];
        let found = reachable(&pages, &Shown::default(), &Saved::new());
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].context, "in the main row (1 of 3)");
        assert_eq!(found[1].context, "in Games (2 of 3)");
        assert_eq!(found[2].context, "in the main row (3 of 3)");
        assert_eq!(found[0].identity, ("Browser".to_owned(), 0));
        assert_eq!(found[2].identity, ("Browser".to_owned(), 1));
        assert_eq!((found[2].page.as_str(), found[2].index), ("main", 2));
        // A name that only one tile has is left plain.
        let one = [page("main", vec![("Browser", app(None))])];
        let found = reachable(&one, &Shown::default(), &Saved::new());
        assert_eq!(found[0].context, "in the main row");
    }

    #[test]
    fn filtering_matches_every_word_in_the_name_description_or_place() {
        let pages = [
            page(
                "main",
                vec![
                    ("Firefox", app(Some("Web browser"))),
                    ("Files", app(Some("Browse your folders"))),
                    ("Games", opens("Games")),
                ],
            ),
            page("submenu:Games", vec![("Chess", app(None))]),
        ];
        let found = reachable(&pages, &Shown::default(), &Saved::new());
        let hits = |query: &str| {
            matches(&found, query, None)
                .into_iter()
                .map(|index| found[index].name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(hits(""), ["Firefox", "Files", "Chess"]);
        assert_eq!(hits("FIRE"), ["Firefox"]);
        assert_eq!(hits("brows"), ["Firefox", "Files"]);
        assert_eq!(hits("web brow"), ["Firefox"]);
        assert_eq!(hits("in games"), ["Chess"]);
        assert!(hits("nothing like this").is_empty());
    }

    #[test]
    fn letter_groups_list_the_initials_of_the_matches() {
        let pages = [page(
            "main",
            vec![
                ("Zed", app(None)),
                ("apple", app(None)),
                ("Avocado", app(None)),
                ("1Password", app(None)),
                ("Écran", app(None)),
            ],
        )];
        let found = reachable(&pages, &Shown::default(), &Saved::new());
        assert_eq!(initials(&found, ""), ['#', 'A', 'Z', 'É']);
        assert_eq!(initials(&found, "av"), ['A']);
        assert_eq!(matches(&found, "", Some('A')), [1, 2]);
        assert_eq!(matches(&found, "", Some('#')), [3]);
        assert_eq!(matches(&found, "ap", Some('Z')), Vec::<usize>::new());
        assert_eq!(initial("  --x"), 'X');
        assert_eq!(initial("???"), '#');
    }
}
