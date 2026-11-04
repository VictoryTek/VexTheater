//! The streaming services the launcher offers on its own, so a tile for one can be added
//! from the TV without the configuration (see `tiles.rs`). Each opens as a web app in the
//! browser (`webapp.rs`).
//!
//! An id is saved in `tiles.json` once a user adds that service, so it must never change
//! after it ships.

/// A service of the catalog.
#[derive(Debug, PartialEq)]
pub struct App {
    /// Stable, and part of the icon name and the profile directory.
    pub id: &'static str,
    /// The tile's name. A configured main-row tile with this name hides the entry.
    pub name: &'static str,
    pub url: &'static str,
    pub description: &'static str,
    /// Whether the site is shown its TV interface, which needs a smart-TV user agent.
    pub tv_user_agent: bool,
}

impl App {
    /// The icon-theme name of the service's logo, installed by `nix/package.nix` into the
    /// hicolor theme. Without the package (a development run) the tile shows the
    /// placeholder.
    pub fn icon(&self) -> String {
        format!("vextheater-{}", self.id)
    }
}

const fn app(
    id: &'static str,
    name: &'static str,
    url: &'static str,
    description: &'static str,
) -> App {
    App {
        id,
        name,
        url,
        description,
        tv_user_agent: false,
    }
}

/// In the order they are offered.
pub const APPS: &[App] = &[
    app(
        "netflix",
        "Netflix",
        "https://www.netflix.com",
        "Movies and series",
    ),
    app(
        "disney-plus",
        "Disney+",
        "https://www.disneyplus.com",
        "Disney, Pixar, Marvel, Star Wars",
    ),
    app(
        "hulu",
        "Hulu",
        "https://www.hulu.com",
        "Series, movies and live TV",
    ),
    app(
        "max",
        "HBO Max",
        "https://www.hbomax.com",
        "HBO series and movies",
    ),
    app(
        "peacock",
        "Peacock",
        "https://www.peacocktv.com",
        "Series, movies and live sports",
    ),
    app(
        "paramount-plus",
        "Paramount+",
        "https://www.paramountplus.com",
        "Series, movies and live TV",
    ),
    app(
        "prime-video",
        "Prime Video",
        "https://www.primevideo.com",
        "Amazon's movies and series",
    ),
    app(
        "apple-tv",
        "Apple TV",
        "https://tv.apple.com",
        "Apple TV+, full screen",
    ),
    App {
        tv_user_agent: true,
        ..app(
            "youtube",
            "YouTube",
            "https://www.youtube.com/tv",
            "YouTube TV interface, full screen",
        )
    },
    app(
        "youtube-tv",
        "YouTube TV",
        "https://tv.youtube.com",
        "Live TV and DVR, full screen",
    ),
    app(
        "crunchyroll",
        "Crunchyroll",
        "https://www.crunchyroll.com",
        "Anime",
    ),
];

/// The service with this id, if the catalog has one. A saved id that no longer matches
/// is the caller's to ignore.
pub fn find(id: &str) -> Option<&'static App> {
    APPS.iter().find(|app| app.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_and_names_are_unique() {
        let ids: HashSet<_> = APPS.iter().map(|app| app.id).collect();
        assert_eq!(ids.len(), APPS.len());
        let names: HashSet<_> = APPS.iter().map(|app| app.name).collect();
        assert_eq!(names.len(), APPS.len());
    }

    #[test]
    fn ids_are_safe_in_a_path_and_an_icon_name() {
        for app in APPS {
            assert!(
                !app.id.is_empty()
                    && app
                        .id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{}",
                app.id
            );
        }
    }

    #[test]
    fn urls_are_https() {
        for app in APPS {
            assert!(app.url.starts_with("https://"), "{}", app.id);
        }
    }

    #[test]
    fn finds_a_service_by_id() {
        assert_eq!(find("netflix").map(|app| app.name), Some("Netflix"));
        assert_eq!(find("no-such-service"), None);
    }

    #[test]
    fn only_youtube_asks_for_the_tv_user_agent() {
        let tv: Vec<_> = APPS
            .iter()
            .filter(|a| a.tv_user_agent)
            .map(|a| a.id)
            .collect();
        assert_eq!(tv, ["youtube"]);
    }

    // The package installs one logo per service, named after the icon the launcher asks
    // for, so a service added here without one would show the placeholder when installed.
    #[test]
    fn the_package_installs_a_logo_for_every_service() {
        let package = include_str!("../nix/package.nix");
        for app in APPS {
            assert!(
                package.contains(&format!("\"{}\"", app.id)),
                "nix/package.nix lacks a logo for {}",
                app.id
            );
        }
    }
}
