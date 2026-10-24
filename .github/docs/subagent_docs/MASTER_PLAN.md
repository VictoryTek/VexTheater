# VexTheater: master plan

Date: 2026-10-24 · Commit: `d1ea354` · Sources: `ANALYSIS_BUGS.md` (UI-, CFG-, LC-, ORD-,
CLK-, T-), `ANALYSIS_ARCH.md` (A1–A11), `ANALYSIS_FEATURES.md` (FEAT-01–16). All IDs are
the originals.

## How this list is ordered

1. Security bugs, then data/state-loss bugs.
2. Then high, medium, and low priority.
3. Within each tier, smallest effort first.

Related items are grouped into batches that can go into one change. A batch sits in the
tier of its highest-priority item. The tier column follows CLAUDE.md: a change to the
config schema or module interface, or a new or upgraded dependency, is Tier 3.

Baseline (from the analyses, all at `d1ea354`): build OK, 63 tests pass, clippy
`-D warnings` clean, `cargo fmt --check` clean, `cargo audit` reports 0 vulnerabilities,
`cargo machete` finds no unused dependencies.

## Duplicates merged

| Kept together | Why |
|---|---|
| LC-5 = A6 | Same defect: `load_flatpaks` drops a corrupt or unreadable record file without logging it. |
| T-3 ⊂ A4 | T-3 (`start_flatpak` bypasses `Env`) is one part of A4 (the `Env` test seam covers only the stop half). |
| T-4 ↔ UI-1 | T-4 is the test that would have caught UI-1. Fix them together. |
| T-2 ↔ LC-4 | The give-up test from T-2 is where LC-4's combined result gets tested. |
| ORD-6 ↔ A7 | `order::clear` is one of the state-file operations A7 wants to put in one place. |

## Re-verification of high-priority items

Each claim was checked against the code at `d1ea354`.

| ID | Original | Verdict | Evidence |
|---|---|---|---|
| FEAT-01 | high | **Holds, stays high** | `window.rs:94-124`: once a window exists, `ui()` only calls `present()` and returns `None`. The config is never read again, the `StatusPage` has no buttons or key handling, and `module.nix:63` has `Restart = "on-failure"`, so Alt+F4 (a clean exit) does not bring the launcher back. |
| FEAT-02 | high | **Holds, stays high** | `is_enter` (`window.rs:1182`), `is_navigation` (`:1188`) and the Escape arms (`:357`, `:611`, `:626`) match no `Back`, `Select` or `HomePage`. All three constants exist in the locked gdk4 0.9.6 (`keys.rs:605, 1573, 2224`). On this machine GNOME does not grab `XF86HomePage` (`home-static` is `XF86Explorer`, `www-static` is `XF86WWW`). What a given remote sends is still unverified; check with `wev` first. |
| FEAT-03 | high | **Holds, stays high** | `config.rs` `load`/`parse`/`validate` use no GTK. `load_stylesheet`, the only call that needs a display, runs in `connect_startup` (`main.rs:31-35`), so a flag handled before `application.run()` works headless. `module.nix:46` writes `json.generate` output unchecked. Caveat: running the binary at build time does not work when cross-compiling. |
| FEAT-04 | high | **Downgraded to medium** | The mechanism holds: `close_then` (`window.rs:1016-1054`) stops the running app before `launch::start` (`launch.rs:101-119`) finds out that a desktop id is missing. The impact does not hold up. The launcher is normally in front only when no app is running, because home closes every app and `app_exited` brings the launcher back. ANALYSIS_FEATURES itself drops "resume the running app" for that reason. So the "user loses what they were watching" case is rare. What is left is a nicer "Not installed" tile. |

Medium bugs that this plan puts at the top of their tier were also checked:

- **UI-1 holds.** At 1920 px, 13 tiles give a room of 1728 − 480 = 1248, and 1248 / 13 = 96, which clamps up to 100. The row is then 13×100 + 12×40 = 1780 px wide, more than the 1728 px available. README line 51 still says "a row of any length stays on screen".
- **CFG-2 holds.** `config.rs` has `#[serde(flatten)]` on the action and no `deny_unknown_fields` anywhere. The scratch-crate run in ANALYSIS_BUGS was not repeated.
- **T-1 holds.** The stub's `stop)` arm always exits 0 (`launch.rs:1093`).
- **T-2 holds.** No test record has `kill_attempts` other than 0, and no test matches `GaveUp`.
- **LC-4 holds.** `launch.rs:277-283` returns `GaveUp` before it looks at `closed` or `still_running`.
- **LC-5/A6 holds.** `launch.rs:639-644` turns any read or parse error into `.ok()`, and `:275` then overwrites the file.

