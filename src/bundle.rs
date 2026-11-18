//! Export and restore of the layout the user made: the services added, the configured
//! tiles removed, and the order of each page. The two state files (`tiles.json` and
//! `order.json`) are carried in one versioned bundle, with the names of the configured
//! tiles at the time, so a restore can say what no longer matches the configuration.
//! Browser profiles, running-app and mode records, GNOME snapshots and the system
//! configuration are not part of it.

use crate::model::PageModel;
use crate::order::{self, Identity, Saved};
use crate::tiles::{self, Shown};
use crate::{catalog, files};
use gio::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

const FORMAT: &str = "vextheater-layout";
const VERSION: u32 = 1;

/// The page key and configured tile names of each page, as `model::configured_names`
/// gives them.
pub type Pages = [(String, Vec<String>)];

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    format: String,
    version: u32,
    /// Ids of catalog services on the main row, in the order they were added.
    added: Vec<String>,
    /// Configured tiles taken off a page, by page key.
    hidden: BTreeMap<String, Vec<Identity>>,
    /// Tile identities in their arranged order, by page key.
    order: Saved,
    /// The configured tile names of each page when the bundle was made.
    inventory: BTreeMap<String, Vec<String>>,
}

impl Bundle {
    /// The layout in `shown` and `saved`, made against the configuration's `pages`.
    pub fn capture(pages: &Pages, shown: &Shown, saved: &Saved) -> Bundle {
        Bundle {
            format: FORMAT.to_owned(),
            version: VERSION,
            // A service the catalog no longer has could not be read back in.
            added: shown
                .added
                .iter()
                .filter(|id| catalog::find(id).is_some())
                .cloned()
                .collect(),
            hidden: shown.hidden.clone(),
            order: saved.clone(),
            inventory: pages.iter().cloned().collect(),
        }
    }

    /// The bundle as the text of a file.
    pub fn to_text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("a bundle is plain data");
        text.push('\n');
        text
    }

    /// The bundle in `text`, or what is wrong with it. Nothing is accepted that is not
    /// exactly a version this launcher knows.
    pub fn parse(text: &str) -> Result<Bundle, String> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|error| format!("not valid JSON: {error}"))?;
        if value.get("format").and_then(|format| format.as_str()) != Some(FORMAT) {
            return Err(format!(
                "not a VexTheater layout bundle (\"format\" is not \"{FORMAT}\")"
            ));
        }
        match value.get("version").and_then(|version| version.as_u64()) {
            Some(version) if version == u64::from(VERSION) => {}
            Some(version) => {
                return Err(format!(
                    "layout bundle version {version} is not supported (this launcher reads version {VERSION})"
                ));
            }
            None => return Err("the layout bundle has no version".to_owned()),
        }
        let bundle: Bundle = serde_json::from_value(value)
            .map_err(|error| format!("malformed layout bundle: {error}"))?;
        let mut seen = BTreeSet::new();
        for id in &bundle.added {
            if catalog::find(id).is_none() {
                return Err(format!(
                    "the layout bundle adds an unknown service, \"{id}\""
                ));
            }
            if !seen.insert(id) {
                return Err(format!("the layout bundle adds \"{id}\" twice"));
            }
        }
        Ok(bundle)
    }

    fn shown(&self) -> Shown {
        Shown {
            added: self.added.clone(),
            hidden: self.hidden.clone(),
        }
    }
}

/// What a tile is called in a message: its name, and which of that name it is if it is
/// not the first.
fn describe(identity: &Identity) -> String {
    match identity {
        (name, 0) => format!("\"{name}\""),
        (name, n) => format!("\"{name}\" (#{})", n + 1),
    }
}

