{ lib
, rustPlatform
, pkg-config
, wrapGAppsHook4
, glib
, gtk4
, libadwaita
, hicolor-icon-theme
}:

rustPlatform.buildRustPackage {
  pname = "vextheater";
  version = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).package.version;

  src = lib.cleanSource ../.;

  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    wrapGAppsHook4
    # glib-compile-resources, called by build.rs through glib-build-tools.
    glib
    # gtk4-update-icon-cache, called in postInstall.
    gtk4
  ];

  buildInputs = [
    gtk4
    libadwaita
    hicolor-icon-theme
  ];

  # wrapGAppsHook4 bakes XDG_DATA_DIRS from buildInputs into the wrapper but does not
  # add $out/share, so without this GTK cannot find the icon installed below.
  preFixup = ''
    gappsWrapperArgs+=(--prefix XDG_DATA_DIRS : "$out/share")
  '';

  postInstall = ''
    install -Dm644 data/io.github.vextheater.desktop \
      $out/share/applications/io.github.vextheater.desktop
    install -Dm644 data/io.github.vextheater.metainfo.xml \
      $out/share/metainfo/io.github.vextheater.metainfo.xml
    install -Dm644 data/icons/hicolor/scalable/apps/io.github.vextheater.svg \
      $out/share/icons/hicolor/scalable/apps/io.github.vextheater.svg
    gtk4-update-icon-cache -qtf $out/share/icons/hicolor
  '';

  meta = {
    description = "A full-screen TV launcher for GNOME";
    homepage = "https://github.com/VictoryTek/VexTheater";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "vextheater";
  };
}
