{
  description = "nixlayer — a focused package-list manager for NixOS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "nixlayer";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          # nixlayer shells out to these at runtime; wrap the binary so they're
          # found even if the user's PATH doesn't already have them (it almost
          # always will on NixOS, but this makes `nix run` work too).
          nativeBuildInputs = [ pkgs.makeWrapper ];
          postInstall = ''
            wrapProgram $out/bin/nixlayer \
              --suffix PATH : ${pkgs.lib.makeBinPath [ pkgs.nix ]}
          '';

          meta = with pkgs.lib; {
            description = "Manage a NixOS system's package list without owning its whole configuration";
            homepage = "https://github.com/ssunflake/nixlayer";
            license = licenses.mit;
            mainProgram = "nixlayer";
          };
        };

        devShells.default = pkgs.mkShell {
          packages = [ pkgs.cargo pkgs.rustc pkgs.rust-analyzer pkgs.clippy ];
        };
      }
    ) // {
      nixosModules.default = { pkgs, ... }: {
        environment.systemPackages = [ self.packages.${pkgs.system}.default ];
      };
    };
}
