// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// The widget tree. Binds and emits; never spawns a process or touches D-Bus.
//
// NEVER call .present() on anything but the layer-shell window: any other
// gtk::Window becomes an xdg_toplevel, floating above the kiosk and appearing in
// alt-tab. Hence an in-window banner rather than a dialog, and no libadwaita.
//
// Corner menus are in radial.rs, styling in style.rs, the banner in banner.rs.
// What is left here is assembly: status bar, grid, and the overlay stack that
// puts a fan above a scrim above the grid.

use gtk::prelude::*;

use crate::actions;
use crate::banner::Banner;
use crate::config::Kiosk;
use crate::radial;
use crate::status;

/// A leading `/` selects a file; anything else is an icon theme name.
///
/// Shared with radial.rs so grid and menu resolve icons by the same rule.
pub fn icon_image(icon: &str) -> gtk::Image {
    if icon.starts_with('/') {
        gtk::Image::from_file(icon)
    } else {
        gtk::Image::from_icon_name(icon)
    }
}

/// Build the whole kiosk surface content for one output.
///
/// `monitor` is the output size in logical pixels; it sets the corner menus'
/// fan radius -- see `radial::Geometry`.
pub fn build(
    kiosk: &Kiosk,
    monitor: (f64, f64),
    shared: &crate::shared::Shared,
) -> gtk::Widget {
    // This output's own banner widget, registered so a message raised anywhere
    // is shown on every output. `reporter` -- NOT `banner` -- is what buttons
    // and menu items must report through; using the local banner directly is
    // exactly the bug this indirection exists to prevent.
    let banner = Banner::new();
    shared.register_banner(banner.clone());
    let reporter = shared.reporter();

    // ── status bar ──────────────────────────────────────────────────────────
    let bar = gtk::CenterBox::new();
    bar.add_css_class("kiosk-statusbar");

    let title = gtk::Label::new(Some(&kiosk.title));
    title.add_css_class("kiosk-title");
    bar.set_start_widget(Some(&title));

    let clock = std::rc::Rc::new(status::Clock::new(&kiosk.status_bar.clock_format));
    bar.set_center_widget(Some(&clock.label));
    clock.clone().start();

    let right = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    if kiosk.status_bar.show_network {
        right.append(&status::network().container);
    }
    if kiosk.status_bar.show_battery {
        right.append(&status::battery().container);
    }
    bar.set_end_widget(Some(&right));

    // ── button grid ─────────────────────────────────────────────────────────
    let grid = gtk::FlowBox::new();
    grid.set_valign(gtk::Align::Center);
    grid.set_halign(gtk::Align::Center);
    grid.set_selection_mode(gtk::SelectionMode::None);
    grid.set_max_children_per_line(kiosk.layout.columns);
    grid.set_min_children_per_line(1);
    grid.set_row_spacing(36);
    grid.set_column_spacing(36);
    grid.set_homogeneous(true);
    grid.add_css_class("kiosk-grid");

    for spec in &kiosk.buttons {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
        content.set_valign(gtk::Align::Center);
        if let Some(icon) = &spec.icon {
            let image = icon_image(icon);
            image.set_pixel_size(88);
            // Named so style.rs can colour it and so a per-button icon_color has
            // a node to target. Mirrors kiosk-radial-item-icon.
            image.add_css_class("kiosk-button-icon");
            content.append(&image);
        }
        let label = gtk::Label::new(Some(&spec.label));
        label.add_css_class("kiosk-button-label");
        content.append(&label);

        let button = gtk::Button::new();
        button.set_child(Some(&content));
        button.add_css_class("kiosk-button");
        button.add_css_class(&format!("kiosk-button-{}", spec.id));
        if let Some(d) = &spec.description {
            button.set_tooltip_text(Some(d));
        }

        // An unrunnable button still renders, dimmed. Hiding it would leave the
        // operator wondering where it went; disabling it would be
        // indistinguishable from a broken kiosk.
        if matches!(spec.action, crate::config::Action::Unsupported { .. }) {
            button.add_css_class("kiosk-button-unconfigured");
        }

        let action = spec.action.clone();
        let name = spec.label.clone();
        let reporter = reporter.clone();
        // Per button, so a slow one cannot block a different one.
        let busy = actions::Busy::new();
        button.connect_clicked(move |_| actions::dispatch(&action, &name, &reporter, &busy));

        grid.append(&button);
    }

    // ── assembly ────────────────────────────────────────────────────────────
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.append(&bar);
    column.append(&banner.revealer);
    grid.set_vexpand(true);
    column.append(&grid);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&column));

    // Scrim FIRST, so every fan sits above the dimming. It dims the kiosk only:
    // on a BOTTOM-layer surface, another VM's window stays above and bright.
    let scrim = gtk::Box::new(gtk::Orientation::Vertical, 0);
    scrim.add_css_class("kiosk-scrim");
    scrim.set_can_target(false);
    overlay.add_overlay(&scrim);

    // ── the corner menus ────────────────────────────────────────────────────
    let scrim_widget = scrim.clone().upcast::<gtk::Widget>();
    let fans: Vec<radial::Fan> = kiosk
        .menus
        .iter()
        .map(|menu| {
            let fan = radial::build(
                menu,
                monitor,
                &reporter,
                &scrim_widget,
            );
            overlay.add_overlay(&fan.widget);
            // Link this menu to the SAME menu on every other output, so opening
            // the fan on the laptop opens it on the room's screen too.
            shared.register_fan(&menu.trigger.id, &fan);
            fan
        })
        .collect();

    let fans = std::rc::Rc::new(fans);

    // Clicking the dimmed area closes whatever opened it.
    {
        let fans = fans.clone();
        let click = gtk::GestureClick::new();
        click.connect_pressed(move |_, _, _, _| {
            for fan in fans.iter() {
                fan.close();
            }
        });
        scrim.add_controller(click);
    }

    // Escape too. On the overlay, not a fan: the press arrives at whichever
    // member has focus and bubbles up from there.
    {
        let fans = fans.clone();
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape && fans.iter().any(radial::Fan::is_open) {
                for fan in fans.iter() {
                    fan.close();
                }
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
        overlay.add_controller(keys);
    }

    // No exit button, deliberately: nothing on screen an operator can press by
    // accident. Leaving the kiosk is Ctrl+Alt+Shift+L, a COSMIC keybinding
    // declared in tiiuae/ghaf-sfo-laptop that stops the unit -- so ExecStopPost
    // still restores the panel however the kiosk died.
    //
    // It cannot be a key handler here: on layer-shell BOTTOM this surface gets
    // no keyboard focus until clicked, and none while an application window has
    // it -- exactly when a technician wants out. docs/layer-shell-notes.md.

    overlay.upcast()
}
