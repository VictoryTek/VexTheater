# VexTheater — architecture and structure analysis

Date: 2026-10-24 · Commit: `d1ea354` (clean tree) · Scope: architecture and structure only
(no correctness/security review).

## Evidence from tooling

All run inside `nix develop path:.` with `CARGO_TARGET_DIR=$HOME/.cache/vextheater/target`.

| Check | Result |
|---|---|
| `cargo build` | OK (`Finished dev profile … in 47.57s`) |
| `cargo test` | `test result: ok. 63 passed; 0 failed; 0 ignored` |
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets -- -D warnings` (the preflight gate) | clean |
| `cargo clippy --all-targets -- -W clippy::pedantic` (informational) | 13 warnings in the bin, 22 incl. tests; all style-level. Relevant here: `this function has too many lines (184/100)` at `src/window.rs:225` (`Ui::build`) |
| `cargo audit` 0.22.1 (via `nix shell nixpkgs#cargo-audit`, advisory DB in scratch) | 1352 advisories loaded, 111 crates scanned, **no vulnerabilities or warnings reported** |
| `cargo machete` 0.9.2 | "didn't find any unused dependencies" |
| `cargo search` (latest on crates.io) | gtk4 0.11.5, libadwaita 0.9.2, glib 0.22.10, gio 0.22.10, glib-build-tools 0.22.8, env_logger 0.11.11 |
| `grep TODO/FIXME/XXX/HACK/todo!/unimplemented!/#[allow` over `src/ build.rs nix/ scripts/` | no matches |
| CSS classes used in `src/*.rs` vs selectors in `data/style.css` | every class used has a rule; no unused selectors |

## Coverage

Read in full: `src/main.rs`, `config.rs`, `window.rs`, `mode.rs`, `order.rs`, `gnome.rs`,
`files.rs`, `sound.rs`, `clock.rs`, `launch.rs` (non-test code lines 1–862 and the
lifecycle test harness 1067–1275), `Cargo.toml`, `build.rs`, `flake.nix`,
`nix/package.nix`, `nix/module.nix`, `scripts/preflight.sh`, CI workflow, gresource
manifest, `rust-toolchain.toml`, README.

Not covered in depth: the individual test cases in `launch.rs` (864–1066, 1275–1576),
`data/style.css` beyond selector names, `scripts/make-sounds.py` beyond its header, the
desktop/metainfo files (validated by preflight). The vexos-nix consumer module was not
available and was not checked.

## Summary

| ID | Priority | Confidence | Area | Title | Effort |
|---|---|---|---|---|---|
| A1 | medium | high | window.rs | God module: UI, navigation, arranging, app lifecycle and mode transitions in one 1473-line file / one 20-field struct | M–L |
| A2 | medium | high | window.rs | Pages identified by ad-hoc strings, duplicated literals, catch-all fallbacks | S–M |
| A3 | medium | high | config.rs ↔ window.rs | The schema type `Builtin` carries launcher-internal commands | M |
| A4 | medium | high | launch.rs | Test seam `Env` covers only the stop half of the lifecycle | S–M |
| A5 | medium | high | Cargo.toml | gtk-rs stack two release cycles behind; glib/gio pinned separately from gtk4 | M |
| A6 | low | high | launch.rs vs others | Corrupt state files handled four different ways; the Flatpak record fails silently | S |
| A7 | low | high | mode/order/gnome/launch | Same "serialize → write_atomic → log" and state-path code written four times | S |
| A8 | low | high | config.rs → window.rs | Power-tile migration leaves an unreachable submenu page built; `parse` and `load` differ | S |
| A9 | low | high | window.rs / main.rs | Mode transition invariant maintained at three call sites in two modules | S |
| A10 | low | high | clock.rs ↔ window.rs | Dependency cycle: leaf module imports a helper from the UI module | S |
| A11 | low | high | rust-toolchain.toml | Inert toolchain file; second source of truth for the compiler | S |

Category 4 (half-implemented / abandoned code): no stubs, markers, unused modules,
unused dependencies or never-flipped flags were found. The only leftover-shaped code is
the deliberate migration shim in A8.

---

## Findings

### A1 — `window.rs` is a god module

- **Priority:** medium · **Confidence:** high · **Effort:** M–L
- **Files:** `src/window.rs:183-222` (struct), `225-438` (`Ui::build`), `440-465`,
  `587-857`, `941-1097`
