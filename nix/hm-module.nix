{ config, lib, pkgs, ... }:

let
  cfg = config.programs.nixclip;
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
  openFormatted = cfg.settings.keybind.open_formatted or "Super+V";
  openPlain = cfg.settings.keybind.open_plain or "Super+Shift+V";

  normalizeModifier = mod: modifierNames.${lib.toLower mod} or mod;

  normalizeKey =
    key:
    let
      lower = lib.toLower key;
    in
    if builtins.match "^[a-z]$" lower != null then lower else keyNames.${lower} or key;

  toGnomeBinding = str:
    let
      parts = lib.filter (part: part != "") (lib.splitString "+" str);
      mods = lib.init parts;
      rawKey = lib.last parts;
      key = normalizeKey rawKey;
      wrappedMods = lib.concatMapStrings (m: "<${normalizeModifier m}>") mods;
    in
    assert lib.assertMsg (parts != [ ]) "programs.nixclip keybinding must not be empty";
    assert lib.assertMsg (!(builtins.hasAttr (lib.toLower rawKey) modifierNames))
      "programs.nixclip keybinding must end with a non-modifier key";
    "${wrappedMods}${key}";
in

{
  options.programs.nixclip = {
    enable = lib.mkEnableOption "NixClip clipboard manager";

    package = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = null;
      description = ''
        The NixClip package to use. You must set this explicitly.

        Example:
          programs.nixclip.package = pkgs.callPackage /path/to/nixclip {};
      '';
    };

    settings = lib.mkOption {
      type = settingsFormat.type;
      default = { };
      description = ''
        NixClip configuration written to
        <filename>$XDG_CONFIG_HOME/nixclip/config.toml</filename>
        (typically <filename>~/.config/nixclip/config.toml</filename>).

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
          programs.nixclip.package must be set.
          See the NixClip README for installation instructions.
        '';
      }
    ];

    dconf.settings = {
      "org/gnome/shell/keybindings" = {
        toggle-message-tray = [ "<Super>m" ];
      };

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

    # Make both the daemon and the GUI client available in the user environment.
    home.packages = lib.mkIf (cfg.package != null) [ cfg.package ];

    # Home Manager uses the systemd.user.services.<name> attrset structure that
    # mirrors the INI sections of a unit file directly.
    systemd.user.services.nixclipd = {
      Unit = {
        Description = "NixClip clipboard daemon";
        Documentation = "https://github.com/adityapidaparti/nixclip";
        PartOf = [ "graphical-session.target" ];
        After = [ "graphical-session.target" ];
      };

      Service = {
        ExecStart = lib.mkIf (cfg.package != null) "${cfg.package}/bin/nixclipd";
        Restart = "on-failure";
        RestartSec = 3;
        Environment = [
          "NIXCLIP_DISABLE_PORTAL_HOTKEYS=1"
        ];

        # Resource limits.
        MemoryMax = "256M";

        # Hardening.
        PrivateTmp = true;
        NoNewPrivileges = true;

        Type = "simple";
      };

      Install = {
        WantedBy = [ "graphical-session.target" ];
      };
    };

    # Write the TOML config only when the user has provided settings.
    xdg.configFile."nixclip/config.toml" = lib.mkIf (cfg.settings != { }) {
      source = settingsFormat.generate "nixclip-config.toml" cfg.settings;
    };
  };
}
