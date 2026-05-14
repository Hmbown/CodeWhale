{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      fenix,
      ...
    }@inputs:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forEachSystem =
        f:
        inputs.nixpkgs.lib.genAttrs systems (
          system:
          f {
            inherit system;
            pkgs = import inputs.nixpkgs {
              inherit system;
              overlays = [
                inputs.self.overlays.default
              ];
            };
          }
        );
      rev = self.shortRev or self.dirtyShortRev or "dirty";
    in
    {
      packages = forEachSystem (
        { pkgs, system }:
        {
          default = self.packages.${system}.deepseek-tui;
          deepseek-tui = pkgs.callPackage ./nix/package.nix {
            inherit rev;
            rustPlatform = pkgs.makeRustPlatform {
              cargo = pkgs.rustToolchain;
              rustc = pkgs.rustToolchain;
            };
          };
        }
      );

      overlays.default = final: prev: {
        rustToolchain =
          with fenix.packages.${prev.stdenv.hostPlatform.system};
          combine (
            with stable;
            [
              rustc
              cargo
              clippy
              rustfmt
              rust-src
            ]
          );
      };

      devShells = forEachSystem (
        { pkgs, system }:
        {
          default = pkgs.mkShell {
            packages = [
              pkgs.rustToolchain
              pkgs.rust-analyzer
              pkgs.lldb
              pkgs.pkg-config
              pkgs.openssl
              pkgs.python3
              self.formatter.${system}
            ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
              pkgs.dbus
            ];

            env = {
              # Required by rust-analyzer
              RUST_SRC_PATH = "${pkgs.rustToolchain}/lib/rustlib/src/rust/library";
            } // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
              LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
                with pkgs;
                [
                  openssl
                  dbus
                ]
              );
            };
          };
        }
      );

      formatter = forEachSystem ({ pkgs, ... }: pkgs.nixfmt);
    };
}
