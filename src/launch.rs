//! Starting a tile's app and stopping it again.
//!
//! An ordinary app runs in its own systemd scope, which holds every process it starts
//! (Brave's renderers, a wrapper's children), so stopping the scope closes the whole
//! app. A Flatpak app is started directly instead, because Flatpak moves it into a
//! scope of its own; the launcher records that instance (its process id and start
//! time) in a runtime file, and closes exactly that instance, never others of the same
//! app, also after a launcher crash.

use crate::config::Action;
use anyhow::{bail, Context};
use gio::prelude::*;
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Matches the scope of every app any launcher instance started.
const ALL_SCOPES: &str = "vextheater-app-*.scope";
/// How long an app gets after its SIGTERM before it is killed.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// How long killed apps get to disappear, and the limit on each helper command.
const KILL_GRACE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(200);
/// `flatpak kill` is tried this many times on one app before the launcher gives up on it.
const MAX_KILL_ATTEMPTS: u32 = 2;
/// How long [`check_early_exit`] waits for an app to show up in `flatpak ps`.
const EARLY_EXIT_WATCH: Duration = Duration::from_secs(3);

/// The timings, helper programs and record file the lifecycle uses. The default is the
/// real set; the tests give it millisecond timings and stub programs, so the whole
/// start and close lifecycle runs against real processes without Flatpak or systemd.
#[derive(Clone)]
struct Env {
    stop_timeout: Duration,
    kill_grace: Duration,
    poll: Duration,
    flatpak: String,
    systemctl: String,
    /// Where the running Flatpak instances are recorded.
    records: PathBuf,
}

impl Default for Env {
    fn default() -> Self {
        Env {
            stop_timeout: STOP_TIMEOUT,
            kill_grace: KILL_GRACE,
            poll: POLL,
            flatpak: "flatpak".into(),
            systemctl: "systemctl".into(),
            records: flatpaks_file(),
        }
    }
}

/// An app started from a tile.
pub struct Running {
    /// Identifies this launch, e.g. in its exit callback.
    pub id: String,
    /// The scope an ordinary app runs in.
    scope: Option<String>,
    /// The instance record of a Flatpak app.
    flatpak: Option<FlatpakApp>,
    pub process: gio::Subprocess,
    pub started: Instant,
}

impl Running {
    /// The record of a Flatpak app, for [`check_early_exit`].
    pub fn flatpak_app(&self) -> Option<FlatpakApp> {
        self.flatpak.clone()
    }
}

/// A Flatpak app the launcher started, identified by its first process: the launched
/// `sh`, exec'd into `flatpak run` and then into Flatpak's wrapper (bwrap), so one
/// process id from launch to exit. The start time tells it apart from a later process
/// that reuses the id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlatpakApp {
    app: String,
    pid: u32,
    start_time: u64,
    /// Its one SIGTERM has been sent. A later close only waits and kills, never sends
    /// a second, which would kill a Chromium-based app that is shutting down cleanly.
    #[serde(default)]
    termed: bool,
    /// How many times `flatpak kill` has failed to remove it.
    #[serde(default)]
    kill_attempts: u32,
}

thread_local! {
    static NEXT_LAUNCH: Cell<u32> = const { Cell::new(0) };
}

/// Starts the app behind a `command`, `desktop` or `flatpak` action.
pub fn start(action: &Action) -> anyhow::Result<Running> {
    match action {
        Action::Command(command) => start_in_scope(command),
        Action::Flatpak(id) => start_flatpak(id, &format!("exec flatpak run {}", shell_quote(id))),
        Action::Desktop(id) => {
            let info = gio::DesktopAppInfo::new(id)
                .with_context(|| format!("no installed application has the id {id}"))?;
            let exec = info
                .commandline()
                .with_context(|| format!("{id} has no Exec line"))?;
            let command = desktop_command(&exec.to_string_lossy())?;
            match info.string("X-Flatpak") {
                Some(app) => start_flatpak(&app, &command),
                None => start_in_scope(&command),
            }
        }
        Action::Submenu(_) | Action::Builtin(_) => bail!("not an app: {action:?}"),
    }
}

fn next_id() -> String {
    let n = NEXT_LAUNCH.with(|next| next.replace(next.get() + 1));
    format!("vextheater-app-{}-{n}", std::process::id())
}

fn start_in_scope(command: &str) -> anyhow::Result<Running> {
    let id = next_id();
    let argv = scope_argv(&id, command);
    let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    let process = gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE)
        .with_context(|| format!("cannot start {command}"))?;
    Ok(Running {
        scope: Some(format!("{id}.scope")),
        id,
        flatpak: None,
        process,
        started: Instant::now(),
    })
}

fn start_flatpak(app: &str, command: &str) -> anyhow::Result<Running> {
    let argv = [OsStr::new("sh"), OsStr::new("-c"), OsStr::new(command)];
    let process = gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE)
        .with_context(|| format!("cannot start {command}"))?;

    // Read while the process cannot have exited unnoticed: it is ours and unreaped.
    let record = process
        .identifier()
        .and_then(|pid| pid.parse().ok())
        .and_then(|pid| {
            proc_stat(pid).map(|stat| FlatpakApp {
                app: app.to_owned(),
                pid,
                start_time: stat.start_time,
                termed: false,
                kill_attempts: 0,
            })
        });
    match &record {
        Some(record) => {
            let records = flatpaks_file();
            let mut apps = load_flatpaks(&records);
            apps.push(record.clone());
            save_flatpaks(&records, &apps);
        }
        // Practically unreachable: the child was just spawned and is not reaped until
        // the main loop runs, so /proc/<pid>/stat exists (at worst as a zombie). If it
        // does fail, say so loudly; closing then falls back to the launched process.
        None => log::error!(
            "cannot identify the {app} process; closing it signals the launched process only"
        ),
    }

    Ok(Running {
        id: next_id(),
        scope: None,
        flatpak: record,
        process,
        started: Instant::now(),
    })
}

/// Why the previous app could not be confirmed closed.
pub enum StopError {
    /// Still running at the time limit; the next app starts anyway.
    Timeout,
    /// These Flatpak apps would not close even after repeated `flatpak kill`s, and are
    /// no longer tracked; the next app starts anyway.
    GaveUp(Vec<String>),
    /// The scopes could not be listed, so whether an app is still running is unknown.
    /// The next app is not started, rather than risk starting it over a live one. The
    /// launched app is handed back untouched, for the next close to retry with.
    Unknown(Option<Running>),
}

