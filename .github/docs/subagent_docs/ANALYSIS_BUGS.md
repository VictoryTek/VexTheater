# VexTheater: bug and code-quality analysis

Date: 2026-10-24 · Commit: `d1ea354` (main, clean apart from untracked `ANALYSIS_ARCH.md`)

## Tool evidence

All run with `CARGO_TARGET_DIR=$HOME/.cache/vextheater/target` inside `nix develop path:.`.

| Check | Result |
|---|---|
| `cargo build` | exit 0 |
| `cargo test` | `test result: ok. 63 passed; 0 failed; 0 ignored` (3.15 s) |
| `cargo clippy --all-targets -- -D warnings` | exit 0, no warnings |
| `cargo clippy --all-targets -- -W clippy::pedantic` | 22 warnings, all stylistic (redundant closures, doc backticks, `too_many_lines` on `Ui::build`, guarded `u32 as i32` pid casts). None is a defect. |
| `cargo fmt --check` | exit 0 |
| `cargo audit` (cargo-audit from nixpkgs, 1352 advisories) | 111 crates scanned, 0 vulnerabilities, exit 0 |

Extra check for CFG-2: a scratch crate in the session scratchpad, pinned to the
lockfile's `serde 1.0.229` / `serde_json 1.0.154` and given the same `#[serde(flatten)]`
shape as `config.rs`, produced this output:

```
{"name":"A","icon":"a","command":"mpv","flatpak":"org.x.Y"}  => action: Command("mpv")
{"name":"A","icon":"a","flatpak":"org.x.Y","command":"mpv"}  => action: Flatpak("org.x.Y")
{"name":"A","icon":"a","comand":"typo","flatpak":"org.x.Y"}  => action: Flatpak("org.x.Y")
{"homekey":"F10","tiles":[]}                                 => home_key: None (Ok)
```

## Coverage

Reviewed line by line: every file in `src/` (`main.rs`, `config.rs`, `window.rs`,
`launch.rs`, `order.rs`, `gnome.rs`, `mode.rs`, `files.rs`, `sound.rs`, `clock.rs`),
plus `build.rs`, `nix/module.nix`, `nix/package.nix`, `flake.nix` and
`scripts/preflight.sh`.

Not covered: `data/style.css`, the AppStream and desktop files (preflight validates
these), `scripts/make-sounds.py`, the CI workflow, `.claude/` hooks, and anything that
needs a live GNOME session. I did not run the app or take screenshots, so the UI
findings come from reading the code and doing the arithmetic.

## Summary

| ID | Priority | Confidence | Area | Title |
|---|---|---|---|---|
| UI-1 | medium | high | window.rs | Rows of 13 or more tiles run off a 1920-px-wide screen, despite the README promise |
| CFG-2 | medium | high | config.rs | A tile with two actions, or a misspelled key, is accepted silently |
| UI-3 | low | high | window.rs | The power button still works while a tile is picked up, and the power menu then does nothing |
| LC-4 | low | high | launch.rs | `GaveUp` hides a simultaneous timeout of other apps |
| LC-5 | low | high | launch.rs | A corrupt Flatpak record file is discarded without a log line |
| ORD-6 | low | high | order.rs / window.rs | "Tile order reset" is shown even when deleting the file failed |
| CLK-7 | low | high | clock.rs | The clock can show the previous minute for up to 5 s |
| T-1 | medium | high | launch.rs tests | The scope SIGKILL fallback is never tested |
| T-2 | medium | high | launch.rs tests | The give-up path (`MAX_KILL_ATTEMPTS`) is never tested |
| T-3 | low | high | launch.rs | Writing the Flatpak record in `start_flatpak` is not testable or tested |
| T-4 | low | high | window.rs tests | `tile_size` tests cover only 6-tile rows |

