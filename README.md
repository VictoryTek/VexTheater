# VexTheater

A full-screen TV launcher for GNOME: a row of large tiles for streaming apps, web apps
and games, driven by keyboard, mouse or TV remote. It runs inside the normal GNOME
session, so "desktop mode" is just GNOME with the launcher hidden.

Built for the HTPC role of [vexos-nix](https://github.com/VictoryTek/vexos-nix).

## Install (NixOS flake)

```nix
inputs.vextheater = {
  url = "github:VictoryTek/VexTheater";
  inputs.nixpkgs.follows = "nixpkgs";
};

# in a NixOS module:
imports = [ inputs.vextheater.nixosModules.default ];
programs.vextheater = {
  enable = true;
  autostart = true;
  homeKey = "F10";               # optional: the home key, see "The home key" below
  settings = { /* see below */ };
};
```

## Configuration

`programs.vextheater.settings` is written to `/etc/vextheater/config.json`
(override the path with `$VEXTHEATER_CONFIG`):

```json
{
  "title": "VexOS Theater",
  "homeKey": "F10",
  "background": "/path/to/wallpaper.jpg",
  "tiles": [
    { "name": "YouTube", "icon": "/path/to/youtube.svg", "description": "YouTube TV interface, full screen",
      "command": "brave --kiosk --app=https://youtube.com/tv" },
    { "name": "Files",   "icon": "org.gnome.Nautilus",   "desktop": "org.gnome.Nautilus.desktop" },
    { "name": "FreeTube","icon": "io.freetubeapp.FreeTube", "flatpak": "io.freetubeapp.FreeTube" }
  ]
}
```

Each tile has a `name`, an `icon` (an icon-theme name, or an absolute path to an
image), an optional one-line `description` shown while it is selected, and exactly
one action: `command`, `desktop`, `flatpak`, `submenu` or `builtin`
(`desktop-mode`, `shutdown`, `restart`). A `submenu` names an entry in the optional
`"submenus"` object, which maps a name to a list of tiles like `tiles`.

The `icon` may be left out of a `desktop` tile, which then shows its application's icon,
and of a `flatpak` tile, which shows the icon named like its application id. An icon
that cannot be found (no such desktop file, no icon in it, a name the icon theme lacks,
a file that is not there) shows a generic placeholder instead, and the launcher logs a
warning, so a tile is never blank.

The schema is strict: an unknown key anywhere (at the top, in a tile, in a submenu's
tiles), a tile with no action or with more than one, or a tile that needs an icon and
has none, is an error, and the launcher shows the configuration error screen. This is
the contract with vexos-nix: **a new configuration key has to land in VexTheater before
vexos-nix starts sending it**, because strict parsing, and the check the NixOS module
runs while building (see `--check-config` below), would otherwise reject it.

A configuration with no tiles shows "No tiles configured" and the power button.

A `desktop` tile whose desktop file is not installed, and a `flatpak` tile whose app is
not (its exported desktop file is not found, and it has no installation directory in
`/var/lib/flatpak/app` or `~/.local/share/flatpak/app`), is dimmed and says "Not
installed"; choosing it only shows a message, and the open app is left running. This is
checked when the launcher is built and each time it is shown again. `command` tiles are
not checked.

Tiles shrink from 200 px to fit the row on a narrow screen. A long row then closes up
the gaps between tiles before shrinking the tiles below 100 px (past about 15 tiles on a
1920 px screen), so a row of any length stays on screen.

`vextheater --check-config [FILE]` checks a configuration file (by default the one the
launcher would read) without a display. It exits 0 for a good file, or 1 after printing
the file and the field at fault, e.g. `tiles[2].icon: missing (tile "Files")` (2 for a
bad argument). The NixOS module runs
it on `settings` while building, so a bad configuration fails `nixos-rebuild` instead
of showing the launcher's error screen. When the build machine cannot run the target's
binaries (cross-compiling), the check is skipped. On the error screen, Retry reads the file
again; so do `vextheater` and `vextheater --home`.

### Power menu

The round power button beside the clock is part of the launcher, not of the
configuration: press Up from the tile row to select it (Down goes back to the tiles) and
Enter, or click it, to open the power menu with Desktop Mode, Arrange Tiles, Reset Tile
Order, Restart and Shutdown. Esc leaves the menu. Restart, Shutdown and Reset Tile Order
ask for confirmation first, with Cancel selected, so a stray key press cannot power the
machine off. Tiles with a `shutdown` or `restart` builtin in a submenu of the
configuration ask the same way.

A TV remote's keys work too, when its receiver reports them as such: Back like Esc,
Select or OK like Enter, and Home returns to the tile row.

### Arranging tiles

The tiles of the main row and of the configured submenus can be reordered from the
launcher (the power menu cannot). Pick a tile up by holding Enter on it, or pressing Menu
or F2; or choose Arrange Tiles in the power menu, after which Enter picks up the selected
tile. A picked-up tile is lifted and ringed, ← → move it one place (it stops at the ends),
Enter drops it, and Back cancels and puts it back. Back leaves Arrange Tiles. After a
drop, the selection stays on the tile.

A remote cannot hold a key: the HDMI-CEC bridge in vexos-nix presses and releases at once,
and sends no Menu or F2. Use Arrange Tiles from the power menu there (main row only).

The order is saved per page in `$XDG_STATE_HOME/vextheater/order.json`
(`~/.local/state/vextheater/`), on top of the configuration: a tile added to the
configuration later goes last, and a removed one is dropped from the saved order. Tiles
are identified by name; tiles on one page that share a name are told apart by their place
among them in the configuration (the first "Browser", the second "Browser"), so each
keeps its own position. Adding or removing an earlier tile of the same name in the
configuration shifts the later ones. Reset Tile Order deletes the file and returns every
row to the configured order.

**Upgrading:** the Desktop Mode and Power tiles used to come from the configuration (the
vexos-nix HTPC module sent them). The button replaces them, so the launcher leaves out of
the main row any tile with a `desktop-mode`, `shutdown` or `restart` builtin, and any tile
opening a submenu of only those. Such tiles in a submenu mixed with other tiles stay.

All top-level keys are optional. `title` is shown under the greeting, `homeKey` names
the key that closes the open app in the key hints, and without `background` the
launcher uses a plain dark gradient.

## Behaviour

- Moving between tiles, by key or by the pointer, plays a short tick, and choosing a
  tile, opening the power menu or going back plays a click, like a streaming box. The
  sounds are bundled (`data/sounds/`, written by `scripts/make-sounds.py`) and play
  through GStreamer to the default audio output; with no audio output the launcher stays
  silent and logs a warning. They follow GNOME's Sound setting "Event sounds"
  (`org.gnome.desktop.sound event-sounds`): turn it off and the launcher is silent, at
  once, without a restart.
- An ordinary tile app runs in its own systemd user scope; a Flatpak app is started
  directly, because Flatpak moves it into a scope of its own, and the launcher records
  that one instance (its process id and start time) in
  `$XDG_RUNTIME_DIR/vextheater/flatpak-apps.json`. Closing an app never targets
  others: a copy of the same Flatpak opened from the desktop is left alone.
- Starting another tile closes the previous app and waits for it to exit before
  starting the new one, so a second Brave cannot hand its window to one that is
  shutting down. Only one close runs at a time: tiles chosen (or home pressed) during
  a close replace each other, and only the last one is acted on when it finishes.
  Choosing Desktop Mode cancels a pending tile.
  `vextheater --home` closes the app and shows the launcher again (see "The home key"
  below for the shortcut). Both close every
  `vextheater-app-*` scope and recorded Flatpak instance, including apps left running
  by a launcher that crashed. When the launcher comes back, whether by home or because
  the app exited, it selects the tile that opened that app (for a tile in a submenu, the
  main-row tile that opens the submenu); this is not saved across restarts.
  - `vextheater --desktop` hides the launcher like Desktop Mode, and `vextheater
    --toggle` shows it when it is hidden and hides it when it is shown, so one key can
    switch between theater and desktop mode.
  - `vextheater --reload` reads the configuration again and rebuilds the tiles, titles
    and background in the running launcher, which is also done whenever the launcher
    is shown again and the file has changed. The open app, a close in progress and the
    saved tile order are not touched. A configuration that cannot be read leaves the
    tiles as they were and says why; nothing is rebuilt while an app is closing or a
    tile is picked up (a reload asked for says so).
  - No process gets two SIGTERMs: Chromium-based apps like Brave start a clean
    shutdown on the first and die on a second. An ordinary app is stopped through its
    scope; the app started last is signalled directly only if its scope has not
    registered yet, so a close that begins right after choosing a tile (home pressed
    at once, a double Enter) still closes it. A Flatpak app's own process inside the
    sandbox gets one SIGTERM, so Flatpak Chromium apps shut down cleanly too. That is
    the sandbox's first process (`child-pid` in `flatpak ps`), or, when that is bwrap's
    init, its children, or the init itself if it has none yet. Nothing is signalled
    until `flatpak ps` lists the instance with its sandbox info (`child-pid` not 0),
    which can take a moment after the launch: the launcher keeps polling for up to 5 s,
    since signalling the launcher-side process earlier takes the sandbox with it and
    the app never shuts down cleanly. If it still is not ready after 5 s (`flatpak ps`
    failing, or the instance never listed or never ready), the recorded launcher-side
    process gets the one SIGTERM instead, and a SIGKILL after another 5 s. Signals are
    sent with `kill(2)` directly, never to pid 0 (the launcher's own process group),
    pid 1 or the launcher itself, and the recorded process only after its pid and
    start time are checked again right before sending, so a reused pid is not hit. A
    record whose SIGTERM an earlier close already sent does not wait for readiness
    either: one `flatpak ps` listing is enough. With the instance ready it then gets
    `flatpak kill`; with it unlisted or not ready, which `flatpak kill` cannot reach,
    the recorded process gets the SIGKILL.
  - Scopes get 5 s to exit; any still there are then killed, by name, and get up to
    2 s more to disappear. A Flatpak instance gets 5 s after its SIGTERM, then
    `flatpak kill` on that instance (or the SIGKILL above). A `systemctl stop` that
    fails is checked against a fresh listing rather than taken as success.
  - If something is still running at the end, the launcher shows "Couldn't close the
    previous app" and starts the next tile anyway. A Flatpak instance that did not
    close stays recorded with a note that its SIGTERM was sent; every later close
    (home or a tile) only waits for it and runs `flatpak kill` again, never a second
    SIGTERM. After two failed `flatpak kill`s the launcher gives up on it, shows
    "<app> won't close; giving up on it" and stops tracking it, so a stuck app does
    not slow down every later close.
  - If the running scopes cannot be listed at all, whether an app is still running is
    unknown: the launcher shows "Couldn't check whether the previous app closed" and
    starts nothing. Choosing the tile again retries.
  - A close takes at most about 20 s, and normally well under a second: up to 2 s to
    list the scopes, then the slowest of the scopes (5 s to stop, 2 s to re-check a
    failed stop, 2 s of kill grace and 2 s for its last check, 11 s), an app started
    last whose scope registered during the close (5 s to exit, 2 s to kill, then the
    same 11 s for its scope, 18 s) and a Flatpak instance (5 s to find it ready, 5 s to
    exit after its SIGTERM, then `flatpak kill` or the SIGKILL and its effect share
    2 s, 12 s).
  - If a launched Flatpak's process exits within moments while `flatpak ps` lists an
    instance of the app that started after the launch, the launcher logs a warning (in
    the journal, as `vextheater`) that the one-process-id assumption does not hold on
    this system. Older instances of the same app (a stuck earlier one, or one opened
    from the desktop earlier) are ignored.
- Desktop mode (in the power menu) hides the launcher; running `vextheater` again
  (e.g. "Theater Mode" in the app grid) shows it.
- With `autostart`, the launcher runs as the `vextheater --resume` systemd user
  service, part of the graphical session and restarted if it crashes. The current mode
  is kept in `$XDG_RUNTIME_DIR/vextheater/mode`, so a launcher restarted after a crash
  in desktop mode stays in desktop mode, and one that crashed in theater mode comes
  back full screen. A clean exit removes the file, and a new login starts in theater
  mode.
- While the launcher is showing, GNOME's hot corner, the Super key's overview and
  notification banners are off (the Super key by clearing `org.gnome.mutter overlay-key`,
  so it does nothing in theater mode; Super+key shortcuts are not touched).
  The values they had before are saved to `~/.local/state/vextheater/gnome-settings.json`
  and put back on desktop mode, quitting, closing the window (Alt+F4), SIGTERM and
  SIGINT. A launcher restarted after a crash keeps the saved file, so the original
  values still come back. State files are written atomically: a temporary file, synced
  and renamed over the old one, then the directory synced.
- An app that exits with an error within 5 s of starting (not installed, command not
  found), or a refused shutdown or restart, shows "Couldn't open …".
- A `desktop` tile's Exec line is parsed by the Desktop Entry spec's rules (quoted
  arguments keep their spaces, `\"` `` \` `` `\$` `\` are unescaped inside double
  quotes, `%%` is `%`, the file and URL field codes are dropped) and each argument is
  quoted for `sh`.
- An app that hands its window to an instance that is already running and then exits
  (Nautilus when it is already open, or Brave left open in desktop mode) lives outside
  the launcher's control: home cannot close it, and the launcher comes back in front
  of it.

### The home key

`programs.vextheater.homeKey = "F10";` (a GNOME accelerator name such as `F10` or
`<Super>h`) is the one place the home key is set. It sets `settings.homeKey`, which the
key hints show, and the launcher makes sure GNOME has a custom shortcut for it, named
"VexTheater home", that runs `vextheater --home`, so the key closes the open app and
brings the launcher back from any app and from desktop mode. The command is
`/run/current-system/sw/bin/vextheater --home` where that exists, else `vextheater --home`;
never a store path, which changes with every rebuild and goes away with a garbage
collection.

The launcher does this itself, when it starts and when `--reload` changes `homeKey`,
because a NixOS dconf database cannot: dconf does not merge lists, so a list of custom
shortcuts declared there is hidden by the user's own, if they have one. Instead the
launcher adds its shortcut to the list the user sees and leaves every other shortcut
alone. A shortcut that is already right is not written again. **If you edit this one
shortcut by hand in GNOME Settings, the launcher puts it back at its next start**; to
change the key, change `homeKey`. Without `homeKey` the launcher removes its shortcut.
Setting `settings.homeKey` to a different value as well is a conflict the module reports.

The launcher owns the shortcut whenever it loads a configuration, whichever file that is.
A development run with `VEXTHEATER_CONFIG=<file.json>` on a machine that has the module
therefore rewrites the real shortcut to that file's `homeKey`, or removes it if the file
has none, and it stays so until the real launcher next starts or reloads.

If the shortcut never appears, the launcher may not see GNOME's schemas (the same
`XDG_DATA_DIRS` issue as under Troubleshooting); start it with `RUST_LOG=info` to see
what it did or why it did nothing.

Uninstalling VexTheater leaves the shortcut behind, pointing at a program that is gone.
Remove it by hand:

```sh
gsettings get org.gnome.settings-daemon.plugins.media-keys custom-keybindings
# set that list again without the .../custom-keybindings/vextheater-home/ entry, e.g.
gsettings set org.gnome.settings-daemon.plugins.media-keys custom-keybindings "[]"
```

(The last command empties the whole list; put back any shortcuts of your own that the
first command showed.)

### Restoring GNOME settings by hand

Uninstalling VexTheater, or removing its NixOS module, while the launcher is running
cannot put the hot corner, the Super key and banners back: the program that would do it
is gone. Restore them yourself:

```sh
cat ~/.local/state/vextheater/gnome-settings.json   # the saved values, if any
gsettings set org.gnome.desktop.interface enable-hot-corners true
gsettings set org.gnome.desktop.notifications show-banners true
gsettings reset org.gnome.mutter overlay-key       # or set the value from the file
rm -f ~/.local/state/vextheater/gnome-settings.json
```

(Use the values from the file if it exists; `true` is GNOME's default for the first two,
and `gsettings reset` puts the Super key back to GNOME's default.)

### Troubleshooting

- The hot corner, Super key or banners never turn off: the launcher finds GNOME's settings
  schemas through `XDG_DATA_DIRS`, and if `systemctl --user show-environment` shows it
  empty, the launcher quietly can't change GNOME settings.

## Verify on the device

These depend on the real desktop, so they can only be checked on the machine:

- **Home from desktop mode.** In desktop mode, press the home key (the shortcut the
  launcher registers for `homeKey`; see "The home key"), also over a running app. GNOME
  50 must bring the launcher to the front; if it only shows a "VexTheater is ready"
  notification, the launcher cannot raise itself from there. Check that GNOME runs the
  shortcut at all: `gsettings get org.gnome.settings-daemon.plugins.media-keys
  custom-keybindings` lists it after the launcher has started.
- **What the remote sends.** Run `wev` and press the remote's Back, Select (OK) and
  Home buttons. The launcher takes `XF86Back` as Esc, `XF86Select`, `XF86OK` and the
  old `Select` as Enter, and `XF86HomePage` as "back to the tile row", on top of the
  keyboard keys; if the remote reports other key names, they need adding.
- **GNOME leaves Home to the launcher.** If `wev` shows no event for the remote's Home
  button, GNOME grabbed it first, and the button never reaches the launcher. Look for
  the binding with
  `gsettings list-recursively org.gnome.settings-daemon.plugins.media-keys | grep -i homepage`
  (no output on the development machine, where Home was not grabbed).
- **The Super key.** In theater mode, pressing Super must do nothing (no overview over a
  film). After Desktop Mode, a clean exit, or a crash followed by `--resume`, it must
  open the overview again. The values are checked off the device (the state file, and
  `gsettings get org.gnome.mutter overlay-key`); only GNOME itself shows that the key
  really stops reacting.
- **GNOME settings reachable.** `systemctl --user show-environment | grep XDG_DATA_DIRS`
  must not be empty (see Troubleshooting above).
- **Flatpak keeps one process id.** The launcher closes a Flatpak app through the
  process it started, which assumes `flatpak run` execs into the sandbox's wrapper
  with the same process id. With FreeTube installed:

  ```sh
  flatpak run io.freetubeapp.FreeTube & p=$!; sleep 8
  echo "launched PID $p is: $(ps -o comm= -p $p)"
  flatpak ps --columns=instance,pid,child-pid,application | grep -i freetube
  kill $p; sleep 3; flatpak ps --columns=application | grep -qi freetube \
    && echo "STILL RUNNING" || echo "closed by one SIGTERM"
  ```

  It holds if the launched PID shows as `bwrap`, matches the `pid` column of the
  `flatpak ps` line, and the last line prints "closed by one SIGTERM".

## Development

```sh
nix develop -c cargo run        # VEXTHEATER_CONFIG=./example.json to try a layout
bash scripts/preflight.sh       # fmt, clippy, tests, desktop/AppStream validation, nix build
```

The start and close lifecycle is covered end to end by `launch::lifecycle_tests`: real
processes, stub `flatpak`, `systemctl` and `systemd-run` programs and millisecond
timings, so it runs without Flatpak or systemd. Starting an app and then closing it go
through the same stubs. The device checks above still need the real machine.
