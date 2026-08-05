# SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
# SPDX-License-Identifier: Apache-2.0
{
  perSystem =
    { pkgs, ... }:
    let
      package = pkgs.callPackage ./package.nix { };
    in
    {
      checks = {
        # buildRustPackage runs `cargo test` in checkPhase.
        build = package;

        clippy = package.overrideAttrs (old: {
          pname = "${old.pname}-clippy";
          nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
          buildPhase = "cargo clippy --all-targets -- --deny warnings";
          installPhase = "touch $out";
          doCheck = false;
        });
      };
    };
}
