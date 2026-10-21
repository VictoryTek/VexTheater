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

    autostart = lib.mkEnableOption "starting VexTheater with every graphical session";

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
    environment.systemPackages = [ cfg.package ]
      ++ lib.optional cfg.autostart (pkgs.makeAutostartItem {
        name = "io.github.vextheater";
        package = cfg.package;
      });

    environment.etc."vextheater/config.json".source = json.generate "vextheater-config.json" cfg.settings;

    # Same reason as VexPortal: GNOME Shell's app grid reads the aggregated icon
    # cache, which is only rebuilt when this is on (it defaults to
    # services.xserver.enable, false on Wayland-only GNOME hosts).
    gtk.iconCache.enable = lib.mkDefault true;
  };
}
