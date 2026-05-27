{
  pkgs,
  config,
  lib,
  ...
}:
let
  cfg = config.services.not-yet;
  staticUser = cfg.user != null && cfg.group != null;
in
{
  options.services.not-yet = {
    enable = lib.mkEnableOption "an LLM based notification control tool";
    package = lib.mkPackageOption pkgs "not-yet-telegram" { };

    user = lib.mkOption {
      type = with lib.types; nullOr str;
      default = null;
      example = "notyet";
      description = ''
        User account under which to run not-yet. Defaults to [`DynamicUser`](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html#DynamicUser=)
        when set to `null`.

        The user will automatically be created, if this option is set to a non-null value.
      '';
    };
    group = lib.mkOption {
      type = with lib.types; nullOr str;
      default = cfg.user;
      defaultText = lib.literalExpression "config.services.not-yet.user";
      example = "notyet";
      description = ''
        Group under which to run not-yet. Only used when `services.not-yet.user` is set.

        The group will automatically be created, if this option is set to a non-null value.
      '';
    };

    botToken = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Telegram bot token.";
    };
    dataPath = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/not-yet";
      description = "The data directory where database and dialog histories are stored.";
    };
    extraEnv = lib.mkOption {
      default = null;
      type = lib.types.nullOr lib.types.envVar;
      description = "Extra environment variables to use.";
    };
    extraOpts = lib.mkOption {
      default = null;
      type = lib.types.nullOr lib.types.str;
      description = "Extra command line options to use.";
    };
  };

  config = lib.mkIf cfg.enable {
    users = lib.mkIf staticUser {
      users.${cfg.user} = {
        inherit (cfg) home;
        isSystemUser = true;
        group = cfg.group;
      };
      groups.${cfg.group} = { };
    };
    systemd.services.not-yet = {
      description = "not-yet telegram bot daemon, an LLM based notification control app";
      requires = [ "network-online.target" ];
      after = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig =
        lib.optionalAttrs staticUser {
          User = cfg.user;
          Group = cfg.group;
        }
        // {
          ExecStart =
            (lib.concatStringsSep " " [
              (lib.getExe cfg.package)
              (lib.optionalString (cfg.botToken != null) "--bot-token ${cfg.botToken}")
              "--config ${cfg.dataPath}"
            ])
            + lib.optionalString (cfg.extraOpts != null) cfg.extraOpts;
          Environment = lib.optional (cfg.extraEnv != null) cfg.extraEnv;
          DynamicUser = true;
          WorkingDirectory = cfg.dataPath;
          ReadWritePaths = [ cfg.dataPath ];
        };
    };
  };
}
