{
  description = "NixClip - clipboard history manager for GNOME Wayland";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, crane, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        craneLib = crane.mkLib pkgs;

        # Filter source to only include Rust-relevant files.
        src = craneLib.cleanCargoSource ./.;

        # Common arguments shared by all crane derivations.
        commonArgs = {
          inherit src;
          strictDeps = true;

          nativeBuildInputs = with pkgs; [
            pkg-config
            wrapGAppsHook4
          ];

          buildInputs = with pkgs; [
            # GTK4 / libadwaita (UI crate)
            gtk4
            libadwaita
            glib

            # Wayland (daemon crate)
            wayland
            wayland-protocols

            # Graphics stack pulled in transitively; listed explicitly so
            # pkg-config can locate them without relying on propagation.
            cairo
            pango
            gdk-pixbuf
            graphene

            # rusqlite uses the "bundled" feature (compiles SQLite from
            # source), so no system sqlite is required at link time.
            # pkg-config is still needed for the libraries above.
          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            darwin.apple_sdk.frameworks.SystemConfiguration
          ];
        };

        # Build only the Cargo dependencies — this derivation is cached
        # between rebuilds so that the full workspace can be compiled
        # incrementally.
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # Build the full workspace. crane installs all workspace binaries
        # automatically: nixclipd, nixclip (CLI), and nixclip-ui (GTK popup).
        nixclip = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;

          postInstall = ''
            # Verify that all expected binaries were installed.
            for bin in nixclipd nixclip nixclip-ui; do
              if [ ! -f "$out/bin/$bin" ]; then
                echo "Warning: expected binary '$bin' not found in \$out/bin"
              fi
            done

            # Install the freedesktop desktop entry.
            install -Dm644 \
              ${./nix/desktop-entry.desktop} \
              "$out/share/applications/com.nixclip.NixClip.desktop"
          '';

          meta = with pkgs.lib; {
            description = "Clipboard history manager for GNOME Wayland";
            license = licenses.mit;
            # GTK4 / Wayland — Linux only.
            platforms = platforms.linux;
            mainProgram = "nixclip";
          };
        });
      in
      {
        packages = {
          default = nixclip;
          inherit nixclip;
        };

        checks = {
          # Build check — reuses the package derivation.
          inherit nixclip;

          # Clippy — treat all warnings as errors.
          nixclip-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          });

          # Formatting check.
          nixclip-fmt = craneLib.cargoFmt { inherit src; };
        };

        devShells.default = craneLib.devShell {
          # Pull in all check derivations so their inputs are available.
          checks = self.checks.${system};

          packages = with pkgs; [
            # Rust tooling
            rust-analyzer
            cargo-watch
            cargo-expand

            # Helpful for inspecting GTK/GLib at runtime
            glib
            gtk4
          ];
        };
      }
    ) // {
      # NixOS system-level module (manages the nixclipd systemd user service).
      nixosModules.default = import ./nix/module.nix;

      # Home Manager module (user-scoped daemon + config).
      homeManagerModules.default = import ./nix/hm-module.nix;

      # Package overlay — lets consumers add nixclip to their nixpkgs.
      overlays.default = final: prev: {
        nixclip = self.packages.${prev.system}.default;
      };
    };
}
