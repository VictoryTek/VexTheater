# VexTheater: feature recommendations

Analysis date: 2026-10-24, on `main` at `d1ea354` (working tree: only untracked analysis
files, no source changes).

## Evidence from tooling

| Check | Command | Result |
|---|---|---|
| Build | `CARGO_TARGET_DIR=$HOME/.cache/vextheater/target nix develop path:. -c cargo build` | OK, no warnings |
| Tests | `… nix develop path:. -c cargo test` | `test result: ok. 63 passed; 0 failed` (3.15 s) |
| TODO/FIXME/`dead_code`/`unimplemented!` | `grep -rn` over `src/ build.rs nix/ scripts/` | none found |

So there are no stubs or TODO markers to finish. Every recommendation below comes from
behaviour the code already has but doesn't carry through (a value that is only shown, a
state that is tracked but not used, or a path with no way out), or from an extension
point that is already in place.

## Coverage

All of it was read: `src/` (main, config, window, launch, order, gnome, mode, clock,
sound, files), `nix/module.nix`, `nix/package.nix`, `flake.nix`, `data/` (CSS, desktop
file, metainfo, gresource), `scripts/preflight.sh`, CI, README.

Not covered:
- The app was not run on a display (no cage/grim screenshots), so the UI notes come from
  the code, not from looking at it.
- The vexos-nix side (`modules/htpc-theater.nix`, the HDMI-CEC bridge) is not in this
  repository. Statements about the keys it sends rely on the README alone.
- Bugs and architecture findings already listed in
  `.github/docs/subagent_docs/ANALYSIS_BUGS.md` and `ANALYSIS_ARCH.md` are not repeated
  here.

## Summary

| ID | Feature | Priority | Effort | Tier* |
|---|---|---|---|---|
| FEAT-01 | A way out of the "Configuration error" screen | high | S | 2 |
| FEAT-02 | Accept TV-remote key names (Back, Home, Select) | high | S | 2 |
| FEAT-03 | `--check-config`, and validation at `nixos-rebuild` time | high | S–M | 3 |
| FEAT-04 | Mark tiles whose app is not installed, without closing the current app | high | M | 2 |
| FEAT-05 | Return to the last launched tile, not the first | medium | S | 2 |
| FEAT-06 | Empty-launcher page that explains how to add tiles | medium | S | 2 |
| FEAT-07 | `--desktop` and `--toggle` command-line verbs | medium | S | 2 |
| FEAT-08 | Turn off the Super-key overview in theater mode | medium | M | 2 |
| FEAT-09 | Follow GNOME's "event sounds" setting for the ticks and clicks | medium | S | 2 |
| FEAT-10 | Reload the configuration without restarting the launcher | medium | M | 2 |
| FEAT-11 | Optional `icon`, taken from the desktop entry or Flatpak id | medium | S | 3 |
| FEAT-12 | `programs.vextheater.homeKey`: binding and hint from one option | medium | M | 3 |
| FEAT-13 | `--diagnose`: the README's device checks, run by the launcher | low | M | 2 |
| FEAT-14 | Arrange submenu rows with a remote | low | M | 2 |
| FEAT-15 | `--print-order`: export the arranged order for the Nix config | low | S | 2 |
| FEAT-16 | Gamepad navigation | low | L | 3 |

\* Tier by CLAUDE.md's rules: a change to the config schema or the module interface is
Tier 3, and so is a new dependency.

---

## 1. Partly built or clearly intended, but unfinished

### FEAT-01: A way out of the "Configuration error" screen
- **Priority:** high · **Effort:** S
- **Where:** `src/window.rs:104-123` (`ui()`, the `Err(error)` arm); `src/main.rs`
  (`connect_activate`/`connect_command_line`).
- **Today:** a config that fails to parse or validate shows a full-screen
  `adw::StatusPage` with no key handling and no buttons. From then on, `ui()` returns
  early with `window.present()` for every call, so:
  - running `vextheater` or `vextheater --home` again only brings the same error back to
    the front; the file is never read again;
  - a remote can't leave the screen. Only Alt+F4 works, and that is a clean exit, so the
    `Restart=on-failure` service does not come back.
