{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";

    # for building rust packages
    naersk.url = "github:nix-community/naersk";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    wild = {
      url = "github:wild-linker/wild";
      flake = false;
    };
  };
  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      flake-utils,
      naersk,
      wild,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [
            (import rust-overlay)
            (import wild)
          ];
        };

        toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        naersk' = pkgs.callPackage naersk {
          cargo = toolchain;
          rustc = toolchain;
        };
        wildStdenv = pkgs.useWildLinker pkgs.stdenv;
        nativeBuildInputs = with pkgs; [
          sqlite
          pkg-config
          openssl_3
        ];
      in
      {
        packages = rec {
          reviewqueue-bin = naersk'.buildPackage {
            src = ./.;
            inherit nativeBuildInputs;
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
        devShells.default =
          with pkgs;
          mkShell.override { stdenv = wildStdenv; } rec {
            inherit nativeBuildInputs;
            buildInputs = nativeBuildInputs ++ [
              ffmpeg
              clang
              llvmPackages_latest.bintools
              toolchain
            ];
            packages = [
            ];

            env = {
              DB_PATH = "db.sqlite";
              HOST = "http://localhost:3000";
            };

            shellHook = ''
              export LIBCLANG_PATH="${lib.makeLibraryPath [ llvmPackages_latest.libclang.lib ]}"
              export LD_LIBRARY_PATH="'$LD_LIBRARY_PATH:${lib.makeLibraryPath nativeBuildInputs}"
              PKG_CONFIG_PATH="${openssl.dev}/lib/pkgconfig";
            '';
          };
      }
    );
}
