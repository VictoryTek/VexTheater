//! `vextheater --diagnose`: what the running launcher needs from its session, checked in
//! the launcher's own environment. A service started by systemd does not have the
//! environment of a login shell, so a problem such as GNOME's schemas not being found
//! shows here and not when the same command is tried in a terminal.

use crate::config::{Action, Config, Tile};
use crate::{keybinding, launch, webapp};
use std::ffi::OsStr;
use std::future::Future;
use std::path::Path;
use std::time::Duration;

/// How long a helper command gets.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a helper that was killed gets to be reaped.
const KILL_GRACE: Duration = Duration::from_secs(1);

/// One thing checked: what, and either what was found or why it failed.
#[derive(Debug, PartialEq)]
pub struct Check {
    pub what: String,
    pub outcome: Result<String, String>,
}

impl Check {
    fn ok(what: impl Into<String>, found: impl Into<String>) -> Check {
        Check {
            what: what.into(),
            outcome: Ok(found.into()),
        }
    }

    fn failed(what: impl Into<String>, why: impl Into<String>) -> Check {
        Check {
            what: what.into(),
            outcome: Err(why.into()),
        }
    }
}

/// All the checks. `config` is None when the configuration could not be loaded, which is
/// itself reported; `web_services` is whether a web service of the catalog is on the
/// launcher, which is what needs a browser.
///
/// Everything that can be read at once is read here, before the future is returned, so
/// the future holds no borrow of the configuration while the helper commands run. They
/// run without blocking the main loop, so the window stays usable meanwhile.
pub fn checks(config: Option<&Config>, web_services: bool) -> impl Future<Output = Vec<Check>> {
    let mut first = schemas();
    first.push(data_dirs(
        std::env::var("XDG_DATA_DIRS").ok().as_deref(),
        |dir| dir.join("glib-2.0/schemas/gschemas.compiled").is_file(),
    ));
    let rest = about_the_configuration(config, web_services);
    async move {
        let mut checks = first;
        checks.push(command("systemd-run --user", &SYSTEMD_RUN).await);
        checks.push(command("systemctl --user", &SYSTEMCTL).await);
        checks.extend(rest);
        checks
    }
}

const SYSTEMD_RUN: [&str; 7] = [
    "systemd-run",
    "--user",
    "--scope",
    "--quiet",
    "--collect",
    "--",
    "true",
];
const SYSTEMCTL: [&str; 6] = [
    "systemctl",
    "--user",
    "list-units",
    "--type=scope",
    "--no-legend",
    "--plain",
];

/// The checks that need the configuration.
fn about_the_configuration(config: Option<&Config>, web_services: bool) -> Vec<Check> {
    let Some(config) = config else {
        return vec![Check::failed(
            "configuration",
            "it cannot be loaded; see the log, or run --check-config",
        )];
    };
    let mut checks = vec![Check::ok(
        "configuration",
        format!(
            "{} tiles, {} submenus",
            config.tiles.len(),
            config.submenus.len()
        ),
    )];
    checks.extend(browser(
        config.browser.as_deref(),
        web_services,
        webapp::find_browser,
    ));
    checks.extend(
        keybinding::status(config.home_key.as_deref()).map(|outcome| Check {
            what: "home key shortcut".to_owned(),
            outcome,
        }),
    );
    checks.extend(flatpak(config, |name| {
        glib::find_program_in_path(name).is_some()
    }));
    checks.extend(apps(config, launch::is_installed));
    checks.extend(files(config, |path| path.is_file()));
    checks
}

/// The GNOME settings schemas the launcher uses, each found or not.
fn schemas() -> Vec<Check> {
    let source = gio::SettingsSchemaSource::default();
    let mut names: Vec<&str> = crate::gnome::SCHEMAS.to_vec();
    names.extend(crate::keybinding::SCHEMAS);
    names.push(crate::sound::SCHEMA);
    names
        .into_iter()
        .map(|name| {
            let what = format!("GNOME schema {name}");
            match &source {
                Some(source) if source.lookup(name, true).is_some() => Check::ok(what, "found"),
                _ => Check::failed(what, "not found; is it in XDG_DATA_DIRS?"),
            }
        })
        .collect()
}

