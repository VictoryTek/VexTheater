//! Starting a tile's app in its own systemd scope, and stopping it again.
//!
//! The scope holds every process the app starts (Brave's renderers, a wrapper's
//! children), so stopping it closes the whole app. Flatpak moves the sandboxed app
//! into a scope of its own, so a Flatpak app is also closed with `flatpak kill`; its
//! id is kept in a runtime file so a launcher restarted after a crash can still do so.

use crate::config::Action;
use anyhow::{bail, Context};
use gio::prelude::*;
use std::cell::Cell;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Matches the scope of every app any launcher instance started.
const ALL_SCOPES: &str = "vextheater-app-*.scope";
/// How long a closing app gets before it is killed (scopes) or given up on (Flatpak);
/// the scopes and a Flatpak app each get their own.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// How long killed scopes get to disappear.
const KILL_GRACE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(200);

/// An app started from a tile.
pub struct Running {
    pub unit: String,
    pub process: gio::Subprocess,
    pub started: Instant,
}

thread_local! {
    static NEXT_UNIT: Cell<u32> = const { Cell::new(0) };
}

/// Starts the app behind a `command`, `desktop` or `flatpak` action.
pub fn start(action: &Action) -> anyhow::Result<Running> {
    let (command, flatpak) = match action {
        Action::Command(command) => (command.clone(), None),
        Action::Flatpak(id) => (
            format!("exec flatpak run {}", shell_quote(id)),
            Some(id.clone()),
        ),
        Action::Desktop(id) => {
            let info = gio::DesktopAppInfo::new(id)
                .with_context(|| format!("no installed application has the id {id}"))?;
            let exec = info
                .commandline()
                .with_context(|| format!("{id} has no Exec line"))?;
            let command = format!("exec {}", strip_field_codes(&exec.to_string_lossy()));
            (command, info.string("X-Flatpak").map(String::from))
        }
        Action::Submenu(_) | Action::Builtin(_) => bail!("not an app: {action:?}"),
    };

    let unit = NEXT_UNIT.with(|next| {
        let n = next.get();
        next.set(n + 1);
        format!("vextheater-app-{}-{n}", std::process::id())
    });
    let argv = scope_argv(&unit, &command);
    let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    let process = gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE)
        .with_context(|| format!("cannot start {command}"))?;

    match flatpak {
        Some(id) => {
            if let Err(error) = std::fs::create_dir_all(runtime_dir())
                .and_then(|()| std::fs::write(flatpak_file(), id))
            {
                log::warn!("cannot record the Flatpak app id: {error}");
            }
        }
        None => {
            let _ = std::fs::remove_file(flatpak_file());
        }
    }

    Ok(Running {
        unit,
        process,
        started: Instant::now(),
    })
}

/// Why the previous app could not be confirmed closed.
#[derive(Debug, PartialEq)]
pub enum StopError {
    /// Still running at the time limit; the next app starts anyway.
    Timeout,
    /// The scopes could not be listed, so whether an app is still running is unknown.
    /// The next app is not started, rather than risk starting it over a live one.
    Unknown,
}

/// Closes every app started from a tile, by this launcher or by one that crashed, and
/// returns once they are gone. Starting the next app only then means it cannot hand
/// its window to the closing one, as a second Brave would to a Brave that is still
/// shutting down. The caller runs one of these at a time.
///
/// A Flatpak app and the scopes are closed side by side, each with its own
/// [`STOP_TIMEOUT`], so a Flatpak that never quits cannot use up the time the scopes
/// get to close normally before they are killed. The whole close takes at most about
/// [`STOP_TIMEOUT`] plus [`KILL_GRACE`].
pub async fn stop_all() -> Result<(), StopError> {
    // Only the scopes that exist now. The launcher starts nothing while a close is
    // running, so these are all the apps there are.
    let Some(scopes) = running_scopes().await else {
        log::error!("cannot list the app scopes; not starting anything over them");
        return Err(StopError::Unknown);
    };

    let flatpak = std::fs::read_to_string(flatpak_file())
        .ok()
        .map(|id| id.trim().to_owned());
    let flatpak_closed = flatpak
        .clone()
        .map(|id| glib::MainContext::default().spawn_local(async move { stop_flatpak(&id).await }));
    let mut closed = scopes.is_empty() || stop_scopes(&scopes).await;
    if let Some(flatpak_closed) = flatpak_closed {
        let flatpak_closed = flatpak_closed.await.unwrap_or(false);
        // Kept while the app may still run, so the next close tries again; start()
        // replaces or removes it when the next app starts.
        if flatpak_closed {
            let _ = std::fs::remove_file(flatpak_file());
        }
        closed &= flatpak_closed;
    }

    if closed {
        Ok(())
    } else {
        Err(StopError::Timeout)
    }
}

