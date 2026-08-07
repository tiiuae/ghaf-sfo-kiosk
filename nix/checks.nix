# SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
# SPDX-License-Identifier: Apache-2.0
{
  perSystem =
    { config, pkgs, ... }:
    let
      package = pkgs.callPackage ./package.nix { };
    in
    {
      checks = {
        # buildRustPackage runs `cargo test` in checkPhase.
        build = package;

        # The devshell, built rather than merely evaluated.
        #
        # `nix flake check` does not build devShells, and `nix flake show`
        # prints "development environment 'ghaf-sfo-kiosk'" for a shell that
        # cannot be assembled -- so the whole gate passes while `direnv allow`
        # on a fresh clone fails. Costs one profile symlink tree.
        devshell = config.devShells.default;

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
