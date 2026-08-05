<!--
    SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
    SPDX-License-Identifier: CC-BY-SA-4.0
-->

# What cosmic-comp actually does with a BOTTOM-layer surface

Every claim the kiosk's design rests on, with the line of cosmic-comp 1.2.0 that establishes it.
Recorded because these are behaviours, not documented interfaces: they can change under a ghaf bump,
and when they do the symptom will be a kiosk that misbehaves in a way no error message explains.

Source read: the `src` of `cosmic-comp-1.2.0.drv` in the ghaf-pinned nixpkgs. (Its internal
`Cargo.toml` says `1.0.0`; that is stale upstream, not a different tree.)

| Question                                  | Answer                                                                                                     | Where                              |
| ----------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ---------------------------------- |
| Who may bind `zwlr_layer_shell_v1`?       | Only clients with **no** Wayland security context — or with `sandbox_engine == "com.system76.CosmicPanel"` | `src/state.rs:164-170`, `:704`     |
| Keyboard focus **on map**?                | **No.** `wants_focus = matches!(layer, Top \| Overlay) && …`                                               | `src/shell/mod.rs:2952-2958`       |
| Keyboard focus **on click**?              | **Yes**, if interactivity is not `None`                                                                    | `src/input/mod.rs:2151-2168`       |
| Does focus stick?                         | Yes — validity is just "is it in this output's layer map"                                                  | `src/shell/focus/mod.rs:646-648`   |
| Do pointer events arrive?                 | Yes; `BOTTOM` is the fallback hit target, after workspace windows                                          | `src/shell/focus/order.rs:338-345` |
| Is `Exclusive` keyboard mode useful here? | **No** — only `Top`/`Overlay` are considered                                                               | `src/shell/focus/mod.rs:747-763`   |
| Can Super+Q / Alt+F4 close it?            | **No** — `close_focused()` yields `None` for a layer surface                                               | `src/shell/mod.rs:1953-1989`       |
| Does it show in alt-tab?                  | **No** — the toplevel info state tracks only xdg/X11 toplevels                                             | `src/state.rs:286`                 |
| `exclusive_zone = -1` means?              | `DontCare`: ignore other surfaces' zones, take the whole output                                            | smithay `wlr_layer/types.rs:218`   |
| Output unplugged?                         | The compositor sends `closed` on that output's layer surfaces                                              | `src/shell/mod.rs:931-934`         |

## What follows for the code

- **`KeyboardInteractivity::OnDemand`.** `Exclusive` is a no-op at this layer, and `None` would make
  the buttons unreachable by keyboard entirely.
- **"Not closable" needs no shortcut stripping.** It is a property of the layer, already guaranteed.
  The shortcut lockdown in `ghaf-sfo-laptop` is about the _launcher_, not about protecting the kiosk.
- **`respect_close(false)` plus our own reconciliation.** The compositor does send `closed` on output
  removal; gtk4-layer-shell 1.3 swallows it by default. `outputs.rs` reconciles against
  `Display::monitors()` so an unplugged screen cannot leave a live window with a dead surface.
- **No keyboard focus until the first click.** If the device ever has to be keyboard-drivable from
  boot with no pointer input at all, that is not achievable on `BOTTOM` in this compositor version —
  and the fix (moving to `Top`) would put application windows _behind_ the kiosk, which defeats the
  whole design. Raise it rather than working around it.

## Verified empirically, too

Against this exact compositor binary, nested (`COSMIC_BACKEND=winit`):

```
[INFO  ghaf_sfo_kiosk] layer-shell available, protocol version 4
[INFO  ghaf_sfo_kiosk::outputs] output added (WINIT-0); creating a kiosk surface
```

No GTK, GSK or Vulkan errors, and the compositor stayed up. So the filter above admits a plain local
client, which is the one thing the whole design depends on.
