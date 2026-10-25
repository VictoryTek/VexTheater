//! `vextheater --diagnose`: what the running launcher needs from its session, checked in
//! the launcher's own environment. A service started by systemd does not have the
//! environment of a login shell, so a problem such as GNOME's schemas not being found
//! shows here and not when the same command is tried in a terminal.

use crate::config::{Action, Config, Tile};
use std::path::Path;
use std::time::{Duration, Instant};

/// How long a helper command gets.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// One thing checked: what, and either what was found or why it failed.
#[derive(Debug, PartialEq)]
struct Check {
    what: String,
    outcome: Result<String, String>,
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

/// The checks, as a report and whether they all passed. `config` is None when the
/// configuration could not be loaded, which is itself reported.
pub fn run(config: Option<&Config>) -> (String, bool) {
    let mut checks = Vec::new();
    checks.extend(schemas());
    checks.push(data_dirs(
        std::env::var("XDG_DATA_DIRS").ok().as_deref(),
        |dir| dir.join("glib-2.0/schemas/gschemas.compiled").is_file(),
    ));
    checks.push(command(
        "systemd-run --user",
        &[
            "systemd-run",
            "--user",
            "--scope",
            "--quiet",
            "--collect",
            "--",
            "true",
        ],
    ));
    checks.push(command(
        "systemctl --user",
        &[
            "systemctl",
            "--user",
            "list-units",
            "--type=scope",
            "--no-legend",
            "--plain",
        ],
    ));
    match config {
        Some(config) => {
            checks.push(Check::ok(
                "configuration",
                format!(
                    "{} tiles, {} submenus",
                    config.tiles.len(),
                    config.submenus.len()
                ),
            ));
            checks.extend(flatpak(config, |name| {
                glib::find_program_in_path(name).is_some()
            }));
            checks.extend(files(config, |path| path.is_file()));
        }
        None => checks.push(Check::failed(
            "configuration",
            "it cannot be loaded; see the log, or run --check-config",
        )),
    }
    render(&checks)
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
fn command(what: &str, argv: &[&str]) -> Check {
    match run_command(argv) {
        Ok(()) => Check::ok(what, "works"),
        Err(why) => Check::failed(what, why),
    }
}

fn run_command(argv: &[&str]) -> Result<(), String> {
    let mut child = std::process::Command::new(argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot run {}: {error}", argv[0]))?;
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{} failed: {status}", argv[0])),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{} did not finish in {:?}",
                    argv[0], COMMAND_TIMEOUT
                ));
            }
            Err(error) => return Err(format!("cannot wait for {}: {error}", argv[0])),
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
fn render(checks: &[Check]) -> (String, bool) {
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

    // A helper that works passes; one that fails or is not there does not.
    #[test]
    fn a_helper_command_must_run_and_succeed() {
        assert!(command("true", &["true"]).outcome.is_ok());
        assert!(command("false", &["false"]).outcome.is_err());
        let missing = command("nothing", &["/no/such/program-at-all"]);
        assert!(missing.outcome.unwrap_err().contains("cannot run"));
    }
}