/// `flatpak kill`, then waits for a successful `flatpak ps` that no longer lists the
/// app. A failing `flatpak ps` proves nothing, so it is retried until the time limit.
async fn stop_flatpak(id: &str) -> bool {
    let deadline = Instant::now() + STOP_TIMEOUT;
    run(&["flatpak", "kill", id], deadline).await;
    loop {
        let listed = run(&["flatpak", "ps", "--columns=application"], deadline).await;
        if let Some(Output {
            stdout,
            success: true,
        }) = listed
        {
            if !lists_app(&stdout, id) {
                return true;
            }
        }
        if Instant::now() >= deadline {
            log::warn!("Flatpak app {id} still running after {STOP_TIMEOUT:?}");
            return false;
        }
        glib::timeout_future(POLL).await;
    }
}

/// `systemctl stop` returns once the scopes are gone. At the time limit the ones still
/// there are killed, by name, so nothing started since the snapshot is touched.
async fn stop_scopes(scopes: &[String]) -> bool {
    let deadline = Instant::now() + STOP_TIMEOUT;
    let names = scopes.iter().map(String::as_str);
    let stop: Vec<&str> = ["systemctl", "--user", "stop"]
        .into_iter()
        .chain(names.clone())
        .collect();
    // Exits non-zero when a scope vanished on its own, which is fine: it is gone.
    if run(&stop, deadline).await.is_some() {
        return true;
    }

    log::warn!("{scopes:?} still running after {STOP_TIMEOUT:?}; killing them");
    let grace = Instant::now() + KILL_GRACE;
    let kill: Vec<&str> = ["systemctl", "--user", "kill", "--signal=SIGKILL"]
        .into_iter()
        .chain(names)
        .collect();
    run(&kill, grace).await;
    loop {
        // A listing that fails says nothing; keep polling until the grace ends.
        if let Some(left) = running_scopes().await {
            if !scopes.iter().any(|scope| left.contains(scope)) {
                return true;
            }
        }
        if Instant::now() >= grace {
            log::warn!("{scopes:?} still there after SIGKILL");
            return false;
        }
        glib::timeout_future(POLL).await;
    }
}

/// Names of the loaded `vextheater-app-*` scopes; None if they could not be listed.
async fn running_scopes() -> Option<Vec<String>> {
    let list = [
        "systemctl",
        "--user",
        "list-units",
        "--type=scope",
        "--no-legend",
        "--plain",
        ALL_SCOPES,
    ];
    match run(&list, Instant::now() + KILL_GRACE).await {
        Some(Output {
            stdout,
            success: true,
        }) => Some(parse_scopes(&stdout)),
        _ => None,
    }
}

fn parse_scopes(listed: &str) -> Vec<String> {
    listed
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|unit| unit.starts_with("vextheater-app-") && unit.ends_with(".scope"))
        .map(String::from)
        .collect()
}

fn lists_app(listed: &str, id: &str) -> bool {
    listed.lines().any(|line| line.trim() == id)
}

/// A command that exited.
struct Output {
    stdout: String,
    success: bool,
}

