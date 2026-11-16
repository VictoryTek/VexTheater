# VexTheater architecture and structure analysis

Reviewed 2026-11-12 at commit `ab074b25e5b98d63e8e42c0af92ac93083f8873e`.

## Scope and method

Read `CLAUDE.md` and `README.md` first. This is an architecture-only review: no source, configuration, dependency, or test changes were made. The only authored file is this report. The working tree was clean before the review. Requested validation uses WSL Ubuntu and the project's Nix environment, with Cargo artifacts outside the repository at `$HOME/.cache/vextheater/target`; checks also use their normal external caches and temporary files.

Read the production implementation in all 17 Rust modules, module declarations and direct dependency use sites, `Cargo.toml`, `Cargo.lock`, `build.rs`, `flake.nix`, `flake.lock`, both `nix/` files, the resource manifest, preflight script, and CI workflow. Inspected relevant configuration, persistence, page/queue, and process-lifecycle tests. Findings below are supported by actual call paths; conditional failure consequences are code-derived, not claims of a reproduced desktop failure.

Not covered: live GTK rendering or input, a real GNOME/systemd/Flatpak session, streaming/DRM behavior, the external vexos-nix consumer, third-party implementation internals, or a release-by-release dependency maintenance review. Asset styling and historical planning documents were not assessed. Test execution does not establish those device behaviors.

## Validation evidence

Commands run from `/mnt/c/Projects/VexTheater` through `wsl -e bash -lc`, with `CARGO_TARGET_DIR=$HOME/.cache/vextheater/target` exported. The first sandboxed WSL inventory returned `Wsl/E_ACCESSDENIED`; elevated WSL execution subsequently worked.

| Check | Command | Observed result |
| --- | --- | --- |
| Build | `nix develop path:. -c cargo build --locked` | Passed: ``Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.99s``. |
| Test suite | `nix develop path:. -c cargo test --locked` | Passed: `178 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.84s`. |
| Linter | `nix develop path:. -c cargo clippy --locked --all-targets -- -D warnings` | Passed: ``Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.40s``, with no lint diagnostics. |
| Initial audit invocation | `nix develop path:. -c cargo audit` | Unavailable: ``error: no such command: `audit` ``. The dev shell does not provide the subcommand; the separate pinned-Nix audit below completed successfully. |
| Dependency audit | `nix shell github:NixOS/nixpkgs/cd4ea8d57de16f4810bc5f14207bac45fbf19f67#cargo-audit -c cargo audit --file Cargo.lock` | Passed, exit 0, using `cargo-audit 0.22.1` from the exact Nixpkgs revision in `flake.lock`. Output below. |
| Project preflight | `bash scripts/preflight.sh` | Passed, exit 0: `Preflight passed.` Formatting, Clippy, tests, desktop-file validation, AppStream validation, and the Nix package build all completed. |

Audit output:

```text
Fetching advisory database from `https://github.com/RustSec/advisory-db.git`
Loaded 1490 security advisories (from /home/nimda/.cargo/advisory-db)
Updating crates.io index
Scanning Cargo.lock for vulnerabilities (106 crate dependencies)
```

No vulnerabilities or advisory warnings were printed. This is a Cargo lockfile advisory result, not a guarantee about every native library supplied by Nix or the installed browsers.

Preflight's test rerun reported `178 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.47s`. Its Clippy rerun finished in `0.31s` without diagnostics. AppStream emitted one informational message and succeeded:

```text
I: io.github.vextheater:~: content-rating-missing

✔ Validation was successful: infos: 1
```

This informational metadata message is retained as validation evidence, not promoted into an architectural finding.

## Findings

Four verified findings: three medium priority and one low priority. Confidence is high for all four. No high-priority architectural issue is established by this review. Effort: S = localized change; M = coordinated change across a few components; L = staged responsibility/state extraction.

### ARCH-01 — Tile state and application orchestration are embedded in the GTK window

**Priority:** medium. **Confidence:** high. **Effort:** L.

**Files and lines:** [src/window.rs](C:/Projects/VexTheater/src/window.rs:89) lines 89–163, 283–304, 483–562, 896–961, 1453–1475, 1755–1808; [src/grid.rs](C:/Projects/VexTheater/src/grid.rs:73) lines 73–80, 117–119; [src/main.rs](C:/Projects/VexTheater/src/main.rs:105) lines 105–115; [src/tiles.rs](C:/Projects/VexTheater/src/tiles.rs:14) lines 14, 99–134.

**Evidence:**

```text
main::print_order
  -> window::order_listing
     -> window::Page::key + tiles::listed + tiles::visible

Ui::drop_picked
  -> Ui::save_order
     -> TileGrid::buttons()                      // GTK buttons are the current order
     -> Arrangeable.buttons lookup              // button -> configured index
     -> order::identities + tiles::order_to_save
     -> mutate Ui.saved_order -> order::save

Ui::close_then
  -> take Ui.running -> launch::stop_all().await
  -> interpret StopError -> start tile / hide window / notify
