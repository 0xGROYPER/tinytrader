{
  description = "Glacier Labs: services, deployment modules and an operator console";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    flake-parts.url = "github:hercules-ci/flake-parts";
    crane.url = "github:ipetkov/crane";
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
      ];

      flake = {
        nixosModules.services = ./infra/modules/services.nix;

        # The example host's services as plain systemd unit files, for Linux
        # distributions other than NixOS that have Nix installed: copy them to
        # /etc/systemd/system. Generated from the same glacier.services
        # definitions, so they match what NixOS runs.
        systemdUnits.x86_64-linux.example =
          let
            pkgs = nixpkgs.legacyPackages.x86_64-linux;
            units = self.nixosConfigurations.example.config.systemd.units;
            ours = nixpkgs.lib.filterAttrs (name: _: nixpkgs.lib.hasPrefix "glacier-" name) units;
          in
          pkgs.linkFarm "glacier-example-units" (
            nixpkgs.lib.mapAttrsToList (name: unit: {
              inherit name;
              path = pkgs.writeText name unit.text;
            }) ours
          );

        # A minimal host running the quickstart services under systemd. It shows
        # the module in use and is evaluated by `nix flake check`.
        nixosConfigurations.example = nixpkgs.lib.nixosSystem {
          system = "x86_64-linux";
          modules = [
            self.nixosModules.services
            (
              { pkgs, ... }:
              let
                packages = self.packages.x86_64-linux;
              in
              {
                glacier.services.ticker = {
                  enable = true;
                  package = packages.ticker;
                  environment = {
                    TICKER_DATA_DIR = "/var/lib/glacier/ticker";
                    TICKER_LISTEN = "127.0.0.1:9100";
                  };
                };
                glacier.services.console = {
                  enable = true;
                  package = packages.console;
                  environment = {
                    CONSOLE_LISTEN = "127.0.0.1:8080";
                    CONSOLE_TOPOLOGY = "${./examples/quickstart/topology.json}";
                  };
                };

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
        { pkgs, ... }:
        {
          formatter = pkgs.nixfmt;

          devShells.default = pkgs.mkShell {
            packages = with pkgs; [
              nixfmt
              git
              jq
              sops
              age
              cargo
              rustc
              clippy
              rustfmt
              rust-analyzer
            ];
          };
        };
    };
}
