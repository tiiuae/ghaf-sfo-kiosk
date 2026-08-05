<!--
    SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
    SPDX-License-Identifier: CC-BY-SA-4.0
-->

# Testing

Three tiers, in increasing cost and decreasing convenience. The point of the first two is to make
the third — which needs a laptop on a desk — as short as possible.

## 1. No display: `nix flake check`

```bash
nix flake check
```

Runs the build (unit tests execute in the check phase), `clippy --deny warnings`, and `rustfmt`.

The unit tests are all in `src/config.rs` and cover the parts where a mistake is silent rather than
loud:

- `examples/sfo.json` parses with **every** action resolved — so the shipped example cannot rot;
- one malformed button does not stop the others parsing;
- the `givc-app` argv is built exactly as ghaf's own launchers build it, asserted element by element;
- a `version` from the future is refused rather than half-understood;
- an empty button list is fatal;
- unknown fields on a known action kind are tolerated.

## 2. A real compositor, no device

cosmic-comp has a built-in kiosk mode: given a program as its first argument it runs that program as
its only client, with `WAYLAND_DISPLAY` already set. So the surface can be exercised against the
**same compositor build the device runs**, on any workstation with a graphical session:

```bash
nix build .#ghaf-sfo-kiosk
COSMIC_BACKEND=winit cosmic-comp ./result/bin/ghaf-sfo-kiosk --config examples/sfo.json
```

**This genuinely proves:** that the compositor offers layer-shell to a plain local client; that the
surface maps at `BOTTOM` and anchors to the whole output; that GTK renders with no GSK or Vulkan
complaint; that the process is stable. Click around and you also cover focus-on-click, the button
grid, the banner, and that Super+Q does nothing.

**It does not prove** anything about the parts that only exist inside a Ghaf image — see below.

## 3. Only on the device

- Windows from **another VM** stacking above the kiosk, and their per-VM security-context border.
  There is no bench equivalent: it needs waypipe and a real security context.
- **GIVC** end to end, with real certificates against a real admin-VM.
- **UPower** and **NetworkManager**. Neither exists on a workstation the way it does in gui-VM, where
  NetworkManager is net-VM's, republished onto gui-VM's system bus over a GIVC socket proxy.
- The **COSMIC panel** disappearing and coming back. Cycle it at least five times: a leak in
  cosmic-panel's internal space list would show up as "comes back once, then never", which reads as
  flakiness and gets misdiagnosed.
- **Crash and power-loss recovery.** `systemctl --user kill -s KILL sfo-kiosk` must leave a usable
  desktop, and so must yanking the power mid-session. gui-VM's home persists, so a failure here
  survives a reboot — this is the one bug that strands a field user.
- Multi-output hotplug, and touch input if the hardware has a touchscreen.
