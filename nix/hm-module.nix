{ config, lib, pkgs, ... }:

let
  cfg = config.programs.nixclip;
  settingsFormat = pkgs.formats.toml { };
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

    # Make both the daemon and the GUI client available in the user environment.
    home.packages = lib.mkIf (cfg.package != null) [ cfg.package ];

    # Home Manager uses the systemd.user.services.<name> attrset structure
    # that mirrors the INI sections of a unit file directly.
    systemd.user.services.nixclipd = {
      Unit = {
        Description = "NixClip clipboard daemon";
        Documentation = "https://github.com/your-org/nixclip";
        PartOf = [ "graphical-session.target" ];
        After = [ "graphical-session.target" ];
      };

      Service = {
        ExecStart = lib.mkIf (cfg.package != null) "${cfg.package}/bin/nixclipd";
        Restart = "on-failure";
        RestartSec = 3;

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
