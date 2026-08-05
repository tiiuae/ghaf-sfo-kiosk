<!--
    SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
    SPDX-License-Identifier: Apache-2.0
-->

# ghaf-sfo-kiosk

The kiosk shell for [ghaf-sfo-laptop](https://github.com/tiiuae/ghaf-sfo-laptop): a full-screen
surface that owns the display from login, presents a handful of large buttons, and falls back into
view whenever the last application closes.

It is **config-driven**. This binary contains no knowledge of what the buttons do — it renders the
list in `/etc/sfo-kiosk/config.json`, which the SFO nix module generates. Adding a button is a
change there, not here.

## Why it is a layer-shell surface, not a window

"Always on top", "apps come on top of it" and "the kiosk is the desktop" only reconcile one way: the
kiosk is a `wlr-layer-shell` surface on the **`BOTTOM`** layer.

```
OVERLAY    ─ (nothing)
TOP        ─ cosmic-panel / dock          ← hidden while the kiosk runs
──────────── normal windows                ← come on top
BOTTOM     ─ THE KIOSK                     ← "the desktop"
BACKGROUND ─ wallpaper                     ← never visible; the kiosk is opaque
```

Everything needed follows from that one choice, and each of these is read out of cosmic-comp 1.2.0's
source rather than assumed — see [`docs/layer-shell-notes.md`](docs/layer-shell-notes.md):

| Property                               | Why it holds                                                                                                     |
| -------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| Cannot be closed with Super+Q / Alt+F4 | `close_focused()` returns `None` for a layer surface                                                             |
| Never appears in alt-tab               | only xdg/X11 toplevels are registered with the toplevel info state                                               |
| Does not steal focus when it maps      | `wants_focus` is `Top`/`Overlay` only                                                                            |
| Takes focus when clicked, and keeps it | layer surfaces are valid keyboard-focus targets                                                                  |
| Apps always draw above it              | `BOTTOM` is below the workspace layer                                                                            |
| No app VM can do the same              | layer-shell is refused to any client with a Wayland security context, and waypipe stamps one on every remote app |

That last row is worth dwelling on: it is what stops an application in another VM from creating its
own layer surface and covering the kiosk.

## Building

```bash
nix build .#ghaf-sfo-kiosk        # unit tests run in the check phase
nix flake check                   # + clippy (deny warnings) and rustfmt
```

## Running it without a device

The compositor has a built-in kiosk mode — `cosmic-comp <program>` runs the program as its only
client — so the surface can be exercised on a normal workstation against the **real** compositor:

```bash
COSMIC_BACKEND=winit cosmic-comp \
  ./result/bin/ghaf-sfo-kiosk --config examples/sfo.json
```

A successful start logs:

```
[INFO  ghaf_sfo_kiosk] layer-shell available, protocol version 4
[INFO  ghaf_sfo_kiosk::outputs] output added (WINIT-0); creating a kiosk surface
```

See [`docs/testing.md`](docs/testing.md) for what this does and does not prove.

## The one guard worth knowing about

If the compositor will not offer `zwlr_layer_shell_v1`, the kiosk **refuses to start** and exits 3.

That is deliberate. Without the check, GTK silently falls back to an ordinary window: one with a
titlebar, in the alt-tab list, closable with Alt+F4. It looks like the kiosk half-working, and it is
the single most expensive thing to diagnose on a device. The usual cause is being launched through
waypipe instead of natively in the gui-VM.

## Configuration

[`docs/config.md`](docs/config.md) is the schema. [`examples/sfo.json`](examples/sfo.json) is the
real SFO button set, and is also a unit-test fixture — a test asserts it parses with every action
resolved, so the example cannot rot.
