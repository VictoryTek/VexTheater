# CLAUDE.md
**VexTheater** — full-screen TV launcher for GNOME (the HTPC "theater mode" of vexos-nix). Languages: Rust, Nix. Frameworks: GTK4 (gtk-rs 0.9), libadwaita (0.7), serde. Package manager(s): Cargo (`Cargo.lock`), Nix flakes (`flake.lock`).

## Commands

All Rust commands go through the flake devShell, which provides GTK4/libadwaita via pkg-config.

Build: `nix develop path:. -c cargo build` · package: `nix build path:.`
Test: `nix develop path:. -c cargo test`
Run: `VEXTHEATER_CONFIG=<file.json> nix develop path:. -c cargo run`
Preflight (final gate, also run by CI): `bash scripts/preflight.sh`

On a Windows checkout, run these through WSL (`wsl -e bash -lc 'cd /mnt/c/Projects/VexTheater && …'`).

## Repository

- `src/` — `main.rs` (GApplication), `config.rs` (JSON schema + validation), `window.rs` (launcher UI: pages, tiles, navigation, pointer hiding), `clock.rs` (time, date, greeting)
- `data/` — desktop file, AppStream metainfo, icon, `style.css`, gresource manifest (compiled by `build.rs`)
- `nix/` — `package.nix` (rustPlatform.buildRustPackage), `module.nix` (`programs.vextheater`: install, autostart, `settings` → `/etc/vextheater/config.json`)
- Consumer: vexos-nix `modules/htpc-theater.nix` builds `settings` from `vexos.htpc.theater.apps` and binds the home key. The JSON schema in `config.rs` (documented in README.md) is a contract with that module: change both together.
- Architecture: the launcher is a fullscreen window in the GNOME session. Tile apps run in their own `systemd-run --user --scope` so the home action can stop the whole app; closing it returns focus to the launcher. Sizes are logical pixels; GNOME's display scale handles 4K.
- Untracked files are invisible to a `git+file` flake ref; always use `path:.`.

## Forbidden commands

- `git add`, `git commit`, `git push`, `git stash` (any form), or any other git command that stages, commits, pushes or stashes. Read-only git is fine. The user commits; every change ends with a commit message for them.
- Bare `cargo build|test|check|clippy|run` outside `nix develop` — reason: GTK4/libadwaita are only on `PKG_CONFIG_PATH` inside the devShell. Use `nix develop path:. -c cargo …`.

## How to work

Same tiers and rules as vexos-nix's CLAUDE.md:

- **Tier 1** (questions, reading): just answer. **Tier 2** (bug fix, small feature): brief plan → implement → verify → preflight → commit message. **Tier 3** (new dependency, config schema change, NixOS module interface, CI): full plan, wait for approval → implement → verify → review → preflight → commit message.
- Minimum code, surgical changes, match the surrounding style. State assumptions; ask when a request has several readings.
- Done means verified: report real output, never "passes" for something not run. Preflight must exit 0. After 2 failed fix cycles, stop and report.
- UI changes: verify visually, e.g. `nix shell nixpkgs#cage nixpkgs#grim` and screenshot the window at 1920x1080 and at 3840x2160 with scale 2.

### Commit message (final step of every change)

```
<type>(<scope>): <description — MAX 72 characters total>

<PARAGRAPH EXPLAINING WHAT CHANGED AND WHY>

Modified Files:
- path/to/file

✔ Build successful
✔ Tests passed
✔ Review approved
✔ Preflight passed
```

Types: `feat`, `fix`, `chore`, `refactor`, `docs`, `test`, `perf`. Checkmark lines only for checks that ran and passed this session.
