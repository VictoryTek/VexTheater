{ lib
, fetchurl
, rustPlatform
, pkg-config
, wrapGAppsHook4
, glib
, gtk4
, libadwaita
, hicolor-icon-theme
, gst_all_1
}:

let
  # The logos of the services in src/catalog.rs, one per catalog id, installed into the
  # icon theme as vextheater-<id>. They are fetched at build time, not kept in the
  # repository: the collections allow redistribution (selfhst/icons is CC BY 4.0,
  # homarr-labs/dashboard-icons Apache-2.0), but the logos are the services' trademarks.
  # Both are pinned to a commit. The "-light" files are the white versions, which stay
  # visible on the dark theme. A test in src/catalog.rs checks every catalog id is here.
  selfhst = file: "https://cdn.jsdelivr.net/gh/selfhst/icons@1d65d45a42e6323b54b8096967b1ea4b65321f97/svg/${file}.svg";
  homarr = file: "https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@d3b9234dd56f3af5862f06741f0dce101193b3df/svg/${file}.svg";
  catalogLogos = {
    "netflix" = { url = selfhst "netflix"; hash = "sha256-j7231HA9ZjPZY5PGfbxMXrOXS46y2ibleAUyBAeH8MM="; };
    "disney-plus" = { url = selfhst "disney-plus-light"; hash = "sha256-c8TcbTsAFaH0lM7C05vHYUISXqKXg47Tl//0bEHQsMg="; };
    "hulu" = { url = selfhst "hulu"; hash = "sha256-y3aQgCyeOjHQcQHpF/RnV+Fj97OEEbNWgpOTeg7gTv4="; };
    "max" = { url = selfhst "hbo-max-light"; hash = "sha256-aYMi8pzGZ6Z2XkZ7DZOooygvKUeErO38/VJfHFVoAF8="; };
    "peacock" = { url = selfhst "peacock-light"; hash = "sha256-HpqXmhAai5zHuWTJuzq0brh3DOZVyLEJ81yBAUvsisM="; };
    "paramount-plus" = { url = selfhst "paramount-plus"; hash = "sha256-Q5/aAz3MzQW2RcATalG2/s0vWm54fKQ3pZfTk88CedE="; };
    "prime-video" = { url = homarr "prime-video-light"; hash = "sha256-mtdgy3Qwe0pb9QWUl1NrmB3l3RsmCCFfGARa9UzhmG0="; };
    "apple-tv" = { url = selfhst "apple-tv-light"; hash = "sha256-RihWIftS0/ybX+8OUW1Iy31hvFNHoaM+bBGo9o6oRpU="; };
    "youtube" = { url = selfhst "youtube"; hash = "sha256-iKHp5Eg3VRwF1uiClmSSGmm6T2zIariL9kkB3cHz538="; };
    "youtube-tv" = { url = homarr "youtube-tv"; hash = "sha256-Bf8mTwOGkijP6KuZS8dPhO8V4SBUidYbBgSkX9F4sEU="; };
    "crunchyroll" = { url = selfhst "crunchyroll"; hash = "sha256-lr8Se0zuJ2xf3UtL7ee4qby8Ou1jaaS6UuhKJZ6ijA4="; };
  };
  installCatalogLogos = lib.concatStringsSep "\n" (lib.mapAttrsToList
    (id: logo: ''
      install -Dm644 ${fetchurl logo} \
        $out/share/icons/hicolor/scalable/apps/vextheater-${id}.svg
    '')
    catalogLogos);
in
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
    # The click sounds: GTK plays them through GStreamer. -base has the player and
    # audio conversion, -good the WAV parser and the output to PipeWire's PulseAudio
    # server; wrapGAppsHook4 puts their plugin directories on the wrapper's path.
    gst_all_1.gst-plugins-base
    gst_all_1.gst-plugins-good
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
    install -Dm644 data/icons/hicolor/512x512/apps/io.github.vextheater.png \
      $out/share/icons/hicolor/512x512/apps/io.github.vextheater.png
    ${installCatalogLogos}
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
