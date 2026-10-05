# checks.<system>.generic: fail if deployment-specific details leak into the
# platform. A backstop for CONTRIBUTING.md, not a replacement for it.
{ ... }:
{
  perSystem =
    { pkgs, lib, ... }:
    let
      # Every tracked file except this one, which necessarily contains the patterns.
      src = lib.fileset.toSource {
        root = ../.;
        fileset = lib.fileset.difference (lib.fileset.gitTracked ../.) ./generic.nix;
      };
      patterns = [
        "/Users/[A-Za-z]" # macOS home directories
        "/home/[a-z]" # Linux home directories
        "\\.ts\\.net" # Tailscale MagicDNS names
        "\\b100\\.(6[4-9]|[7-9][0-9]|1[01][0-9]|12[0-7])\\.[0-9]+\\.[0-9]+\\b" # Tailscale CGNAT addresses
        "\\b192\\.168\\.[0-9]+\\.[0-9]+\\b" # home LAN addresses
        "\\b10\\.[0-9]+\\.[0-9]+\\.[0-9]+\\b" # private addresses
        "BEGIN [A-Z ]*PRIVATE KEY" # key material
      ];
    in
    {
      checks.generic =
        pkgs.runCommand "glacier-generic-check" { nativeBuildInputs = [ pkgs.gnugrep ]; }
          ''
            cd ${src}
            status=0
            ${lib.concatMapStrings (p: ''
              if grep -rnIE ${lib.escapeShellArg p} . ; then
                echo "generic check: found a deployment-specific pattern: ${lib.escapeShellArg p}" >&2
                status=1
              fi
            '') patterns}
            if [ "$status" -ne 0 ]; then
              echo "Move deployment-specific details to your deployment repository (see CONTRIBUTING.md)." >&2
              exit 1
            fi
            touch $out
          '';
    };
}
