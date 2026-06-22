{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    naersk.url = "github:nix-community/naersk";
    nixpkgs-mozilla = {
      url = "github:mozilla/nixpkgs-mozilla";
      flake = false;
    };
  };

  outputs =
    {
      self,
      flake-utils,
      naersk,
      nixpkgs,
      nixpkgs-mozilla,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = (import nixpkgs) {
          inherit system;
          overlays = [
            (import nixpkgs-mozilla)
          ];
        };

        toolchain =
          (pkgs.rustChannelOf {
            rustToolchain = ./rust-toolchain.toml;
            sha256 = "sha256-sqSWJDUxc+zaz1nBWMAJKTAGBuGWP25GCftIOlCEAtA=";
          }).rust;

        naersk' = pkgs.callPackage naersk { };

      in
      {
        packages = rec {
          reviewqueue-bin = naersk'.buildPackage {
            src = ./.;
            nativeBuildInputs = with pkgs; [
              sqlite
              pkg-config
              openssl_3
            ];
            PKG_CONFIG_PATH = "${pkgs.openssl_3.dev}/lib/pkgconfig";
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
