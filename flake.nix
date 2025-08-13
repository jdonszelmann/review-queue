{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    naersk.url = "github:nix-community/naersk";
    nixpkgs-mozilla = {
      url = "github:mozilla/nixpkgs-mozilla";
      flake = false;
    };
  };

  outputs = { self, flake-utils, naersk, nixpkgs, nixpkgs-mozilla }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = (import nixpkgs) {
          inherit system;
          overlays = [
            (import nixpkgs-mozilla)
          ];
        };

        toolchain = (pkgs.rustChannelOf {
          rustToolchain = ./rust-toolchain.toml;
          sha256 = "sha256-+9FmLhAOezBZCOziO0Qct1NOrfpjNsXxc/8I0c7BdKE=";
        }).rust;

        naersk' = pkgs.callPackage naersk {
          cargo = toolchain;
          rustc = toolchain;
        };

      in {
        packages = rec {
          reviewqueue-bin = naersk'.buildPackage {
            src = ./.;
            nativeBuildInputs = with pkgs; [sqlite];
          };
          default = pkgs.stdenv.mkDerivation {
            name = "reviewqueue";
            src = ./.;
            buildInputs = [ reviewqueue-bin ];
            installPhase = ''
              mkdir -p $out/bin

              cat >$out/bin/reviewqueue <<EOF
              #!/usr/bin/env bash
              export ASSETS_DIR=${./assets}
              ${reviewqueue-bin}/bin/reviewqueue
              EOF

              chmod +x $out/bin/reviewqueue
            '';
          };
        };

        devShell = pkgs.mkShell {
          nativeBuildInputs = [ toolchain ];
        };
      }
    );
}