No security findings. Config-driven `sh -c` is by design: the config is a root-owned
file in `/etc`, or `$VEXTHEATER_CONFIG` set by the same user. Exec-line arguments are
`shell_quote`d. Signals go only to pids that `signalable()` accepts, after their start
time is checked again. State files live in `0700` XDG directories and are written
atomically.

---

## Findings

### UI-1: Rows of 13 or more tiles run off a 1920-px-wide screen
- **Priority:** medium · **Confidence:** high · **Effort:** S–M
- **Files:** `src/window.rs:22-23`, `src/window.rs:1198-1202`, README "Tiles shrink … so a row of any length stays on screen"

```rust
const TILE_MAX: i32 = 200;
const TILE_MIN: i32 = 100;
fn tile_size(width: i32, count: usize) -> i32 {
    let count = i32::try_from(count.max(1)).unwrap_or(i32::MAX);
    let room = width - 2 * PAGE_MARGIN - TILE_SPACING * (count - 1);
    (room / count).clamp(TILE_MIN, TILE_MAX)
}
```

**What goes wrong:** the `clamp` sets a floor of 100 px per tile, so the README promise
holds only up to a certain count. On a 1920-px logical width (1080p, or 4K at scale 2)
the row has 1920 − 2×96 = 1728 px:

| tiles | size | row width |
|---|---|---|
| 12 | 107 | 1724 ✔ |
| 13 | 100 | 1780 ✘ (52 px over) |
| 14 | 100 | 1920 ✘ |

From 13 tiles on, the centred row overflows. The buttons also start with a 100×100
size request (`tile_button`), so the content asks for more than the monitor's width. The
outer tiles get clipped, and GTK may warn about a fullscreen window smaller than its
content. Submenus, which have no power-tile filtering, hit the same limit.

**Fix:** lower or drop `TILE_MIN`, or scale the spacing as well, so that
`n*size + (n-1)*spacing <= width - 2*PAGE_MARGIN` always holds. The other option is to
wrap the row in a horizontally scrolling `gtk::ScrolledWindow` that follows focus, and
correct the README to match. Add a test that asserts the row fits for every `count` in
`1..=30` (see T-4).

### CFG-2: A tile with two actions, or a misspelled key, is accepted silently
- **Priority:** medium · **Confidence:** high · **Effort:** M (schema contract with vexos-nix, so Tier 3)
- **Files:** `src/config.rs:77-107`, README "exactly one action"

```rust
pub struct Tile {
    pub name: String,
    pub icon: String,
    #[serde(default)] pub description: Option<String>,
    #[serde(flatten)] pub action: Action,
}
```

**What goes wrong:** with `#[serde(flatten)]` on an externally tagged enum, serde takes
the first key that names a variant and ignores the rest. Neither `Config` nor `Tile`
rejects unknown fields. The scratch-crate output above shows the effects:
- `{"command": …, "flatpak": …}` is accepted, and whichever key comes first wins. The
  vexos-nix module writes this file with `pkgs.formats.json`, which sorts keys, so for
  that consumer the winner is the alphabetically first action (`builtin` < `command` <
  `desktop` < `flatpak` < `submenu`). A tile meant to be a Flatpak can quietly run a
  leftover `command`.
- A misspelled key (`"comand"`, `"homekey"`, `"backgound"`) is dropped without a word.
  `homeKey` and `background` are optional, so the launcher starts with the hint or
  wallpaper missing and nothing explains why.

**Fix:** deserialize each tile through an intermediate struct with one `Option<String>`
per action and `#[serde(deny_unknown_fields)]`, check in `validate()` that exactly one
action is set, and convert to `Action`. Add `deny_unknown_fields` to `Config` too. This
is a contract change: check the vexos-nix module in the same change.

### UI-3: The power button still works while a tile is picked up, and the power menu then does nothing
- **Priority:** low · **Confidence:** high · **Effort:** S
- **Files:** `src/window.rs:335-341`, `src/window.rs:601-617`, `src/window.rs:682-686`, `src/window.rs:741-749`

Call path, with the pointer visible and a tile picked up (F2 or Menu):

