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

          # auditable=false here only. buildRustPackage puts both a cargo-auditable
          # wrapper and plain cargo in nativeBuildInputs; a real build resolves the
          # two bin/cargo by PATH order, but devshell merges these inputs into one
          # buildEnv, where the collision is a hard error. Dropping the wrapper is
          # also what we want interactively -- cargo below is the plain toolchain.
          packagesFrom = [ (config.packages.ghaf-sfo-kiosk.override { auditable = false; }) ];

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

        # devshell merges packagesFrom into $DEVSHELL_DIR without running
        # stdenv's setup hooks, so nothing populates PKG_CONFIG_PATH and
        # gtk4-sys' build script fails to find gtk4.pc.
        env = [
          {
            name = "PKG_CONFIG_PATH";
            prefix = "$DEVSHELL_DIR/lib/pkgconfig";
          }
        ];

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
