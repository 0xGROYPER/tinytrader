# The options of tinytrader.services.<name>, shared by the NixOS module
# (systemd units, services.nix) and the nix-darwin module (launchd agents,
# darwin-services.nix), so one service definition runs on either.
{ lib, ... }:
let
  inherit (lib) mkOption types;
in
{
  options.tinytrader.services = mkOption {
    default = { };
    description = "tinytrader services to run on this host.";
    type = types.attrsOf (
      types.submodule (
        { name, ... }:
        {
          options = {
            enable = lib.mkEnableOption "the tinytrader ${name} service";
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
            secrets = mkOption {
              type = types.attrsOf types.str;
              default = { };
              example = {
                DATABENTO_API_KEY = "/run/secrets/databento-api-key";
              };
              description = ''
                Secrets the service reads, as environment variable name → the
                file holding the value (decrypted at boot into memory, as
                sops-nix does into /run/secrets). Each is handed to the service
                as a systemd credential, readable by it alone, and named to it
                by `<NAME>_FILE`, which tinytrader services read in place of
                `<NAME>`. The value never appears in its environment, its unit
                or the store.
              '';
            };
            writablePaths = mkOption {
              type = types.listOf types.str;
              default = [ ];
              description = ''
                Extra paths the service may write, such as a data root. Everything
                else outside its state directory is read-only to it.
              '';
            };
            resources = {
              memory = mkOption {
                type = types.str;
                default = "1G";
                description = "Hard memory limit (systemd MemoryMax); throttling starts at 85% of it.";
              };
              cpu = mkOption {
                type = types.nullOr types.str;
                default = null;
                example = "150%";
                description = "CPU cap (systemd CPUQuota), e.g. 150% for one and a half cores. Unlimited by default.";
              };
              tasks = mkOption {
                type = types.ints.positive;
                default = 512;
                description = "Maximum threads and processes (TasksMax).";
              };
              openFiles = mkOption {
                type = types.ints.positive;
                default = 65536;
                description = "Maximum open files and sockets (LimitNOFILE).";
              };
            };
            launchd = mkOption {
              type = types.enum [
                "agent"
                "daemon"
              ];
              default = "agent";
              description = ''
                macOS only: how launchd runs the service, always as the primary
                user. "agent" (launchd.user.agents) starts it when the user logs
                in; "daemon" (launchd.daemons, with UserName set) starts it at
                boot without a login, so it comes back after a reboot or power
                cut. With FileVault on, boot waits for the disk to be unlocked
                at the login window either way. Ignored on Linux.
              '';
            };
            observe = {
              port = mkOption {
                type = types.nullOr types.port;
                default = null;
                description = "Port for the service's health, metrics and other endpoints.";
              };
              bind = mkOption {
                type = types.str;
                default = "127.0.0.1";
                description = "Address to bind the endpoints to; loopback unless exposed on purpose.";
              };
            };
          };
        }
      )
    );
  };
}