/// Closes every app started from a tile, by this launcher or by one that crashed, and
/// returns once they are gone. Starting the next app only then means it cannot hand
/// its window to the closing one, as a second Brave would to a Brave that is still
/// shutting down. The caller runs one of these at a time.
///
/// `launched` is the app this launcher started last. It is tracked through its own
/// process too, since a close can begin before systemd or Flatpak know about it.
///
/// Every process gets at most one SIGTERM: Chromium-based apps such as Brave start a
/// clean shutdown on the first and die on a second, losing their session. A Flatpak
/// app that outlives its close stays recorded with `termed` set, so a later close only
/// waits for it and uses `flatpak kill`; and after [`MAX_KILL_ATTEMPTS`] failed kills
/// the launcher gives up on it, with a toast, instead of costing every close the wait.
///
/// The scopes, the launched app and each recorded Flatpak instance are closed side by
/// side.
///
/// The worst case is about 20 s: up to 2 s to list the scopes, then the slowest of
/// - the scopes: 5 s to stop, 2 s to re-list after a failed stop, 2 s to SIGKILL and
///   2 s for the last listing in that grace: 11 s;
/// - a launched app whose scope registered during the close: 5 s for it to exit, 2 s
///   to SIGKILL it, then the same 11 s for its scope: 18 s;
/// - a Flatpak instance: 5 s to find it, 5 s for it to exit after its SIGTERM, then
///   `flatpak kill` and its effect share 2 s: 12 s. Signals are system calls and take
///   no time.
pub async fn stop_all(launched: Option<Running>) -> Result<(), StopError> {
    stop_all_with(launched, Rc::new(Env::default())).await
}

async fn stop_all_with(launched: Option<Running>, env: Rc<Env>) -> Result<(), StopError> {
    // The launcher starts nothing while a close is running, so these are all the apps
    // there are, apart from a launched app that has not registered yet.
    let Some(scopes) = running_scopes(&env).await else {
        log::error!("cannot list the app scopes; not starting anything over them");
        return Err(StopError::Unknown(launched));
    };

    let mut flatpaks = load_flatpaks(&env.records);
    if let Some(record) = launched.as_ref().and_then(|l| l.flatpak.clone()) {
        if !flatpaks.iter().any(|saved| saved.same_process(&record)) {
            flatpaks.push(record);
        }
    }

    let context = glib::MainContext::ref_thread_default();
    let flatpak_tasks: Vec<_> = flatpaks
        .into_iter()
        .map(|record| {
            let task = context.spawn_local(stop_flatpak(record.clone(), env.clone()));
            (record, task)
        })
        .collect();
    // A Flatpak launch is closed through its record; any other app through its scope
    // and process, or through its process alone should it have no scope.
    let launched_task = launched
        .filter(|launched| launched.flatpak.is_none())
        .map(|launched| {
            let scope = launched.scope.clone();
            let registered = scope.as_ref().is_some_and(|scope| scopes.contains(scope));
            let env = env.clone();
            context.spawn_local(async move {
                stop_process(&launched, scope.as_deref(), registered, &env).await
            })
        });

    let mut closed = stop_scopes(&scopes, &env).await;
    if let Some(task) = launched_task {
        closed &= task.await.unwrap_or(false);
    }
    let mut still_running = Vec::new();
    let mut gave_up = Vec::new();
    for (original, task) in flatpak_tasks {
        match task.await {
            Ok((_, FlatpakOutcome::Closed)) => {}
            Ok((record, FlatpakOutcome::Stuck)) => still_running.push(record),
            Ok((record, FlatpakOutcome::GaveUp)) => gave_up.push(record.app),
            Err(_) => still_running.push(original),
        }
    }
    save_flatpaks(&env.records, &still_running);

    if !gave_up.is_empty() {
        Err(StopError::GaveUp(gave_up))
    } else if closed && still_running.is_empty() {
        Ok(())
    } else {
        Err(StopError::Timeout)
    }
}

/// Waits for a launched app's process to exit, killing it at the time limit. If it has
/// a scope and that was registered, the scope is being stopped with the others, which
/// sends the SIGTERM. Otherwise the app gets its SIGTERM here, and its scope, should it
/// have registered meanwhile, is stopped once that process is gone, with the same budget
/// and check as the others, so nothing in it is signalled twice.
async fn stop_process(
    launched: &Running,
    scope: Option<&str>,
    registered: bool,
    env: &Env,
) -> bool {
    if scope.is_none() || !registered {
        launched.process.send_signal(libc::SIGTERM);
    }
    let mut exited = glib::future_with_timeout(env.stop_timeout, launched.process.wait_future())
        .await
        .is_ok();
    if !exited {
        log::warn!(
            "{} still running after {:?}; killing it",
            launched.id,
            env.stop_timeout
        );
        launched.process.force_exit();
        exited = glib::future_with_timeout(env.kill_grace, launched.process.wait_future())
            .await
            .is_ok();
    }
    match scope {
        Some(scope) if exited && !registered => stop_scopes(&[scope.to_owned()], env).await,
        _ => exited,
    }
}

/// How closing one recorded Flatpak instance ended.
enum FlatpakOutcome {
    Closed,
    /// Still running; stays recorded for a later close to try again.
    Stuck,
    /// Still running after [`MAX_KILL_ATTEMPTS`] failed kills; no longer tracked.
    GaveUp,
}

