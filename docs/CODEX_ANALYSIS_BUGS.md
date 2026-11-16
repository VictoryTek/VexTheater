# VexTheater bugs and code quality analysis

Reviewed 2026-11-12 at commit `ab074b25e5b98d63e8e42c0af92ac93083f8873e`.

## Scope and method

Read `CLAUDE.md` and `README.md` first. This review covers bugs and code quality. Only this report was authored; source files, tests, configuration, and lockfiles were not changed. The existing `CODEX_ANALYSIS_ARCH.md` was left untouched. Build/test tools use their normal external caches and temporary files, with Cargo output at `$HOME/.cache/vextheater/target`.

Reviewed module by module: launch/process lifecycle and desktop-entry parsing; GNOME settings, mode, and keybindings; window input, reload, navigation, and tile mutations; config parsing/migration; files, order, and tile persistence; grid, widgets, clock, sound, catalog, and webapp helpers. Also inspected the entry point, relevant tests, Cargo manifests, Nix package/module, build script, and preflight/CI wiring. The preceding architecture review read these same production modules at the unchanged commit; this pass retraced the paths cited below with a bug focus.

Not covered: live GTK input/rendering, real GNOME/systemd/Flatpak integration, streaming/DRM behavior, the external vexos-nix consumer, or a comprehensive review of third-party implementation internals. No desktop failures or crash scenarios below were reproduced on a physical device. Findings are verified source-level control/data-flow defects; their stated triggers are conditional, not claims of observed user incidents. Existing test execution is reported separately. No tests or reproduction files were added.

## Validation evidence

Commands run through WSL from `/mnt/c/Projects/VexTheater`, using the documented Nix environment and external `CARGO_TARGET_DIR`. A prior validation process could not be reattached after continuation, so its unavailable output is not counted as evidence for this pass.

| Check | Command | Observed result |
| --- | --- | --- |
| Build | `nix develop path:. -c cargo build --locked` | Passed, exit 0: ``Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.57s``. |
| Formatting | `nix develop path:. -c cargo fmt --check` (preflight step 1) | Passed without diagnostics. |
| Linter | `nix develop path:. -c cargo clippy --all-targets -- -D warnings` (preflight step 2) | Passed: ``Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.85s``, without lint diagnostics. |
| Test suite | `nix develop path:. -c cargo test` (preflight step 3) | Passed: `178 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.48s`. |
| Final project gate | `bash scripts/preflight.sh` | Passed, exit 0: `Preflight passed.` All six steps completed, including desktop-file validation, AppStream validation, and the Nix release package build. |
| Dependency audit | `nix shell github:NixOS/nixpkgs/cd4ea8d57de16f4810bc5f14207bac45fbf19f67#cargo-audit -c cargo audit --file Cargo.lock` | Passed, exit 0; 1,490 advisories loaded, 106 crate dependencies scanned, no vulnerability/advisory diagnostics. |

Actual audit output:

```text
Fetching advisory database from `https://github.com/RustSec/advisory-db.git`
Loaded 1490 security advisories (from /home/nimda/.cargo/advisory-db)
Updating crates.io index
Scanning Cargo.lock for vulnerabilities (106 crate dependencies)
```

The audit covers Cargo.lock advisories, not all native Nix libraries or installed browsers. No vulnerable-dependency finding is established by this output. Build and test success do not validate the untested transitions described below.

The Nix package build reported ``Finished `release` profile [optimized] target(s) in 2m 20s``. Its separate release-mode test run reported `178 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.36s`, followed by successful installation/wrapping and `Preflight passed.`

AppStream succeeded with one informational notice:

```text
I: io.github.vextheater:~: content-rating-missing

✔ Validation was successful: infos: 1
```

This notice is validation evidence, not a bug finding. Final verification found no tracked-file diff, and the existing architecture report's SHA-256 remained unchanged. Only this report was written during the bug review; its validation section was completed after the commands returned, with no intervening source changes.

## Findings

Seven verified findings, all medium priority and high confidence. No high-priority security issue was established. Effort: S = localized fix; M = coordinated behavior/persistence change; L = substantial redesign. Suggested regression tests are part of each fix, not separately counted findings.

### BUG-01 — Home always skips the automatic configuration reload

**Priority:** medium. **Confidence:** high. **Effort:** S.

**Files and lines:** [src/window.rs:450](C:/Projects/VexTheater/src/window.rs:450) lines 450–463, 862–879, 965–979, 1755–1793.

**Evidence / call path:**

```text
Ui::home
  -> close_then(Home)
     -> CloseQueue::submit -> closing.replace(true)
     -> schedule asynchronous stop_all
  -> show -> reload(false)
     -> if closes.is_busy() { return; }