/// Runs a command until it exits or `deadline` passes. None if it timed out or did not
/// start; a command still running at the deadline is left to finish on its own.
async fn run(argv: &[&str], deadline: Instant) -> Option<Output> {
    let argv_os: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    let flags = gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE;
    let process = gio::Subprocess::newv(&argv_os, flags)
        .inspect_err(|error| log::warn!("cannot run {argv:?}: {error}"))
        .ok()?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    match glib::future_with_timeout(remaining, process.communicate_utf8_future(None)).await {
        // communicate returns once the process has exited.
        Ok(Ok((stdout, _))) => Some(Output {
            stdout: stdout.map(String::from).unwrap_or_default(),
            success: process.is_successful(),
        }),
        Ok(Err(error)) => {
            log::warn!("{argv:?}: {error}");
            None
        }
        Err(_) => None,
    }
}

/// Runs a `systemctl` verb such as `poweroff` or `reboot`. Logind lets the user of the
/// active local session do this without a password; the caller waits on the process
/// to learn whether it was refused.
pub fn systemctl(verb: &str) -> anyhow::Result<gio::Subprocess> {
    gio::Subprocess::newv(
        &[OsStr::new("systemctl"), OsStr::new(verb)],
        gio::SubprocessFlags::NONE,
    )
    .with_context(|| format!("cannot run systemctl {verb}"))
}

fn runtime_dir() -> PathBuf {
    glib::user_runtime_dir().join("vextheater")
}

fn flatpak_file() -> PathBuf {
    runtime_dir().join("flatpak-app")
}

/// `systemd-run --scope` moves itself into the new scope and then execs the command,
/// so the subprocess the launcher waits on is the app's main process.
fn scope_argv(unit: &str, command: &str) -> Vec<String> {
    [
        "systemd-run",
        "--user",
        "--scope",
        "--quiet",
        "--collect",
        &format!("--unit={unit}"),
        "--",
        "sh",
        "-c",
        command,
    ]
    .map(String::from)
    .to_vec()
}

/// Drops the desktop-entry field codes (`%U`, `%f`, …) from an Exec line, since the
/// launcher never passes files or URLs, and turns `%%` back into `%`.
fn strip_field_codes(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
        } else if chars.next() == Some('%') {
            out.push('%');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_field_codes() {
        assert_eq!(strip_field_codes("firefox %u"), "firefox");
        assert_eq!(strip_field_codes("app %F --flag"), "app --flag");
        assert_eq!(strip_field_codes("printf 100%%"), "printf 100%");
        assert_eq!(
            strip_field_codes("/usr/bin/flatpak run --branch=stable io.example.App @@u %U @@"),
            "/usr/bin/flatpak run --branch=stable io.example.App @@u @@"
        );
    }

    #[test]
    fn wraps_the_command_in_a_named_scope() {
        assert_eq!(
            scope_argv("vextheater-app-1-0", "exec mpv"),
            [
                "systemd-run",
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "--unit=vextheater-app-1-0",
                "--",
                "sh",
                "-c",
                "exec mpv"
            ]
        );
    }

    #[test]
    fn parses_scope_names_from_list_units() {
        let listed = "vextheater-app-12-0.scope loaded active running sh -c brave\n\
                      vextheater-app-12-1.scope loaded deactivating stop-sigterm sh\n\
                      app-gnome-org.gnome.Nautilus-5.scope loaded active running\n\
                      \n";
        assert_eq!(
            parse_scopes(listed),
            ["vextheater-app-12-0.scope", "vextheater-app-12-1.scope"]
        );
        assert!(parse_scopes("").is_empty());
    }

    #[test]
    fn finds_the_app_in_flatpak_ps() {
        let listed = "com.brave.Browser\norg.test.App\n";
        assert!(lists_app(listed, "org.test.App"));
        assert!(!lists_app(listed, "org.test"));
        assert!(!lists_app("", "org.test.App"));
    }

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(shell_quote("io.example.App"), "'io.example.App'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