/// Closes one recorded Flatpak instance: one SIGTERM, ever, to the app inside the
/// sandbox so it can shut down cleanly, and `flatpak kill` on that instance if it is
/// still there afterwards.
///
/// Nothing is signalled until the instance is listed with its sandbox info (`child-pid`
/// not 0), which can take a moment after the launch: signalling the launcher-side
/// process earlier takes the sandbox down with it, so the app never shuts down cleanly.
/// If that has not happened after the stop timeout (`flatpak ps` failing, the instance
/// never listed or never ready), the recorded process gets the one SIGTERM instead, and
/// a SIGKILL after another timeout. The record is returned with what was done to it.
async fn stop_flatpak(mut record: FlatpakApp, env: Rc<Env>) -> (FlatpakApp, FlatpakOutcome) {
    let deadline = Instant::now() + env.stop_timeout;
    let ready = loop {
        if !record.alive() {
            return (record, FlatpakOutcome::Closed);
        }
        let listed = flatpak_instances(&env, deadline).await;
        let answered = listed.is_some();
        let ours = listed.and_then(|all| all.into_iter().find(|i| record.owns(i)));
        if let Some(targets) = ours.as_ref().and_then(Instance::term_targets) {
            break ours.map(|instance| (instance, targets));
        }
        // A record an earlier close already SIGTERMed has no SIGTERM left to wait for,
        // so one listing is enough. Here the instance is unlisted or not ready, and
        // `flatpak kill` cannot reach either: the recorded process gets the SIGKILL.
        if record.termed && answered {
            break None;
        }
        if Instant::now() >= deadline {
            break None;
        }
        glib::timeout_future(env.poll).await;
    };

    if !record.termed {
        match &ready {
            Some((_, targets)) => {
                for &pid in targets {
                    send_signal(pid, libc::SIGTERM);
                }
            }
            None => {
                log::warn!(
                    "no ready Flatpak instance of {} after {:?}; signalling its process",
                    record.app,
                    env.stop_timeout
                );
                record.signal(libc::SIGTERM);
            }
        }
        record.termed = true;
    }
    if wait_until_gone(&record, Instant::now() + env.stop_timeout, &env).await {
        return (record, FlatpakOutcome::Closed);
    }

    log::warn!("{} still running; killing it", record.app);
    record.kill_attempts += 1;
    let grace = Instant::now() + env.kill_grace;
    match ready {
        Some((instance, _)) => {
            run(&[&env.flatpak, "kill", &instance.instance], grace).await;
        }
        None => {
            record.signal(libc::SIGKILL);
        }
    }
    if wait_until_gone(&record, grace, &env).await {
        (record, FlatpakOutcome::Closed)
    } else if record.kill_attempts >= MAX_KILL_ATTEMPTS {
        log::warn!(
            "giving up on {}: still running after {} kills",
            record.app,
            record.kill_attempts
        );
        (record, FlatpakOutcome::GaveUp)
    } else {
        (record, FlatpakOutcome::Stuck)
    }
}

async fn wait_until_gone(record: &FlatpakApp, deadline: Instant, env: &Env) -> bool {
    loop {
        if !record.alive() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        glib::timeout_future(env.poll).await;
    }
}

/// Whether `pid` may be signalled. Never 0 (kill(2) with 0 signals the caller's whole
/// process group, which is the launcher's own), never 1 (init), never the launcher
/// itself, and never anything that does not fit a `pid_t`.
fn signalable(pid: u32) -> bool {
    pid > 1 && i32::try_from(pid).is_ok() && pid != std::process::id()
}

/// Sends `signal` to one process; false if it was refused or failed.
fn send_signal(pid: u32, signal: i32) -> bool {
    if !signalable(pid) {
        log::error!("refusing to send signal {signal} to pid {pid}");
        return false;
    }
    // SAFETY: kill(2) with a pid above 1 other than our own only delivers a signal.
    unsafe { libc::kill(pid as libc::pid_t, signal) == 0 }
}

impl FlatpakApp {
    /// Whether `other` is the same process (a pid and start time).
    fn same_process(&self, other: &FlatpakApp) -> bool {
        self.pid == other.pid && self.start_time == other.start_time
    }

    /// Still running: the process exists, is the same one, and is not a zombie.
    fn alive(&self) -> bool {
        proc_stat(self.pid).is_some_and(|stat| {
            stat.start_time == self.start_time && !matches!(stat.state, 'Z' | 'X')
        })
    }

    /// Sends `signal` to this process if it is still the recorded one, checked right
    /// before sending, so a pid that was reused meanwhile is never signalled.
    fn signal(&self, signal: i32) -> bool {
        self.alive() && send_signal(self.pid, signal)
    }

    /// Whether an instance whose wrapper process started at `started` (clock ticks, as
    /// in [`ProcStat`]; None if it is gone) can have come from this launch, that is, did
    /// not start before it. An older instance of the same app, stuck from before or
    /// opened from the desktop, is not this launch's.
    fn could_come_from(&self, started: Option<u64>) -> bool {
        started.is_some_and(|started| started >= self.start_time)
    }

    /// Whether `instance` is this app's: Flatpak's wrapper is this process, or (should
    /// `flatpak run` fork rather than exec it) a child of it.
    fn owns(&self, instance: &Instance) -> bool {
        instance.app == self.app
            && (instance.pid == self.pid
                || proc_stat(instance.pid).is_some_and(|stat| stat.ppid == self.pid))
    }
}

/// A row of `flatpak ps`.
#[derive(Debug, PartialEq)]
struct Instance {
    instance: String,
    /// The wrapper process, outside the sandbox.
    pid: u32,
    /// The first process inside the sandbox; 0 until Flatpak has written the
    /// instance's sandbox info.
    child_pid: u32,
    app: String,
}

impl Instance {
    /// The processes to send SIGTERM for a clean shutdown; None if the sandbox is not
    /// ready yet.
    fn term_targets(&self) -> Option<Vec<u32>> {
        let is_init = std::fs::read_to_string(format!("/proc/{}/comm", self.child_pid))
            .is_ok_and(|comm| comm.trim() == "bwrap");
        let kids = if is_init {
            children(self.child_pid)
        } else {
            Vec::new()
        };
        term_targets(self.child_pid, is_init, &kids)
    }
}

/// Picks who gets the SIGTERM from the sandbox's first process `child_pid`. That is
/// bwrap's init when it runs one: init ignores signals it has no handler for, so its
/// children, the app itself, get the signal instead, and the init itself only when it
/// has none yet. None when `child_pid` cannot be signalled: it is 0 until Flatpak has
/// written the sandbox info, so the instance is not ready.
fn term_targets(child_pid: u32, is_init: bool, children: &[u32]) -> Option<Vec<u32>> {
    if !signalable(child_pid) {
        return None;
    }
    if !is_init {
        return Some(vec![child_pid]);
    }
    let kids: Vec<u32> = children
        .iter()
        .copied()
        .filter(|&p| signalable(p))
        .collect();
    Some(if kids.is_empty() {
        vec![child_pid]
    } else {
        kids
    })
}

/// Called when a launched Flatpak's process has exited. The launcher closes a Flatpak
/// app through that process, which assumes `flatpak run` execs into the sandbox's
/// wrapper under the same pid. If the process is gone within moments while Flatpak
/// still lists the app, the assumption does not hold on this system, so say so: it then
/// shows up in the journal instead of as an app that cannot be closed.
///
/// The app may take a moment to appear in `flatpak ps`, so this watches for
/// [`EARLY_EXIT_WATCH`] rather than looking once.
pub async fn check_early_exit(record: FlatpakApp) {
    if early_exit(&record, &Env::default(), EARLY_EXIT_WATCH).await {
        log::warn!(
            "the {} process the launcher started has exited, but Flatpak still lists \
             the app: the assumption that `flatpak run` keeps one process id does not \
             hold on this system, so the launcher cannot close this app",
            record.app
        );
    }
}