close completion -> closes.finish() -> Some(Request::Home) => {}
```

**What goes wrong:** In an already initialized launcher, every Home request marks the close queue busy before `show()` attempts to reload. The guard silently rejects that reload, and the eventual Home completion never retries it. This also happens when no application is running, because the asynchronous close still gets queued. A configuration changed while the launcher is hidden therefore remains stale after Home, contrary to the documented reload-on-show behavior. Repeated Home presses repeat the same sequence.

**Suggested fix:** Remember the automatic reload request and perform it once closing has completed and the effective request is Home. Preserve the guard against rebuilding during a close, and restore focus using the rebuilt tile identities. Add a transition test for changed config → Home → close completion, both with and without a running app; queue-only tests do not exercise the reload guard.

### BUG-02 — Back auto-repeat dismisses the reset prompt it just opened

**Priority:** medium. **Confidence:** high. **Effort:** S.

**Files and lines:** [src/window.rs:724](C:/Projects/VexTheater/src/window.rs:724) lines 724–729, 793–813, 1212–1272, 1280–1306, 2092–2098.

**Evidence / call path:**

```text
held Back on Main -> arrange_key -> back_pressed -> start RESET_HOLD timer
timer -> open(Confirm(ResetOrder))
next Back repeat -> arrange_key: on_main_page() is false -> Proceed
bubble key handler -> ui.back() -> pop confirmation history -> Main
```

**What goes wrong:** With key repeat enabled, holding Back beyond the two-second threshold opens the confirmation and then dismisses it on the next repeat. `back_down` suppresses repeats only while `back_pressed()` is reached; after the page changes, its caller no longer routes repeats there. The user must release within the repeat interval to keep the prompt visible. Enter already has `enter_swallow` to handle its corresponding long-press transition.

**Suggested fix:** Consume the rest of a Back press, including repeats after the timer changes pages, until release. Reset the held-key state appropriately on cancellation/focus loss. Test press → timer → repeat → release and assert that the confirmation remains open; a separate new Back press should dismiss it.

### BUG-03 — An older GNOME backup prevents restoration of the Super key

**Priority:** medium. **Confidence:** high. **Effort:** M.

**Files and lines:** [src/gnome.rs:39](C:/Projects/VexTheater/src/gnome.rs:39) lines 39–55, 80–98, 121–141, 177–184, 244–262.

**Evidence:**

```rust
if !path.exists() {
    if let Err(error) = save(&path, &current_values()) { /* ... return */ }
}
// All current KEYS are then disabled, including overlay-key.
Kind::Text => settings.set_string(key, ""),
```

Restore skips entries absent from the backup (`values.get(...)`, lines 125–127). The existing `a_file_from_an_older_launcher_still_loads` fixture contains only the two boolean settings and explicitly asserts that `org.gnome.mutter overlay-key` is absent.

**What goes wrong:** If that supported older-format backup survives a crash and the newer launcher starts, the file-exists check prevents saving the current Super-key value. The launcher nevertheless clears `overlay-key`. Returning to desktop skips restoring it and removes the backup, so the key remains disabled. A later theater entry then saves the already empty value. Deserialization compatibility alone does not preserve settings across this upgrade path.

**Suggested fix:** Load and validate an existing snapshot, merge current values for newly managed keys without overwriting saved originals, and durably save the augmented snapshot before disabling those keys. If saving fails, leave the uncovered settings unchanged. Extend the old-format test through theater entry and restoration and assert the original overlay-key value returns.

### BUG-04 — GNOME restoration deletes the recovery snapshot before applying it

**Priority:** medium. **Confidence:** high. **Effort:** M.

**Files and lines:** [src/gnome.rs:121](C:/Projects/VexTheater/src/gnome.rs:121) lines 121–141, 177–184, 222–241; [src/mode.rs:28](C:/Projects/VexTheater/src/mode.rs:28) lines 28–36.

**Evidence / call path:**

```text
enter_desktop / exit -> gnome::restore
  -> take(snapshot)
     -> read_json(snapshot)
     -> remove(snapshot)
  -> look up schemas and saved values
  -> set_boolean / set_string (errors only logged)
  -> Settings::sync()