/// What restoring `bundle` would do against the configuration's `pages` and the
/// `current` layout, as the text to show. Lines marked `!` are what does not match.
pub fn preview(bundle: &Bundle, pages: &Pages, current: (&Shown, &Saved)) -> String {
    let known: BTreeMap<&str, Vec<Identity>> = pages
        .iter()
        .map(|(key, names)| {
            let page = PageModel::from_names(key, names.clone());
            (key.as_str(), page.identities().to_vec())
        })
        .collect();
    // The tiles of a page as the launcher lists them: with the added services, which
    // an order can name.
    let shown = bundle.shown();
    let listed: BTreeMap<&str, Vec<Identity>> = pages
        .iter()
        .map(|(key, names)| {
            let page = PageModel::listed(key, names, &shown);
            (key.as_str(), page.identities().to_vec())
        })
        .collect();
    let matches = |page: &str, identity: &Identity| {
        listed
            .get(page)
            .is_some_and(|identities| identities.contains(identity))
    };

    let hidden_count: usize = bundle.hidden.values().map(Vec::len).sum();
    let mut text = format!(
        "Layout bundle: {} added service(s), {hidden_count} removed tile(s), an order for {} page(s).\n",
        bundle.added.len(),
        bundle.order.len(),
    );
    let mut problems = Vec::new();

    for (page, identities) in &bundle.hidden {
        let stray: Vec<String> = identities
            .iter()
            .filter(|identity| !matches(page, identity))
            .map(describe)
            .collect();
        if !stray.is_empty() {
            problems.push(format!(
                "page {page}: removed {} not in this configuration",
                stray.join(", ")
            ));
        }
    }
    for (page, identities) in &bundle.order {
        let stray: Vec<String> = identities
            .iter()
            .filter(|identity| !matches(page, identity))
            .map(describe)
            .collect();
        if !stray.is_empty() {
            problems.push(format!(
                "page {page}: ordered {} not in this configuration",
                stray.join(", ")
            ));
        }
    }

    // How the configuration differs from the one the bundle was made for.
    for (page, identities) in &known {
        let was = bundle.inventory.get(*page).map_or(Vec::new(), |names| {
            PageModel::from_names(page, names.clone())
                .identities()
                .to_vec()
        });
        let new: Vec<String> = identities
            .iter()
            .filter(|identity| !was.contains(identity))
            .map(describe)
            .collect();
        if !new.is_empty() {
            problems.push(format!(
                "page {page}: configured since the export: {}",
                new.join(", ")
            ));
        }
    }
    for (page, names) in &bundle.inventory {
        let identities = known.get(page.as_str());
        let was = PageModel::from_names(page, names.clone())
            .identities()
            .to_vec();
        let gone: Vec<String> = was
            .iter()
            .filter(|identity| !identities.is_some_and(|known| known.contains(identity)))
            .map(describe)
            .collect();
        if identities.is_none() {
            problems.push(format!("page {page}: no longer in the configuration"));
        } else if !gone.is_empty() {
            problems.push(format!(
                "page {page}: configured at the export, gone now: {}",
                gone.join(", ")
            ));
        }
    }

    // An added service whose name a main-row tile has taken would be left out.
    let main: &[Identity] = known.get(tiles::MAIN).map_or(&[], Vec::as_slice);
    for id in &bundle.added {
        if let Some(app) = catalog::find(id) {
            if main.iter().any(|(name, _)| name == app.name) {
                problems.push(format!(
                    "{} is not shown: a tile of that name is on the main row",
                    app.name
                ));
            }
        }
    }

    if problems.is_empty() {
        text.push_str("Everything in it matches this configuration.\n");
    } else {
        for problem in problems {
            text.push_str("  ! ");
            text.push_str(&problem);
            text.push('\n');
        }
    }
    if *current.0 == bundle.shown() && *current.1 == bundle.order {
        text.push_str("It is already your layout: nothing would change.\n");
    } else {
        text.push_str("Restoring it would replace your current layout.\n");
    }
    text
}

/// Where the layout is kept, so tests can use a directory of their own.
pub struct Targets {
    pub tiles: PathBuf,
    pub order: PathBuf,
    /// Where the layout from before a restore is saved, as a bundle.
    pub previous: PathBuf,
}

impl Targets {
    pub fn user() -> Targets {
        Targets {
            tiles: tiles::file(),
            order: order::file(),
            previous: files::state_file("previous-layout.json"),
        }
    }

    /// The layout that is saved now.
    pub fn load(&self) -> (Shown, Saved) {
        (
            files::read_json(&self.tiles, "the added and removed tiles").unwrap_or_default(),
            files::read_json(&self.order, "the saved tile order").unwrap_or_default(),
        )
    }
}

