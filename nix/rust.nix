# Rust workspace builds. Crane delegates to Cargo: Cargo.toml and Cargo.lock
# stay the source of truth, and one dependency build is shared by every crate.
{ inputs, ... }:
{
  perSystem =
    {
      pkgs,
      lib,
      toolchain,
      ...
    }:
    let
      craneLib = (inputs.crane.mkLib pkgs).overrideToolchain (_: toolchain);

      # Only what the Rust build reads, so docs and Nix edits don't trigger rebuilds.
      src = lib.fileset.toSource {
        root = ../.;
        fileset = lib.fileset.unions [
          ../Cargo.toml
          ../Cargo.lock
          ../libs/observe
          ../libs/fleet
          ../apps/tui
          ../services/ticker
          ../services/databento-live
          ../services/alpaca-live
          ../services/dbn-tcp
          ../services/catalog
          ../services/alpaca-news
          ../services/alpaca-account
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
      glacier-tui = service "glacier-tui" "Terminal view of a Glacier deployment";
      catalog = service "catalog" "Read-only catalog of data roots";
      alpaca-news = service "alpaca-news" "Example service serving market news from Alpaca (Benzinga)";
      alpaca-account = service "alpaca-account" "Example service serving an Alpaca account's balances and positions";

    in
    {
      packages = {
        inherit
          ticker
          console
          databento-live
          alpaca-live
          dbn-tcp
          glacier-tui
          catalog
          alpaca-news
          alpaca-account
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
