# SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
# SPDX-License-Identifier: Apache-2.0
{ inputs, lib, ... }:
{
  imports = [ inputs.devshell.flakeModule ];

  perSystem =
    { config, pkgs, ... }:
    let
      # buildRustPackage's inputs carry two cargos: the real one, and nixpkgs'
      # auditable-cargo wrapper, which records the crate graph into a built
      # binary. devshell links every package into a single directory, so both
      # claim bin/cargo and the profile does not build at all:
      #
      #   pkgs.buildEnv error: two given paths contain a conflicting subpath:
      #     ...-auditable-cargo-1.97.1/bin/cargo
      #     ...-cargo-1.97.1/bin/cargo
      #
      # packagesFrom stays -- it is what brings pkg-config and the GTK
      # libraries in. Only the wrapper goes, and it matters to a released
      # artifact rather than to an interactive shell.
      devInputs = config.packages.ghaf-sfo-kiosk.overrideAttrs (old: {
        nativeBuildInputs = lib.filter (
          p: !lib.hasPrefix "auditable-cargo" (lib.getName p)
        ) old.nativeBuildInputs;
      });

      # Nothing sets PKG_CONFIG_PATH in a devshell, and a library's `.pc` lives
      # in its `dev` output, which the profile does not link -- so pkg-config
      # resolves nothing and cargo dies in a -sys build script. stdenv's setup
      # hooks do this inside the sandbox, which is why `nix build` is fine.
      #
      # closePropagation, over the package's own buildInputs: a `.pc` names
      # others in `Requires:` -- pango.pc alone reaches harfbuzz, fribidi and
      # freetype2 -- so the direct dependencies leave `--modversion pango`
      # answering while `--libs pango` still fails. Reading the list off the
      # package means adding a dependency in package.nix cannot forget this.
      pkgConfigPath = lib.concatStringsSep ":" (
        lib.concatMap (p: [
          "${lib.getDev p}/lib/pkgconfig"
          "${lib.getDev p}/share/pkgconfig"
        ]) (lib.closePropagation config.packages.ghaf-sfo-kiosk.buildInputs)
      );
    in
    {
      devshells.default = {
        devshell = {
          name = "ghaf-sfo-kiosk";
          meta.description = "ghaf-sfo-kiosk development environment";

          packagesFrom = [ devInputs ];

          # No cargo or rustc here: packagesFrom already provides both.
          packages = [
            pkgs.cachix
            pkgs.clippy
            pkgs.cosmic-comp
            pkgs.reuse
            pkgs.rust-analyzer
            pkgs.wayland-utils
            config.treefmt.build.wrapper
          ]
          ++ lib.attrValues config.treefmt.build.programs;
        };

        env = [
          {
            name = "PKG_CONFIG_PATH";
            value = pkgConfigPath;
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