/// A file as it is, or none if it is not there.
fn read_raw(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn put_back(path: &Path, raw: &Option<Vec<u8>>) -> std::io::Result<()> {
    match raw {
        Some(bytes) => files::write_atomic(path, bytes),
        None => files::remove(path),
    }
}

/// Makes `bundle` the saved layout. First the layout from before is saved as a bundle
/// in `targets.previous`; then the two state files are written. Each write is atomic,
/// but the two are not one transaction: if the second fails, the first is put back, and
/// the message says so (and where the previous layout is, if even that failed).
pub fn apply(bundle: &Bundle, pages: &Pages, targets: &Targets) -> Result<(), String> {
    let read = |path: &Path| {
        read_raw(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
    };
    let old_tiles = read(&targets.tiles)?;

    let (shown, saved) = targets.load();
    let previous = Bundle::capture(pages, &shown, &saved);
    files::write_atomic(&targets.previous, previous.to_text().as_bytes()).map_err(|error| {
        format!(
            "cannot save your current layout to {}, so nothing was changed: {error}",
            targets.previous.display()
        )
    })?;
    let kept = format!("Your previous layout is in {}.", targets.previous.display());

    files::write_json(&targets.tiles, &bundle.shown()).map_err(|error| {
        format!(
            "cannot write {}, so nothing was changed: {error}",
            targets.tiles.display()
        )
    })?;
    if let Err(error) = files::write_json(&targets.order, &bundle.order) {
        return Err(match put_back(&targets.tiles, &old_tiles) {
            Ok(()) => format!(
                "cannot write {}: {error}. The added and removed tiles were put back as they were. {kept}",
                targets.order.display()
            ),
            Err(undo) => format!(
                "cannot write {}: {error}, and the added and removed tiles could not be put back ({undo}), so the layout is half restored. {kept}",
                targets.order.display()
            ),
        });
    }
    Ok(())
}

/// Whether a launcher is running in this session, or why that cannot be told.
pub fn launcher_running() -> Result<bool, String> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .map_err(|error| format!("cannot reach the session bus: {error}"))?;
    let reply = bus
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&(crate::APP_ID,).to_variant()),
            Some(glib::VariantTy::new("(b)").expect("a valid type string")),
            gio::DBusCallFlags::NONE,
            2000,
            gio::Cancellable::NONE,
        )
        .map_err(|error| format!("cannot ask the session bus: {error}"))?;
    reply
        .get::<(bool,)>()
        .map(|(running,)| running)
        .ok_or_else(|| "the session bus gave an unexpected reply".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(list: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        list.iter()
            .map(|(key, names)| {
                (
                    (*key).to_owned(),
                    names.iter().map(|n| (*n).to_owned()).collect(),
                )
            })
            .collect()
    }

    fn id(name: &str, n: usize) -> Identity {
        (name.to_owned(), n)
    }

    /// A directory of its own for the files of one test.
    fn targets(name: &str) -> Targets {
        let dir =
            std::env::temp_dir().join(format!("vextheater-bundle-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Targets {
            tiles: dir.join("tiles.json"),
            order: dir.join("order.json"),
            previous: dir.join("previous-layout.json"),
        }
    }

    /// The tiles of each page as the launcher would show them.
    fn effective(pages: &Pages, shown: &Shown, saved: &Saved) -> Vec<(String, Vec<String>)> {
        pages
            .iter()
            .map(|(key, names)| {
                let page = PageModel::listed(key, names, shown);
                let shown_names = page
                    .shown(shown, saved)
                    .into_iter()
                    .map(|index| page.names()[index].clone())
                    .collect();
                (key.clone(), shown_names)
            })
            .collect()
    }

    fn a_service() -> &'static str {
        catalog::APPS[0].id
    }

    /// A layout with a service added, a duplicate-named tile hidden, and a reordering.
    fn a_layout() -> (Vec<(String, Vec<String>)>, Shown, Saved) {
        let pages = pages(&[
            ("main", &["Browser", "Files", "Browser", "Music"]),
            ("submenu:Games", &["Alpha", "Beta"]),
        ]);
        let shown = Shown {
            added: vec![a_service().to_owned()],
            hidden: BTreeMap::from([("main".to_owned(), vec![id("Browser", 1)])]),
        };
        let saved = Saved::from([
            (
                "main".to_owned(),
                vec![
                    id("Music", 0),
                    id("Browser", 1),
                    id("Browser", 0),
                    id("Files", 0),
                ],
            ),
            (
                "submenu:Games".to_owned(),
                vec![id("Beta", 0), id("Alpha", 0)],
            ),
        ]);
        (pages, shown, saved)
    }

    #[test]
    fn a_round_trip_gives_the_same_listing() {
        let (pages, shown, saved) = a_layout();
        let before = effective(&pages, &shown, &saved);
        assert_eq!(before[0].1[0], "Music", "the layout does something");
        let browsers = before[0].1.iter().filter(|name| *name == "Browser").count();
        assert_eq!(browsers, 1, "a tile is hidden");

        let text = Bundle::capture(&pages, &shown, &saved).to_text();
        let bundle = Bundle::parse(&text).unwrap();
        let there = targets("round-trip");
        apply(&bundle, &pages, &there).unwrap();

        let (shown_back, saved_back) = there.load();
        assert_eq!(effective(&pages, &shown_back, &saved_back), before);
        assert_eq!((shown_back, saved_back), (shown, saved));
    }

    #[test]
    fn what_is_not_a_known_bundle_is_rejected() {
        let (pages, shown, saved) = a_layout();
        let good = Bundle::capture(&pages, &shown, &saved).to_text();
        assert!(Bundle::parse(&good).is_ok());

        let bad = |text: &str| Bundle::parse(text).unwrap_err();
        assert!(bad("").contains("not valid JSON"));
        assert!(bad("{ nope").contains("not valid JSON"));
        assert!(bad("[]").contains("not a VexTheater layout bundle"));
        assert!(bad(r#"{"format":"other","version":1}"#).contains("not a VexTheater"));
        assert!(bad(r#"{"format":"vextheater-layout"}"#).contains("no version"));
        assert!(bad(&good.replace("\"version\": 1", "\"version\": 2")).contains("version 2"));
        assert!(bad(&good.replace("\"version\": 1", "\"version\": \"1\"")).contains("no version"));
        assert!(bad(&good.replace("\"added\"", "\"extra\": 1, \"added\"")).contains("malformed"));
        assert!(bad(&good.replace("\"inventory\"", "\"inventor\"")).contains("malformed"));
        assert!(bad(&good.replace(a_service(), "no-such-service")).contains("unknown service"));
        let twice = good.replace(
            &format!("\"{}\"", a_service()),
            &format!("\"{0}\", \"{0}\"", a_service()),
        );
        assert!(bad(&twice).contains("twice"));
    }

    #[test]
    fn a_rejected_bundle_changes_nothing() {
        // Parsing is all that happens before apply; a bad file never reaches it.
        let there = targets("rejected");
        assert!(Bundle::parse("{ nope").is_err());
        assert!(!there.tiles.exists() && !there.order.exists() && !there.previous.exists());
    }

    #[test]
    fn a_changed_configuration_reports_what_no_longer_matches() {
        let (pages, shown, saved) = a_layout();
        let bundle = Bundle::capture(&pages, &shown, &saved);

        let same = preview(&bundle, &pages, (&shown, &saved));
        assert!(same.contains("Everything in it matches"), "{same}");
        assert!(same.contains("nothing would change"), "{same}");

        // The configuration lost "Music" and the second "Browser", gained "Radio", lost
        // the Games page, and has a tile named like the added service.
        let service = catalog::find(a_service()).unwrap().name;
        let changed = self::pages(&[("main", &["Browser", "Files", "Radio", service])]);
        let text = preview(&bundle, &changed, (&Shown::default(), &Saved::new()));
        assert!(
            text.contains("page main: removed \"Browser\" (#2) not in this configuration"),
            "{text}"
        );
        assert!(
            text.contains(
                "page main: ordered \"Music\", \"Browser\" (#2) not in this configuration"
            ),
            "{text}"
        );
        assert!(
            text.contains("page submenu:Games: ordered \"Beta\", \"Alpha\" not in"),
            "{text}"
        );
        assert!(
            text.contains("page main: configured since the export: \"Radio\""),
            "{text}"
        );
        assert!(
            text.contains("gone now: \"Browser\" (#2), \"Music\""),
            "{text}"
        );
        assert!(
            text.contains("page submenu:Games: no longer in the configuration"),
            "{text}"
        );
        assert!(text.contains(&format!("{service} is not shown")), "{text}");
        assert!(text.contains("would replace your current layout"), "{text}");
    }

    #[test]
    fn an_order_naming_an_added_service_matches_it() {
        let pages = pages(&[("main", &["Files"])]);
        let service = catalog::find(a_service()).unwrap().name;
        let shown = Shown {
            added: vec![a_service().to_owned()],
            hidden: BTreeMap::new(),
        };
        let saved = Saved::from([("main".to_owned(), vec![id(service, 0), id("Files", 0)])]);
        let bundle = Bundle::capture(&pages, &shown, &saved);

        let text = preview(&bundle, &pages, (&shown, &saved));
        assert!(text.contains("Everything in it matches"), "{text}");

        // Without the service in the bundle's added list, the order entry matches nothing.
        let mut without = Bundle::capture(&pages, &shown, &saved);
        without.added.clear();
        let text = preview(&without, &pages, (&shown, &saved));
        assert!(
            text.contains(&format!("ordered \"{service}\" not in")),
            "{text}"
        );
    }

    #[test]
    fn the_capture_leaves_out_a_service_the_catalog_does_not_have() {
        let (pages, mut shown, saved) = a_layout();
        shown.added.push("retired-service".to_owned());
        let bundle = Bundle::capture(&pages, &shown, &saved);
        assert_eq!(bundle.added, [a_service()]);
        assert!(Bundle::parse(&bundle.to_text()).is_ok());
    }

    #[test]
    fn the_previous_layout_is_saved_before_anything_is_replaced() {
        let (pages, shown, saved) = a_layout();
        let there = targets("previous");
        files::write_json(&there.tiles, &shown).unwrap();
        files::write_json(&there.order, &saved).unwrap();

        let empty = Bundle::capture(&pages, &Shown::default(), &Saved::new());
        apply(&empty, &pages, &there).unwrap();
        assert_eq!(there.load(), (Shown::default(), Saved::new()));

        let previous = Bundle::parse(&std::fs::read_to_string(&there.previous).unwrap()).unwrap();
        assert_eq!((previous.shown(), previous.order), (shown, saved));
    }

    #[test]
    fn a_failure_on_the_second_write_puts_the_first_file_back() {
        let (pages, shown, saved) = a_layout();
        let there = targets("second-write");
        files::write_json(&there.tiles, &shown).unwrap();
        files::write_json(&there.order, &saved).unwrap();
        let tiles_before = std::fs::read(&there.tiles).unwrap();
        let order_before = std::fs::read(&there.order).unwrap();

        // The order file cannot be replaced: a directory is in the way of its temporary.
        std::fs::create_dir_all(there.order.with_extension("json.tmp")).unwrap();
        let other = Bundle::capture(&pages, &Shown::default(), &Saved::new());
        let error = apply(&other, &pages, &there).unwrap_err();

        assert!(error.contains("put back as they were"), "{error}");
        assert!(error.contains("previous-layout.json"), "{error}");
        assert_eq!(std::fs::read(&there.tiles).unwrap(), tiles_before);
        assert_eq!(std::fs::read(&there.order).unwrap(), order_before);
        let previous = Bundle::parse(&std::fs::read_to_string(&there.previous).unwrap()).unwrap();
        assert_eq!((previous.shown(), previous.order), (shown, saved));
    }

    #[test]
    fn a_failure_on_the_first_write_changes_nothing() {
        let (pages, shown, saved) = a_layout();
        let there = targets("first-write");
        files::write_json(&there.order, &saved).unwrap();
        let order_before = std::fs::read(&there.order).unwrap();

        std::fs::create_dir_all(there.tiles.with_extension("json.tmp")).unwrap();
        let bundle = Bundle::capture(&pages, &shown, &saved);
        let error = apply(&bundle, &pages, &there).unwrap_err();

        assert!(error.contains("nothing was changed"), "{error}");
        assert!(!there.tiles.exists());
        assert_eq!(std::fs::read(&there.order).unwrap(), order_before);
    }

    #[test]
    fn nothing_is_replaced_if_the_previous_layout_cannot_be_saved() {
        let (pages, shown, saved) = a_layout();
        let there = targets("no-previous");
        files::write_json(&there.tiles, &Shown::default()).unwrap();
        let tiles_before = std::fs::read(&there.tiles).unwrap();

        std::fs::create_dir_all(there.previous.with_extension("json.tmp")).unwrap();
        let bundle = Bundle::capture(&pages, &shown, &saved);
        let error = apply(&bundle, &pages, &there).unwrap_err();

        assert!(error.contains("so nothing was changed"), "{error}");
        assert_eq!(std::fs::read(&there.tiles).unwrap(), tiles_before);
        assert!(!there.order.exists());
    }
}