- **Evidence:** the file is 1473 lines (next largest non-test module is `config.rs` at
  268). `struct Ui` has 20 fields spanning unrelated concerns:

  ```rust
  struct Ui {
      config: Config, window: …, toasts: …,
      running: RefCell<Option<launch::Running>>,   // app lifecycle
      closes: CloseQueue<Request>,                  // app lifecycle
      stack, own_pages, first_tiles, rows, history, // page navigation
      pointer_visible, sounds, navigating, clock,   // input / chrome
      arrangeable, saved_order, picked, arranging,  // tile arranging
      enter_down, enter_timer, arrange_hint,        // tile arranging
  }
  ```

  The module also owns the app-start/close orchestration (`close_then`, `start`,
  `app_exited`, `power`, 1002-1097), the theater/desktop transitions (`show`,
  `desktop_mode`, 440-465) and the full arrange feature (587-857). Clippy pedantic
  flags `Ui::build` at 184 lines.
- **What goes wrong:** every feature lands in the same struct and file (the last three
  commits — power menu, sounds, arranging — all grew `Ui`). The lifecycle orchestration
  (`CloseQueue` + `running` + `StopError` handling) is only testable through GTK
  widgets, which is why only `CloseQueue` itself has unit tests and `close_then`'s
  `StopError::Unknown` retry path has none. Borrow discipline across 9 `RefCell`s in
  one struct is easy to break (e.g. `reset_order` re-borrows `arrangeable` inside a
  loop, 833-840).
- **Suggested fix:** split along the existing seams, keeping `Ui` as the composer:
  - `arrange.rs`: `Arrangeable`, `Picked`, `arranging/enter_down/enter_timer`,
    `pick_up/move_picked/drop_picked/cancel_picked/save_order/reset_order`.
  - an app controller (e.g. in `launch.rs` or `apps.rs`): `CloseQueue`, `running`,
    `close_then/start/app_exited`, reporting failures through a callback instead of
    `self.toasts`.
  - move the mode transitions out (see A9).

### A2 — Pages identified by ad-hoc strings

- **Priority:** medium · **Confidence:** high · **Effort:** S–M
- **Files:** `src/window.rs:16-18, 930-939, 944, 954-958, 1204-1206, 1273, 1286, 1299, 1316-1323`
- **Evidence:**

  ```rust
  const MAIN_PAGE: &str = "main";  const POWER_PAGE: &str = "power";
  const CONFIRM_PREFIX: &str = "confirm:";
  // 931 — literal duplicates submenu_page() at 1205
  let tiles = match page.strip_prefix("submenu:") {
      Some(name) => &self.config.submenus[name],
      None => match self.own_pages.iter().find(|own| own.key == page) {
          Some(own) => &own.tiles,
          None => &self.config.tiles,          // any unknown key → main row
      },
  };
  // 1273/1286/1299 — literals duplicate confirm_page()'s mapping
  key: "confirm:reset", … key: "confirm:shutdown", … key: "confirm:restart",
  // 1317
  let action = match builtin {
      Builtin::Restart => "restart", Builtin::ResetOrder => "reset",
      _ => "shutdown",                         // catch-all
  };
  ```

  The same keys are also the on-disk format: `order.json` is keyed by page name
  (`window.rs:515, 804`), so `"main"` and `"submenu:<name>"` are persisted.
- **What goes wrong:** the page set is a closed set encoded as open strings, and the
  two mappings are kept in step by hand. Adding a confirmable builtin to the arm at
  955 without touching `confirm_page` opens the *Shutdown* confirmation (the `_` arm),
  whose confirm tile is `Builtin::Shutdown`. A typo in a key passed to `tile()` silently
  returns a main-row tile (or panics on index). The existing test
  (`every_power_off_or_reset_action_has_a_confirmation…`) only checks today's three
  builtins.
- **Suggested fix:** `enum Page { Main, Submenu(String), Power, Confirm(Builtin) }` with
  one `key()`/`from_key()` pair used for the `gtk::Stack` name and `order.json`;
  exhaustive `match` (no `_`) in the confirm mapping; `tile()` matches on `Page`.
  Keep the key strings identical so existing `order.json` files still load.

### A3 — Schema type `Builtin` carries launcher-internal commands

