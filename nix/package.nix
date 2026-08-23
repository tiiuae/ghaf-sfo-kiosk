# SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
# SPDX-License-Identifier: Apache-2.0
#
# buildRustPackage, not crane: crane is an extra input whose incremental caching
# only pays off in this repo's CI, while every consumer resolves it. Revisit when
# the givc-client git dependency is linked in.
{
  lib,
  rustPlatform,
  pkg-config,
  wrapGAppsHook4,
  glib,
  gtk4,
  gtk4-layer-shell,
  cairo,
  pango,
  gdk-pixbuf,
  graphene,
  # On by default so release binaries carry an SBOM. The devshell overrides it to
  # false for its own copy; see nix/devshell.nix for why.
  auditable ? true,
}:
rustPlatform.buildRustPackage (
  {
    pname = "ghaf-sfo-kiosk";
    version = "0.1.0";

    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        ../Cargo.toml
        ../Cargo.lock
        ../src
        ../examples # included on purpose: a unit test asserts it parses
        # The two COSMIC Wayland protocol extensions toplevels.rs binds,
        # generated from these at compile time -- see src/protocols.rs for
        # why this is vendored XML rather than a git dependency.
        ../protocols
      ];
    };

    cargoLock.lockFile = ../Cargo.lock;

    nativeBuildInputs = [
      pkg-config
      # Without it gtk::Image::from_icon_name silently renders nothing.
      wrapGAppsHook4
    ];

    buildInputs = [
      glib
      gtk4
      gtk4-layer-shell
      cairo
      pango
      gdk-pixbuf
      graphene
    ];

    # GDK_BACKEND: gui-vm runs Xwayland so DISPLAY is set, and the X11 backend
    # makes layer-shell silently do nothing.
    # GSK_RENDERER: GTK >= 4.14 prefers Vulkan; gui-vm's display is virtio-gpu, and
    # a Vulkan failure shows up as "starts, no window ever appears".
    preFixup = ''
      gappsWrapperArgs+=(
        --set GDK_BACKEND wayland
        --set GSK_RENDERER gl
      )
    '';

    meta = {
      description = "Config-driven kiosk shell for Ghaf-based SFO laptops";
      license = lib.licenses.asl20;
      mainProgram = "ghaf-sfo-kiosk";
      platforms = lib.platforms.linux;
    };
  }
  # Passed only when disabling. buildRustPackage forwards `auditable` straight
  # into the derivation env, so setting it unconditionally -- even to its default
  # -- changes the output hash and costs every consumer a rebuild.
  // lib.optionalAttrs (!auditable) { inherit auditable; }
)
