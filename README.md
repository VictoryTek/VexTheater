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

## Development

```sh
nix develop -c cargo run        # VEXTHEATER_CONFIG=./example.json to try a layout
bash scripts/preflight.sh       # fmt, clippy, tests, desktop/AppStream validation, nix build
```