/// `XDG_DATA_DIRS`, where GLib looks for the schemas: set, and with compiled schemas in
/// some directory of it (`has_schemas` says whether a directory has them).
fn data_dirs(value: Option<&str>, has_schemas: impl Fn(&Path) -> bool) -> Check {
    let what = "XDG_DATA_DIRS";
    let dirs: Vec<&str> = value
        .unwrap_or_default()
        .split(':')
        .filter(|dir| !dir.is_empty())
        .collect();
    if dirs.is_empty() {
        return Check::failed(what, "unset or empty, so GNOME's settings cannot be found");
    }
    let with_schemas = dirs
        .iter()
        .filter(|dir| has_schemas(Path::new(dir)))
        .count();
    if with_schemas == 0 {
        return Check::failed(
            what,
            format!(
                "{} directories, none with compiled GNOME schemas",
                dirs.len()
            ),
        );
    }
    Check::ok(
        what,
        format!(
            "{} directories, {with_schemas} with compiled schemas",
            dirs.len()
        ),
    )
}

/// A helper command that must work: it runs and succeeds within [`COMMAND_TIMEOUT`].
async fn command(what: &str, argv: &[&str]) -> Check {
    match run_command(argv, COMMAND_TIMEOUT).await {
        Ok(()) => Check::ok(what, "works"),
        Err(why) => Check::failed(what, why),
    }
}

/// Runs a command to the end, or for `timeout`; a command still running then is killed
/// and waited for, so no helper is left behind. Waits on the main loop, never blocks it.
async fn run_command(argv: &[&str], timeout: Duration) -> Result<(), String> {
    let argv_os: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    // Without a flag for it, stdin is /dev/null.
    let flags = gio::SubprocessFlags::STDOUT_SILENCE | gio::SubprocessFlags::STDERR_SILENCE;
    let process = gio::Subprocess::newv(&argv_os, flags)
        .map_err(|error| format!("cannot run {}: {error}", argv[0]))?;
    match glib::future_with_timeout(timeout, process.wait_future()).await {
        Ok(Ok(())) if process.is_successful() => Ok(()),
        Ok(Ok(())) if process.has_exited() => Err(format!(
            "{} failed with exit status {}",
            argv[0],
            process.exit_status()
        )),
        Ok(Ok(())) => Err(format!("{} was stopped by a signal", argv[0])),
        Ok(Err(error)) => Err(format!("cannot wait for {}: {error}", argv[0])),
        Err(_) => {
            process.force_exit();
            // Reaped here, within a grace of its own; GLib reaps it anyway should it
            // take longer.
            let _ = glib::future_with_timeout(KILL_GRACE, process.wait_future()).await;
            Err(format!("{} did not finish in {timeout:?}", argv[0]))
        }
    }
}

fn all_tiles(config: &Config) -> impl Iterator<Item = &Tile> {
    config
        .tiles
        .iter()
        .chain(config.submenus.values().flatten())
}

/// `flatpak` on the `PATH`, if any tile starts a Flatpak app (`on_path` says whether a
/// program is there).
fn flatpak(config: &Config, on_path: impl Fn(&str) -> bool) -> Option<Check> {
    let uses = all_tiles(config).any(|tile| matches!(tile.action, Action::Flatpak(_)));
    uses.then(|| {
        if on_path("flatpak") {
            Check::ok("flatpak", "on the PATH")
        } else {
            Check::failed(
                "flatpak",
                "a tile starts a Flatpak app, but flatpak is not on the PATH",
            )
        }
    })
}

/// The browser that opens the web services (`find` looks for one, given the configured
/// one). Without a service on the launcher (`web_services`) an unset browser is not a
/// problem, so nothing is reported; a browser that was configured must be there anyway.
fn browser(
    configured: Option<&str>,
    web_services: bool,
    find: impl Fn(Option<&str>) -> Option<String>,
) -> Option<Check> {
    let what = "browser";
    match configured.filter(|browser| !browser.is_empty()) {
        Some(name) => Some(match find(Some(name)) {
            Some(found) => Check::ok(what, found),
            None => Check::failed(
                what,
                format!(
                    "\"{name}\" is set as the browser, but it is not an executable file or \
                     a program on the PATH"
                ),
            ),
        }),
        None => match find(None) {
            Some(found) => Some(Check::ok(what, found)),
            None if web_services => Some(Check::failed(
                what,
                format!(
                    "a web service is on the launcher, but none of {} is on the PATH",
                    webapp::BROWSERS.join(", ")
                ),
            )),
            None => None,
        },
    }
}

