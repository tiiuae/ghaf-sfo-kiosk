# SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
# SPDX-License-Identifier: Apache-2.0
{
  description = "A config-driven kiosk shell for Ghaf-based SFO laptops";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };

    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    devshell = {
      url = "github:numtide/devshell";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      imports = [ ./nix/flake-module.nix ];

      # prev.callPackage, so the consumer's nixpkgs builds it -- otherwise an
      # image taking this and ghaf-ctrl-panel gets two gtk4 closures.
      flake.overlays.default = _final: prev: {
        ghaf-sfo-kiosk = prev.callPackage ./nix/package.nix { };
      };

      perSystem =
        { pkgs, ... }:
        {
          packages.ghaf-sfo-kiosk = pkgs.callPackage ./nix/package.nix { };
          packages.default = pkgs.callPackage ./nix/package.nix { };

          # Deliberately not in the overlay above: a maintenance tool for this
          # checkout, not something a consumer's nixpkgs should grow.
          packages.update-deps = pkgs.callPackage ./nix/update-deps.nix { };
        };
    };
}