1. Click the power button. `power_button.connect_clicked` calls `ui.open(POWER_PAGE, …)`
   without checking `picked`.
2. The power page shows, but `picked` is still `Some`:
   - Clicking any power tile goes through `click_while_arranging`, which sees
     `picked.is_some()`, calls `drop_picked()` and returns `true`. The tile's action
     never runs.
   - Enter goes through `arrange_key` → `enter_pressed`, which does the same.
   - ← and → go through `arrange_key` → `move_picked`, which reorders the hidden main
     row and plays the tick sound. ↑ and ↓ are swallowed.
   - `drop_picked` and `cancel_picked` call `grab_focus` on a button on the hidden main
     page.
   - The "← → move · Enter drop · Back cancel" hint stays over the power menu.

A milder form of the same gap: while in Arrange Tiles mode with no tile picked up, ↑
then Enter opens the power menu with `arranging` still set, and `ARRANGING_HINT` stays
on screen there. Back on the power page does not end arranging either, because the
Escape branch of `arrange_key` requires `on_main_page()`.

**Fix:** in the power button's click handler, call `ui.stop_arranging()` (or return
early while `picked` is `Some`) before `ui.open(POWER_PAGE, …)`.

### LC-4: `GaveUp` hides a simultaneous timeout of other apps
- **Priority:** low · **Confidence:** high · **Effort:** S
- **File:** `src/launch.rs:277-283`, consumed at `src/window.rs:1026-1048`

```rust
if !gave_up.is_empty() {
    Err(StopError::GaveUp(gave_up))
} else if closed && still_running.is_empty() {
    Ok(())
} else {
    Err(StopError::Timeout)
}
```

**What goes wrong:** when one Flatpak is given up on in the same close where a scope
also failed to stop (`closed == false`) or another Flatpak is stuck, only `GaveUp` is
returned. The user sees "<app> won't close; giving up on it" but not "Couldn't close the
previous app", even though another app is still running when the next tile starts.
Nothing is logged at this level either. The per-app warnings exist only in the debug
log.

**Fix:** carry both facts, for example `GaveUp { apps, others_still_running: bool }`,
and show both toasts, or join them into one message.

### LC-5: A corrupt Flatpak record file is discarded without a log line
- **Priority:** low · **Confidence:** high · **Effort:** S
- **File:** `src/launch.rs:639-644`

```rust
fn load_flatpaks(file: &Path) -> Vec<FlatpakApp> {
    std::fs::read_to_string(file).ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}
```