```

`Ui` owns configuration and its derived entries, tile visibility and saved order, running-process state and the close queue, alongside the widgets and their maps. `rebuild` explicitly clears or filters `first_tiles`, `labels`, `missing`, `titles`, `arrangeable`, and `rows` before recreating pages. The same private `Page` type supplies both GTK stack names and persistent order-file keys; `tiles.rs` separately defines the main-page key.

**What goes wrong:** The domain's current tile order is recovered from widget identity rather than maintained independently and rendered. The display-free CLI consequently reaches into the window module for domain listing, while mutations such as saving a move require `Ui` and GTK button mappings. Page replacement requires coordinating several separate collections in one component. These are existing layering and maintenance costs, not a claim that a particular reload currently crashes. Pure helpers already exist in `tiles` and `order`, but the state transitions and persistence assembly remain in the view.

**Suggested fix:** Extract a plain tile/page model that owns stable page keys, configured identities, visibility, and current order, and provides the listing used by both CLI and UI. Have grid movement update that model and render its order. Separately move close-queue/running-app transitions into a small application controller, with notifications and presentation left in `Ui`. Keep the existing JSON keys and lifecycle semantics during staged extraction.

### ARCH-02 — Equivalent persistence operations expose incompatible failure contracts

**Priority:** medium. **Confidence:** high. **Effort:** S.

**Files and lines:** [src/order.rs](C:/Projects/VexTheater/src/order.rs:26) lines 26–36; [src/tiles.rs](C:/Projects/VexTheater/src/tiles.rs:37) lines 37–47; [src/window.rs](C:/Projects/VexTheater/src/window.rs:1394) lines 1394–1404, 1453–1475, 1491–1499, 2047–2050; [src/files.rs](C:/Projects/VexTheater/src/files.rs:21) lines 21–24, 45–49.

**Evidence:**

```rust
// src/order.rs:26–30
pub fn save(saved: &Saved) {
    if let Err(error) = files::write_json(&file(), saved) {
        log::warn!("cannot save the tile order: {error}");
    }
}

// src/window.rs:1474–1475
saved.insert(key, order);
order::save(&saved);

// src/window.rs:2047–2050
fn save_shown(&self) {
    if !self.shown.borrow().save() {
        self.notify("Couldn't save the change; it is lost at the next start");
    }
}
```

The common file layer returns `io::Result`. Above it, order saving discards the error and returns `()`, tile visibility saving returns `bool`, and order resetting returns `io::Result`. Reset checks that result before clearing in-memory state (`window.rs:1494–1499`).

**What goes wrong:** If writing `order.json` fails, the UI has already updated its in-memory order, and the save API gives it no way to report the failure. Dropping the tile continues normally, including the selection sound. On restart, the persisted order is used again. The corresponding add/remove operation explicitly warns about lost persistence, and reset aborts on failure. The inconsistency crosses the storage/UI boundary and produces different user feedback for closely related state changes. The error branch and callers establish this consequence; no filesystem failure was injected during this review.

**Suggested fix:** Return `io::Result<()>` consistently from order and visibility saves. Handle errors at the UI boundary with the same explicit persistence warning, and document whether an unsuccessful save retains the temporary in-memory change. Preserve reset's existing rule of changing nothing when deletion fails.

### ARCH-03 — Diagnostics use blocking process polling inside the event-driven application

**Priority:** medium. **Confidence:** high. **Effort:** M.

**Files and lines:** [src/main.rs](C:/Projects/VexTheater/src/main.rs:60) lines 60–61; [src/window.rs](C:/Projects/VexTheater/src/window.rs:237) lines 237–242, 1760–1764; [src/diagnose.rs](C:/Projects/VexTheater/src/diagnose.rs:11) lines 11, 38–67, 144–177; [src/launch.rs](C:/Projects/VexTheater/src/launch.rs:785) lines 785–805.

**Evidence:**

```text
GApplication command-line signal
  -> window::command_line("--diagnose")
     -> diagnose::run
        -> command(systemd-run) -> run_command
        -> command(systemctl)   -> run_command
           -> std::process::Command::spawn
           -> child.try_wait() + std::thread::sleep(20 ms)