- **Feature:** two buttons on the error page:
  - **Retry** reads the configuration again and builds the launcher if it now loads.
  - **Desktop Mode** hides the window.

  `--home` and plain `vextheater` should also retry the load instead of only presenting
  the error window. Hide the pointer (`hide_idle_pointer`) and focus Retry, so Enter on a
  remote works.
- **Why it fits:** `Config::load_default` is already a pure, repeatable call, and
  `desktop_mode()` already does the hiding. All that's missing is a widget and the
  retry.

### FEAT-02: Accept TV-remote key names (Back, Home, Select)
- **Priority:** high · **Effort:** S
- **Where:** `src/window.rs`:
  - `is_enter` (L1182)
  - the Escape/BackSpace arms in the `keys` controller (L357) and in `arrange_key`
    (L611, L626)
  - `is_navigation` (L1188)
- **Today:** only `Return/KP_Enter/ISO_Enter`, `Escape/BackSpace`, and the arrow and Tab
  keys are handled. Bluetooth and USB TV remotes, and many IR receivers, send the evdev
  keys `KEY_BACK`, `KEY_HOMEPAGE` and `KEY_SELECT`. These arrive as the keysyms
  `XF86Back`, `XF86HomePage` and `Select`, and the launcher ignores all three. The
  README's power-menu workaround exists because remotes are already a target.
- **Feature:**
  - Treat `Key::Back` like Escape: go back, cancel a pick-up, leave arranging.
  - Treat `Key::Select` like Enter.
  - While the launcher has focus, treat `Key::HomePage` as an in-launcher home
    (`main_row()`). Closing an app from inside the app stays GNOME's keybinding.

  All three constants exist in gdk4 0.9.6 (`gdk4-0.9.6/src/keys.rs`). Check which keysyms
  a given remote actually sends with `wev` on the device before choosing the list.
- **Why it fits:** each check is a `matches!` in a single place, so adding names changes
  the behaviour everywhere at once.

### FEAT-05: Return to the last launched tile, not the first
- **Priority:** medium · **Effort:** S
- **Where:** `src/window.rs`: `home()` (L450), `main_row()` (L469), `Request::Tile(name, …)`
  (L131), `first_tiles`.
- **Today:** pressing home after watching something always selects the first tile of the
  main row, so a user who opened the fifth tile has to step back to it. The launcher
  already knows the last tile chosen: `close_then` receives its name in `Request::Tile`.
- **Feature:** keep the button that launched the running app, plus the page it was on.
  On `home()`, and on `app_exited` when the window is shown again, focus that button
  instead of `first_tiles[MAIN_PAGE]`. If it was in a submenu, focus the main-row tile
  that opens that submenu (the history entry already stores the opener). Keep this in
  memory only; a new session starting on the first tile is fine.
- **Why it fits:** buttons are kept stable across reorders (`Arrangeable.buttons`), so a
  saved `gtk::Button` stays valid after arranging.

### FEAT-14: Arrange submenu rows with a remote
- **Priority:** low · **Effort:** M
- **Where:** `src/window.rs`: `start_arranging()` (L822), `click_while_arranging()`
  (L682), `arrange_key()`; README, "Arranging tiles".
- **Today:** submenu rows are arrangeable (`add_page(…, true)` for every submenu).
  However, Arrange Tiles calls `main_row()`, and while arranging, Enter on a submenu
  tile picks it up rather than opening it. A remote can't hold Enter or send Menu/F2, so
  it can never reach a submenu to arrange it. The README documents this as "main row
  only".
- **Feature:** while arranging (and with no tile picked up), **Down** on a tile that
  opens a submenu opens it, still in arranging mode; Back returns to the main row. Down
  does nothing on a tile today, so no existing binding changes meaning. Update the
  `ARRANGING_HINT` text and the README.

---

## 2. Natural extensions of existing data and code

