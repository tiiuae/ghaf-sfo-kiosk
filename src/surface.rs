// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// The only file that touches gtk4-layer-shell.
//
// A BOTTOM-layer surface: above the wallpaper, below every application window,
// which is what makes it behave like a desktop rather than an overlay. Three
// consequences on cosmic-comp 1.2.0, all read out of its source: layer-shell is
// refused to clients with a Wayland security context (so no waypipe'd app-VM
// window can fight us), a BOTTOM surface takes focus on click but not on map
// (hence OnDemand), and Super+Q / Alt+F4 cannot close it.

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

/// Build the kiosk surface for one output.
///
/// The call order matters: `init_layer_shell()` must happen before the window is
/// realized, and `set_monitor()` before it is presented, or the surface lands on
/// whichever output the compositor felt like.
pub fn build(
    app: &gtk::Application,
    monitor: &gtk::gdk::Monitor,
    child: &impl IsA<gtk::Widget>,
) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::new(app);

    window.init_layer_shell();
    window.set_namespace(Some("ghaf-sfo-kiosk"));
    window.set_layer(Layer::Bottom);

    // All four edges, so the surface is sized to the whole output.
    for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
        window.set_anchor(edge, true);
    }

    // -1 is ExclusiveZone::DontCare: ignore every other surface's exclusive zone
    // and take the full output rectangle. Anything >= 0 would let a panel push us
    // around, and we are the desktop.
    window.set_exclusive_zone(-1);

    // OnDemand, never Exclusive. We must not steal focus from a launched app;
    // we do want focus once the operator touches us.
    window.set_keyboard_mode(KeyboardMode::OnDemand);

    // Explicit, though it is also the gtk4-layer-shell 1.3 default: a compositor
    // `closed` event must not destroy the window. cosmic-comp sends one when an
    // output goes away, and we handle that ourselves in outputs.rs so that an
    // unplugged monitor cannot silently take the kiosk down with it.
    window.set_respect_close(false);

    window.set_monitor(Some(monitor));

    // Deliberately no set_default_size(): it is ignored when anchored to
    // opposite edges, and setting it invites the reader to think it matters.

    window.set_child(Some(child));
    window.add_css_class("kiosk-root");
    window
}

/// Is the compositor offering us layer-shell at all?
///
/// This is the single most valuable check in the program. Without it, running
/// behind a Wayland security context, under X11, or with a bad library link
/// order produces an ordinary toplevel: a window with a titlebar, in the
/// alt-tab list, closable with Alt+F4. That failure *looks* like it half-works
/// and costs a day on the device. Refusing to start turns it into one journal
/// line and an exit code.
pub fn supported() -> bool {
    gtk4_layer_shell::is_supported()
}

pub fn protocol_version() -> u32 {
    gtk4_layer_shell::protocol_version()
}
