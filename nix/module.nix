{ config, lib, pkgs, ... }:

let
  cfg = config.services.nixclip;
  settingsFormat = pkgs.formats.toml { };
in

{
  options.services.nixclip = {
    enable = lib.mkEnableOption "NixClip clipboard manager";

    package = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = null;
      description = ''
        The NixClip package to use. You must set this explicitly; add the
        NixClip flake as an input and pass its package here.

        Example flake usage:
          services.nixclip.package = inputs.nixclip.packages.''${pkgs.system}.default;
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
          Add the NixClip flake as an input and point this option to the package.
        '';
      }
    ];

    # Make both the daemon and the GUI client available system-wide.
    environment.systemPackages = lib.mkIf (cfg.package != null) [ cfg.package ];

    # User-scoped systemd service so it can access the graphical session and
    # the user's clipboard over Wayland / X11.
    systemd.user.services.nixclipd = {
      description = "NixClip clipboard daemon";
      documentation = [ "https://github.com/your-org/nixclip" ];
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
      };
    };

    # Write the TOML config only when the user has provided settings.
    environment.etc."xdg/nixclip/config.toml" = lib.mkIf (cfg.settings != { }) {
      source = settingsFormat.generate "nixclip-config.toml" cfg.settings;
    };
  };
}
