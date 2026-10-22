# NixOS module for VexTheater.
#
# Wired into vexos-nix the same way VexPortal is: a flake input, then this module on
# the HTPC role. The tile layout comes from `settings`, written to
# /etc/vextheater/config.json; see README.md for its schema.
self:
{ config, lib, pkgs, ... }:

let
  cfg = config.programs.vextheater;
  json = pkgs.formats.json { };
in
{
  options.programs.vextheater = {
    enable = lib.mkEnableOption "VexTheater, a full-screen TV launcher for GNOME";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "vextheater.packages.\${system}.default";
      description = "The VexTheater package to install.";
    };

    autostart = lib.mkEnableOption ''
      starting VexTheater with every graphical session, as a systemd user service
      that is restarted if it crashes'';

    settings = lib.mkOption {
      type = json.type;
      default = { };
      example = lib.literalExpression ''
        {
          background = ./wallpaper.jpg;
          tiles = [
            { name = "Files"; icon = "org.gnome.Nautilus"; desktop = "org.gnome.Nautilus.desktop"; }
            { name = "Power"; icon = "system-shutdown-symbolic"; submenu = "Power"; }
          ];
          submenus.Power = [
            { name = "Shutdown"; icon = "system-shutdown-symbolic"; builtin = "shutdown"; }
          ];
        }
      '';
      description = "Launcher configuration, written to /etc/vextheater/config.json.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];

    environment.etc."vextheater/config.json".source = json.generate "vextheater-config.json" cfg.settings;

    # The session's graphical-session.target pattern (systemd.special(7)). Apps
    # started from tiles move into scopes of their own, so a restart of this service
    # leaves them running.
    systemd.user.services.vextheater = lib.mkIf cfg.autostart {
      description = "VexTheater TV launcher";
      wantedBy = [ "graphical-session.target" ];
      partOf = [ "graphical-session.target" ];
      after = [ "graphical-session.target" ];
      unitConfig.ConditionEnvironment = "WAYLAND_DISPLAY";
      # Tile commands run through sh and find their programs (flatpak, browsers, …)
      # on PATH, so keep the session's PATH instead of NixOS's minimal service one.
      enableDefaultPath = false;
      serviceConfig = {
        # --resume: after a crash in desktop mode, come back in desktop mode.
        ExecStart = "${lib.getExe cfg.package} --resume";
        Restart = "on-failure";
        RestartSec = 2;
      };
    };

    # Same reason as VexPortal: GNOME Shell's app grid reads the aggregated icon
    # cache, which is only rebuilt when this is on (it defaults to
    # services.xserver.enable, false on Wayland-only GNOME hosts).
    gtk.iconCache.enable = lib.mkDefault true;
  };
}