/// Whether, within `watch`, `flatpak ps` lists an instance of the app that is not the
/// launched process but started no earlier than it, as if this launch were still
/// running although its process is gone. Older instances of the app are not this
/// launch's and are ignored.
async fn early_exit(record: &FlatpakApp, env: &Env, watch: Duration) -> bool {
    let deadline = Instant::now() + watch;
    loop {
        let listed = flatpak_instances(env, Instant::now() + env.kill_grace).await;
        if listed.is_some_and(|instances| {
            instances.iter().any(|i| {
                i.app == record.app
                    && !record.owns(i)
                    && record.could_come_from(proc_stat(i.pid).map(|stat| stat.start_time))
            })
        }) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        glib::timeout_future(env.poll).await;
    }
}

/// The running Flatpak instances; None if `flatpak ps` failed.
async fn flatpak_instances(env: &Env, deadline: Instant) -> Option<Vec<Instance>> {
    let ps = [
        env.flatpak.as_str(),
        "ps",
        "--columns=instance,pid,child-pid,application",
    ];
    match run(&ps, deadline).await {
        Some(Output {
            stdout,
            success: true,
        }) => Some(parse_instances(&stdout)),
        _ => None,
    }
}

fn parse_instances(listed: &str) -> Vec<Instance> {
    listed
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some(Instance {
                instance: fields.next()?.to_owned(),
                pid: fields.next()?.parse().ok()?,
                child_pid: fields.next()?.parse().ok()?,
                app: fields.next()?.to_owned(),
            })
        })
        .collect()
}

/// The fields of `/proc/<pid>/stat` the launcher uses.
struct ProcStat {
    state: char,
    ppid: u32,
    /// Clock ticks after boot when the process started.
    start_time: u64,
}

fn proc_stat(pid: u32) -> Option<ProcStat> {
    parse_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// `pid (comm) state ppid …`, start time being field 22. The command name can contain
/// spaces and parentheses, so fields are counted from its last `)`.
fn parse_stat(stat: &str) -> Option<ProcStat> {
    let fields: Vec<&str> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
    Some(ProcStat {
        state: fields.first()?.chars().next()?,
        ppid: fields.get(1)?.parse().ok()?,
        start_time: fields.get(19)?.parse().ok()?,
    })
}

fn children(pid: u32) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&child| proc_stat(child).is_some_and(|stat| stat.ppid == pid))
        .collect()
}

fn flatpaks_file() -> PathBuf {
    glib::user_runtime_dir()
        .join("vextheater")
        .join("flatpak-apps.json")
}

fn load_flatpaks(file: &Path) -> Vec<FlatpakApp> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_flatpaks(file: &Path, apps: &[FlatpakApp]) {
    let written = serde_json::to_vec(apps)
        .map_err(std::io::Error::other)
        .and_then(|json| crate::files::write_atomic(file, &json));
    if let Err(error) = written {
        log::error!("cannot record the running Flatpak apps: {error}");
    }
}

/// `systemctl stop` returns once the scopes are gone. At the time limit the ones still
/// there are killed, by name, so nothing started since the snapshot is touched.
async fn stop_scopes(scopes: &[String], env: &Env) -> bool {
    if scopes.is_empty() {
        return true;
    }
    let deadline = Instant::now() + env.stop_timeout;
    let names = scopes.iter().map(String::as_str);
    let stop: Vec<&str> = [env.systemctl.as_str(), "--user", "stop"]
        .into_iter()
        .chain(names.clone())
        .collect();
    match run(&stop, deadline).await {
        Some(Output { success: true, .. }) => return true,
        // Non-zero also when a scope vanished on its own or was never registered;
        // only a listing shows whether any are actually left.
        Some(Output { success: false, .. }) => {
            if running_scopes(env)
                .await
                .is_some_and(|left| !scopes.iter().any(|scope| left.contains(scope)))
            {
                return true;
            }
            log::warn!("systemctl stop failed for {scopes:?}; killing them");
        }
        None => log::warn!(
            "{scopes:?} still running after {:?}; killing them",
            env.stop_timeout
        ),
    }

    let grace = Instant::now() + env.kill_grace;
    let kill: Vec<&str> = [env.systemctl.as_str(), "--user", "kill", "--signal=SIGKILL"]
        .into_iter()
        .chain(names)
        .collect();
    run(&kill, grace).await;
    loop {
        // A listing that fails says nothing; keep polling until the grace ends.
        if let Some(left) = running_scopes(env).await {
            if !scopes.iter().any(|scope| left.contains(scope)) {
                return true;
            }
        }
        if Instant::now() >= grace {
            log::warn!("{scopes:?} still there after SIGKILL");
            return false;
        }
        glib::timeout_future(env.poll).await;
    }
}

/// Names of the loaded `vextheater-app-*` scopes; None if they could not be listed.
async fn running_scopes(env: &Env) -> Option<Vec<String>> {
    let list = [
        env.systemctl.as_str(),
        "--user",
        "list-units",
        "--type=scope",
        "--no-legend",
        "--plain",
        ALL_SCOPES,
    ];
    match run(&list, Instant::now() + env.kill_grace).await {
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

/// The shell command that runs a desktop entry's Exec line: its arguments, parsed by
/// the Desktop Entry spec's rules, each quoted for `sh`.
fn desktop_command(exec: &str) -> anyhow::Result<String> {
    let argv = exec_argv(exec)?;
    let quoted: Vec<String> = argv.iter().map(|arg| shell_quote(arg)).collect();
    Ok(format!("exec {}", quoted.join(" ")))
}

/// Splits an Exec line into arguments (Desktop Entry spec, "The Exec key").
///
/// Field codes are expanded first, over the whole line, as GIO does: `%%` becomes `%`
/// and the codes that stand for files, URLs, the icon or the entry (`%f %F %u %U %i %c
/// %k`, and the deprecated `%d %D %n %N %v %m`) are dropped, since the launcher passes
/// none. Then arguments are separated by unquoted spaces; inside double quotes a
/// backslash escapes `"`, `` ` ``, `$` and `\`. Single quotes have no special meaning.
fn exec_argv(exec: &str) -> anyhow::Result<Vec<String>> {
    let mut expanded = String::with_capacity(exec.len());
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('%', Some('%')) => {
                chars.next();
                expanded.push('%');
            }
            ('%', Some(code)) if "fFuUickdDnNvm".contains(*code) => {
                chars.next();
            }
            _ => expanded.push(c),
        }
    }

    let mut argv = Vec::new();
    let mut current: Option<String> = None;
    let mut chars = expanded.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => argv.extend(current.take()),
            '"' => {
                let arg = current.get_or_insert_with(String::new);
                loop {
                    match chars.next() {
                        None => bail!("unterminated quote in Exec line: {exec}"),
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(escaped @ ('"' | '`' | '$' | '\\')) => arg.push(escaped),
                            Some(other) => {
                                arg.push('\\');
                                arg.push(other);
                            }
                            None => bail!("unterminated quote in Exec line: {exec}"),
                        },
                        Some(other) => arg.push(other),
                    }
                }
            }
            other => current.get_or_insert_with(String::new).push(other),
        }
    }
    argv.extend(current);
    if argv.is_empty() {
        bail!("empty Exec line");
    }
    Ok(argv)
}

fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parses_flatpak_instances() {
        let listed = "1234567 4001 4010 com.brave.Browser\nbroken line\n89 5 6 org.test.App\n";
        assert_eq!(
            parse_instances(listed),
            [
                Instance {
                    instance: "1234567".into(),
                    pid: 4001,
                    child_pid: 4010,
                    app: "com.brave.Browser".into()
                },
                Instance {
                    instance: "89".into(),
                    pid: 5,
                    child_pid: 6,
                    app: "org.test.App".into()
                },
            ]
        );
    }

    #[test]
    fn parses_proc_stat_with_awkward_names() {
        let stat = "4242 (my (odd) app) S 17 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 \
                    20 0 1 0 987654 12345678 300 18446744073709551615";
        let parsed = parse_stat(stat).unwrap();
        assert_eq!(parsed.state, 'S');
        assert_eq!(parsed.ppid, 17);
        assert_eq!(parsed.start_time, 987654);
        assert!(parse_stat("4242 (truncated) S").is_none());
    }

    #[test]
    fn never_signals_pid_0_pid_1_ourselves_or_an_out_of_range_pid() {
        let me = std::process::id();
        assert!(!signalable(0));
        assert!(!signalable(1));
        assert!(!signalable(me));
        assert!(!signalable(u32::MAX));
        assert!(!signalable(i32::MAX as u32 + 1));
        assert!(signalable(me + 1));
        // The guard is in send_signal itself: had these gone through, kill(0, ..) would
        // have signalled this whole test process group.
        assert!(!send_signal(0, libc::SIGTERM));
        assert!(!send_signal(1, libc::SIGTERM));
        assert!(!send_signal(me, libc::SIGTERM));
    }

    #[test]
    fn picks_the_sigterm_targets_in_the_sandbox() {
        // Not ready yet: flatpak ps shows child-pid 0 until the sandbox info exists.
        assert_eq!(term_targets(0, false, &[]), None);
        assert_eq!(term_targets(0, true, &[10]), None);
        assert_eq!(term_targets(1, false, &[]), None);
        // An ordinary first process is signalled itself.
        assert_eq!(term_targets(500, false, &[]), Some(vec![500]));
        // bwrap's init ignores SIGTERM: its children are signalled instead,
        assert_eq!(term_targets(500, true, &[501, 502]), Some(vec![501, 502]));
        // never pid 0 or 1 among them,
        assert_eq!(term_targets(500, true, &[0, 1, 503]), Some(vec![503]));
        // and the init itself when it has no children yet.
        assert_eq!(term_targets(500, true, &[]), Some(vec![500]));
        assert_eq!(term_targets(500, true, &[0, 1]), Some(vec![500]));
    }

    #[test]
    fn this_process_is_alive_and_a_changed_start_time_is_not() {
        let pid = std::process::id();
        let start_time = proc_stat(pid).unwrap().start_time;
        let me = FlatpakApp {
            app: "x".into(),
            pid,
            start_time,
            termed: false,
            kill_attempts: 0,
        };
        assert!(me.alive());
        let reused = FlatpakApp {
            start_time: start_time + 1,
            ..me
        };
        assert!(!reused.alive());
    }

    #[test]
    fn only_an_instance_that_started_no_earlier_can_come_from_the_launch() {
        let launch = FlatpakApp {
            app: "x".into(),
            pid: 10,
            start_time: 500,
            termed: false,
            kill_attempts: 0,
        };
        assert!(launch.could_come_from(Some(500)));
        assert!(launch.could_come_from(Some(501)));
        assert!(!launch.could_come_from(Some(499)));
        assert!(!launch.could_come_from(None));
    }

    #[test]
    fn splits_exec_lines_by_the_spec() {
        assert_eq!(exec_argv("firefox %u").unwrap(), ["firefox"]);
        assert_eq!(exec_argv("app %F --flag").unwrap(), ["app", "--flag"]);
        assert_eq!(exec_argv("app \"\" x").unwrap(), ["app", "", "x"]);
        assert_eq!(
            exec_argv("/usr/bin/flatpak run --branch=stable io.example.App @@u %U @@").unwrap(),
            [
                "/usr/bin/flatpak",
                "run",
                "--branch=stable",
                "io.example.App",
                "@@u",
                "@@"
            ]
        );
        assert!(exec_argv("   ").is_err());
    }

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(shell_quote("io.example.App"), "'io.example.App'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}

#[cfg(test)]
mod exec_tests {
    use super::*;

    /// Runs `desktop_command(exec)` through sh with `printf` standing in for the app,
    /// one argument per line in brackets, and returns what it printed.
    fn argv_seen(exec: &str) -> String {
        let command = desktop_command(exec).unwrap();
        let output = std::process::Command::new("sh")
            .args(["-c", &command])
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn keeps_spaces_inside_quoted_arguments() {
        assert_eq!(argv_seen(r#"printf "[%%s]\n" "a  b" c"#), "[a  b]\n[c]\n");
    }

    #[test]
    fn unescapes_quoted_specials_and_keeps_them_literal() {
        assert_eq!(
            argv_seen(r#"printf "[%%s]\n" "q\"d\\b\$HOME\`x" '$(true)'"#),
            "[q\"d\\b$HOME`x]\n['$(true)']\n"
        );
    }

    #[test]
    fn drops_field_codes_and_unescapes_percent() {
        assert_eq!(argv_seen(r#"printf "[%%s]\n" %U 100%% %f"#), "[100%]\n");
    }

    #[test]
    fn rejects_an_unterminated_quote() {
        assert!(desktop_command(r#"app "unterminated"#).is_err());
    }
}

/// The start and close lifecycle, end to end: real processes, a stub `flatpak` and a
/// stub `systemctl` in a scratch directory, and millisecond timings.
#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::cell::RefCell;
    use std::future::Future;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::OnceLock;

    const APP: &str = "org.example.App";

    /// One script behind both stubs; it acts as the program it is invoked as and keeps
    /// its answers and logs in its own directory, which tests fill in.
    const STUB: &str = r#"#!/bin/sh
d=$(dirname "$0")
case "$(basename "$0")" in
flatpak)
  case "$1" in
  ps) [ -e "$d/ps_fail" ] && exit 1; [ -e "$d/ps.txt" ] && cat "$d/ps.txt"; exit 0 ;;
  kill) echo "$2" >> "$d/flatpak-kill.log"; exit 0 ;;
  esac ;;
systemctl)
  echo "$*" >> "$d/systemctl.log"
  case "$2" in
  list-units) [ -e "$d/list_fail" ] && exit 1; [ -e "$d/units.txt" ] && cat "$d/units.txt"; exit 0 ;;
  stop) [ -e "$d/stop_term_pid" ] && kill -TERM "$(cat "$d/stop_term_pid")" 2>/dev/null; exit 0 ;;
  kill) exit 0 ;;
  esac ;;
esac
exit 1
"#;

    /// Shuts down on SIGTERM, logging it.
    const POLITE: &str = r#"trap 'echo term >> "$1"; exit 0' TERM
touch "$1.ready"; while :; do sleep 0.05; done"#;
    /// Logs SIGTERM and carries on; only SIGKILL stops it.
    const STUBBORN: &str = r#"trap 'echo term >> "$1"' TERM
touch "$1.ready"; while :; do sleep 0.05; done"#;
    /// Chromium-like: the first SIGTERM starts a clean shutdown that takes `$2` ticks of
    /// 50 ms; a second one kills it on the spot, losing its session.
    const CHROMIUM: &str = r#"n=0; stopping=0; i=0
trap 'n=$((n+1)); echo term >> "$1"; if [ $n -ge 2 ]; then echo died >> "$1"; exit 1; fi; stopping=1' TERM
touch "$1.ready"
while :; do
  sleep 0.05
  if [ $stopping = 1 ]; then i=$((i+1)); if [ $i -ge $2 ]; then echo clean >> "$1"; exit 0; fi; fi
done"#;
    /// Stands in for Flatpak's wrapper: runs while process `$1` does.
    const WATCHER: &str = r#"while [ -r /proc/$1/stat ]; do sleep 0.05; done"#;

    fn stub_script() -> &'static Path {
        static STUBS: OnceLock<PathBuf> = OnceLock::new();
        STUBS.get_or_init(|| {
            // Written once, before any test forks a helper, so it is never executed
            // while another thread's child still holds it open for writing.
            let dir = std::env::temp_dir().join(format!("vextheater-stub-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let script = dir.join("stub.sh");
            std::fs::write(&script, STUB).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            script
        })
    }

    /// Runs `future` on a main context of its own, so tests can run in parallel.
    fn block_on<F: Future>(future: F) -> F::Output {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| context.block_on(future))
            .unwrap()
    }

    /// A scratch directory with the stubs in it. Processes started through it are
    /// killed when it is dropped.
    struct Lab {
        dir: PathBuf,
        pids: RefCell<Vec<u32>>,
    }

    impl Lab {
        fn new() -> Lab {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let script = stub_script();
            let dir = script
                .parent()
                .unwrap()
                .join(format!("lab-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&dir).unwrap();
            for name in ["flatpak", "systemctl"] {
                symlink(script, dir.join(name)).unwrap();
            }
            Lab {
                dir,
                pids: RefCell::default(),
            }
        }

        /// Timings in milliseconds: `stop` is the stop timeout.
        fn env(&self, stop: u64) -> Rc<Env> {
            Rc::new(Env {
                stop_timeout: Duration::from_millis(stop),
                kill_grace: Duration::from_millis(1000),
                poll: Duration::from_millis(20),
                flatpak: self.dir.join("flatpak").to_string_lossy().into_owned(),
                systemctl: self.dir.join("systemctl").to_string_lossy().into_owned(),
                records: self.dir.join("records.json"),
            })
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.join(name)
        }

        /// What the script named `log` wrote to its log, one word per line.
        fn log(&self, log: &str) -> String {
            std::fs::read_to_string(self.path(log)).unwrap_or_default()
        }

        fn write(&self, name: &str, text: &str) {
            write_replacing(&self.path(name), text);
        }

        /// Writes `text` to `name` after `ms` milliseconds.
        fn write_later(&self, ms: u64, name: &str, text: String) {
            let path = self.path(name);
            glib::spawn_future_local(async move {
                glib::timeout_future(Duration::from_millis(ms)).await;
                write_replacing(&path, &text);
            });
        }

        /// Starts `script` as a plain child, returning once it has installed its signal
        /// handlers.
        fn spawn(&self, script: &str, log: &str, args: &[&str]) -> u32 {
            let log_path = self.path(log);
            let child = Command::new("sh")
                .args(["-c", script, "stub"])
                .arg(&log_path)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let pid = child.id();
            self.pids.borrow_mut().push(pid);
            // Reaped as soon as it exits, as init would, so it is gone for `kill -0`.
            std::thread::spawn(move || {
                let mut child = child;
                let _ = child.wait();
            });
            wait_for(&self.path(&format!("{log}.ready")));
            pid
        }

        /// Starts `script` the way the launcher starts an app: a `gio::Subprocess`.
        fn process(&self, script: &str, args: &[&str]) -> gio::Subprocess {
            let mut argv = vec!["sh".to_owned(), "-c".into(), script.into(), "stub".into()];
            argv.extend(args.iter().map(|arg| arg.to_string()));
            let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
            let process = gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE).unwrap();
            self.pids.borrow_mut().push(pid_of(&process));
            process
        }

        /// A launched Flatpak app, recorded as `start_flatpak` records it.
        fn launched_flatpak(&self, script: &str, args: &[&str]) -> Running {
            let process = self.process(script, args);
            let pid = pid_of(&process);
            Running {
                id: "vextheater-app-test".into(),
                scope: None,
                flatpak: Some(FlatpakApp {
                    app: APP.into(),
                    pid,
                    start_time: proc_stat(pid).unwrap().start_time,
                    termed: false,
                    kill_attempts: 0,
                }),
                process,
                started: Instant::now(),
            }
        }

        /// A launched ordinary app in the scope `scope`.
        fn launched_scoped(&self, scope: &str, script: &str, log: &str) -> Running {
            let log_path = self.path(log);
            let process = self.process(script, &[log_path.to_str().unwrap()]);
            wait_for(&self.path(&format!("{log}.ready")));
            Running {
                id: "vextheater-app-test".into(),
                scope: Some(scope.into()),
                flatpak: None,
                process,
                started: Instant::now(),
            }
        }
    }

    impl Drop for Lab {
        fn drop(&mut self) {
            for pid in self.pids.borrow().iter() {
                // SAFETY: a test helper that was started by this lab; at worst it is gone.
                unsafe { libc::kill(*pid as libc::pid_t, libc::SIGKILL) };
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn pid_of(process: &gio::Subprocess) -> u32 {
        process.identifier().unwrap().parse().unwrap()
    }

    fn write_replacing(path: &Path, text: &str) {
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, text).unwrap();
        std::fs::rename(temp, path).unwrap();
    }

    fn wait_for(path: &Path) {
        for _ in 0..300 {
            if path.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("{} never appeared", path.display());
    }

    /// A `flatpak ps` line: this wrapper process and sandbox first process.
    fn listing(wrapper: u32, child: u32) -> String {
        format!("1234567 {wrapper} {child} {APP}\n")
    }

    fn pid_gone(pid: u32) -> bool {
        proc_stat(pid).is_none_or(|stat| matches!(stat.state, 'Z' | 'X'))
    }

    // Item 1: signalling the launcher-side process before the instance is listed takes
    // the sandbox down with it, so the app never shuts down cleanly.
    #[test]
    fn an_instance_not_listed_yet_is_waited_for_not_signalled() {
        let lab = Lab::new();
        let sandbox = lab.spawn(POLITE, "sandbox.log", &[]);
        let app = lab.launched_flatpak(WATCHER, &[&sandbox.to_string()]);
        let wrapper = app.flatpak.as_ref().unwrap().pid;
        let env = lab.env(3000);
        let result = block_on(async {
            lab.write_later(250, "ps.txt", listing(wrapper, sandbox));
            stop_all_with(Some(app), env).await
        });
        assert!(result.is_ok());
        assert_eq!(lab.log("sandbox.log"), "term\n");
        assert!(!lab.path("flatpak-kill.log").exists());
    }

    // Item 3: an instance listed with child-pid 0 is not ready, not a failed kill.
    #[test]
    fn a_not_ready_instance_is_waited_for_not_signalled() {
        let lab = Lab::new();
        let sandbox = lab.spawn(POLITE, "sandbox.log", &[]);
        let app = lab.launched_flatpak(WATCHER, &[&sandbox.to_string()]);
        let wrapper = app.flatpak.as_ref().unwrap().pid;
        lab.write("ps.txt", &listing(wrapper, 0));
        let env = lab.env(3000);
        let result = block_on(async {
            lab.write_later(250, "ps.txt", listing(wrapper, sandbox));
            stop_all_with(Some(app), env).await
        });
        assert!(result.is_ok());
        assert_eq!(lab.log("sandbox.log"), "term\n");
        assert!(!lab.path("flatpak-kill.log").exists());
    }

    /// An app that ignores SIGTERM and whose instance `ps` never makes ready: one
    /// SIGTERM to the recorded process at the limit, then SIGKILL.
    fn assert_termed_once_then_killed(lab: &Lab) {
        let log = lab.path("app.log");
        let app = lab.launched_flatpak(STUBBORN, &[log.to_str().unwrap()]);
        wait_for(&lab.path("app.log.ready"));
        let record = app.flatpak.clone().unwrap();
        if !lab.path("ps_fail").exists() && !lab.path("ps.txt").exists() {
            // Neither a failing nor an empty listing: the not-ready case.
            lab.write("ps.txt", &listing(record.pid, 0));
        }
        let started = Instant::now();
        let result = block_on(stop_all_with(Some(app), lab.env(1500)));
        assert!(result.is_ok(), "the app should be gone");
        assert!(!record.alive());
        assert_eq!(lab.log("app.log"), "term\n", "exactly one SIGTERM");
        assert!(
            started.elapsed() >= Duration::from_millis(1500),
            "no signal before the limit"
        );
    }

    // Item 3, at the limit.
    #[test]
    fn a_never_ready_instance_gets_one_sigterm_then_sigkill() {
        assert_termed_once_then_killed(&Lab::new());
    }

    // Item 5: `flatpak ps` failing throughout.
    #[test]
    fn a_failing_flatpak_ps_still_gets_a_sigterm_before_the_sigkill() {
        let lab = Lab::new();
        lab.write("ps_fail", "");
        assert_termed_once_then_killed(&lab);
    }

    // The instance never being listed gets the same treatment.
    #[test]
    fn an_instance_never_listed_gets_one_sigterm_then_sigkill() {
        let lab = Lab::new();
        lab.write("ps.txt", "");
        assert_termed_once_then_killed(&lab);
    }

    // One SIGTERM per process: a Chromium-like app that needs longer than the stop
    // timeout to shut down is closed over two closes, and the second must not term it.
    #[test]
    fn a_slow_chromium_like_app_gets_one_sigterm_across_closes() {
        let lab = Lab::new();
        let sandbox = lab.spawn(CHROMIUM, "sandbox.log", &["60"]);
        let app = lab.launched_flatpak(WATCHER, &[&sandbox.to_string()]);
        let wrapper = app.flatpak.as_ref().unwrap().pid;
        lab.write("ps.txt", &listing(wrapper, sandbox));
        block_on(async {
            let first = stop_all_with(Some(app), lab.env(800)).await;
            assert!(
                matches!(first, Err(StopError::Timeout)),
                "still shutting down"
            );
            // Generous: the app needs 3 s, and a loaded machine can stretch that.
            let again = stop_all_with(None, lab.env(8000)).await;
            assert!(again.is_ok(), "the second close only waits");
        });
        assert_eq!(lab.log("sandbox.log"), "term\nclean\n");
    }

    /// Closes a stubborn app whose SIGTERM an earlier close already sent, with `ps`
    /// listing it as `listed`; returns the result, with nothing signalled twice.
    fn close_termed_record(lab: &Lab, listed: impl Fn(u32) -> String) -> Result<(), StopError> {
        let pid = lab.spawn(STUBBORN, "app.log", &[]);
        let record = FlatpakApp {
            app: APP.into(),
            pid,
            start_time: proc_stat(pid).unwrap().start_time,
            termed: true,
            kill_attempts: 0,
        };
        let env = lab.env(500);
        save_flatpaks(&env.records, &[record]);
        lab.write("ps.txt", &listed(pid));
        let result = block_on(stop_all_with(None, env));
        assert_eq!(lab.log("app.log"), "", "no second SIGTERM");
        result
    }

    // A record an earlier close already SIGTERMed is not held up waiting for the
    // sandbox to be ready, and `flatpak kill` cannot reach an instance that is not
    // ready, so the recorded process gets the SIGKILL.
    #[test]
    fn a_termed_record_with_a_not_ready_instance_is_sigkilled() {
        let lab = Lab::new();
        let result = close_termed_record(&lab, |pid| listing(pid, 0));
        assert!(result.is_ok(), "the SIGKILL removed it");
        assert!(!lab.path("flatpak-kill.log").exists());
    }

    // With the instance ready, `flatpak kill` is the way to remove it.
    #[test]
    fn a_termed_record_with_a_ready_instance_gets_flatpak_kill() {
        let lab = Lab::new();
        // The stub's `flatpak kill` removes nothing, so the app stays: stuck.
        let result = close_termed_record(&lab, |pid| listing(pid, pid));
        assert!(matches!(result, Err(StopError::Timeout)));
        assert_eq!(lab.log("flatpak-kill.log"), "1234567\n");
    }

    // Item 2: a recycled pid is never signalled.
    #[test]
    fn a_record_whose_pid_was_reused_does_not_signal() {
        let lab = Lab::new();
        let pid = lab.spawn(POLITE, "app.log", &[]);
        let start_time = proc_stat(pid).unwrap().start_time;
        let record = |start_time| FlatpakApp {
            app: APP.into(),
            pid,
            start_time,
            termed: false,
            kill_attempts: 0,
        };
        assert!(!record(start_time + 1).signal(libc::SIGTERM));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(lab.log("app.log"), "", "the other process was left alone");
        assert!(record(start_time).signal(libc::SIGTERM));
        wait_until_logged(&lab, "app.log", "term\n");
    }

    fn wait_until_logged(lab: &Lab, log: &str, expected: &str) {
        for _ in 0..300 {
            if lab.log(log) == expected {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(lab.log(log), expected);
    }

    /// A record for a launch whose process has since exited.
    fn exited_launch(lab: &Lab) -> FlatpakApp {
        let pid = lab.spawn(POLITE, "launch.log", &[]);
        let record = FlatpakApp {
            app: APP.into(),
            pid,
            start_time: proc_stat(pid).unwrap().start_time,
            termed: false,
            kill_attempts: 0,
        };
        // SAFETY: the helper just started above.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        for _ in 0..300 {
            if pid_gone(pid) {
                return record;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the launch process did not exit");
    }

    // Item 4: another instance of the app, started before this launch, is not this
    // launch's leftover.
    #[test]
    fn an_older_instance_of_the_app_is_not_an_early_exit() {
        let lab = Lab::new();
        let older = lab.spawn(POLITE, "older.log", &[]);
        std::thread::sleep(Duration::from_millis(60));
        let record = exited_launch(&lab);
        lab.write("ps.txt", &listing(older, older));
        let env = lab.env(500);
        let warned = block_on(early_exit(&record, &env, Duration::from_millis(1500)));
        assert!(!warned);
    }

    #[test]
    fn an_instance_started_by_the_launch_after_its_process_exits_is_reported() {
        let lab = Lab::new();
        let record = exited_launch(&lab);
        // What a `flatpak run` that forks rather than execs would leave behind.
        let leftover = lab.spawn(POLITE, "leftover.log", &[]);
        lab.write("ps.txt", &listing(leftover, leftover));
        let env = lab.env(500);
        let warned = block_on(early_exit(&record, &env, Duration::from_millis(1500)));
        assert!(warned);
    }

    #[test]
    fn another_app_listed_is_not_an_early_exit() {
        let lab = Lab::new();
        let record = exited_launch(&lab);
        let other = lab.spawn(POLITE, "other.log", &[]);
        lab.write("ps.txt", &format!("1 {other} {other} org.example.Other\n"));
        let env = lab.env(500);
        let warned = block_on(early_exit(&record, &env, Duration::from_millis(1500)));
        assert!(!warned);
    }

    // A close that begins before the app registered with systemd: the launcher
    // signals the app's own process, once, and still stops its scope afterwards.
    #[test]
    fn a_close_before_the_scope_registers_still_closes_the_app() {
        let lab = Lab::new();
        let app = lab.launched_scoped("vextheater-app-t-0.scope", POLITE, "app.log");
        let pid = pid_of(&app.process);
        let result = block_on(stop_all_with(Some(app), lab.env(3000)));
        assert!(result.is_ok());
        assert!(pid_gone(pid));
        assert_eq!(lab.log("app.log"), "term\n");
        let calls = std::fs::read_to_string(lab.path("systemctl.log")).unwrap();
        assert!(calls.contains("--user stop vextheater-app-t-0.scope"));
    }

    // Once registered, systemd delivers the SIGTERM; the launcher sends none itself.
    #[test]
    fn a_registered_scope_gets_its_sigterm_from_systemctl_only() {
        let lab = Lab::new();
        let app = lab.launched_scoped("vextheater-app-t-0.scope", POLITE, "app.log");
        lab.write(
            "units.txt",
            "vextheater-app-t-0.scope loaded active running\n",
        );
        lab.write("stop_term_pid", &pid_of(&app.process).to_string());
        let result = block_on(stop_all_with(Some(app), lab.env(3000)));
        assert!(result.is_ok());
        assert_eq!(lab.log("app.log"), "term\n");
    }

    // Scopes that cannot be listed: nothing is signalled and the app is handed back.
    #[test]
    fn an_unlistable_scope_set_signals_nothing_and_returns_the_app() {
        let lab = Lab::new();
        let app = lab.launched_scoped("vextheater-app-t-0.scope", POLITE, "app.log");
        lab.write("list_fail", "");
        let result = block_on(stop_all_with(Some(app), lab.env(3000)));
        assert!(matches!(result, Err(StopError::Unknown(Some(_)))));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(lab.log("app.log"), "");
    }
}
