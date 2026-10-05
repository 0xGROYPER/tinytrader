# glacier.services.<name>: run a Glacier service as a hardened systemd unit.
#
#   glacier.services.ticker = {
#     enable = true;
#     package = glacier.packages.x86_64-linux.ticker;
#     environment.TICKER_LISTEN = "127.0.0.1:9100";
#   };
#
# Each service gets its own transient user, a private state directory
# (/var/lib/glacier/<name>), write access to nothing else unless granted
# through `writablePaths`, and restart on failure. Health and metrics stay on
# loopback; exposing them beyond the host is a separate, explicit choice.
{ config, lib, ... }:
let
  inherit (lib) mkOption types;
  enabled = lib.filterAttrs (_: s: s.enable) config.glacier.services;
in
{
  options.glacier.services = mkOption {
    default = { };
    description = "Glacier services to run on this host.";
    type = types.attrsOf (
      types.submodule (
        { name, ... }:
        {
          options = {
            enable = lib.mkEnableOption "the Glacier ${name} service";
            package = mkOption {
              type = types.package;
              description = "Package providing the service; its main program is run.";
            };
            args = mkOption {
              type = types.listOf types.str;
              default = [ ];
              description = "Command-line arguments.";
            };
            environment = mkOption {
              type = types.attrsOf types.str;
              default = { };
              description = "Environment variables (non-secret configuration).";
            };
            writablePaths = mkOption {
              type = types.listOf types.str;
              default = [ ];
              description = ''
                Extra paths the service may write, such as a data root. Everything
                else outside its state directory is read-only to it.
              '';
            };
          };
        }
      )
    );
  };

  config.systemd.services = lib.mapAttrs' (
    name: svc:
    lib.nameValuePair "glacier-${name}" {
      description = "Glacier ${name}";
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];
      environment = svc.environment;
      serviceConfig = {
        ExecStart = lib.escapeShellArgs ([ (lib.getExe svc.package) ] ++ svc.args);
        Restart = "on-failure";
        RestartSec = 2;
        DynamicUser = true;
        StateDirectory = "glacier/${name}";
        WorkingDirectory = "/var/lib/glacier/${name}";
        ReadWritePaths = svc.writablePaths;
        # Sandboxing
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        NoNewPrivileges = true;
        CapabilityBoundingSet = "";
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectControlGroups = true;
        ProtectClock = true;
        ProtectHostname = true;
        RestrictSUIDSGID = true;
        RestrictNamespaces = true;
        RestrictRealtime = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        SystemCallArchitectures = "native";
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
        ];
      };
    }
  ) enabled;
}
