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

The unit tests are in `src/config.rs` and `src/radial.rs`, and cover the parts where a mistake is
silent rather than loud.

`src/config.rs` — the contract with `ghaf-sfo-laptop`:

- `examples/sfo.json` parses with **every** action resolved, and splits into exactly the three grid
  buttons and three menu members the product ships — the split, not a bare count, because a
  regression that swept every button into the menu would keep the total right;
- one malformed button does not stop the others parsing;
- the `givc-app` argv is built exactly as ghaf's own launchers build it, asserted element by element;
- a `version` from the future is refused rather than half-understood;
- an empty button list is fatal;
- unknown fields on a known action kind are tolerated;
- a `menu` naming nothing puts its button back in the grid rather than losing it, and menus do not
  nest.

`src/radial.rs` — the arc arithmetic, which has no GTK in it for exactly this reason:

- every member's box stays inside the fan, for 0–6 members, with and without exit, on four output
  sizes. This is "bounded by the left vertical edge and the bottom edge" as an assertion rather than
  as an impression of one screenshot;
- no two icon circles come closer than the minimum gap — the property that decides whether the arc
  looks deliberate or crowded, and the reason the radius grows past the output's share when there are
  many members;
- exit sits alone at the top with a wider gap below it than the members have between them. That gap
  is the mis-tap mitigation, so it is asserted rather than left to a constant nobody re-reads.

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

The corner menu is worth walking by steps rather than by impression, because two of these fail in
ways that look like nothing at all:

1. Three tiles in one row, a trigger bottom-left, and **nothing** in the bottom-right corner.
2. Click the trigger — the members fan out staggered, the rest of the surface dims, and the trigger
   takes its `:checked` styling. It keeps its own icon; it does not become an ✕.
3. Escape closes it. Re-open, click the dimmed area — closes. Re-open, click the trigger — closes.
4. With the fan **closed**, Tab must never land on a member, and a click where a member would be must
   hit whatever is underneath. An opacity-0 `Fixed` child is still focusable and still clickable, so
   this is the regression to watch for.
5. Press Network: the fan closes first and `cosmic-settings` comes up above the kiosk, never behind
   an open fan.
6. Resize the winit window small. The fan must stay inside the surface — the radius is derived from
   the output size, and the line it logged on startup says what it chose.
7. Exit is the topmost member, visibly muted, with a clear gap below it.

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
- Multi-output hotplug, and touch input if the hardware has a touchscreen. The corner menu has no
  hover fallback anywhere, deliberately — but a member's hit target is its whole 132×104 box rather
  than the 72px circle it draws, and only a device says whether that is enough.
- **Exit by accident.** It moved from a 34px button at 35% opacity in the far corner into a fan the
  operator opens to reach Network. The gap above the last member, the muting and the icon are the
  mitigations; whether they are sufficient is a question about thumbs, not about code.
