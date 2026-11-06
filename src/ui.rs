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
use crate::confirm;
use crate::radial;
use crate::status;

/// A leading `/` selects a file; anything else is an icon theme name.
///
/// Shared with radial.rs so grid and menu resolve icons by the same rule.
/// Status bar text is 15px with 10px padding; this keeps the logo inside that
/// band rather than growing the bar.
const LOGO_PIXEL_SIZE: i32 = 24;

pub fn icon_image(icon: &str) -> gtk::Image {
    if icon.starts_with('/') {
        gtk::Image::from_file(icon)
    } else {
        gtk::Image::from_icon_name(icon)
    }
}

/// Cards built per output but added to the overlay last; see `build`.
#[derive(Default)]
pub struct PendingCards {
    pub confirms: Vec<(String, confirm::Confirm)>,
    pub settimes: Vec<(String, crate::settime::SetTime)>,
}

/// The most of an output's height the customer's logo may take.
///
/// It is configured in pixels and the screen it lands on is not known until
/// here, so a height that is merely generous on the laptop can leave the grid
/// no room on a shorter output. A sixth is far above the 88 px the design asks
/// for and still bounds a typo.
const BRANDING_MAX_FRACTION: f64 = 1.0 / 6.0;

/// Breathing room above and below the logo.
///
/// In code rather than in CSS because the band is sized from the logo and CSS
/// padding is not visible from here.
const BRANDING_MARGIN_Y: i32 = 24;

/// The square `pixel_size` box that fits `logo` inside `max_width` x
/// `max_height`.
///
/// `pixel_size` is a BOX, not a size: GTK fits the image into that many logical
/// pixels square with its aspect ratio kept, so a portrait emblem comes out
/// `box` tall while a 3.4:1 wordmark comes out less than a third of that. A
/// square box cannot express a rectangular one, so the artwork's own ratio has
/// to be read -- and the file is the only place it exists.
///
/// An icon-theme name has no file to measure and is square by convention, so
/// there the box is the height. Same for a file GDK cannot decode: it will not
/// draw either, and `branding_band` has already warned about a missing one.
fn logo_box(logo: &str, max_width: i32, max_height: i32) -> i32 {
    if !logo.starts_with('/') {
        return max_height;
    }
    match gtk::gdk::Texture::from_filename(logo) {
        Ok(texture) if texture.height() > 0 => {
            box_for(max_width, max_height, texture.width(), texture.height())
        }
        _ => max_height,
    }
}

/// The arithmetic of [`logo_box`], apart from it so it can be tested without a
/// display or a file.
///
/// The drawn height is whichever bound bites first -- `max_height`, or the
/// height at which the artwork is already `max_width` wide -- and the square
/// box that produces it is that height times the ratio, or the height itself
/// for anything at least as tall as it is wide.
fn box_for(max_width: i32, max_height: i32, width: i32, natural: i32) -> i32 {
    let ratio = f64::from(width) / f64::from(natural);
    let drawn = f64::from(max_height).min(f64::from(max_width) / ratio);
    (drawn * ratio.max(1.0)).round() as i32
}

/// The customer's logo, centred in the band below the grid.
///
/// Same widget as every other icon, at a size two orders of magnitude bigger --
/// and `pixel_size` is doing more work here than it looks, which is why the box
/// it wants is computed by [`logo_box`] rather than being the height itself.
///
/// **That bound is the point.** The first version of this used `gtk::Picture`,
/// which fills what it is given -- and a vexpanding `Picture` measures its
/// natural WIDTH from the height it is offered, so a 260 px panel asked for
/// 1160 px and took nearly half the screen from the grid. `set_size_request`
/// does not help: it is a minimum, not a maximum. Measured in a nested
/// cosmic-comp, not reasoned about.
///
/// `None` when the logo names a path with nothing at it: nothing is drawn, and
/// a line in the journal says which file.
///
/// A real state, not a theoretical one. In an SFO image the file is staged at
/// boot by the HOST, outside this VM, so the kiosk can be asked for a brand the
/// host has not written yet or failed to write. Left to GTK that draws a
/// broken-image icon at full size -- alarming on a customer's kiosk, and, being
/// a picture, silent about what actually went wrong. A clean screen and a
/// warning puts the evidence where it can be read.
fn branding_band(branding: &crate::config::Branding, monitor: (f64, f64)) -> Option<gtk::Widget> {
    if branding.logo.starts_with('/') && !std::path::Path::new(&branding.logo).exists() {
        log::warn!(
            "branding: {} does not exist, drawing no logo",
            branding.logo
        );
        return None;
    }

    let cap = (monitor.1 * BRANDING_MAX_FRACTION) as i32;
    let max_height = if cap > 0 && branding.max_height > cap {
        log::warn!(
            "branding: max_height {} is more than a sixth of a {}px output, using {cap}",
            branding.max_height,
            monitor.1 as i32,
        );
        cap
    } else {
        branding.max_height
    };

    // Same path-or-icon-name rule as a button's icon, so a brand may be either.
    let image = icon_image(&branding.logo);
    image.set_pixel_size(logo_box(&branding.logo, branding.max_width, max_height).max(1));
    image.set_margin_top(BRANDING_MARGIN_Y);
    image.set_margin_bottom(BRANDING_MARGIN_Y);
    // On the screen's centre line, not the grid's: the grid is centred too, but
    // a FlowBox of two tiles is narrower than one of three and the logo must
    // not move with it.
    image.set_halign(gtk::Align::Center);

    let band = gtk::Box::new(gtk::Orientation::Vertical, 0);
    band.add_css_class("kiosk-branding");
    band.append(&image);

    Some(band.upcast())
}

