# SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
# SPDX-License-Identifier: Apache-2.0
{ inputs, ... }:
{
  imports = [ inputs.treefmt-nix.flakeModule ];

  perSystem =
    { config, pkgs, ... }:
    {
      treefmt = {
        projectRootFile = "flake.nix";

        programs = {
          nixfmt.enable = true;
          nixfmt.package = pkgs.nixfmt;
          deadnix.enable = true;
          statix.enable = true;

          rustfmt.enable = true;
          rustfmt.edition = "2021";

          prettier.enable = true;
          prettier.settings.proseWrap = "preserve";

          taplo.enable = true;

          shellcheck.enable = true;
          shfmt.enable = true;

          actionlint.enable = true;
          keep-sorted.enable = true;
        };

        settings.global.excludes = [
          # keep-sorted start
          "*.license"
          "*.lock"
          "*.png"
          "*.svg"
          ".git*"
          "LICENSES/**"
          # keep-sorted end
        ];
      };

      formatter = config.treefmt.build.wrapper;
    };
}