/// The apps tiles start by desktop file or Flatpak id, each found or missing (`installed`
/// says whether one is). A command is not looked for: it is what the user typed.
fn apps(config: &Config, installed: impl Fn(&Action) -> bool) -> Vec<Check> {
    let looked_for: Vec<&Tile> = all_tiles(config)
        .filter(|tile| matches!(tile.action, Action::Desktop(_) | Action::Flatpak(_)))
        .collect();
    let missing: Vec<Check> = looked_for
        .iter()
        .filter(|tile| !installed(&tile.action))
        .map(|tile| {
            let why = match &tile.action {
                Action::Desktop(id) => format!("no desktop file {id}; is the app installed?"),
                Action::Flatpak(id) => format!("the Flatpak app {id} is not installed"),
                _ => unreachable!("only desktop and Flatpak tiles were looked for"),
            };
            Check::failed(format!("app \"{}\"", tile.name), why)
        })
        .collect();
    if !missing.is_empty() {
        return missing;
    }
    if looked_for.is_empty() {
        return Vec::new();
    }
    vec![Check::ok(
        "apps",
        format!("all {} installed", looked_for.len()),
    )]
}

/// The background, and the icons given as a file, exist (`exists` says whether a file
/// does).
fn files(config: &Config, exists: impl Fn(&Path) -> bool) -> Vec<Check> {
    let mut checks = Vec::new();
    if let Some(background) = &config.background {
        let what = "background";
        checks.push(if exists(background) {
            Check::ok(what, background.display().to_string())
        } else {
            Check::failed(what, format!("{} does not exist", background.display()))
        });
    }
    for tile in all_tiles(config) {
        let Some(icon) = tile.icon.as_deref().filter(|icon| icon.starts_with('/')) else {
            continue;
        };
        let what = format!("icon of \"{}\"", tile.name);
        checks.push(if exists(Path::new(icon)) {
            Check::ok(what, icon)
        } else {
            Check::failed(what, format!("{icon} does not exist"))
        });
    }
    checks
}