/// Build the whole kiosk surface content for one output.
///
/// `monitor` is the output size in logical pixels; it sets the corner menus'
/// fan radius -- see `radial::Geometry`.
pub fn build(kiosk: &Kiosk, monitor: (f64, f64), shared: &crate::shared::Shared) -> gtk::Widget {
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

    // A logo replaces the title text outright. Same path-or-icon-name rule as a
    // button's icon, so a store path from the nix side just works.
    if let Some(logo) = &kiosk.logo {
        let image = icon_image(logo);
        image.set_pixel_size(LOGO_PIXEL_SIZE);
        image.add_css_class("kiosk-logo");
        image.set_tooltip_text(Some(&kiosk.title));
        bar.set_start_widget(Some(&image));
    } else {
        let title = gtk::Label::new(Some(&kiosk.title));
        title.add_css_class("kiosk-title");
        bar.set_start_widget(Some(&title));
    }

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

    // Collected, not added here: every card must sit ABOVE every fan, and only
    // the assembly below knows when the last fan has been added.
    let mut pending = PendingCards::default();

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

        if let crate::config::Action::SetTime {
            host,
            port,
            reboot_threshold_sec,
            verdict_host,
            verdict_port,
        } = &spec.action
        {
            // The kiosk owns this one: the press opens a card, nothing dispatches.
            let card = crate::settime::build(
                &spec.id,
                host,
                *port,
                *reboot_threshold_sec,
                verdict_host,
                *verdict_port,
                shared,
            );
            pending.settimes.push((spec.id.clone(), card.clone()));
            button.connect_clicked(move |_| card.open());
        } else {
            let action = spec.action.clone();
            let name = spec.label.clone();
            let reporter = reporter.clone();
            // Per button id, not per output: shared.busy_for gives the SAME flag
            // to this button on every screen, so a press on one screen is visible
            // to the others. See Shared::busy_for.
            let busy = shared.busy_for(&spec.id);
            let fire = move || actions::dispatch(&action, &name, &reporter, &busy);

            if let Some(spec_confirm) = &spec.confirm {
                // Opening the card is all the press does; `fire` is handed to the
                // card and runs only if the operator confirms.
                let card = confirm::build(spec_confirm, &spec.label, fire);
                pending.confirms.push((spec.id.clone(), card.clone()));
                button.connect_clicked(move |_| card.open());
            } else {
                button.connect_clicked(move |_| fire());
            }
        }

        grid.append(&button);
    }

    // ── assembly ────────────────────────────────────────────────────────────
    //
    // The logo goes BELOW the grid: centred on the screen, and centred again in
    // what the grid leaves under it. The status bar spans the width above both,
    // because the platform mark and the clock belong to the screen rather than
    // to the customer.
    let body: gtk::Widget = match kiosk
        .branding
        .as_ref()
        .and_then(|b| branding_band(b, monitor))
    {
        None => {
            grid.set_vexpand(true);
            grid.clone().upcast()
        }
        Some(band) => {
            // A spacer above the grid and the band below it, both expanding and
            // neither holding anything that measures: GTK hands them the slack
            // in equal shares, which leaves the grid centred in the body and
            // the logo centred in the gap under it. The grid must NOT expand
            // here -- if it does it takes the slack and the band collapses onto
            // the bottom edge.
            let stack = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
            spacer.set_vexpand(true);
            band.set_vexpand(true);
            band.set_valign(gtk::Align::Center);

            stack.append(&spacer);
            stack.append(&grid);
            stack.append(&band);
            stack.upcast()
        }
    };

    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.append(&bar);
    column.append(&banner.revealer);
    column.append(&body);

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
        .enumerate()
        .map(|(i, menu)| {
            let fan = radial::build(
                menu,
                // The first menu sits bottom-left, the second bottom-right.
                i > 0,
                monitor,
                &reporter,
                &scrim_widget,
                shared,
                &mut pending,
            );
            overlay.add_overlay(&fan.widget);
            // Link this menu to the SAME menu on every other output, so opening
            // the fan on the laptop opens it on the room's screen too.
            shared.register_fan(&menu.trigger.id, &fan);
            fan
        })
        .collect();

    let fans = std::rc::Rc::new(fans);

    // Last, so a card is above every fan and above the fans' scrim: the
    // stacking is grid -> scrim -> fans -> confirms, bottom to top. A menu
    // member can confirm too, and its card must not open behind the fan it
    // was pressed in.
    let confirms: Vec<confirm::Confirm> = pending
        .confirms
        .into_iter()
        .map(|(id, card)| {
            overlay.add_overlay(&card.widget);
            // Link this button's card to the SAME button's card on every other
            // output, so the question is asked and answered on both screens.
            shared.register_confirm(&id, &card);
            card
        })
        .collect();
    let confirms = std::rc::Rc::new(confirms);

    // Above the confirm cards, below the restart screen: the Set Time card is a
    // deliberate flow the operator opened, but a pending restart still trumps it.
    for (id, card) in pending.settimes {
        overlay.add_overlay(&card.widget);
        shared.register_settime(&id, &card);
    }

    // Above even the cards: a pending restart covers the whole kiosk. Hidden
    // until the marker file appears -- see shutdown.rs. Same logo as the bar.
    let restarting = crate::shutdown::build(kiosk.logo.as_deref());
    overlay.add_overlay(&restarting.widget);
    shared.register_restarting(restarting);

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
        let confirms = confirms.clone();
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                // Confirms first: they are the topmost thing, and Escape on an
                // open card must cancel it rather than close the fan beneath.
                if confirms.iter().any(confirm::Confirm::is_open) {
                    for card in confirms.iter() {
                        card.close();
                    }
                    return gtk::glib::Propagation::Stop;
                }
                if fans.iter().any(radial::Fan::is_open) {
                    for fan in fans.iter() {
                        fan.close();
                    }
                    return gtk::glib::Propagation::Stop;
                }
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

