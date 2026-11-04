//! Opening a catalog service as a web app: a kiosk browser window with a profile of its
//! own, so logins persist per service and a still-running instance can never swallow a
//! different user agent. It is the logic of vexos-nix's `mkWebApp`, done here so the
//! catalog does not depend on the configuration.
//!
//! The browser should be Brave: Chromium ships without Widevine, so DRM services do not
//! play in it.

use crate::catalog::App;
use crate::launch::shell_quote;
use std::path::{Path, PathBuf};

/// The user agent of a smart TV, which makes youtube.com/tv serve its TV interface.
const SMART_TV_USER_AGENT: &str = "Mozilla/5.0 (Linux; Tizen 2.3; SmartHub; SMART-TV; SmartTV; U; Maple2012) AppleWebKit/538.1+ (KHTML, like Gecko) TV Safari/538.1+";

/// Browsers looked for on `PATH`, best first, when the configuration names none.
const BROWSERS: [&str; 5] = [
    "brave-origin",
    "brave",
    "brave-browser",
    "google-chrome-stable",
    "chromium",
];

/// Brave plays DRM video only once it has opted in to Widevine; a fresh profile is
/// seeded with that so a service works on its first launch.
const LOCAL_STATE: &str = "{\"brave\":{\"widevine_opted_in\":true}}\n";

/// Where a service keeps its browser profile.
pub fn profile_dir(app: &App) -> PathBuf {
    glib::user_data_dir()
        .join("vextheater")
        .join("webapps")
        .join(app.id)
}

/// The browser to run: the configured one, else the first known one on `PATH`. None if
/// it is not there, so the tile can say so.
pub fn find_browser(configured: Option<&str>) -> Option<String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    find_browser_in(configured, &dirs)
}

fn find_browser_in(configured: Option<&str>, dirs: &[PathBuf]) -> Option<String> {
    match configured.filter(|browser| !browser.is_empty()) {
        Some(browser) => is_runnable(browser, dirs).then(|| browser.to_owned()),
        None => BROWSERS
            .into_iter()
            .find(|browser| is_runnable(browser, dirs))
            .map(str::to_owned),
    }
}

/// A path is looked at as it is; a bare program name in each of `dirs`, as a shell would.
fn is_runnable(program: &str, dirs: &[PathBuf]) -> bool {
    if program.contains('/') {
        return is_executable(Path::new(program));
    }
    dirs.iter().any(|dir| is_executable(&dir.join(program)))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// The browser's arguments for a service.
fn arguments(app: &App, profile: &Path) -> Vec<String> {
    let mut arguments = vec![
        "--kiosk".to_owned(),
        format!("--app={}", app.url),
        format!("--user-data-dir={}", profile.display()),
    ];
    if app.tv_user_agent {
        arguments.push(format!("--user-agent={SMART_TV_USER_AGENT}"));
    }
    arguments
}

/// The shell line that runs `browser` for the service: every word quoted, so nothing in
/// a path or a URL is read by the shell.
fn command_line(browser: &str, app: &App, profile: &Path) -> String {
    let words = std::iter::once(browser.to_owned()).chain(arguments(app, profile));
    let quoted: Vec<String> = words.map(|word| shell_quote(&word)).collect();
    format!("exec {}", quoted.join(" "))
}

/// The host a `SingletonLock` names, from its link target. Chromium makes it a symlink
/// to `<host>-<pid>`.
fn lock_host(target: &str) -> &str {
    target.rsplit_once('-').map_or(target, |(host, _)| host)
}

/// Whether the lock was left by another host. Brave exits with no window when it finds
/// such a lock, which is what a hostname change after an unclean exit leaves behind.
fn lock_is_stale(target: &str, hostname: &str) -> bool {
    let host = lock_host(target);
    !host.is_empty() && host != hostname
}

/// This machine's name, as `hostname` prints it.
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .unwrap_or_default()
}

/// Gets the profile directory ready: it exists, a lock from another host is gone, and a
/// new profile has the Widevine opt-in.
fn prepare_in(profile: &Path, hostname: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(profile)?;
    let lock = profile.join("SingletonLock");
    if let Ok(target) = std::fs::read_link(&lock) {
        if lock_is_stale(&target.to_string_lossy(), hostname) {
            for name in ["SingletonLock", "SingletonCookie", "SingletonSocket"] {
                crate::files::remove(&profile.join(name))?;
            }
        }
    }
    let state = profile.join("Local State");
    if !state.exists() {
        std::fs::write(state, LOCAL_STATE)?;
    }
    Ok(())
}