- **Priority:** medium · **Confidence:** high · **Effort:** M (touches the schema module → Tier 3 per CLAUDE.md, although the JSON contract does not change)
- **Files:** `src/config.rs:58-75`, `src/window.rs:1215-1222, 954-973`
- **Evidence:**

  ```rust
  pub enum Builtin {
      DesktopMode, Shutdown, Restart,
      /// … Only the launcher's own pages use it: a configuration cannot ask for it.
      #[serde(skip_deserializing)] Cancel,
      /// … Only the power menu uses it.
      #[serde(skip_deserializing)] Arrange,
      #[serde(skip_deserializing)] ResetOrder,
  }
  ```

  `window.rs` builds its own pages out of `config::Tile`/`Action::Builtin`
  (`builtin_tile`, 1215) and dispatches on them in `activate`.
- **What goes wrong:** `config.rs` is documented in CLAUDE.md as the contract with
  vexos-nix, but half of `Builtin` is UI vocabulary. Each new menu action edits the
  contract module and needs a `skip_deserializing` guard plus a negative test (two such
  tests already exist, `config.rs:214-228`) to keep it out of the schema. The boundary
  depends on remembering the attribute.
- **Suggested fix:** keep `config::Builtin` to the three schema values. In `window.rs`,
  define a UI command type, e.g.
  `enum Command { Config(Action), Cancel, Arrange, ResetOrder }`, and a UI-side tile
  struct for own pages (or a `Tile` with `Command`). `activate` matches on `Command`.

### A4 — Test seam `Env` covers only the stop half of the lifecycle

- **Priority:** medium · **Confidence:** high · **Effort:** S–M
- **Files:** `src/launch.rs:32-57, 101-181, 534-536, 768-793, 1232-1249`; README "Development"
- **Evidence:** `Env` injects timings, the `flatpak`/`systemctl` programs and the record
  file, but only `stop_all_with` takes it. The start path and two other entry points
  bypass it:

  ```rust
  // 161, start_flatpak — real runtime path, not env.records
  let records = flatpaks_file();
  // 780, scope_argv — hard-coded program
  "systemd-run",
  // 535, check_early_exit
  early_exit(&record, &Env::default(), EARLY_EXIT_WATCH).await
  // 768-770, systemctl() for poweroff/reboot
  OsStr::new("systemctl")
  ```

  The lifecycle tests therefore build `Running` by hand:

  ```rust
  /// A launched Flatpak app, recorded as `start_flatpak` records it.
  fn launched_flatpak(&self, …) -> Running { … FlatpakApp { app: APP.into(), pid, … } }
  ```
- **What goes wrong:** the README says "The start and close lifecycle is covered end to
  end by `launch::lifecycle_tests`", but `start`, `start_flatpak`'s
  load → push → save of the record file, and the `systemd-run` argv are never run by
  those tests; the fixture duplicates `start_flatpak`'s record construction and can
  drift from it unnoticed. The two halves of one lifecycle are wired differently.
- **Suggested fix:** add `start_with(action, &Env)` (with `start` calling it with
  `Env::default()`), give `Env` a `systemd_run` field, and use `env.records` in
  `start_flatpak`; pass `Env` into `early_exit` from the caller. Then replace
  `launched_flatpak` with a real `start_with(&Action::Flatpak(..), &env)` against the
  stub. Either that, or narrow the README sentence to "close lifecycle".

### A5 — Outdated gtk-rs stack, glib/gio pinned separately

- **Priority:** medium · **Confidence:** high · **Effort:** M (dependency upgrade → Tier 3)
- **Files:** `Cargo.toml:11-14, 22-23`; `Cargo.lock:317-318, 347-348, 465-466, 588-589`
- **Evidence:**

  | crate | locked | latest (crates.io, 2026-10-24) |
  |---|---|---|
  | gtk4 | 0.9.7 | 0.11.5 |
  | libadwaita | 0.7.2 | 0.9.2 |
  | glib | 0.20.12 | 0.22.10 |
  | gio | 0.20.12 | 0.22.10 |
  | glib-build-tools | 0.20 | 0.22.8 |

  `glib = "0.20"` and `gio = "0.20"` are direct dependencies next to `gtk4 = "0.9"`;
  the code uses them directly (62 `glib::`/`gio::` paths across `src/`). Today the lock
  has a single glib/gio version, and `cargo audit` reports no advisories.
- **What goes wrong:** gtk-rs ships fixes only on its latest release line, so the
  project is on an unmaintained line two cycles back. The separate pins are a
  coupling hazard: bumping `gtk4` without `glib`/`gio`/`glib-build-tools` in the same
  change pulls two glib versions, and `glib::` types from the direct dependency stop
  matching what `gtk4` expects (compile errors at every crossing).
