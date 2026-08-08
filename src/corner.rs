// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// Controls anchored to the bottom edge of the surface rather than placed in the
// grid: the things an operator needs but does not spend the day in.
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
// A circle, where an application is a rounded square. That difference is what
// tells an operator at a glance which things are work and which are the machine,
// before any label is read. The circle is a styled box rather than something
// drawn with cairo, so it picks up the same hover and focus states as every
// other control and the palette lives in exactly one place.
//
// Placement lives in `row`, not on the control, so that several controls can
// share one position. See `ui::build` for how the rows are attached.

use gtk::prelude::*;

use crate::config::{Position, ResolvedCorner};

/// Diameter of the disc, and the icon inside it. Both are set here rather than
/// in CSS because GTK sizes an image in pixels, not by style.
const DISC: i32 = 108;
const ICON: i32 = 44;
/// Distance from the screen edges. Far enough in that the compositor's own edge
/// gestures do not compete for the first few pixels.
const INSET: i32 = 44;
/// Between controls sharing a position. Wide enough that a gloved thumb aiming
/// at one cannot land on its neighbour.
const GAP: i32 = 40;

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
    row.set_margin_bottom(INSET);
    row
}

/// Build one edge control. `on_activate` runs on a plain tap.
pub fn build<F: Fn() + 'static>(corner: &ResolvedCorner, on_activate: F) -> gtk::Button {
    let disc = gtk::Box::new(gtk::Orientation::Vertical, 0);
    disc.set_size_request(DISC, DISC);
    disc.set_halign(gtk::Align::Center);
    disc.add_css_class("kiosk-corner-disc");

    if let Some(name) = &corner.icon {
        let icon = if name.starts_with('/') {
            gtk::Image::from_file(name)
        } else {
            gtk::Image::from_icon_name(name)
        };
        icon.set_pixel_size(
            corner
                .icon_size
                .and_then(|n| i32::try_from(n).ok())
                .unwrap_or(ICON),
        );
        icon.set_vexpand(true);
        icon.set_halign(gtk::Align::Center);
        icon.set_valign(gtk::Align::Center);
        icon.add_css_class("kiosk-corner-icon");
        disc.append(&icon);
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.append(&disc);

    let label = gtk::Label::new(Some(&corner.label));
    label.add_css_class("kiosk-corner-label");
    content.append(&label);

    let button = gtk::Button::new();
    button.set_child(Some(&content));
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
