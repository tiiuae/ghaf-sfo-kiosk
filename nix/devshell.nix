# SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
# SPDX-License-Identifier: Apache-2.0
{ inputs, lib, ... }:
{
  imports = [ inputs.devshell.flakeModule ];

  perSystem =
    { config, pkgs, ... }:
    {
      devshells.default = {
        devshell = {
          name = "ghaf-sfo-kiosk";
          meta.description = "ghaf-sfo-kiosk development environment";

          packagesFrom = [ config.packages.ghaf-sfo-kiosk ];

          packages = [
            pkgs.cachix
            pkgs.cargo
            pkgs.clippy
            pkgs.cosmic-comp
            pkgs.reuse
            pkgs.rust-analyzer
            pkgs.rustc
            pkgs.wayland-utils
            config.treefmt.build.wrapper
          ]
          ++ lib.attrValues config.treefmt.build.programs;
        };

        commands = [
          {
            name = "format-repo";
            command = "treefmt";
            help = "Format the whole tree";
            category = "linters";
          }
          {
            name = "check-license";
            command = "reuse lint";
            help = "Check SPDX headers and licences";
            category = "linters";
          }
          {
            name = "check-all";
            command = "nix flake check --all-systems --keep-going -L";
            help = "Run the full PR gate, exactly as CI does";
            category = "linters";
          }
        ];
      };
    };
}