- **Suggested fix:** upgrade the five crates together in one change, reading the
  gtk-rs 0.10 and 0.11 release notes for API changes. Optionally use the re-exports
  (`gtk::glib`, `gtk::gio`) and drop the direct `glib`/`gio` dependencies so the
  versions cannot diverge (`glib-build-tools` stays a separate build dependency).

### A6 — Corrupt state files handled four ways; the Flatpak record fails silently

- **Priority:** low · **Confidence:** high · **Effort:** S
- **Files:** `src/launch.rs:639-644, 275`; `src/order.rs:310-315`; `src/gnome.rs:275-281`; `src/mode.rs:94-96`
- **Evidence:**

  ```rust
  // launch.rs — no log at all
  fn load_flatpaks(file: &Path) -> Vec<FlatpakApp> {
      std::fs::read_to_string(file).ok()
          .and_then(|text| serde_json::from_str(&text).ok())
          .unwrap_or_default()
  }
  // order.rs — logs a warning
  serde_json::from_str(text).unwrap_or_else(|error| { log::warn!("ignoring the saved tile order: {error}"); … })
  // gnome.rs — logs a warning and deletes the file
  // mode.rs  — silent (acceptable: a missing file is the normal case)
  ```
- **What goes wrong:** if `flatpak-apps.json` is unreadable, `stop_all_with` treats it as
  "no Flatpak apps" (233) and `save_flatpaks(&env.records, &still_running)` (275)
  overwrites it. The recorded instances are then never closed by home, and the
  journal has no hint why, unlike the equivalent failures in `order` and `gnome`.
- **Suggested fix:** distinguish "missing" from "unparsable" in `load_flatpaks` and
  `log::warn!` on the latter, as `order::parse` does. Optionally centralise this in
  `files` together with A7.

### A7 — Same persistence code written four times

- **Priority:** low · **Confidence:** high · **Effort:** S
- **Files:** `src/mode.rs:80-92`, `src/order.rs:284-303`, `src/gnome.rs:262-271`, `src/launch.rs:633-653`
- **Evidence:** each module builds its own path and its own serialize-and-write:

  ```rust
  glib::user_runtime_dir().join("vextheater").join("mode")              // mode.rs:81
  glib::user_state_dir().join("vextheater").join("order.json")          // order.rs:285
  glib::user_state_dir().join("vextheater").join("gnome-settings.json") // gnome.rs:263
  glib::user_runtime_dir().join("vextheater").join("flatpak-apps.json") // launch.rs:634

  // gnome.rs: anyhow
  crate::files::write_atomic(path, serde_json::to_string(values)?.as_bytes())?;
  // order.rs and launch.rs: io::Error::other
  serde_json::to_vec(x).map_err(std::io::Error::other).and_then(|b| crate::files::write_atomic(&file(), &b))
  ```
- **What goes wrong:** the app's directory name and error style are repeated; the
  error type differs by module (anyhow vs `io::Result` vs log-and-swallow) for the
  same operation. Small today, but every new state file copies one of three variants.
- **Suggested fix:** in `files.rs`, add `state_file(name)` / `runtime_file(name)` and
  `write_json<T: Serialize>(path, &T) -> io::Result<()>` (and a matching
  `read_json` that logs on parse failure, covering A6). Callers keep their own
  log-or-propagate choice.

### A8 — Power-tile migration leaves an unreachable page; `parse` ≠ `load`

- **Priority:** low · **Confidence:** high · **Effort:** S
- **Files:** `src/config.rs:93-115`, `src/window.rs:326-329`
- **Evidence:**

  ```rust
  pub fn load(path) -> … { let mut config = Self::parse(&text)…?; config.drop_power_tiles(); Ok(config) }
  fn drop_power_tiles(&mut self) {
      let submenus = &self.submenus;
      self.tiles.retain(|tile| !is_power_tile(tile, submenus));   // submenus untouched
  }
  // window.rs
  for (name, tiles) in &ui.config.submenus {
      ui.add_page(&submenu_page(name), Some(name), tiles, true);  // builds every submenu
  }
  ```
