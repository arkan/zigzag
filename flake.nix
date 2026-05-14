{
  description = "Zigzag - TUI/CLI project manager for Zellij";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };
        rustToolchain = pkgs.rust-bin.fromRustupToolchain {
          channel = "1.95.0";
          components = [ "clippy" "rustfmt" "rust-src" "rust-analyzer" ];
        };
        rustPlatform = pkgs.makeRustPlatform {
          cargo = rustToolchain;
          rustc = rustToolchain;
        };
        zigzagPackage = pkgs.callPackage ./package.nix { inherit rustPlatform; };
        zigzagApp = {
          type = "app";
          program = "${zigzagPackage}/bin/zigzag";
          meta = zigzagPackage.meta;
        };
      in
      {
        packages = {
          default = zigzagPackage;
          zigzag = zigzagPackage;
        };

        apps = {
          default = zigzagApp;
          zigzag = zigzagApp;
        };

        devShells.default = pkgs.mkShell {
          buildInputs = [
            rustToolchain
            pkgs.cargo-watch
            pkgs.nodejs_22
            pkgs.zellij
            pkgs.gnumake
          ];

          shellHook = ''
            echo "zigzag dev shell ready — rust $(rustc --version | cut -d' ' -f2), node $(node --version)"
          '';
        };
      }
    );
}
