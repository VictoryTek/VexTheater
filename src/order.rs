//! The tile order the user arranged in the launcher. The configuration is generated and
//! read-only, so the order is kept beside it, in the state directory, as a list of tile
//! identities per page. It is applied on top of the configured order, so tiles added to
//! the configuration later still show up.

use crate::files;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Which tile on a page: its name, and how many tiles with the same name come before it
/// in the configured order (0 for a tile whose name is unique).
pub type Identity = (String, usize);

/// Tile identities in their arranged order, by page.
pub type Saved = BTreeMap<String, Vec<Identity>>;

fn file() -> PathBuf {
    files::state_file("order.json")
}

/// The saved order, or none if there is none or the file cannot be used.
pub fn load() -> Saved {
    files::read_json(&file(), "the saved tile order").unwrap_or_default()
}

pub fn save(saved: &Saved) {
    if let Err(error) = files::write_json(&file(), saved) {
        log::warn!("cannot save the tile order: {error}");
    }
}

/// Forgets the saved order, so the tiles are back in the configured order. It is an
/// error if the file is there and cannot be removed: the order would come back at the
/// next start.
pub fn clear() -> std::io::Result<()> {
    files::remove(&file())
}

/// The identity of each of a page's tiles, from their names in configured order. Tiles
/// are told apart by name; ones sharing a name by their place among those, so each keeps
/// its own position. (Adding or removing an earlier tile of the same name in the
/// configuration shifts the later ones' identities.)
pub fn identities(names: &[&str]) -> Vec<Identity> {
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let before = names[..index].iter().filter(|other| *other == name).count();
            ((*name).to_owned(), before)
        })
        .collect()
}

/// The configured indexes of a page's tiles, in the order to show them: the tiles
/// identified in `saved`, in that order, then any others in configured order.
pub fn arrange(names: &[&str], saved: Option<&Vec<Identity>>) -> Vec<usize> {
    let identities = identities(names);
    let mut used = vec![false; names.len()];
    let mut order = Vec::with_capacity(names.len());
    for wanted in saved.into_iter().flatten() {
        if let Some(index) = identities.iter().position(|identity| identity == wanted) {
            if !std::mem::replace(&mut used[index], true) {
                order.push(index);
            }
        }
    }
    order.extend((0..names.len()).filter(|&i| !used[i]));
    order
}

/// Where a tile at `position` in a row of `len` goes when moved `delta` places, or none
/// at either end of the row: tiles do not wrap around.
pub fn step(position: usize, delta: isize, len: usize) -> Option<usize> {
    position.checked_add_signed(delta).filter(|&p| p < len)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A saved order of tiles with unique names.
    fn saved(names: &[&str]) -> Vec<Identity> {
        names.iter().map(|n| ((*n).to_owned(), 0)).collect()
    }

    /// A saved order of (name, occurrence) pairs.
    fn saved_with(identities: &[(&str, usize)]) -> Vec<Identity> {
        identities
            .iter()
            .map(|(n, i)| ((*n).to_owned(), *i))
            .collect()
    }

    #[test]
    fn no_saved_order_keeps_the_configured_order() {
        assert_eq!(arrange(&["A", "B", "C"], None), [0, 1, 2]);
        assert_eq!(arrange(&[], None), Vec::<usize>::new());
    }

    #[test]
    fn follows_the_saved_order() {
        let order = saved(&["C", "A", "B"]);
        assert_eq!(arrange(&["A", "B", "C"], Some(&order)), [2, 0, 1]);
    }

    #[test]
    fn a_tile_added_to_the_configuration_goes_last() {
        let order = saved(&["B", "A"]);
        assert_eq!(arrange(&["A", "B", "New"], Some(&order)), [1, 0, 2]);
    }

    #[test]
    fn a_saved_name_no_longer_configured_is_ignored() {
        let order = saved(&["Gone", "B", "A"]);
        assert_eq!(arrange(&["A", "B"], Some(&order)), [1, 0]);
    }

    #[test]
    fn tiles_sharing_a_name_are_told_apart_by_their_place_among_them() {
        let names = ["Browser", "Files", "Browser"];
        assert_eq!(
            identities(&names),
            saved_with(&[("Browser", 0), ("Files", 0), ("Browser", 1)])
        );
    }

    #[test]
    fn each_of_two_same_named_tiles_keeps_its_own_position() {
        // The second "Browser" was moved to the front: it stays the second one.
        let names = ["Browser", "Files", "Browser"];
        let order = saved_with(&[("Browser", 1), ("Browser", 0), ("Files", 0)]);
        assert_eq!(arrange(&names, Some(&order)), [2, 0, 1]);
        let order = saved_with(&[("Files", 0), ("Browser", 1), ("Browser", 0)]);
        assert_eq!(arrange(&names, Some(&order)), [1, 2, 0]);
    }

    #[test]
    fn a_same_named_tile_missing_from_the_saved_order_goes_last() {
        let names = ["Browser", "Files", "Browser"];
        let order = saved_with(&[("Browser", 1)]);
        assert_eq!(arrange(&names, Some(&order)), [2, 0, 1]);
    }

    #[test]
    fn a_saved_identity_with_no_such_tile_or_repeated_is_ignored() {
        let order = saved_with(&[("A", 2), ("A", 0), ("A", 0), ("B", 0)]);
        assert_eq!(arrange(&["A", "B"], Some(&order)), [0, 1]);
    }

    #[test]
    fn arrange_always_returns_every_tile_once() {
        let names = ["A", "B", "A", "C"];
        for order in [
            saved(&[]),
            saved(&["C", "A"]),
            saved_with(&[("A", 1), ("A", 1), ("Z", 0), ("A", 0)]),
        ] {
            let mut result = arrange(&names, Some(&order));
            result.sort_unstable();
            assert_eq!(result, [0, 1, 2, 3]);
        }
    }

    #[test]
    fn a_tile_moves_one_place_but_not_past_either_end() {
        assert_eq!(step(1, -1, 4), Some(0));
        assert_eq!(step(1, 1, 4), Some(2));
        assert_eq!(step(0, -1, 4), None);
        assert_eq!(step(3, 1, 4), None);
        assert_eq!(step(0, 1, 1), None);
    }

    /// The order read from a file holding `text`.
    fn read(name: &str, text: &str) -> Saved {
        let dir = std::env::temp_dir().join(format!("vextheater-order-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        files::read_json(&path, "the test order").unwrap_or_default()
    }

    #[test]
    fn reads_the_order_it_writes() {
        let mut order = Saved::new();
        order.insert("main".into(), saved_with(&[("B", 0), ("A", 1)]));
        let text = serde_json::to_string(&order).unwrap();
        assert_eq!(read("good.json", &text), order);
    }

    #[test]
    fn a_corrupt_file_is_no_saved_order() {
        assert!(read("empty.json", "").is_empty());
        assert!(read("garbage.json", "{ not json").is_empty());
        assert!(read("string.json", r#"{ "main": "A" }"#).is_empty());
        assert!(read("short.json", r#"{ "main": ["A"] }"#).is_empty());
    }
}