#[cfg(test)]
mod tests {
    use super::box_for;

    /// What the box does to the three brands an SFO image actually carries.
    ///
    /// Brand `a` is a bare wordmark, bounded by the WIDTH, and comes out 302x88
    /// -- the C-specs mock's own measurement, reproduced exactly. The stacked
    /// lockups are bounded by the HEIGHT instead, which is the whole point:
    /// each stacks two lines of wordmark under an emblem, and at `a`'s 88 those
    /// lines are 8 px tall.
    #[test]
    fn each_mark_is_bounded_by_whichever_edge_bites_first() {
        // As packages/by-name/sfo-branding renders them, measured not assumed:
        // its renderHeight is 320 and the widths follow each source's ratio.
        let brand_a = (1097, 320);
        let brand_i = (388, 320);
        let brand_d = (372, 320);

        let drawn = |(w, h): (i32, i32)| {
            let b = f64::from(box_for(302, 160, w, h));
            let scale = b / f64::from(w.max(h));
            (
                (f64::from(w) * scale).round() as i32,
                (f64::from(h) * scale).round() as i32,
            )
        };

        assert_eq!(drawn(brand_a), (302, 88), "the mock's own measurement");
        assert_eq!(drawn(brand_i), (194, 160));
        assert_eq!(drawn(brand_d), (186, 160));

        // Nothing may exceed the box on either edge.
        for art in [brand_a, brand_i, brand_d] {
            let (w, h) = drawn(art);
            assert!(
                w <= 302 && h <= 160,
                "{art:?} drawn {w}x{h}, outside the box"
            );
        }
    }

    /// A square mark takes the height, and a 1x1 ratio must not be special
    /// cased to get there.
    #[test]
    fn a_square_mark_is_drawn_at_the_height_of_the_box() {
        assert_eq!(box_for(302, 160, 512, 512), 160);
        assert_eq!(box_for(302, 120, 64, 64), 120);
    }
}
