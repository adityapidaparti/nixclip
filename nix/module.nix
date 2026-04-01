{ config, lib, pkgs, ... }:

let
  cfg = config.services.nixclip;
  settingsFormat = pkgs.formats.toml { };
  modifierNames = {
    ctrl = "Control";
    control = "Control";
    alt = "Alt";
    shift = "Shift";
    super = "Super";
    meta = "Meta";
    primary = "Primary";
  };
  keyNames = {
    esc = "Escape";
    escape = "Escape";
    enter = "Return";
    return = "Return";
    tab = "Tab";
    space = "space";
    backspace = "BackSpace";
    delete = "Delete";
    insert = "Insert";
    home = "Home";
    end = "End";
    pageup = "Page_Up";
    "page-up" = "Page_Up";
    pagedown = "Page_Down";
    "page-down" = "Page_Down";
    left = "Left";
    right = "Right";
    up = "Up";
    down = "Down";
    plus = "plus";
    minus = "minus";
    comma = "comma";
    period = "period";
  };

  # Resolve keybinds from settings or fall back to defaults.
  openFormatted = cfg.settings.keybind.open_formatted or "Super+V";
  openPlain = cfg.settings.keybind.open_plain or "Super+Shift+V";

  normalizeModifier = mod: modifierNames.${lib.toLower mod} or mod;

  normalizeKey =
    key:
    let
      lower = lib.toLower key;
    in
    if builtins.match "^[a-z]$" lower != null then lower else keyNames.${lower} or key;

  # Convert "Super+Shift+V" -> "<Super><Shift>v" for GNOME dconf.
  toGnomeBinding = str:
    let
      parts = lib.filter (part: part != "") (lib.splitString "+" str);
      mods = lib.init parts;
      rawKey = lib.last parts;
      key = normalizeKey rawKey;
      wrappedMods = lib.concatMapStrings (m: "<${normalizeModifier m}>") mods;
    in
    assert lib.assertMsg (parts != [ ]) "services.nixclip keybinding must not be empty";
    assert lib.assertMsg (!(builtins.hasAttr (lib.toLower rawKey) modifierNames))
      "services.nixclip keybinding must end with a non-modifier key";
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
        # GNOME stores the list of custom bindings as one string array. This
        # sets a system default, but it cannot merge with an existing per-user
        # custom-keybindings value in user-db:user.
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
    # the user's clipboard on Wayland.
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
        Environment = [
          "NIXCLIP_DISABLE_PORTAL_HOTKEYS=1"
        ];

        # Import WAYLAND_DISPLAY, DISPLAY, etc. from the graphical session
        # so the daemon can connect to the compositor. GNOME's session manager
        # exports these to the systemd user manager, but services don't
        # automatically inherit them without PassEnvironment.
        PassEnvironment = [
          "WAYLAND_DISPLAY"
          "DISPLAY"
          "XDG_SESSION_TYPE"
        ];

        # Resource limits.
        MemoryMax = "256M";

        # Hardening.
        PrivateTmp = true;
        NoNewPrivileges = true;

        Type = "simple";
      };
    };

    # Write the TOML config only when the user has provided settings.
    environment.etc."xdg/nixclip/config.toml" = lib.mkIf (cfg.settings != { }) {
      source = settingsFormat.generate "nixclip-config.toml" cfg.settings;
    };
  };
}
