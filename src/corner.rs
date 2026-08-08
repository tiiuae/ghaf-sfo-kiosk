// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// Controls anchored to an edge of the surface rather than placed in the grid.
//
// NAMING, because it will trip you up: this module, the `Corner` type and the
// `corners` key in the config are all called "corner", but a control can sit at
// `bottom-center`, which is not one. Read "corner" as "edge control".
//
// Position is policy, not decoration:
//
//   - the two CORNERS are the hardest places on a touchscreen to hit by
//     accident and still easy to reach with a thumb, so they suit things you
//     need but rarely want;
//   - `bottom-center` is the easiest place on the screen to hit, which makes it
//     the opposite: right for something used constantly, wrong for anything you
//     would regret.
//
// Drawn as a dashed circle rather than a filled tile, which is what separates a
// utility from an application at a glance: the grid is solid squares. The icon
// comes from the desktop's own theme, so it matches the rest of the machine
// rather than being a hand-drawn approximation of it.
//
// Placement lives in `row`, not on the control, so that several controls can
// share one position. See `ui::build` for how the rows are attached.

use gtk::prelude::*;

use crate::config::{Position, ResolvedCorner};

/// Diameter of the dashed ring, and the icon inside it.
const RING: i32 = 112;
const ICON: i32 = 52;
/// Distance from the screen edges. Far enough in that the compositor's own edge
/// gestures do not compete for the first few pixels.
const INSET: i32 = 46;
/// Between controls sharing a position. Wide enough that a gloved thumb aiming
/// at one cannot land on its neighbour.
const GAP: i32 = 32;

/// Every position, in the order rows are added to the surface. Iterating a fixed
/// list rather than the config means the layout does not depend on the order the
/// controls happen to be written in.
pub const POSITIONS: [Position; 3] = [Position::Left, Position::Center, Position::Right];

/// The container for every control sharing one position.
///
/// A row even when it holds a single control: one code path, and adding a second
/// control to a position then changes nothing but the config.
pub fn row(position: Position) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, GAP);

    row.set_halign(match position {
        Position::Left => gtk::Align::Start,
        Position::Center => gtk::Align::Center,
        Position::Right => gtk::Align::End,
    });
    row.set_valign(gtk::Align::End);
    row.set_margin_start(INSET);
    row.set_margin_end(INSET);
    row.set_margin_top(INSET);
    row.set_margin_bottom(INSET);
    row
}

/// Build one edge control. `on_activate` runs on a plain tap.
pub fn build<F: Fn() + 'static>(corner: &ResolvedCorner, on_activate: F) -> gtk::Button {
    let ring = gtk::DrawingArea::new();
    ring.set_size_request(RING, RING);
    ring.set_draw_func(draw_ring);

    let disc = gtk::Overlay::new();
    disc.set_child(Some(&ring));
    if let Some(name) = &corner.icon {
        let icon = if name.starts_with('/') {
            gtk::Image::from_file(name)
        } else {
            gtk::Image::from_icon_name(name)
        };
        icon.set_pixel_size(ICON);
        icon.set_halign(gtk::Align::Center);
        icon.set_valign(gtk::Align::Center);
        icon.add_css_class("kiosk-corner-icon");
        disc.add_overlay(&icon);
    }

    let text = gtk::Box::new(gtk::Orientation::Vertical, 6);
    text.append(&disc);

    let label = gtk::Label::new(Some(&corner.label));
    label.add_css_class("kiosk-corner-label");
    text.append(&label);

    let button = gtk::Button::new();
    button.set_child(Some(&text));
    button.set_has_frame(false);
    button.add_css_class("kiosk-corner");

    // Same convention as the grid: a control that cannot run still renders,
    // dimmed, and says why when pressed. One that vanished would leave the
    // operator hunting for something that was never there.
    if matches!(corner.action, crate::config::Action::Unsupported { .. }) {
        button.add_css_class("kiosk-button-unconfigured");
    }

    button.connect_clicked(move |_| on_activate());
    button
}

fn draw_ring(_area: &gtk::DrawingArea, cr: &gtk::cairo::Context, width: i32, height: i32) {
    let (w, h) = (f64::from(width), f64::from(height));
    cr.set_source_rgba(0.55, 0.60, 0.62, 1.0);
    cr.set_line_width(3.0);
    cr.set_dash(&[7.0, 6.0], 0.0);
    cr.arc(w / 2.0, h / 2.0, w / 2.0 - 4.0, 0.0, std::f64::consts::TAU);
    let _ = cr.stroke();
}