### FEAT-04: Mark tiles whose app is not installed, without closing the current app
- **Priority:** high · **Effort:** M
- **Where:**
  - `src/launch.rs:101-119` (`start`: `gio::DesktopAppInfo::new(id)` fails with "no
    installed application")
  - `src/window.rs`: `launch()` → `close_then()` → `start()` (L1004-1067), and
    `tile_button` (L1339)
- **Today:** a `desktop` tile whose id is not installed, or a `flatpak` tile whose app
  was removed, is only found out after `close_then` has already closed the running app.
  The user loses what they were watching and gets "Couldn't open …".
- **Feature:** when the pages are built (and again on `show()`), check each tile:
  - `desktop`: `gio::DesktopAppInfo::new(id).is_some()`
  - `flatpak`: the same check on `"<app-id>.desktop"`, since Flatpak exports its desktop
    files to `XDG_DATA_DIRS`

  A tile that fails the check gets a `.unavailable` CSS class (dimmed) and the
  description "Not installed". Choosing it shows the toast without calling
  `close_then`. `command` tiles stay unchecked, because a shell line can't be checked
  reliably.
- **Why it fits:** the check is the same call `start` already makes, done earlier.

### FEAT-07: `--desktop` and `--toggle` command-line verbs
- **Priority:** medium · **Effort:** S
- **Where:** `src/window.rs:39-79` (`command_line`, the usage text); `desktop_mode()`,
  `show()`; `mode::saved()`.
- **Today:** `--home` and `--resume` are the only verbs. Entering desktop mode takes the
  power menu, and the only way back is `vextheater` (or `--home`, which also closes the
  app).
- **Feature:**
  - `vextheater --desktop` runs `desktop_mode()` in the running instance.
  - `vextheater --toggle` shows the launcher if it is hidden, and otherwise enters desktop
    mode.

  This lets one remote button or GNOME shortcut switch between the two modes. Both verbs
  forward to the primary instance like `--home` does (`HANDLES_COMMAND_LINE`).
- **Why it fits:** each verb is one arm in the existing `match flags.as_slice()`.

### FEAT-08: Turn off the Super-key overview in theater mode
- **Priority:** medium · **Effort:** M
- **Where:** `src/gnome.rs`: `KEYS` (L562), `type Values = BTreeMap<String, bool>`
  (L568), `theater_mode()`/`restore()`.
- **Today:** the hot corner and notification banners are turned off while the launcher
  is showing. A keyboard (or a remote that sends Super/Meta) still opens GNOME's
  overview over the TV with one tap, because `org.gnome.mutter overlay-key` is left
  alone (the key exists; checked with `gsettings list-keys org.gnome.mutter`).
- **Feature:**
  - Save `overlay-key` and set it to `''` in theater mode; restore it on every exit path
    that already restores the other two.
  - `overlay-key` is a string, so `Values` has to hold more than `bool`. Store
    `glib::Variant` text (`variant.print(true)` / `Variant::parse`) for each key, and keep
    reading old files that hold `true`/`false`.
  - Update the README section "Restoring GNOME settings by hand".
- **Why it fits:** save-before-change, crash-safe restore and the four exit paths
  already exist. This adds one entry to an array that was built to be extended.

### FEAT-09: Follow GNOME's "event sounds" setting for the ticks and clicks
- **Priority:** medium · **Effort:** S
- **Where:** `src/sound.rs` (`Sounds::moved`/`chosen`); the same pattern as
  `clock.rs::uses_24h_clock` (`gnome::settings(...)`).
- **Today:** the sounds always play. There is no way to silence them except having no
  audio output.
- **Feature:** play only while `org.gnome.desktop.sound event-sounds` is true (the key
  exists: "Whether to play sounds on user events"). Read it through a `gio::Settings`
  kept in `Sounds`, so a change in GNOME Settings applies at once. This needs no schema
  change and no new option.

### FEAT-10: Reload the configuration without restarting the launcher
- **Priority:** medium · **Effort:** M
- **Where:**
  - `src/window.rs`: `ui()` (config loaded once, cached in `UI`), `Ui::build`
  - `nix/module.nix`: no `restartTriggers` on the user service
  - `src/main.rs`: shutdown → `mode::clear()`
- **Today:** after a `nixos-rebuild switch` that changes the tiles, the launcher keeps
  showing the old ones until the next login or a manual restart. Adding `restartTriggers`
  is not enough:
  - a restart sends SIGTERM, which is a clean exit, so `mode::clear()` removes the mode
    file;
  - `--resume` then comes back in theater mode, which would pop the launcher over a user
    who is in desktop mode.
- **Feature:**
  - `vextheater --reload` re-reads the configuration. If it loads, it rebuilds the pages
    in the existing window: a new `Ui` that is handed the old `running` app, the close
    queue and the toast overlay, so a running app stays tracked. If it fails, it keeps
    the old tiles and shows a toast with the error.
  - Also reload on `show()` when the file's contents differ from the last load, which
    covers a rebuild done while in desktop mode.
  - Rebuild only when no close is running (`closes.closing`) and no tile is picked up.

### FEAT-15: `--print-order`: export the arranged order for the Nix config
- **Priority:** low · **Effort:** S
- **Where:** `src/order.rs` (`load`, `arrange`); `src/window.rs` (`command_line`); README,
  "Arranging tiles".
- **Today:** the arranged order lives only in `~/.local/state/vextheater/order.json`.
  Reset Tile Order, or a new machine, loses it, and putting it back into
  `vexos.htpc.theater.apps` means reading identities like `["Browser", 1]` by hand.
- **Feature:** `vextheater --print-order` prints each page's tile names in the order on
  screen (`order::arrange` over the configured names), as JSON or one name per line. This
  is the backup/export for the one piece of user-made state, and it makes the order easy
  to copy into the declarative config. It runs in the primary instance or standalone, and
  needs only `Config::load_default` and `order::load`.

---

## 3. Gaps users of a TV launcher would expect

### FEAT-06: Empty-launcher page that explains how to add tiles
- **Priority:** medium · **Effort:** S
- **Where:** `nix/module.nix` (`settings` defaults to `{ }`); `src/config.rs`
  (`empty_object_is_an_empty_launcher` test); `src/window.rs` (`add_page`, `hints`).
- **Today:** `programs.vextheater.enable = true` with no settings, or a config in which
  every tile is dropped by `drop_power_tiles`, shows a greeting, an empty row, and the
  hint "Enter Open". Nothing is focused, and nothing says what to do.
- **Feature:** when the main row has no tiles, show a short message in its place: "No
  tiles configured. Add them in `programs.vextheater.settings` (written to
  `/etc/vextheater/config.json`)". Focus the power button, so Desktop Mode is one Enter
  away, and leave out the "Enter Open" hint.

### FEAT-13: `--diagnose`: the README's device checks, run by the launcher
- **Priority:** low · **Effort:** M
- **Where:**
  - README, "Troubleshooting" and "Verify on the device" (three manual shell checks)
  - `src/gnome.rs:606-610` (`settings()` returns `None` silently, so "the launcher
    quietly can't change GNOME settings")
  - `src/launch.rs` (`run`, `parse_instances`, `proc_stat`)
- **Today:** a hot corner that stays on, or a Flatpak that won't close, can only be
  diagnosed by following the README by hand. No log line says that the GNOME schemas
  were not found.
- **Feature:** `vextheater --diagnose` reports pass/fail for each check, using the
  existing helpers:
  - the GNOME schemas are found (`gnome::settings` for each `KEYS` schema), and
    `XDG_DATA_DIRS` is set
  - `systemd-run --user` and `systemctl --user list-units` work
  - `flatpak` is on `PATH`, if any tile uses it
  - the configured background file and icon files exist

  It should run inside the primary instance (command lines are forwarded to it), so it
  checks the environment of the systemd service, which is what matters. Also log a single
  `warn!` from `gnome::theater_mode` when a schema is missing.

---

## 4. Integrations and automation the structure is ready for

### FEAT-03: `--check-config`, and validation at `nixos-rebuild` time
- **Priority:** high · **Effort:** S–M
- **Where:**
  - `src/config.rs` (`Config::load`/`parse`/`validate`: pure, no GTK)
  - `src/main.rs` (startup needs a display: `load_stylesheet` panics without one)
  - `nix/module.nix` (`json.generate`)
- **Today:** a wrong `submenu` name, an unknown `builtin`, or a tile without an action is
  only found when the TV boots, as the error screen of FEAT-01. The module writes
  whatever `settings` holds.
- **Feature:**
  - `vextheater --check-config [FILE]` loads and validates the file, prints the error,
    and exits 1, or exits 0 if the file is valid. It has to be handled in `main()` before
    `application.run()`, so it works without a display or a session.
  - In the module, check the generated file in a derivation, e.g.
    `pkgs.runCommand "vextheater-config.json" {} "${lib.getExe cfg.package} --check-config ${generated} && cp ${generated} $out"`.
    A bad config then fails `nixos-rebuild` with the launcher's own error message.
  - The check can't tell whether apps are installed (that is FEAT-04's job at run time).
- **Tier 3:** this changes the module's build behaviour, and vexos-nix will now see
  build failures it didn't see before.

### FEAT-11: Optional `icon`, taken from the desktop entry or Flatpak id
- **Priority:** medium · **Effort:** S
- **Where:** `src/config.rs` (`Tile.icon: String`, required); `src/window.rs`
  (`tile_button`); README example (`"icon": "org.gnome.Nautilus"` beside
  `"desktop": "org.gnome.Nautilus.desktop"`).
- **Today:** every tile must name its icon, even when the action already identifies an
  app that has one. The README example repeats the app id in both fields for Files and
  FreeTube.
- **Feature:** make `icon` `Option<String>`. When it is missing:
  - a `desktop` tile uses `DesktopAppInfo::icon()`
  - a `flatpak` tile uses the app id as its icon name (Flatpak exports icons under that
    name)
  - any other tile is a validation error, raised by `validate()` (and so by FEAT-03 at
    build time)
- **Tier 3:** this is a schema change, a contract with vexos-nix
  `modules/htpc-theater.nix`. The change is backward compatible (old configs still
  parse), but README and module docs change together.

### FEAT-12: `programs.vextheater.homeKey`: binding and hint from one option
- **Priority:** medium · **Effort:** M
- **Where:** `src/config.rs` (`home_key`: "Name of the key … shown in the key hints",
  display only); `src/window.rs:923` (the hint); `nix/module.nix`; README, "Verify on
  the device" ("run `vextheater --home` from a GNOME keyboard shortcut").
- **Today:** the launcher only *shows* `homeKey`. The binding that makes it work is set
  elsewhere: by vexos-nix (per CLAUDE.md), or by hand for someone using the flake on its
  own, as the README's install section shows. The hint and the real binding can drift
  apart, and a user of the module alone has no home key at all.
- **Feature:** a module option `homeKey` (e.g. `"F10"`) that:
  - sets `settings.homeKey` to a readable form of the key;
  - adds a GNOME custom keybinding running `vextheater --home` through
    `programs.dconf.profiles.user.databases`. The option exists in this nixpkgs, checked
    with the NixOS option search. Custom keybindings are a list under
    `org/gnome/settings-daemon/plugins/media-keys`, and that list must be merged with
    the user's own rather than replacing it, which needs care.

  vexos-nix could then drop its own binding.
- **Tier 3:** a change to the module interface.

### FEAT-16: Gamepad navigation
- **Priority:** low · **Effort:** L
- **Where:** `src/window.rs` (every input path is a `gtk::EventControllerKey`); the
  README describes tiles for "streaming apps, web apps and games".
- **Today:** a game controller does nothing in the launcher unless something outside
  turns it into key presses. GTK4 has no gamepad events.
- **Feature:** map the D-pad to arrows, A to Enter, B to Back and Start to the power
  menu, through libmanette, sending the same actions the key handlers use. This needs a
  new native dependency and a Rust binding whose maintenance status must be checked
  first. The alternative is a uinput mapper in vexos-nix (as the HDMI-CEC bridge already
  does for remotes), which needs no change here. Compare the two before building
  anything; the vexos-nix route is likely the cheaper one.
- **Tier 3:** a new dependency.

---

## Considered and dropped

- **Search, categories, multiple rows:** rows are short (tiles shrink to fit one row on
  screen), and the configuration is declarative. Nothing in the code points toward these.
- **An in-app settings or tile editor:** the configuration is generated by Nix and
  read-only (`order.rs` explains why the order lives beside it). An editor would work
  against the module.
- **A notification centre for the banners that were hidden:** the banners are turned off
  on purpose; GNOME keeps the notifications in its own list for desktop mode.
- **"Resume the running app" instead of restarting it:** the launcher is normally only in
  front when no app is running (home closes the app). Raising another client's window is
  also not possible from a Wayland client.
- **Telemetry or metrics:** the logs already reach the journal through `env_logger` and
  `RUST_LOG`. FEAT-13 covers what is actually missing, which is diagnosis.