Nothing was dropped. The only security conclusion is that there are **no security
findings**. `sh -c` running config commands is by design (a root-owned `/etc` file, or the
same user's `$VEXTHEATER_CONFIG`).

---

## Tier 0: Security

- None found (`cargo audit` clean, and the code review found no security issues).

## Tier 1: Data/state-loss bugs

### Batch 1.1: Don't silently throw away the Flatpak record (Tier 2, S)
- [x] **LC-5 / A6**: in `load_flatpaks` (`launch.rs:639`), stay silent on `NotFound` and `log::warn!` on any other read error or a parse error, as `order::parse` does. The priority is low, but it is listed first because the next `save_flatpaks` overwrites the lost record, and those Flatpak apps can then never be closed by home.
  → verify: a unit test with a garbage `flatpak-apps.json` gets an empty list and a logged warning; `cargo test`.

## Tier 2: High priority

### Batch 2.1: A way out of the config error screen (Tier 2, S)
- [x] **FEAT-01**: add **Retry** and **Desktop Mode** buttons to the error `StatusPage`, focus Retry, and hide the idle pointer. Have plain `vextheater` and `--home` try to load the config again instead of only calling `present()`.
  → verify: start with a broken `VEXTHEATER_CONFIG`, fix the file, and press Retry or run `vextheater --home`: the launcher appears. Take screenshots with cage and grim at 1920x1080 and at 3840x2160 with scale 2.
  - Land this before Batch 3.6 (CFG-2), which makes more configs fail to load.

### Batch 2.2: TV-remote key names (Tier 2, S)
- [x] **FEAT-02**: treat `Key::Back` like Escape/BackSpace (at all three sites), `Key::Select` like Enter (`is_enter`), and `Key::HomePage` as an in-launcher home (`main_row()`). Before choosing the list, check what the target remote sends with `wev`.
  → verify: extend the key test at `window.rs:1392-1395`; on the device, check back, select and home.
  - Done in code and checked in cage with `wtype` (`XF86Back`, `Select`, `XF86HomePage`). Still open: what the real remote sends (`wev`) and whether GNOME grabs `XF86HomePage` on the device. Select needed its own activation arm, because GTK buttons only activate on Enter and space.

### Batch 2.3: `--check-config` and build-time validation (Tier 3, S–M)
- [x] **FEAT-03**: handle `vextheater --check-config [FILE]` in `main()` before `application.run()`: print the error and exit 1, or exit 0. Then have `module.nix` run it on the generated JSON in a `runCommand`, so a bad config fails `nixos-rebuild`. Coordinate with vexos-nix, which will now see build failures it did not see before. Decide what to do when cross-compiling (skip the check when `buildPlatform != hostPlatform`).
  → verify: exit codes on a valid file, an invalid file and a missing file, with no display (`env -u WAYLAND_DISPLAY -u DISPLAY`); `nix build path:.`; a NixOS VM or `nix eval` test of the module with a bad `settings` fails.

## Tier 3: Medium priority

### Batch 3.1: Lifecycle test gaps (Tier 2, S)
- [x] **T-1**: add `stop_fail` and `stop_hang` switches to the `systemctl` stub. Test that `kill --signal=SIGKILL <scope>` is issued, and that the result is `Ok` or `Timeout` depending on `units.txt`.
- [x] **T-2**: test a record with `kill_attempts: MAX_KILL_ATTEMPTS - 1`: the result is `Err(GaveUp([APP]))` and `records.json` ends up as `[]`.
- [x] **LC-4** (low, rides along): carry both facts, e.g. `GaveUp { apps, others_still_running }`, and show both toasts in `close_then` (`window.rs:1031-1037`). Add a T-2 variant with a stuck second app.
  → verify: `cargo test`; the new tests fail before the LC-4 change and pass after it.

### Batch 3.2: Small remote-UX features (Tier 2, S each)
- [x] **FEAT-05**: remember the button (and page) that launched the running app, and focus it on `home()` or `app_exited`. For a submenu tile, focus the main-row tile that opens the submenu. Keep this in memory only.
- [x] **FEAT-06**: when the main row is empty, show a "No tiles configured" message, focus the power button, and leave out the "Enter Open" hint.
- [x] **FEAT-07**: add `--desktop` and `--toggle` verbs to `command_line` (`window.rs:49-79`) and to the usage text and the README.
  → verify: run each with `VEXTHEATER_CONFIG` and take screenshots (FEAT-06), forward each verb from a second process (FEAT-07), `cargo test`.

### Batch 3.3: Follow GNOME's event-sounds setting (Tier 2, S)
- [x] **FEAT-09**: in `Sounds`, play only while `org.gnome.desktop.sound event-sounds` is true, read through a `gio::Settings` kept in the struct, as in `clock.rs::uses_24h_clock`.
  → verify: toggle the key with `gsettings` while the launcher runs; the ticks stop and start again.

### Batch 3.4: Long rows fit the screen (Tier 2, S–M)
- [x] **UI-1**: make `n*size + (n-1)*TILE_SPACING <= width - 2*PAGE_MARGIN` hold for every count. Either lower or drop `TILE_MIN` (scaling the spacing too), or use a `ScrolledWindow` that follows focus and correct README line 51. The existing assertion `tile_size(400, 6) == TILE_MIN` (`window.rs:1418`) will change.
- [x] **T-4**: loop over `count in 1..=30` at the target widths and assert that the row fits. Write it first; it fails today from 13 tiles on.
  → verify: `cargo test`; screenshot 14 tiles at 1920x1080 and at 3840x2160 with scale 2.

### Batch 3.5: Test seam for the start half of the lifecycle (Tier 2, S–M)
- [x] **A4**: add `start_with(action, &Env)`, give `Env` a `systemd_run` field, and pass `Env` into `early_exit`.
- [x] **T-3**: make `start_flatpak` write `env.records`. Replace `Lab::launched_flatpak` with a real `start_with(&Action::Flatpak(..), &env)` against the stub.
  → verify: `cargo test`, the same 63 tests plus new start→stop tests. If this is not done, narrow the README sentence about "end to end" to the close lifecycle.

### Batch 3.6: Tile schema rework, one vexos-nix coordination (Tier 3, M)
- [x] **CFG-2**: deserialize each tile through an intermediate struct with one `Option` per action and `deny_unknown_fields`. Check in `validate()` that exactly one action is set. Add `deny_unknown_fields` to `Config`. Before landing, check that `modules/htpc-theater.nix` sends no extra keys, or every launcher refuses its config.
- [x] **FEAT-11** (same struct, same contract change): make `icon` optional. A `desktop` tile falls back to `DesktopAppInfo::icon()` and a `flatpak` tile to its app id; any other tile without an icon is a validation error.
  → verify: config tests for two actions, a misspelled key, a missing icon on each action kind, and the README example; update README and the module docs. Needs Batches 2.1 and 2.3 first.

### Batch 3.7: Page and command types (Tier 3 because it touches `config.rs`, M)
- [ ] **A2**: add `enum Page { Main, Submenu(String), Power, Confirm(Builtin) }` with one `key()`/`from_key()` pair, use an exhaustive confirm mapping with no `_` arm, and have `tile()` match on `Page`. Keep the key strings byte-for-byte the same so existing `order.json` files still load.
- [ ] **A3**: reduce `config::Builtin` to the three schema values, and add a UI-side `enum Command { Config(Action), Cancel, Arrange, ResetOrder }` in `window.rs`. The JSON contract does not change.
  → verify: `cargo test` (the `skip_deserializing` tests at `config.rs:214-228` become type-level guarantees, so remove or rewrite them); an `order.json` saved before the change still loads.

### Batch 3.8: Mark tiles whose app is not installed (Tier 2, M) (downgraded from high)
- [x] **FEAT-04**: when building the pages and on `show()`, check `desktop` tiles with `DesktopAppInfo::new(id)` and `flatpak` tiles with `"<id>.desktop"`. A tile that fails gets the `.unavailable` CSS class and the description "Not installed", and choosing it shows a toast without calling `close_then`. `command` tiles are not checked.
  → verify: configure a desktop tile that does not exist and take screenshots; `cargo test`.

### Batch 3.9: Turn off the Super-key overview in theater mode (Tier 2, M)
- [ ] **FEAT-08**: add `org.gnome.mutter overlay-key` to `gnome::KEYS`. `Values` must then hold variant text instead of `bool`, and old files holding `true`/`false` must still load. Update the README's restore-by-hand section.
  → verify: unit tests for reading an old-format file; on the device, Super does nothing in theater mode and works again after desktop mode, a clean exit, or a crash followed by `--resume`.

### Batch 3.10: Reload the config (Tier 2, M)
- [ ] **FEAT-10**: `vextheater --reload` rebuilds the pages in the existing window and keeps `running`, the close queue and the toasts. When loading fails, keep the old tiles and show a toast. Also reload on `show()` when the file has changed. Don't rebuild while a close is running or a tile is picked up. This is easier after A1 (Batch 3.13).
  → verify: edit the file while the launcher runs, then run `--reload`; an app that is running stays tracked (home still closes it).

### Batch 3.11: Home key from one module option (Tier 3, M)
- [ ] **FEAT-12**: add a `programs.vextheater.homeKey` option that sets `settings.homeKey` and a GNOME custom keybinding (through `programs.dconf.profiles.user.databases`) running `vextheater --home`. The new binding must be merged with the user's existing custom keybindings, not replace them. Coordinate with vexos-nix, which can then drop its own binding.
  → verify: a NixOS VM test or a check of the generated dconf database; `nix build path:.`.

### Batch 3.12: Upgrade the gtk-rs stack (Tier 3, M)
- [ ] **A5**: upgrade gtk4, libadwaita, glib, gio and glib-build-tools together, reading the gtk-rs 0.10 and 0.11 release notes first. Optionally use `gtk::glib` and `gtk::gio` and drop the direct glib/gio dependencies. Check that the nixpkgs GTK and libadwaita versions meet the new crates' minimum versions.
  → verify: build, `cargo test`, clippy, `cargo audit`, `nix build path:.`, and screenshots at both resolutions.

### Batch 3.13: Split `window.rs` (Tier 2, M–L), do after 3.7
- [ ] **A10** (low, prerequisite): move `label` and `hint` into a `widgets.rs` to break the `clock` ↔ `window` cycle.
- [ ] **A9** (low, prerequisite): have `mode.rs` own the transitions (`enter_theater`, `enter_desktop`, `exit`, each also making the `gnome` call), and call them from `window.rs` and `main.rs`.
- [ ] **A1**: move arranging into `arrange.rs` and the app lifecycle (`CloseQueue`, `running`, `close_then`, `start`, `app_exited`) into its own controller that reports through a callback. `Ui` stays the composer.
  → verify: `cargo test` passes before and after each step; clippy pedantic no longer flags `too_many_lines` on `Ui::build`; screenshots show no change.

## Tier 4: Low priority

### Batch 4.1: Small isolated fixes (Tier 2, S each)
- [ ] **CLK-7**: schedule the next clock update at the next minute boundary with `timeout_add_local_once`, instead of the free-running 5 s timer (`clock.rs`).
- [ ] **A11**: delete `rust-toolchain.toml`, or pin it to nixpkgs' rustc 1.95.0 with clippy and rustfmt and note that the flake is the authority.
  → verify: `cargo test`; preflight.

### Batch 4.2: State-file helpers (Tier 2, S)
- [ ] **A7**: add `state_file`/`runtime_file`, `write_json` and a `read_json` that logs, all in `files.rs`, and move `mode`, `order`, `gnome` and `launch` over to them. The logging that Batch 1.1 adds moves into `read_json`.
- [ ] **ORD-6**: `order::clear()` ignores only `NotFound` and logs or returns any other error, and `reset_order` shows a failure toast instead of "Tile order reset".
  → verify: unit tests for each helper; a test where `clear` fails on a read-only directory.

### Batch 4.3: Config migration cleanup (Tier 2, S)
- [ ] **A8**: have `drop_power_tiles` also remove submenus that no remaining tile opens (or have `Ui::build` build only reachable ones), and run the migration inside `parse`, so `parse` and `load` return the same thing. The JSON contract does not change.
  → verify: the existing migration tests (`config.rs:232-256`) without the manual `drop_power_tiles()` call, plus an old-style config whose "Power" submenu is no longer built.

### Batch 4.4: `--print-order` (Tier 2, S)
- [ ] **FEAT-15**: print each page's tile names in the order shown on screen (`order::arrange` over the configured names). It needs only `Config::load_default` and `order::load`.
  → verify: unit test of the output; run it against a saved `order.json`.

### Batch 4.5: Arranging across pages (Tier 2, S then M)
- [ ] **UI-3**: in the power button's click handler (`window.rs:335-341`), call `ui.stop_arranging()` (or return while `picked` is `Some`) before `ui.open(POWER_PAGE, …)`. This also fixes the mild form: ↑ then Enter while arranging no longer reaches the power menu with `arranging` still set.
- [ ] **FEAT-14**: while arranging with nothing picked up, Down on a tile that opens a submenu opens it, still arranging, and Back returns to the main row. Update `ARRANGING_HINT` and the README. Building this on top of UI-3 makes it settle how arranging state behaves across pages.
  → verify: with the pointer shown and a tile picked up, click power and check that the menu works and the hint is gone; arrange a submenu with only arrows, Enter and Back; screenshots.

### Batch 4.6: `--diagnose` (Tier 2, M)
- [ ] **FEAT-13**: have the primary instance report pass/fail for the GNOME schemas and `XDG_DATA_DIRS`, `systemd-run --user`, `systemctl --user list-units`, `flatpak` on `PATH` (if any tile uses it), and whether the background and icon files exist. Also log one `warn!` from `gnome::theater_mode` when a schema is missing.
  → verify: run it on the device, and once with `XDG_DATA_DIRS` cleared.

### Batch 4.7: Gamepad navigation (Tier 3, L): decide before building
- [ ] **FEAT-16**: first compare a uinput mapper in vexos-nix (no change here; likely cheaper) with libmanette plus a Rust binding (a new native dependency; check its maintenance status). Build only the option that is chosen.
