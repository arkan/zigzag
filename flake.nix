{
  description = "z - TUI/CLI project manager for Zellij";

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
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };
        zPackage = pkgs.callPackage ./package.nix { };
        zApp = {
          type = "app";
          program = "${zPackage}/bin/z";
          meta = zPackage.meta;
        };
      in
      {
        packages = {
          default = zPackage;
          z = zPackage;
        };

        apps = {
          default = zApp;
          z = zApp;
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
            echo "z dev shell ready — rust $(rustc --version | cut -d' ' -f2), node $(node --version)"
          '';
        };
      }
    );
}
