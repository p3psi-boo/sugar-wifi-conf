{
  description = "Rust BLE WiFi configuration service";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        rustToolchain = pkgs.pkgsBuildHost.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
          ];
        };

        buildInputs = with pkgs; [
          # BlueZ and DBus for BLE development
          bluez
          dbus

          # System libraries
          pkg-config
        ];

        nativeBuildInputs = with pkgs; [
          rustToolchain
          cargo
          rustc
        ];
      in
      {
        devShells.default = pkgs.mkShell {
          inherit buildInputs nativeBuildInputs;

          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";

          shellHook = ''
            echo "Rust BLE WiFi Configuration Service"
            echo "==================================="
            echo "Rust toolchain: $(rustc --version)"
            echo "Cargo version: $(cargo --version)"
            echo ""
            echo "Available commands:"
            echo "  cargo build              - Build the project"
            echo "  cargo run                - Run the project"
            echo "  cargo test               - Run tests"
            echo "  cargo clippy             - Run linter"
            echo "  cargo fmt                - Format code"
            echo ""
            echo "To run with sudo (required for BLE access):"
            echo "  sudo -E cargo run -- --key pisugar --custom-config ./custom_config.json"
            echo ""
          '';
        };
      }
    );
}
