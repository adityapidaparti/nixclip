# Standalone derivation for non-flake consumption.
# Flake users get this via `packages.${system}.default`; everyone else can
# `callPackage ./default.nix {}` or use fetchTarball + import.

{ pkgs ? import <nixpkgs> {} }:

pkgs.rustPlatform.buildRustPackage {
  pname = "nixclip";
  version = "0.1.0";

  src = pkgs.lib.cleanSource ./.;

  cargoLock.lockFile = ./Cargo.lock;
  cargoBuildFlags = [ "--workspace" "--bins" ];

  nativeBuildInputs = with pkgs; [
    pkg-config
    wrapGAppsHook4
    makeWrapper
  ];

  buildInputs = with pkgs; [
    gtk4
    libadwaita
    glib
    wayland
    wayland-protocols
    cairo
    pango
    gdk-pixbuf
    graphene
  ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
    wl-clipboard
    wayland-utils
  ];

  postFixup = pkgs.lib.optionalString pkgs.stdenv.isLinux ''
    for bin in $out/bin/*; do
      wrapProgram "$bin" \
        --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.wl-clipboard pkgs.wayland-utils ]}
    done
  '';

  installPhase = ''
    runHook preInstall
    mkdir -p "$out/bin"
    for bin in nixclipd nixclip nixclip-ui; do
      install -Dm755 "target/release/$bin" "$out/bin/$bin"
    done
    install -Dm644 \
      ${./nix/desktop-entry.desktop} \
      "$out/share/applications/com.nixclip.NixClip.desktop"
    runHook postInstall
  '';

  meta = with pkgs.lib; {
    description = "Clipboard history manager for GNOME Wayland";
    license = licenses.mit;
    platforms = platforms.linux;
    mainProgram = "nixclip";
  };
}
