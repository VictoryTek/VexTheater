{
  description = "VexTheater — a full-screen TV launcher for GNOME";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        packages.default = pkgs.callPackage ./nix/package.nix { };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            cargo
            rustc
            rust-analyzer
            clippy
            rustfmt
            pkg-config
            glib              # glib-compile-resources, for build.rs
            desktop-file-utils
            appstream         # appstreamcli validate
          ];

          buildInputs = with pkgs; [
            gtk4
            libadwaita
            glib
          ];
        };

        checks.default = self.packages.${system}.default;
      }) // {

      overlays.default = final: prev: {
        vextheater = final.callPackage ./nix/package.nix { };
      };

      # Installs the launcher, writes its tile config, and optionally starts it with
      # the GNOME session. Consumed by vexos-nix's HTPC role.
      nixosModules.default = import ./nix/module.nix self;
      nixosModules.vextheater = self.nixosModules.default;
    };
}
