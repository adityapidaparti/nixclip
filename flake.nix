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
          pname = "nixclip";
          version = "0.1.0";
          strictDeps = true;
          cargoExtraArgs = "--workspace";

          nativeBuildInputs = with pkgs; [
            pkg-config
            wrapGAppsHook4
            makeWrapper
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
          ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
            wl-clipboard
            wayland-utils
          ];

          makeWrapperArgs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            "--prefix"
            "PATH"
            ":"
            (pkgs.lib.makeBinPath [ pkgs.wl-clipboard pkgs.wayland-utils ])
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
            for bin in nixclipd nixclip nixclip-ui; do
              test -x "$out/bin/$bin"
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

        # Evaluate the NixOS module in a real NixOS module graph. This uses a
        # tiny package stand-in so the module wiring check does not duplicate
        # the full Rust package build.
        moduleSmokePackage = pkgs.runCommand "nixclip-module-smoke-package" { } ''
          mkdir -p "$out/bin"
          touch "$out/bin/nixclipd" "$out/bin/nixclip-ui"
          chmod +x "$out/bin/nixclipd" "$out/bin/nixclip-ui"
        '';

        nixosModuleSmoke = nixpkgs.lib.nixosSystem {
          inherit system;
          modules = [
            ./nix/module.nix
            ({ ... }: {
              system.stateVersion = "25.11";
              services.nixclip = {
                enable = true;
                package = moduleSmokePackage;
                settings = {
                  general.max_entries = 250;
                  keybind = {
                    open_formatted = "Super+V";
                    open_plain = "Super+Shift+V";
                  };
                };
              };
            })
          ];
        };

        nixosModuleDconf =
          (builtins.elemAt
            nixosModuleSmoke.config.programs.dconf.profiles.user.databases
            0).settings;
        nixosModuleService =
          nixosModuleSmoke.config.systemd.user.services.nixclipd.serviceConfig;
        nixosModuleProbe = pkgs.writeText "nixclip-nixos-module-smoke.json"
          (builtins.toJSON {
            execStart = nixosModuleService.ExecStart;
            environment = nixosModuleService.Environment;
            passEnvironment = nixosModuleService.PassEnvironment;
            formattedBinding =
              nixosModuleDconf."org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/nixclip0".binding;
            formattedCommand =
              nixosModuleDconf."org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/nixclip0".command;
            plainBinding =
              nixosModuleDconf."org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/nixclip1".binding;
            plainCommand =
              nixosModuleDconf."org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/nixclip1".command;
            messageTray =
              nixosModuleDconf."org/gnome/shell/keybindings".toggle-message-tray;
            customKeybindings =
              nixosModuleDconf."org/gnome/settings-daemon/plugins/media-keys".custom-keybindings;
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
          nixclip-fmt = craneLib.cargoFmt {
            inherit src;
            pname = "nixclip";
            version = "0.1.0";
          };

          nixclip-nixos-module = pkgs.runCommand "nixclip-nixos-module-smoke"
            {
              nativeBuildInputs = with pkgs; [
                gnugrep
                jq
              ];
            } ''
            jq -e '
              .execStart | test("/bin/nixclipd$")
            ' ${nixosModuleProbe} >/dev/null

            jq -e '
              .formattedBinding == "<Super>v" and
              (.formattedCommand | test("/bin/nixclip-ui$")) and
              .plainBinding == "<Super><Shift>v" and
              (.plainCommand | test("/bin/nixclip-ui --plain$")) and
              (.environment | index("NIXCLIP_DISABLE_PORTAL_HOTKEYS=1")) and
              (.passEnvironment | index("WAYLAND_DISPLAY")) and
              (.passEnvironment | index("XDG_SESSION_TYPE")) and
              (.messageTray == ["<Super>m"]) and
              (.customKeybindings == [
                "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/nixclip0/",
                "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/nixclip1/"
              ])
            ' ${nixosModuleProbe} >/dev/null

            grep -F 'max_entries = 250' \
              ${nixosModuleSmoke.config.environment.etc."xdg/nixclip/config.toml".source}
            grep -F 'open_formatted = "Super+V"' \
              ${nixosModuleSmoke.config.environment.etc."xdg/nixclip/config.toml".source}

            mkdir -p "$out"
            touch "$out/passed"
          '';

          # Headless Wayland smoke test. This starts a compositor, launches the
          # packaged daemon, writes to the clipboard, verifies IPC/search/restore,
          # sends Super+V/Super+Shift+V through a virtual keyboard, and verifies
          # that the GTK UI commands behind those shortcuts launch.
          nixclip-e2e-smoke = pkgs.runCommand "nixclip-e2e-smoke"
            {
              nativeBuildInputs = with pkgs; [
                bash
                coreutils
                dbus
                gnugrep
                gnused
                jq
                sway
                wayland-utils
                wl-clipboard
                wtype
              ];
            } ''
            export PATH=${pkgs.lib.makeBinPath [
              nixclip
              pkgs.bash
              pkgs.coreutils
              pkgs.dbus
              pkgs.gnugrep
              pkgs.gnused
              pkgs.jq
              pkgs.sway
              pkgs.wayland-utils
              pkgs.wl-clipboard
              pkgs.wtype
            ]}:$PATH
            export DBUS_SESSION_BUS_CONFIG_FILE=${pkgs.dbus}/share/dbus-1/session.conf

            bash ${./nix/e2e-smoke.sh}

            mkdir -p "$out"
            touch "$out/passed"
          '';
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
