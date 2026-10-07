# Container images of the services, built by Nix from the same packages as
# everything else, so each service compiles once. Linux only:
#   nix build .#image-console   (a script that streams the image; pipe it to `docker load`)
# On a Mac, nix/build-images.sh builds them inside Docker's own Linux VM.
#
# Each image holds the service, CA certificates, /app (the service, as the
# Dockerfile images had it) and a /data owned by the runtime user, so a named
# volume mounted there starts writable. It runs as 65532, not root.
{ ... }:
{
  perSystem =
    {
      pkgs,
      lib,
      config,
      ...
    }:
    let
      services = [
        "ticker"
        "console"
        "databento-jobs"
        "store"
        "alpaca-news"
        "alpaca-account"
        "alpaca-data"
        "econ-calendar"
        "relay"
      ];
      image =
        name:
        let
          exe = lib.getExe config.packages.${name};
        in
        pkgs.dockerTools.streamLayeredImage {
          name = "tinytrader/${name}";
          tag = "nix";
          contents = [ pkgs.cacert ];
          extraCommands = ''
            ln -s ${exe} app
            mkdir -p data tmp
            chmod 1777 tmp
          '';
          fakeRootCommands = ''
            chown 65532:65532 data
          '';
          config = {
            Entrypoint = [ "/app" ];
            User = "65532:65532";
            Env = [ "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt" ];
          };
        };
    in
    lib.mkIf pkgs.stdenv.hostPlatform.isLinux {
      packages = lib.listToAttrs (map (n: lib.nameValuePair "image-${n}" (image n)) services);
    };
}