```

```rust
// src/diagnose.rs:164–169
Ok(None) if Instant::now() < deadline => {
    std::thread::sleep(Duration::from_millis(20));
}
Ok(None) => {
    let _ = child.kill();
    let _ = child.wait();
```

In contrast, `Ui::close_then` schedules a local future, and `launch::run` awaits a GIO subprocess with `glib::future_with_timeout`.

**What goes wrong:** Diagnostics synchronously occupy the same application callback thread that serves the UI and launch/close futures. Slow helpers delay input, redraws, timers, and lifecycle progress. The two helper commands run sequentially, each with a five-second timeout budget, plus cleanup time. This behavior is also acknowledged in README's diagnostics section; it is a verified async-pattern inconsistency, not an undisclosed new regression or a measured ten-second freeze.

**Suggested fix:** Make the diagnostic command checks asynchronous with GIO subprocesses and GLib timeout futures, retain the command-line invocation until the report and exit status are ready, and return control to the main loop during waits. Factor shared process execution mechanics out of the launch-specific module if reused, with an explicit timeout/cleanup policy: the existing launch helper deliberately leaves timed-out commands running, whereas diagnostics kills them.

### ARCH-04 — Configuration diagnostics maintain a second handwritten schema

**Priority:** low. **Confidence:** high. **Effort:** M.

**Files and lines:** [src/config.rs](C:/Projects/VexTheater/src/config.rs:14) lines 14–35, 55–117, 135–141, 164–168, 223–240, 247–346, 682–690.

**Evidence:**

```rust
// src/config.rs:164–168 — parsing, then reparsing to explain a failure
let mut config: Self = serde_json::from_str(text).map_err(|error| match locate(text) {
    Some(problem) => anyhow::anyhow!(problem),
    None => error.into(),
})?;

// src/config.rs:109 — semantic validation
if raw.icon.is_none() && !matches!(kind, "desktop" | "flatpak") {
    return Err(format!("tile \"{name}\" has no icon"));
}

// src/config.rs:344 — the same rule in the diagnostic walker
if given("icon").is_none() && !matches!(*action, "desktop" | "flatpak") {
    return Some(format!("{path}.icon: missing{of}"));
}
```

`KEYS` mirrors `Config`'s serde fields; `ACTIONS` mirrors `RawTile` and its conversion; `BUILTINS` mirrors the enum's serialized variants. `locate_in_tile` repeats action-count, value-type, and icon rules already enforced during deserialization/conversion. The builtin consistency test iterates the separate `BUILTINS` list, confirming the two representations have to be kept in agreement.

**What goes wrong:** Schema knowledge is maintained twice in production code, so changing a field, action, or semantic rule requires synchronized edits to both parsing and error reporting. Invalid documents are also parsed again into a generic JSON tree to rediscover errors. This is verified duplication and a maintenance burden; this review does not claim the current accepted schema and diagnostic schema already disagree. Serde remains authoritative for acceptance.

**Suggested fix:** Return structured field/path errors from one validation flow and format them at the boundary. Centralize action metadata and semantic checks so diagnostic formatting consumes those results rather than reimplementing the rules. Retain the precise field-path messages and existing strict-schema tests.

## Other requested areas

- **Abandoned or unfinished code:** Searches of `src`, `nix`, `scripts`, and `Cargo.toml` found no `TODO`, `FIXME`, `todo!`, `unimplemented!`, `allow(dead_code)`, or `cfg(feature...)` matches. Every Rust source module is declared in `main.rs`. No abandoned module or never-enabled project feature was established; no finding is added on that basis.
- **Dependency use:** Each of the 12 direct runtime dependencies and the build dependency has a code use site. Examples include `env_logger` in `main.rs:42`, `glib_unix` in `main.rs:70`, `gio_unix` in `launch.rs:116`, serde/JSON in `files.rs:4–5, 22`, libc in `launch.rs:484`, and `glib_build_tools` in `build.rs:2`. No unused direct dependency was established by this source review.
- **Duplicated dependency versions:** The lockfile contains two versions each of `bitflags` (1.3.2/2.13.2), `syn` (2.0.119/3.0.9), and `system-deps` (7.0.8/9.0.1). Their recorded callers explain the split: `defmt` versus GLib/Cairo, `jiff-static` versus the other macro crates, and `libadwaita-sys` versus the other GTK-family sys crates, respectively. A lockfile can include optional or target-specific packages; duplication alone does not establish a defect in this project's compiled graph. No avoidable duplication finding is asserted.
- **Dependency age and maintenance:** No outdated/unmaintained claim is made from version numbers alone. The completed advisory audit is reported above; an exhaustive comparison against all upstream release and maintenance histories was not performed.
- **Naming and folders:** No independently consequential naming or folder inconsistency was established beyond the ownership/layering issue in ARCH-01. Existing helpers such as `files`, `tiles`, `order`, and the injectable lifecycle `Env` already provide useful boundaries.

## Final verification

The final Nix packaging stage ran `nix build path:. --no-link --print-build-logs`. Selected real output:

```text
vextheater>     Finished `release` profile [optimized] target(s) in 4m 32s
vextheater> Running phase: checkPhase
vextheater>     Finished `release` profile [optimized] target(s) in 34.87s
vextheater> test result: ok. 178 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.28s
vextheater> Running phase: installPhase
vextheater> Running phase: fixupPhase
Preflight passed.
```

The validation sequence exited 0 after preflight. The standalone pinned-Nix advisory audit also exited 0; the initial dev-shell audit invocation was unavailable, as recorded above. Passing checks establish build/test/lint/package health for the unchanged production source; they do not negate the architectural findings.

Final repository verification: `git diff --exit-code HEAD` produced no tracked changes, and `git status --short --untracked-files=all` showed only `?? CODEX_ANALYSIS_ARCH.md`. No source, lockfile, test, or build configuration was modified.