/// One line per check, then a summary; true if every check passed.
pub fn render(checks: &[Check]) -> (String, bool) {
    let mut report = String::new();
    let mut failed = 0;
    for check in checks {
        match &check.outcome {
            Ok(found) => report.push_str(&format!("ok    {}: {found}\n", check.what)),
            Err(why) => {
                failed += 1;
                report.push_str(&format!("FAIL  {}: {why}\n", check.what));
            }
        }
    }
    report.push_str(&format!("{failed} failed of {} checks\n", checks.len()));
    (report, failed == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Config {
        Config::parse(text).unwrap()
    }

    #[test]
    fn the_report_has_a_line_per_check_and_a_summary() {
        let checks = [
            Check::ok("one", "fine"),
            Check::failed("two", "broken"),
            Check::ok("three", "fine"),
        ];
        let (report, all_ok) = render(&checks);
        assert_eq!(
            report,
            "ok    one: fine\nFAIL  two: broken\nok    three: fine\n1 failed of 3 checks\n"
        );
        assert!(!all_ok);
        assert!(render(&[Check::ok("a", "b")]).1);
        assert!(render(&[]).1, "no checks, nothing failed");
    }

    #[test]
    fn xdg_data_dirs_must_be_set_and_hold_schemas() {
        let schemas_in = |wanted: &'static [&'static str]| {
            move |dir: &Path| wanted.contains(&dir.to_str().unwrap())
        };
        assert!(data_dirs(None, |_| true).outcome.is_err());
        assert!(data_dirs(Some(""), |_| true).outcome.is_err());
        assert!(data_dirs(Some(":::"), |_| true).outcome.is_err());
        assert!(data_dirs(Some("/a:/b"), |_| false).outcome.is_err());
        assert_eq!(
            data_dirs(Some("/a:/b:/c"), schemas_in(&["/b", "/c"])).outcome,
            Ok("3 directories, 2 with compiled schemas".to_owned())
        );
    }

    #[test]
    fn flatpak_is_checked_only_when_a_tile_uses_it() {
        let without = config(r#"{ "tiles": [ { "name": "A", "icon": "a", "command": "a" } ] }"#);
        assert_eq!(flatpak(&without, |_| false), None);

        let with = config(
            r#"{ "tiles": [ { "name": "S", "icon": "s", "submenu": "Apps" } ],
                 "submenus": { "Apps": [ { "name": "F", "flatpak": "org.example.App" } ] } }"#,
        );
        assert_eq!(
            flatpak(&with, |_| true).unwrap().outcome,
            Ok("on the PATH".to_owned())
        );
        assert!(flatpak(&with, |_| false).unwrap().outcome.is_err());
    }

    #[test]
    fn the_background_and_icon_files_are_looked_for() {
        let config = config(
            r#"{ "background": "/missing/wall.jpg",
                 "tiles": [
                    { "name": "Here", "icon": "/there/icon.svg", "command": "a" },
                    { "name": "Gone", "icon": "/gone/icon.svg", "command": "b" },
                    { "name": "Themed", "icon": "folder", "command": "c" } ] }"#,
        );
        let checks = files(&config, |path| path == Path::new("/there/icon.svg"));
        let (report, all_ok) = render(&checks);
        assert_eq!(
            report,
            "FAIL  background: /missing/wall.jpg does not exist\n\
             ok    icon of \"Here\": /there/icon.svg\n\
             FAIL  icon of \"Gone\": /gone/icon.svg does not exist\n\
             2 failed of 3 checks\n"
        );
        assert!(!all_ok);
    }

    /// Runs `future` on a main context of its own, so tests can run in parallel.
    fn block_on<F: Future>(future: F) -> F::Output {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| context.block_on(future))
            .unwrap()
    }

    // A helper that works passes; one that fails or is not there does not.
    #[test]
    fn a_helper_command_must_run_and_succeed() {
        assert!(block_on(command("true", &["true"])).outcome.is_ok());
        let failed = block_on(command("false", &["false"]));
        assert!(failed.outcome.unwrap_err().contains("exit status 1"));
        let missing = block_on(command("nothing", &["/no/such/program-at-all"]));
        assert!(missing.outcome.unwrap_err().contains("cannot run"));
    }

    // A helper that hangs is stopped at the time limit and reaped, not waited for, and
    // the main loop keeps running while it hangs.
    #[test]
    fn a_hanging_helper_is_killed_at_the_limit_while_the_loop_keeps_running() {
        let started = std::time::Instant::now();
        let ticks = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let result = block_on(async {
            let counter = ticks.clone();
            glib::spawn_future_local(async move {
                for _ in 0..1000 {
                    glib::timeout_future(Duration::from_millis(10)).await;
                    counter.set(counter.get() + 1);
                }
            });
            run_command(&["sleep", "30"], Duration::from_millis(300)).await
        });
        let why = result.unwrap_err();
        assert!(why.contains("did not finish"), "{why}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "it was waited for"
        );
        assert!(
            ticks.get() >= 5,
            "the loop ran meanwhile: {} ticks",
            ticks.get()
        );
    }

    #[test]
    fn a_browser_problem_names_the_cause() {
        let found = |wanted: Option<&str>| wanted.map(str::to_owned);
        let nothing = |_: Option<&str>| None;
        // Configured and there; configured and not; none configured.
        assert_eq!(
            browser(Some("brave"), false, found).unwrap().outcome,
            Ok("brave".to_owned())
        );
        let gone = browser(Some("brave"), false, nothing).unwrap();
        assert!(gone.outcome.unwrap_err().contains("is set as the browser"));
        let none = browser(None, true, nothing).unwrap();
        assert!(none.outcome.unwrap_err().contains("web service"));
        // Without a web service an unset browser is nothing to report.
        assert_eq!(browser(None, false, nothing), None);
        assert_eq!(browser(Some(""), false, nothing), None);
        // Found by looking, with no web service, is still worth saying.
        assert!(browser(None, false, |_| Some("brave".into())).is_some());
    }

    #[test]
    fn missing_apps_are_named_each_and_present_ones_are_counted() {
        let config = config(
            r#"{ "tiles": [
                    { "name": "Files", "desktop": "org.gnome.Nautilus.desktop" },
                    { "name": "Games", "icon": "g", "submenu": "More" },
                    { "name": "Typed", "icon": "t", "command": "typed" } ],
                 "submenus": { "More": [
                    { "name": "Ghost", "flatpak": "org.example.Ghost" } ] } }"#,
        );
        let checks = apps(&config, |_| true);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].outcome, Ok("all 2 installed".to_owned()));

        let checks = apps(&config, |action| {
            *action != Action::Flatpak("org.example.Ghost".into())
        });
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].what, "app \"Ghost\"");
        assert!(checks[0]
            .outcome
            .as_ref()
            .unwrap_err()
            .contains("org.example.Ghost"));

        // A command is not looked for; no desktop or Flatpak tile, no check.
        let typed = self::config(
            r#"{ "tiles": [ { "name": "Typed", "icon": "t", "command": "typed" } ] }"#,
        );
        assert!(apps(&typed, |_| false).is_empty());
    }
}