```

**What goes wrong:** The only recovery copy is deleted before any saved setting is restored. A failed setter, unavailable schema, or interruption between deletion and completion leaves theater-mode values active without their original values available for retry. `take()` also attempts deletion when reading the file failed. Setter failure is an API-supported condition: GSettings reports failure for a non-writable key. See [GIO's setter contract](https://docs.gtk.org/gio/method.Settings.set_boolean.html).

**Suggested fix:** Read the snapshot without removing it, restore and sync successfully first, and retain unresolved original values when any restoration fails. Preserve unreadable recovery data for retry/diagnosis. The existing round-trip test only verifies save/read/delete; add failure-injection tests asserting that a failed restore retains the snapshot and a retry restores the original values.

### BUG-05 — Flatpak SIGTERM state is not durable during the close operation

**Priority:** medium. **Confidence:** high. **Effort:** M.

**Files and lines:** [src/launch.rs:182](C:/Projects/VexTheater/src/launch.rs:182) lines 182–204, 248–251, 269–319, 387–455, 688–695, 1457–1477.

**Evidence / call path:**

```text
start_flatpak -> persist record with termed = false
stop_all_with -> load records -> spawn per-record stop_flatpak tasks
stop_flatpak -> send SIGTERM -> local record.termed = true -> await exit
stop_all_with -> await scope/process/tasks -> save_flatpaks(still_running)
```

**What goes wrong:** After SIGTERM is delivered, the durable record still says `termed: false` until the entire close finishes. If the launcher terminates in that interval and the app remains alive, a restarted launcher loads the stale record and sends SIGTERM again on its next close. The process identity checks still match the same process and do not prevent this. A final save failure has the same stale-state risk because `save_flatpaks()` only logs errors. This violates the explicit one-SIGTERM invariant; the code documents that a second SIGTERM can interrupt a Chromium app's clean shutdown and lose its session.

**Suggested fix:** Serialize durable termination intent per instance before sending the signal, and treat uncertain delivery after a restart as already attempted to preserve at-most-once behavior. Coordinate writes across concurrent Flatpak-close tasks rather than having each overwrite a stale whole-file snapshot. Propagate persistence failures. Extend the lifecycle harness to resume from the persisted state captured immediately after signaling; the existing two-close test waits for the first close to finish, so it misses this window.

### BUG-06 — Desktop launch discards required metadata field-code arguments

**Priority:** medium. **Confidence:** high. **Effort:** M.

**Files and lines:** [src/launch.rs:115](C:/Projects/VexTheater/src/launch.rs:115) lines 115–124, 840–900, 1124–1127.

**Evidence:**

```rust
let command = desktop_command(&exec.to_string_lossy())?;
// exec_argv:
('%', Some(code)) if "fFuUickdDnNvm".contains(*code) => {
    chars.next();
}
```

**What goes wrong:** `%i`, `%c`, and `%k` are deleted along with the file/URL codes. These metadata codes have values even when the launcher supplies no files: icon arguments, translated application name, and desktop-file location. For example, a valid `Exec=app --title %c --desktop-file %k` becomes `app --title --desktop-file`, losing both option values. Apps that use these codes receive an incorrect argument list and may fail to launch. The [Desktop Entry specification](https://specifications.freedesktop.org/desktop-entry/latest/exec-variables.html) defines these expansions separately from the absent-file behavior of `%f`, `%F`, `%u`, and `%U`.

**Suggested fix:** Pass desktop-entry metadata into argument construction and expand these codes according to the specification, preserving spaces inside each resulting argument and shell-quoting only after expansion. Keep no-file handling for file/URL codes. Add tests using application names and desktop paths containing spaces, present/absent icons, and option values; the existing field-code test covers `%U`, `%f`, and `%%` only.

### BUG-07 — Saving a tile move can fail without telling the user

**Priority:** medium. **Confidence:** high. **Effort:** S.

**Files and lines:** [src/window.rs:1394](C:/Projects/VexTheater/src/window.rs:1394) lines 1394–1404, 1453–1475, 1491–1495; [src/order.rs:26](C:/Projects/VexTheater/src/order.rs:26) lines 26–30; [src/files.rs:64](C:/Projects/VexTheater/src/files.rs:64) lines 64–81.

**Evidence / call path:**

```text
drop_picked -> save_order
  -> update in-memory saved_order
  -> order::save -> files::write_json -> I/O failure -> log warning -> return ()
drop_picked -> focus tile + play chosen sound + update hint
```

**What goes wrong:** A failed write/rename of `order.json` leaves the UI and in-memory order updated while disk retains the previous order. The caller cannot detect failure and completes the normal drop interaction, so the user's arrangement disappears on restart without an on-screen explanation. This is inconsistent with reset, which reports failure and preserves the old arrangement. The persistence-interface mismatch also appears in the architecture report; this finding describes its concrete data-loss/user-feedback consequence.

**Suggested fix:** Return the save result to `Ui`, display a persistence error, and either revert to the last durable order or clearly retain an unsaved state with retry. Add a save-failure test for the mutation/caller path, including that success feedback is not shown for a failed save. Low-level atomic-write tests alone cannot establish this behavior.
