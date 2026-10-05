# Rust workspace builds. Crane delegates to Cargo: Cargo.toml and Cargo.lock
# stay the source of truth, and one dependency build is shared by every crate.
{ inputs, ... }:
{
  perSystem =
    { pkgs, lib, ... }:
    let
      craneLib = inputs.crane.mkLib pkgs;

      # Only what the Rust build reads, so docs and Nix edits don't trigger rebuilds.
      src = lib.fileset.toSource {
        root = ../.;
        fileset = lib.fileset.unions [
          ../Cargo.toml
          ../Cargo.lock
          ../libs/observe
          ../services/ticker
          ../services/databento-live
          ../services/alpaca-live
          ../services/dbn-tcp
          (lib.fileset.fileFilter (f: f.hasExt "rs" || f.name == "Cargo.toml") ../services/console)
          ../services/console/web
        ];
      };

      commonArgs = {
        inherit src;
        strictDeps = true;
        # alpaca-live builds OpenSSL from source (vendored), which needs perl.
        nativeBuildInputs = [ pkgs.perl ];
        pname = "glacier-workspace";
        version = "0.1.0";
      };
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;

      service =
        name: description:
        craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            pname = name;
            cargoExtraArgs = "--locked -p ${name}";
            doCheck = false; # tests run once for the whole workspace in checks.rust-tests
            meta = {
              inherit description;
              mainProgram = name;
            };
          }
        );

      ticker = service "ticker" "Toy service streaming Coinbase public tickers to disk";
      console = service "console" "Glacier Labs console";
      databento-live = service "databento-live" "Example service streaming a Databento live subscription";
      alpaca-live = service "alpaca-live" "Example service streaming Alpaca real-time IEX data";
      dbn-tcp = service "dbn-tcp" "Example service reading a live DBN stream over TCP";

    in
    {
      packages = {
        inherit
          ticker
          console
          databento-live
          alpaca-live
          dbn-tcp
          ;
      };

      checks = {
        rust-tests = craneLib.cargoTest (commonArgs // { inherit cargoArtifacts; });
        rust-clippy = craneLib.cargoClippy (
          commonArgs
          // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          }
        );
      };
    };
}