- **What goes wrong:** with the old vexos-nix config (a "Power" tile opening a
  submenu of shutdown/restart), the tile is dropped but the `"Power"` submenu page is
  still built and registered as arrangeable, with nothing able to open it: dead UI
  kept alive by a migration shim. Separately, the public `Config::parse` validates
  but does not apply the migration, while `load` does, so the two constructors return
  different shapes for the same JSON (the tests call `parse` then
  `drop_power_tiles()` by hand, `config.rs:232-256`).
- **Suggested fix:** in `drop_power_tiles`, also remove submenus no longer referenced
  by any remaining tile (or have `Ui::build` build only reachable submenus). Move the
  migration into `parse` (or make `parse` private to tests) so there is one
  constructor. Consider a removal date for the shim once vexos-nix no longer sends
  these tiles.

### A9 — Mode transition invariant kept at three call sites

- **Priority:** low · **Confidence:** high · **Effort:** S
- **Files:** `src/window.rs:441-446, 458-465`, `src/main.rs:42-45`
- **Evidence:**

  ```rust
  fn show(&self)         { mode::save(Mode::Theater); gnome::theater_mode(); … }  // window.rs
  fn desktop_mode(&self) { …; mode::save(Mode::Desktop); gnome::restore(); … }    // window.rs
  application.connect_shutdown(|_| { gnome::restore(); mode::clear(); });         // main.rs
  ```
- **What goes wrong:** "GNOME settings are off exactly while in theater mode, and the
  mode file matches" is one invariant, but `mode.rs` only persists a value and the
  pairing with `gnome` is repeated by hand in two modules. A new way in or out (e.g.
  another command-line flag) has to remember both calls in the right order.
- **Suggested fix:** let `mode.rs` own the transitions: `mode::enter_theater()`,
  `mode::enter_desktop()`, `mode::exit()`, each doing the `gnome` call and the save.
  `window.rs` and `main.rs` call those.

### A10 — Dependency cycle `clock` ↔ `window`

- **Priority:** low · **Confidence:** high · **Effort:** S
- **Files:** `src/clock.rs:3`, `src/window.rs:5, 1365-1367`
- **Evidence:** `clock.rs`: `use crate::window::label;` while `window.rs`:
  `use crate::clock::Clock;`. `label` is a one-line builder
  (`gtk::Label::builder().css_classes([class]).build()`), `pub` only for this use.
- **What goes wrong:** a leaf widget module depends on the top UI module, so the two
  cannot be understood or moved independently, and it widens `window`'s public
  surface. Splitting `window.rs` (A1) makes this worse if more helpers are borrowed
  the same way.
- **Suggested fix:** move `label` (and `hint`) into a small `widgets.rs`, or inline it
  in `clock.rs`.

### A11 — Inert `rust-toolchain.toml`

- **Priority:** low · **Confidence:** high · **Effort:** S
- **Files:** `rust-toolchain.toml`, `flake.nix:18-24`, `nix/package.nix:12`
- **Evidence:** the file says `channel = "stable"`. Inside `nix develop`, `type -a cargo`
  resolves first to `/nix/store/…-cargo-1.95.0/bin/cargo` (nixpkgs, not rustup), and
  `nix build` uses `rustPlatform`. Only a bare `cargo` through the system's
  `/run/current-system/sw/bin/rustup` would honour it, and CLAUDE.md forbids bare cargo.
- **What goes wrong:** two declared sources for the compiler: the real one (nixpkgs
  via `flake.lock`, rustc 1.95.0) and a floating "stable" that tools such as
  rust-analyzer outside the devShell, or a contributor's rustup, would pick instead,
  giving a different compiler and clippy lint set than CI.
- **Suggested fix:** delete `rust-toolchain.toml`, or pin it to the nixpkgs version
  (`channel = "1.95.0"`, components `clippy`, `rustfmt`) and note that the flake is
  authoritative.

---

## Checked and not reported

- Unused dependencies: none (`cargo machete`, and every crate in `[dependencies]` has
  call sites: `libc` for `kill(2)`/signals, `anyhow` in config/launch/window/gnome,
  `env_logger` in `main.rs`).
- Security advisories: none (`cargo audit`).
- Gresource contents: all five files are referenced (CSS, logo, two sounds; the app
  icon is found by GTK through the application's resource base path).
- `nix/module.nix` / `package.nix`: options, autostart unit and install steps are
  consistent with the README; nothing structural to report.
- Test scratch directories in `files.rs`/`gnome.rs` are not cleaned up, but they live
  under the devShell's per-shell `$TMPDIR`, so they do not accumulate.