**What goes wrong:** a read error other than "not found", or a parse failure (for
example a record format changed by a later version, with a non-`default` field added
while an older launcher's file is still in `$XDG_RUNTIME_DIR`), turns every recorded
instance into "none". Those Flatpak apps can then never be closed by home or by a tile,
the `termed` state is lost, and the next `start_flatpak` or `stop_all` overwrites the
file. Nothing is logged. By contrast, `order::parse` and `gnome::take` both log the same
kind of error.

**Fix:** match on the error, stay silent on `NotFound`, and `log::warn!` on any other
read error or parse error, as `order::parse` does.

### ORD-6: "Tile order reset" is shown even when deleting the file failed
- **Priority:** low · **Confidence:** high · **Effort:** S
- **Files:** `src/order.rs:371-373`, `src/window.rs:829-843`

```rust
pub fn clear() { let _ = std::fs::remove_file(file()); }
// window.rs reset_order():
order::clear();
self.saved_order.borrow_mut().clear();
self.toasts.add_toast(adw::Toast::new("Tile order reset"));
```

**What goes wrong:** if `remove_file` fails with anything other than `NotFound` (for
example a permissions or read-only state directory), the rows are reset on screen and
the toast confirms it, but `order.json` survives. The old order comes back at the next
launcher start, and nothing is logged. `order::save` logs its failures; `clear` does not.

**Fix:** have `clear()` return or log the error, ignoring only `NotFound`, and show a
failure toast instead of "Tile order reset" when it fails.

### CLK-7: The clock can show the previous minute for up to 5 s
- **Priority:** low · **Confidence:** high · **Effort:** S
- **File:** `src/clock.rs:368-380`

```rust
update(&time, &date, &greetings, format);
glib::timeout_add_seconds_local(5, move || { update(...); glib::ControlFlow::Continue });
```

**What goes wrong:** the 5 s timer runs from whenever the window was built, not in step
with the minute, so the "%H:%M" label changes up to 5 s after the minute turns. (The
greeting can lag at 12:00 and 18:00 the same way.) `timeout_add_seconds` is also allowed
to coalesce, which adds about another second.

**Fix:** after each update, schedule the next one at the next minute boundary
(`60 - now.second()`) with a one-shot `timeout_add_local_once`, or tick every second.

---

## Test gaps

### T-1: The scope SIGKILL fallback is never tested
- **Priority:** medium · **Confidence:** high · **Effort:** S
- **Files:** `src/launch.rs:667-704`; stub at `src/launch.rs:1093`

The stub's `stop` always exits 0 (`stop) … exit 0 ;;`), so `stop_scopes` always takes
the `Some(Output { success: true, .. }) => return true` branch. These paths in the
close lifecycle never run in tests:
- `systemctl stop` failing, followed by the re-list (the `success: false` branch);
- `systemctl stop` timing out (`None`);
- `systemctl kill --signal=SIGKILL`, the poll during the kill grace, and
  `"still there after SIGKILL"` returning `false`, which becomes `StopError::Timeout`.

**Fix:** add `stop_fail` and `stop_hang` switches to the stub (`exit 1` or `sleep`), and
lab tests asserting that `kill --signal=SIGKILL <scope>` appears in `systemctl.log` and
that the result is `Ok` or `Timeout` depending on `units.txt`.

### T-2: The give-up path (`MAX_KILL_ATTEMPTS`) is never tested
- **Priority:** medium · **Confidence:** high · **Effort:** S
- **Files:** `src/launch.rs:396-407`, `src/launch.rs:271`, `src/launch.rs:277-278`

No test builds a record with `kill_attempts: 1`, and `grep GaveUp` finds no test. Giving
up and dropping the record from the file (it is not pushed to `still_running`) are
untested, and so is LC-4.

**Fix:** a variant of `close_termed_record` with `kill_attempts: MAX_KILL_ATTEMPTS - 1`
and a ready listing, asserting `Err(StopError::GaveUp([APP]))` and that `records.json`
is `[]` afterwards.

### T-3: Writing the Flatpak record in `start_flatpak` is not testable or tested
- **Priority:** low · **Confidence:** high · **Effort:** S
- **File:** `src/launch.rs:159-165`

`start_flatpak` writes to `flatpaks_file()` directly instead of through `Env`, so the
lifecycle tests cannot run it. They rebuild its record by hand
(`Lab::launched_flatpak`). A change to what `start_flatpak` records, or a failure to
append to an existing file, would go unnoticed.

**Fix:** pass `&Env` (or the records path) into `start_flatpak` and test
`start → stop_all_with` end to end against the lab's records file.

### T-4: `tile_size` tests cover only 6-tile rows
- **Priority:** low · **Confidence:** high · **Effort:** S
- **File:** `src/window.rs:1407-1420`

`tiles_shrink_so_the_row_fits_a_small_screen` checks only `count = 6`. No test checks
the README promise for long rows, which is why UI-1 got through.

**Fix:** loop over `count in 1..=30` and the target widths, asserting
`count*size + (count-1)*TILE_SPACING + 2*PAGE_MARGIN <= width` (this fails today for
13+ tiles at 1920).

Untested, but left out on purpose because only a GTK test harness could cover them:
`Ui::close_then`, `app_exited` id matching, and the `Unknown` hand-back. `CloseQueue`,
their core, is unit-tested.
