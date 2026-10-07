{
  description = "tinytrader: services, deployment modules and an operator console";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    flake-parts.url = "github:hercules-ci/flake-parts";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{
      self,
      flake-parts,
      nixpkgs,
      ...
    }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "aarch64-darwin"
        "x86_64-linux"
        "aarch64-linux"
      ];

      imports = [
        ./nix/rust.nix
        ./nix/generic.nix
        ./nix/web.nix
      ];

      flake = {
        nixosModules.services = ./infra/modules/services.nix;

        # The example host's services as plain systemd unit files, for Linux
        # distributions other than NixOS that have Nix installed: copy the
        # units to /etc/systemd/system and topology.json to /etc/tinytrader.
        # Generated from the same tinytrader.services definitions, so they match
        # what NixOS runs.
        systemdUnits.x86_64-linux.example =
          let
            pkgs = nixpkgs.legacyPackages.x86_64-linux;
            config = self.nixosConfigurations.example.config;
            units = config.systemd.units;
            ours = nixpkgs.lib.filterAttrs (name: _: nixpkgs.lib.hasPrefix "tinytrader-" name) units;
          in
          pkgs.linkFarm "tinytrader-example-units" (
            nixpkgs.lib.mapAttrsToList (name: unit: {
              inherit name;
              path = pkgs.writeText name unit.text;
            }) ours
            ++ [
              {
                # Copy to /etc/tinytrader/topology.json alongside the units.
                name = "topology.json";
                path = pkgs.writeText "topology.json" config.environment.etc."tinytrader/topology.json".text;
              }
            ]
          );

        # A minimal host running the quickstart services under systemd. It shows
        # the module in use and is evaluated by `nix flake check`.
        nixosConfigurations.example = nixpkgs.lib.nixosSystem {
          system = "x86_64-linux";
          modules = [
            self.nixosModules.services
            (
              { config, ... }:
              let
                packages = self.packages.x86_64-linux;
              in
              {
                networking.hostName = "example";
                tinytrader.services.ticker = {
                  enable = true;
                  package = packages.ticker;
                  observe.port = 9105;
                  environment.TICKER_DATA_DIR = "/var/lib/tinytrader/ticker";
                };
                tinytrader.services.console = {
                  enable = true;
                  package = packages.console;
                  observe.port = 8080;
                  # Reads the topology this module generates from the services above.
                  environment.CONSOLE_TOPOLOGY = config.tinytrader.topology.file;
                };
                tinytrader.topology.overrides.focus = "ticker";

                boot.loader.grub.enable = false;
                fileSystems."/" = {
                  device = "none";
                  fsType = "tmpfs";
                };
                system.stateVersion = "26.05";
              }
            )
          ];
        };
      };

      perSystem =
        { system, ... }:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ inputs.rust-overlay.overlays.default ];
          };
          # The toolchain named in rust-toolchain.toml, as rustup would install it.
          toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        in
        {
          _module.args = { inherit pkgs toolchain; };
          formatter = pkgs.nixfmt;

          devShells.default = pkgs.mkShell {
            packages = with pkgs; [
              nixfmt
              git
              jq
              sops
              age
              toolchain
              rust-analyzer
              typescript
            ];
          };
        };
    };
}
