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
    { "name": "FreeTube","icon": "io.freetubeapp.FreeTube", "flatpak": "io.freetubeapp.FreeTube" },
    { "name": "Desktop", "icon": "user-desktop",         "builtin": "desktop-mode" },
    { "name": "Power",   "icon": "system-shutdown-symbolic", "submenu": "Power" }
  ],
  "submenus": {
    "Power": [
      { "name": "Shutdown", "icon": "system-shutdown-symbolic", "builtin": "shutdown" },
      { "name": "Restart",  "icon": "system-reboot-symbolic",   "builtin": "restart" }
    ]
  }
}
```

Each tile has a `name`, an `icon` (an icon-theme name, or an absolute path to an
image), an optional one-line `description` shown while it is selected, and exactly
one action: `command`, `desktop`, `flatpak`, `submenu` or `builtin`
(`desktop-mode`, `shutdown`, `restart`).

All top-level keys are optional. `title` is shown under the greeting, `homeKey` names
the key that closes the open app in the key hints, and without `background` the
launcher uses a plain dark gradient.

## Behaviour

- Each tile app runs in its own systemd user scope. Starting another tile closes the
  previous app and waits for it to exit before starting the new one, so a second
  Brave cannot hand its window to one that is shutting down. Only one close runs at a
  time: tiles chosen (or home pressed) during a close replace each other, and only the
  last one is acted on when the close finishes.
  - Scopes get 5 s to exit; any still there are then killed, by name, and get up to
    2 s more to disappear.
  - A Flatpak app is closed with `flatpak kill` and waited for until a successful
    `flatpak ps` no longer lists it, for up to 5 s of its own, alongside the scopes.
  - If something is still running at the end, the launcher shows "Couldn't close the
    previous app" and starts the next tile anyway.
  - If the running scopes cannot be listed at all, whether an app is still running is
    unknown: the launcher shows "Couldn't check whether the previous app closed" and
    starts nothing. Choosing the tile again retries.
  `vextheater --home` closes the app and shows the launcher again (bind this to a key
  in GNOME's keyboard shortcuts, or to a remote's Home button). Both close every
  `vextheater-app-*` scope, including apps left running by a launcher that crashed.
- Desktop mode hides the launcher; running `vextheater` again (e.g. "Theater Mode" in
  the app grid) shows it.
- While the launcher is showing, GNOME's hot corner and notification banners are off.
  The values they had before are saved to `~/.local/state/vextheater/gnome-settings.json`
  and put back on desktop mode, quitting, closing the window (Alt+F4), SIGTERM and
  SIGINT. A launcher restarted after a crash keeps the saved file, so the original
  values still come back.
- An app that exits with an error within 5 s of starting (not installed, command not
  found), or a refused shutdown or restart, shows "Couldn't open …".

### Restoring GNOME settings by hand

Uninstalling VexTheater, or removing its NixOS module, while the launcher is running
cannot put the hot corner and banners back: the program that would do it is gone.
Restore them yourself:

```sh
cat ~/.local/state/vextheater/gnome-settings.json   # the saved values, if any
gsettings set org.gnome.desktop.interface enable-hot-corners true
gsettings set org.gnome.desktop.notifications show-banners true
rm -f ~/.local/state/vextheater/gnome-settings.json
```

(Use the values from the file if it exists; `true` is GNOME's default for both.)

### Troubleshooting

- The hot corner or banners never turn off: the launcher finds GNOME's settings
  schemas through `XDG_DATA_DIRS`, and if `systemctl --user show-environment` shows it
  empty, the launcher quietly can't change GNOME settings.

### Known limitations

- An app that hands its window to an instance that is already running and then exits
  (Nautilus when it is already open, or Brave left open in desktop mode) lives outside
  the launcher's scope: home cannot close it, and the launcher comes back in front of
  it.
- A `desktop` tile's Exec line has its whitespace collapsed when field codes are
  removed, including runs of spaces inside quoted arguments (`sh -c "a  b"`).
- The saved GNOME settings file is written atomically (temporary file, then rename),
  but the directory is not synced afterwards, so a power cut right after the rename
  may leave the previous file. A crash of the launcher itself cannot.
- With `autostart`, the launcher runs as the `vextheater` systemd user service, part
  of the graphical session and restarted if it crashes. A restarted launcher always
  comes back full screen, even if it crashed while you were in desktop mode. That is
  intended: on a TV, the launcher is the screen to recover to, and home or desktop
  mode is one key away.

## Development

```sh
nix develop -c cargo run        # VEXTHEATER_CONFIG=./example.json to try a layout
bash scripts/preflight.sh       # fmt, clippy, tests, desktop/AppStream validation, nix build
```