/// Prepares the service's profile and returns the shell line that opens it in `browser`.
pub fn prepare(app: &App, browser: &str) -> std::io::Result<String> {
    let profile = profile_dir(app);
    prepare_in(&profile, &hostname())?;
    Ok(command_line(browser, app, &profile))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;
    use std::os::unix::fs::{symlink, PermissionsExt};

    /// A scratch directory of its own.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vextheater-webapp-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn program(dir: &Path, name: &str, mode: u32) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    fn app(id: &str) -> &'static App {
        catalog::find(id).unwrap()
    }

    #[test]
    fn the_profile_is_per_service() {
        let netflix = profile_dir(app("netflix"));
        assert!(netflix.ends_with("vextheater/webapps/netflix"));
        assert_ne!(netflix, profile_dir(app("hulu")));
    }

    #[test]
    fn arguments_open_the_url_in_a_kiosk_window_with_its_own_profile() {
        assert_eq!(
            arguments(app("netflix"), Path::new("/p/netflix")),
            [
                "--kiosk",
                "--app=https://www.netflix.com",
                "--user-data-dir=/p/netflix"
            ]
        );
    }

    #[test]
    fn only_a_tv_service_gets_the_tv_user_agent() {
        let youtube = arguments(app("youtube"), Path::new("/p"));
        assert_eq!(youtube.len(), 4);
        assert_eq!(youtube[3], format!("--user-agent={SMART_TV_USER_AGENT}"));
        assert!(youtube[3].contains("Tizen"));
        assert!(!arguments(app("hulu"), Path::new("/p"))
            .iter()
            .any(|argument| argument.starts_with("--user-agent")));
    }

    #[test]
    fn the_command_line_quotes_every_word() {
        let line = command_line(
            "/opt/my brave/brave",
            app("youtube"),
            Path::new("/home/o'neil/p"),
        );
        assert!(line.starts_with(
            "exec '/opt/my brave/brave' '--kiosk' '--app=https://www.youtube.com/tv' "
        ));
        assert!(line.contains(r"'--user-data-dir=/home/o'\''neil/p'"));
        assert!(line.ends_with('\''));
    }

    // The shell runs the line with the same words the argument list holds.
    #[test]
    fn the_shell_reads_the_line_back_as_the_arguments() {
        let profile = Path::new("/tmp/a b/$HOME;x");
        let line = command_line("printf", app("youtube"), profile)
            .replace("exec 'printf'", "exec printf '[%s]\\n'");
        let out = std::process::Command::new("sh")
            .args(["-c", &line])
            .output()
            .unwrap();
        let expected: String = arguments(app("youtube"), profile)
            .iter()
            .map(|argument| format!("[{argument}]\n"))
            .collect();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), expected);
    }

    #[test]
    fn the_lock_host_is_what_comes_before_the_last_dash() {
        assert_eq!(lock_host("htpc-4242"), "htpc");
        assert_eq!(lock_host("my-box-4242"), "my-box");
        assert_eq!(lock_host("htpc"), "htpc");
        assert_eq!(lock_host(""), "");
    }

    #[test]
    fn only_a_lock_from_another_host_is_stale() {
        assert!(lock_is_stale("old-4242", "new"));
        assert!(!lock_is_stale("new-4242", "new"));
        assert!(!lock_is_stale("my-box-4242", "my-box"));
        // Nothing named: nothing to remove.
        assert!(!lock_is_stale("", "new"));
    }

    #[test]
    fn a_new_profile_is_created_with_the_widevine_opt_in() {
        let dir = scratch("new").join("webapps/netflix");
        prepare_in(&dir, "htpc").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("Local State")).unwrap(),
            LOCAL_STATE
        );
        let state: serde_json::Value = serde_json::from_str(LOCAL_STATE).unwrap();
        assert_eq!(state["brave"]["widevine_opted_in"], true);
    }

    #[test]
    fn an_existing_local_state_is_left_alone() {
        let dir = scratch("keep");
        std::fs::write(dir.join("Local State"), "{\"mine\":1}").unwrap();
        prepare_in(&dir, "htpc").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("Local State")).unwrap(),
            "{\"mine\":1}"
        );
    }

    #[test]
    fn a_lock_from_another_host_is_removed_with_its_companions() {
        let dir = scratch("stale");
        symlink("oldhost-99", dir.join("SingletonLock")).unwrap();
        symlink("x", dir.join("SingletonCookie")).unwrap();
        symlink("y", dir.join("SingletonSocket")).unwrap();
        prepare_in(&dir, "htpc").unwrap();
        for name in ["SingletonLock", "SingletonCookie", "SingletonSocket"] {
            assert!(std::fs::symlink_metadata(dir.join(name)).is_err(), "{name}");
        }
    }

    #[test]
    fn a_lock_from_this_host_is_kept() {
        let dir = scratch("live");
        symlink("htpc-99", dir.join("SingletonLock")).unwrap();
        symlink("x", dir.join("SingletonCookie")).unwrap();
        prepare_in(&dir, "htpc").unwrap();
        assert!(std::fs::symlink_metadata(dir.join("SingletonLock")).is_ok());
        assert!(std::fs::symlink_metadata(dir.join("SingletonCookie")).is_ok());
    }

    #[test]
    fn the_configured_browser_is_used_if_it_is_there() {
        let dir = scratch("configured");
        let brave = program(&dir, "brave", 0o755);
        let dirs = [dir.clone()];
        let path = brave.to_string_lossy().into_owned();
        assert_eq!(find_browser_in(Some(&path), &dirs), Some(path.clone()));
        assert_eq!(find_browser_in(Some("brave"), &dirs), Some("brave".into()));
        assert_eq!(find_browser_in(Some("/no/such/brave"), &dirs), None);
        // A configured browser that is missing is not swapped for another.
        assert_eq!(find_browser_in(Some("firefox"), &dirs), None);
    }

    #[test]
    fn without_one_the_first_known_browser_on_the_path_is_used() {
        let first = scratch("path-a");
        let second = scratch("path-b");
        program(&second, "chromium", 0o755);
        program(&second, "brave", 0o755);
        program(&first, "google-chrome-stable", 0o755);
        let dirs = [first, second];
        assert_eq!(find_browser_in(None, &dirs), Some("brave".into()));
        assert_eq!(find_browser_in(Some(""), &dirs), Some("brave".into()));
        assert_eq!(find_browser_in(None, &[]), None);
    }

    #[test]
    fn a_file_that_is_not_executable_is_not_a_browser() {
        let dir = scratch("plain");
        program(&dir, "brave", 0o644);
        assert_eq!(find_browser_in(None, &[dir]), None);
    }
}
