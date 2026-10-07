# tinytrader.services.<name> on macOS (nix-darwin): the same definitions as
# on NixOS (service-options.nix), run as launchd agents of the host's
# primary user, so a service sees the drives that user attaches.
#
# Each service is com.tinytrader.<name>: started at login, restarted when it
# exits (at most every 10 s), logging to ~/Library/Logs/tinytrader/<name>.log.
# The system's newsyslog rotates those logs past 10 MB, keeping 7.
#
# launchd has no sandbox or memory limits like systemd's: `writablePaths`
# and `resources` other than `openFiles` apply on Linux only.
{ config, lib, ... }:
let
  enabled = lib.filterAttrs (_: s: s.enable) config.tinytrader.services;
  user = config.system.primaryUser;
  logs = "/Users/${user}/Library/Logs/tinytrader";
in
{
  imports = [ ./service-options.nix ];

  config = lib.mkIf (enabled != { }) {
    launchd.user.agents = lib.mapAttrs' (
      name: svc:
      lib.nameValuePair "tinytrader-${name}" {
        serviceConfig = {
          Label = "com.tinytrader.${name}";
          ProgramArguments = [ (lib.getExe svc.package) ] ++ svc.args;
          EnvironmentVariables =
            lib.optionalAttrs (svc.observe.port != null) {
              TINYTRADER_LISTEN = "${svc.observe.bind}:${toString svc.observe.port}";
            }
            // svc.environment;
          RunAtLoad = true;
          KeepAlive = true;
          ThrottleInterval = 10;
          SoftResourceLimits.NumberOfFiles = svc.resources.openFiles;
          StandardOutPath = "${logs}/${name}.log";
          StandardErrorPath = "${logs}/${name}.log";
        };
      }
    ) enabled;

    # launchd opens the log files but doesn't create their directory.
    system.activationScripts.postActivation.text = ''
      mkdir -p ${lib.escapeShellArg logs}
      chown ${lib.escapeShellArg user}:staff ${lib.escapeShellArg logs}
    '';

    environment.etc."newsyslog.d/tinytrader.conf".text = ''
      # logfile                     owner:group  mode count size(KB) when flags
      ${logs}/*.log  ${user}:staff  644  7     10240    *    GJN
    '';
  };
}
