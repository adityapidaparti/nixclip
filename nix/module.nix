{ config, lib, pkgs, ... }:

let
  cfg = config.services.nixclip;
  settingsFormat = pkgs.formats.toml { };

  # Resolve keybinds from settings or fall back to defaults.
  openFormatted = cfg.settings.keybind.open_formatted or "Super+V";
  openPlain = cfg.settings.keybind.open_plain or "Super+Shift+V";

  # Convert "Super+Shift+V" → "<Super><Shift>v" for GNOME dconf.
  toGnomeBinding = str:
    let
      parts = lib.splitString "+" str;
      mods = lib.init parts;
      key = lib.toLower (lib.last parts);
      wrappedMods = lib.concatMapStrings (m: "<${m}>") mods;
    in
    "${wrappedMods}${key}";
in

{
  options.services.nixclip = {
    enable = lib.mkEnableOption "NixClip clipboard manager";

    package = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = null;
      description = ''
        The NixClip package to use. You must set this explicitly.

        Example:
          services.nixclip.package = pkgs.callPackage /path/to/nixclip {};
      '';
    };

    settings = lib.mkOption {
      type = settingsFormat.type;
      default = { };
      description = ''
        NixClip configuration written to
        <filename>/etc/xdg/nixclip/config.toml</filename>.

        See the NixClip documentation for all available options.
      '';
      example = lib.literalExpression ''
        {
          general = {
            max_entries = 2000;
            retention = "6months";
          };
          ignore = {
            apps = [ "org.keepassxc.KeePassXC" ];
          };
        }
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.package != null;
        message = ''
          services.nixclip.package must be set.
          See the NixClip README for installation instructions.
        '';
      }
    ];

    # Enable dconf so GNOME picks up our keybinding overrides.
    programs.dconf.enable = true;

    # Set up GNOME custom keyboard shortcuts via dconf.
    # This is more reliable than the XDG GlobalShortcuts portal for
    # native (non-Flatpak) apps on GNOME.
    programs.dconf.profiles.user.databases = [{
      settings = {
        # Free Super+V from GNOME's notification tray but keep Super+M.
        "org/gnome/shell/keybindings" = {
          toggle-message-tray = [ "<Super>m" ];
        };

        # Register NixClip custom shortcuts.
        "org/gnome/settings-daemon/plugins/media-keys" = {
          custom-keybindings = [
            "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/nixclip0/"
            "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/nixclip1/"
          ];
        };

        "org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/nixclip0" = {
          name = "NixClip (paste with formatting)";
          command = "${cfg.package}/bin/nixclip-ui";
          binding = toGnomeBinding openFormatted;
        };

        "org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/nixclip1" = {
          name = "NixClip (paste as plain text)";
          command = "${cfg.package}/bin/nixclip-ui --plain";
          binding = toGnomeBinding openPlain;
        };
      };
    }];

    # Make both the daemon and the GUI client available system-wide.
    environment.systemPackages = lib.mkIf (cfg.package != null) [ cfg.package ];

    # User-scoped systemd service so it can access the graphical session and
    # the user's clipboard over Wayland / X11.
    systemd.user.services.nixclipd = {
      description = "NixClip clipboard daemon";
      documentation = [ "https://github.com/adityapidaparti/nixclip" ];
      partOf = [ "graphical-session.target" ];
      after = [ "graphical-session.target" ];
      wantedBy = [ "graphical-session.target" ];

      serviceConfig = {
        ExecStart = lib.mkIf (cfg.package != null) "${cfg.package}/bin/nixclipd";
        Restart = "on-failure";
        RestartSec = 3;

        # Resource limits.
        MemoryMax = "256M";

        # Hardening.
        PrivateTmp = true;
        NoNewPrivileges = true;

        Type = "simple";

        # Import WAYLAND_DISPLAY, DISPLAY, etc. from the graphical session
        # so the daemon can connect to the compositor. The actual socket name
        # is session-specific (wayland-0, wayland-1, etc.), so we must not
        # hardcode it.
        PassEnvironment = [
          "WAYLAND_DISPLAY"
          "DISPLAY"
          "XDG_SESSION_TYPE"
        ];
      };
    };

    # Write the TOML config only when the user has provided settings.
    environment.etc."xdg/nixclip/config.toml" = lib.mkIf (cfg.settings != { }) {
      source = settingsFormat.generate "nixclip-config.toml" cfg.settings;
    };
  };
}
