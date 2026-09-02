// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// The corner menu: a trigger in the bottom-left corner that fans its members out
// along a quarter arc bounded by the left and bottom edges.
//
// `Geometry` is GTK-free f64 arithmetic, so "every box on screen" and "no two
// boxes overlapping" are unit tests rather than an impression of one laptop.
//
// Animated by moving gtk::Fixed children from a tick callback. Not GSK
// transforms or a custom LayoutManager: both need glib subclassing.

use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::actions;
use crate::actions::Reporter;
use crate::config::{Action, Menu};

// ── Geometry ────────────────────────────────────────────────────────────────

/// Diameter of the trigger, and of each member's icon circle.
pub const TRIGGER_DIAMETER: f64 = 72.0;
pub const ICON_DIAMETER: f64 = 64.0;

/// A member's whole box: circle plus label below it. The button is the box, so
/// the hit target is the box while only the circle is drawn. Placement is by
/// CIRCLE centre, not box centre.
pub const ITEM_W: f64 = 100.0;
pub const ITEM_H: f64 = 88.0;

const CORNER_MARGIN: f64 = 28.0;
const EDGE_INSET: f64 = 8.0;

/// Smallest gap left between two member BOXES.
///
/// Boxes, not icon circles. Spacing the circles alone was the earlier bug: at
/// four members the circles sat 84px apart while each box is 96px tall, so every
/// label landed on the next circle -- "Update Apps" written across Network. A
/// label is not the empty space the old comment here claimed it was.
const MIN_BOX_GAP: f64 = 8.0;

/// What the fan wants, and the share of the smaller monitor dimension it settles
/// for on an output smaller than the laptop's own panel.
const PREFERRED_RADIUS: f64 = 280.0;
const RADIUS_FRACTION: f64 = 0.35;

/// Degrees anticlockwise from the bottom edge. Every member is spread evenly
/// across it. An earlier layout held one member back at 90 with a wider gap,
/// which read as a broken arc rather than as deliberate.
const MEMBER_ARC: (f64, f64) = (10.0, 90.0);

/// Positions in the fan's own coordinate space; the fan sits flush in the
/// bottom-left corner of the surface.
pub struct Geometry {
    pub width: f64,
    pub height: f64,
    pub radius: f64,
    /// Centre of the trigger.
    pub trigger: (f64, f64),
    /// Centre of each member's icon circle, nearest the corner first.
    pub items: Vec<(f64, f64)>,
}

impl Geometry {
    pub fn new(members: usize, monitor: (f64, f64)) -> Self {
        let all = spread(members, MEMBER_ARC);
        let radius = radius_for(monitor, &all);

        // Inset so the widest box clears the left edge at the top of the arc.
        let tx = (CORNER_MARGIN + TRIGGER_DIAMETER / 2.0).max(ITEM_W / 2.0 + EDGE_INSET);
        let ty = radius + ICON_DIAMETER / 2.0 + EDGE_INSET;

        let place = |deg: f64| {
            let rad = deg.to_radians();
            (tx + radius * rad.cos(), ty - radius * rad.sin())
        };
        let items: Vec<(f64, f64)> = all.iter().copied().map(place).collect();

        // Size to what it contains: nothing clipped, no dead space outside it.
        let corners = items.iter();
        let right = corners
            .clone()
            .map(|p| p.0 + ITEM_W / 2.0)
            .fold(tx + TRIGGER_DIAMETER / 2.0 + CORNER_MARGIN, f64::max);
        let bottom = corners
            .map(|p| p.1 - ICON_DIAMETER / 2.0 + ITEM_H)
            .fold(ty + TRIGGER_DIAMETER / 2.0 + CORNER_MARGIN, f64::max);

        Self {
            width: right + EDGE_INSET,
            height: bottom + EDGE_INSET,
            radius,
            trigger: (tx, ty),
            items,
        }
    }
}

/// `n` angles across `[lo, hi]`. A single member takes the middle of the span.
fn spread(n: usize, (lo, hi): (f64, f64)) -> Vec<f64> {
    match n {
        0 => Vec::new(),
        1 => vec![(lo + hi) / 2.0],
        _ => (0..n)
            .map(|i| lo + (hi - lo) * i as f64 / (n - 1) as f64)
            .collect(),
    }
}

/// The output sets the preferred size; the member count sets the minimum.
///
/// Clamping to a share of the output and stopping there is the trap: six members
/// on a 1280x720 output would then draw through each other.
///
/// Two axis-aligned boxes miss each other when they are clear in EITHER axis, so
/// each adjacent pair needs whichever separation is cheaper to buy:
///
///   dx = r|cos a - cos b| >= ITEM_W + gap    OR    dy = r|sin b - sin a| >= ITEM_H + gap
///
/// Solving each for r and taking the smaller gives what that pair needs; the
/// largest over all pairs is what the fan needs. Exact, so it is a unit test
/// rather than a constant someone has to re-derive.
fn radius_for(monitor: (f64, f64), angles: &[f64]) -> f64 {
    let smaller = monitor.0.min(monitor.1);
    let base = if smaller > 0.0 {
        PREFERRED_RADIUS.min(RADIUS_FRACTION * smaller)
    } else {
        PREFERRED_RADIUS
    };

    let mut required: f64 = 0.0;
    for pair in angles.windows(2) {
        let (a, b) = (pair[0].to_radians(), pair[1].to_radians());
        let dx = (a.cos() - b.cos()).abs();
        let dy = (b.sin() - a.sin()).abs();
        let need_x = if dx > f64::EPSILON {
            (ITEM_W + MIN_BOX_GAP) / dx
        } else {
            f64::INFINITY
        };
        let need_y = if dy > f64::EPSILON {
            (ITEM_H + MIN_BOX_GAP) / dy
        } else {
            f64::INFINITY
        };
        required = required.max(need_x.min(need_y));
    }
    base.max(required)
}

// ── The widget ──────────────────────────────────────────────────────────────

/// How long one member takes to travel, and how far apart their departures are.
const DURATION_MS: f64 = 220.0;
const STAGGER_MS: f64 = 40.0;

#[derive(Clone)]
pub struct Fan {
    /// Add this to the overlay ABOVE the scrim.
    pub widget: gtk::Fixed,
    trigger: gtk::ToggleButton,
}

impl Fan {
    /// Close from anywhere. Everything routes through the trigger's `toggled`
    /// signal, so there is one path that opens or closes.
    pub fn close(&self) {
        self.trigger.set_active(false);
    }

    pub fn is_open(&self) -> bool {
        self.trigger.is_active()
    }

    /// Open or close explicitly. Used to keep the same menu in step across
    /// outputs -- see shared.rs.
    pub fn set_open(&self, open: bool) {
        self.trigger.set_active(open);
    }

    /// Notified whenever this fan opens or closes, however it was driven.
    /// Everything routes through the trigger, so this sees every path.
    pub fn connect_toggled<F: Fn(bool) + 'static>(&self, f: F) {
        self.trigger.connect_toggled(move |t| f(t.is_active()));
    }

    /// Whether two handles are the same fan, so a broadcast can skip its
    /// originator. Compares the underlying GTK object, not the wrapper.
    pub fn same_as(&self, other: &Fan) -> bool {
        self.trigger == other.trigger
    }
}

/// Build one menu's corner trigger and its arc.
pub fn build<R>(
    menu: &Menu,
    monitor: (f64, f64),
    banner: &R,
    scrim: &gtk::Widget,
    shared: &crate::shared::Shared,
    // Out-param, not a field on `Fan`: every card must be added to the overlay
    // after ALL fans, and only ui::build knows when that is. Adding them here
    // would put a later fan above an earlier member's card.
    confirms: &mut Vec<(String, crate::confirm::Confirm)>,
) -> Fan
where
    R: Reporter + Clone,
{
    let geom = Geometry::new(menu.items.len(), monitor);
    // Radius depends on output size and member count; neither is visible on a
    // device, so log what it resolved to.
    log::info!(
        "menu {:?}: {} member(s), radius {:.0} on a {:.0}x{:.0} output",
        menu.trigger.id,
        menu.items.len(),
        geom.radius,
        monitor.0,
        monitor.1
    );

    let fixed = gtk::Fixed::new();
    fixed.add_css_class("kiosk-radial");
    fixed.set_halign(gtk::Align::Start);
    fixed.set_valign(gtk::Align::End);
    fixed.set_size_request(geom.width.ceil() as i32, geom.height.ceil() as i32);

    // ToggleButton for open/closed state and `:checked` styling. The cog does
    // NOT become an X when open: an X here reads as "close the menu", and a
    // thumb's width apart is the mis-tap this layout exists to avoid.
    let trigger = gtk::ToggleButton::new();
    trigger.add_css_class("kiosk-radial-trigger");
    trigger.set_size_request(TRIGGER_DIAMETER as i32, TRIGGER_DIAMETER as i32);
    trigger.set_tooltip_text(Some(&menu.trigger.label));
    if let Some(icon) = &menu.trigger.icon {
        let image = crate::ui::icon_image(icon);
        image.set_pixel_size(34);
        trigger.set_child(Some(&image));
    } else {
        trigger.set_label(&menu.trigger.label);
    }
    fixed.put(
        &trigger,
        geom.trigger.0 - TRIGGER_DIAMETER / 2.0,
        geom.trigger.1 - TRIGGER_DIAMETER / 2.0,
    );

    // Box position from a circle centre; the circle is at the top of the box.
    let box_at = |c: (f64, f64)| (c.0 - ITEM_W / 2.0, c.1 - ICON_DIAMETER / 2.0);
    // Where members rest while closed: under the trigger.
    let collapsed = box_at(geom.trigger);

    let mut sats: Vec<gtk::Button> = Vec::new();
    let mut targets: Vec<(f64, f64)> = Vec::new();

    for (i, item) in menu.items.iter().enumerate() {
        let button = satellite(
            item.icon.as_deref(),
            &item.label,
            item.description.as_deref(),
        );
        button.add_css_class(&format!("kiosk-radial-item-{}", item.id));
        // As in the grid: unrunnable still renders, dimmed, and says why.
        if matches!(item.action, Action::Unsupported { .. }) {
            button.add_css_class("kiosk-button-unconfigured");
        }

        let action = item.action.clone();
        let name = item.label.clone();
        let reporter = banner.clone();
        let trig = trigger.clone();
        // Per member id, not per output: shared.busy_for gives the SAME flag
        // to this menu item on every screen. See Shared::busy_for.
        let busy = shared.busy_for(&item.id);
        let fire = move || actions::dispatch(&action, &name, &reporter, &busy);

        if let Some(spec_confirm) = &item.confirm {
            let card = crate::confirm::build(spec_confirm, &item.label, fire);
            confirms.push((item.id.clone(), card.clone()));
            button.connect_clicked(move |_| {
                // Close FIRST, for the same reason as below: a card must not
                // open behind the fan it was pressed in.
                trig.set_active(false);
                card.open();
            });
        } else {
            button.connect_clicked(move |_| {
                // Close FIRST, so a launched window never appears behind an open fan.
                trig.set_active(false);
                fire();
            });
        }

        fixed.put(&button, collapsed.0, collapsed.1);
        button.set_visible(false);
        targets.push(box_at(geom.items[i]));
        sats.push(button);
    }

    let sats = Rc::new(sats);
    let targets = Rc::new(targets);
    // Bumped per toggle; a tick callback with a stale generation stops itself,
    // so closing halfway through opening works.
    let generation = Rc::new(Cell::new(0u64));

    trigger.connect_toggled({
        let fixed = fixed.clone();
        let scrim = scrim.clone();
        let sats = sats.clone();
        let targets = targets.clone();
        let generation = generation.clone();
        move |t| {
            let opening = t.is_active();
            let mine = generation.get().wrapping_add(1);
            generation.set(mine);

            if opening {
                scrim.add_css_class("kiosk-scrim-open");
                scrim.set_can_target(true);
                for s in sats.iter() {
                    s.set_opacity(0.0);
                    s.set_visible(true);
                }
            } else {
                scrim.remove_css_class("kiosk-scrim-open");
                scrim.set_can_target(false);
            }

            // Unrealized: no frame clock to animate with, so snap.
            let Some(_) = fixed.frame_clock() else {
                settle(&fixed, &sats, &targets, collapsed, opening);
                return;
            };

            let n = sats.len();
            let begin: Cell<Option<i64>> = Cell::new(None);
            let fixed2 = fixed.clone();
            let sats2 = sats.clone();
            let targets2 = targets.clone();
            let generation2 = generation.clone();
            fixed.add_tick_callback(move |_, clock| {
                if generation2.get() != mine {
                    return glib::ControlFlow::Break;
                }
                let now = clock.frame_time();
                let t0 = match begin.get() {
                    Some(t0) => t0,
                    None => {
                        begin.set(Some(now));
                        now
                    }
                };
                let ms = (now - t0) as f64 / 1000.0;
                let total = DURATION_MS + STAGGER_MS * (n.saturating_sub(1)) as f64;

                for (i, s) in sats2.iter().enumerate() {
                    // Closing runs the stagger backwards: folds from the far end.
                    let step = if opening { i } else { n - 1 - i };
                    let p = ((ms - STAGGER_MS * step as f64) / DURATION_MS).clamp(0.0, 1.0);
                    let travelled = if opening {
                        ease_out_back(p)
                    } else {
                        1.0 - ease_out_cubic(p)
                    };
                    let (tx, ty) = targets2[i];
                    fixed2.move_(
                        s,
                        collapsed.0 + (tx - collapsed.0) * travelled,
                        collapsed.1 + (ty - collapsed.1) * travelled,
                    );
                    s.set_opacity(if opening { p } else { 1.0 - p });
                }

                if ms >= total {
                    if !opening {
                        // An opacity-0 Fixed child is still focusable and still
                        // clickable; hiding takes it out of both.
                        for s in sats2.iter() {
                            s.set_visible(false);
                        }
                    }
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });

            if opening {
                // On an idle: the members have not had a layout pass yet, and
                // grab_focus on a not-yet-mappable widget fails silently.
                if let Some(first) = sats.first().cloned() {
                    glib::idle_add_local_once(move || {
                        if first.is_visible() {
                            first.grab_focus();
                        }
                    });
                }
            }
        }
    });

    Fan {
        widget: fixed,
        trigger,
    }
}

/// Put every member straight into its end state, with no animation.
fn settle(
    fixed: &gtk::Fixed,
    sats: &[gtk::Button],
    targets: &[(f64, f64)],
    collapsed: (f64, f64),
    opening: bool,
) {
    for (i, s) in sats.iter().enumerate() {
        let (x, y) = if opening { targets[i] } else { collapsed };
        fixed.move_(s, x, y);
        s.set_opacity(if opening { 1.0 } else { 0.0 });
        s.set_visible(opening);
    }
}

/// One member: a button the size of the whole box, drawing only a circle.
///
/// The hit target is the box (132x104), not the circle (72x72) -- the panel may
/// be a touchscreen and nothing here falls back to hover.
fn satellite(icon: Option<&str>, label: &str, tooltip: Option<&str>) -> gtk::Button {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.set_valign(gtk::Align::Start);

    let disc = gtk::Box::new(gtk::Orientation::Vertical, 0);
    disc.add_css_class("kiosk-radial-item-icon");
    disc.set_size_request(ICON_DIAMETER as i32, ICON_DIAMETER as i32);
    disc.set_halign(gtk::Align::Center);
    if let Some(name) = icon {
        let image = crate::ui::icon_image(name);
        image.set_pixel_size(34);
        image.set_hexpand(true);
        image.set_vexpand(true);
        disc.append(&image);
    }
    column.append(&disc);

    let text = gtk::Label::new(Some(label));
    text.add_css_class("kiosk-radial-item-label");
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.set_max_width_chars(12);
    column.append(&text);

    let button = gtk::Button::new();
    button.set_child(Some(&column));
    button.add_css_class("kiosk-radial-item");
    button.set_size_request(ITEM_W as i32, ITEM_H as i32);
    if let Some(t) = tooltip {
        button.set_tooltip_text(Some(t));
    }
    button
}

/// Small overshoot at the end of the travel: reads as a fan opening rather than
/// buttons appearing.
fn ease_out_back(p: f64) -> f64 {
    const C1: f64 = 1.70158;
    const C3: f64 = C1 + 1.0;
    let q = p - 1.0;
    1.0 + C3 * q * q * q + C1 * q * q
}

/// No overshoot on close: it would read as a bounce off the corner.
fn ease_out_cubic(p: f64) -> f64 {
    let q = 1.0 - p;
    1.0 - q * q * q
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The laptop panel, its taller variant, a projector, a small 4:3 display.
    const SCREENS: [(f64, f64); 4] = [
        (1920.0, 1080.0),
        (1920.0, 1200.0),
        (1280.0, 720.0),
        (1024.0, 768.0),
    ];

    /// "Bounded by the left and bottom edges", asserted rather than eyeballed.
    #[test]
    fn every_member_box_stays_inside_the_fan() {
        for screen in SCREENS {
            for members in 0..=6 {
                let g = Geometry::new(members, screen);
                for (cx, cy) in g.items.iter().copied() {
                    let left = cx - ITEM_W / 2.0;
                    let top = cy - ICON_DIAMETER / 2.0;
                    assert!(
                        left >= 0.0 && top >= 0.0,
                        "{members} members, {screen:?}: box at \
                         ({left}, {top}) crosses the left or top edge"
                    );
                    assert!(
                        left + ITEM_W <= g.width && top + ITEM_H <= g.height,
                        "{members} members, {screen:?}: box at \
                         ({left}, {top}) leaves the {}x{} fan",
                        g.width,
                        g.height
                    );
                }
            }
        }
    }

    /// The regression that shipped: spacing only the circles let every label land
    /// on the next circle. Boxes, so labels are covered.
    #[test]
    fn no_two_member_boxes_overlap() {
        for screen in SCREENS {
            for members in 2..=6 {
                let g = Geometry::new(members, screen);
                for pair in g.items.windows(2) {
                    let dx = (pair[1].0 - pair[0].0).abs();
                    let dy = (pair[1].1 - pair[0].1).abs();
                    // Axis-aligned boxes miss when clear in EITHER axis.
                    assert!(
                        dx >= ITEM_W - 0.5 || dy >= ITEM_H - 0.5,
                        "{members} members, {screen:?}: boxes \
                         only dx={dx:.0} dy={dy:.0} apart, need dx>={ITEM_W} or \
                         dy>={ITEM_H}"
                    );
                }
            }
        }
    }

    /// A smaller output gets a smaller fan; a crowded arc overrides that, which
    /// is what stops members overlapping on a small screen.
    #[test]
    fn the_radius_follows_the_output_until_the_member_count_needs_more() {
        let sparse_big = Geometry::new(2, (1920.0, 1080.0)).radius;
        let sparse_small = Geometry::new(2, (1280.0, 720.0)).radius;
        assert!(
            sparse_small < sparse_big,
            "a smaller output should get a smaller fan: {sparse_small} vs {sparse_big}"
        );

        let crowded = Geometry::new(7, (1280.0, 720.0));
        assert!(
            crowded.radius > RADIUS_FRACTION * 720.0,
            "six members must outgrow the output's share, got {}",
            crowded.radius
        );
    }

    /// Members never grow the fan smaller, and the whole thing still fits the
    /// output it was sized for.
    #[test]
    fn the_fan_grows_monotonically_and_fits_the_output() {
        for screen in SCREENS {
            let radii: Vec<f64> = (1..=6).map(|n| Geometry::new(n, screen).radius).collect();
            for pair in radii.windows(2) {
                assert!(
                    pair[1] >= pair[0],
                    "{screen:?}: adding a member shrank the fan: {radii:?}"
                );
            }
            for members in 0..=6 {
                let g = Geometry::new(members, screen);
                assert!(
                    g.width <= screen.0 && g.height <= screen.1,
                    "{members} members on {screen:?}: fan {}x{} does not fit",
                    g.width,
                    g.height
                );
            }
        }
    }

    /// The shipped SFO arc: Network, Update, Lock, Power, on the laptop's own
    /// panel. The grid is three 280px tiles with 36px gaps, centred -- so it
    /// starts at x=504 and the fan must not reach it.
    #[test]
    fn the_sfo_arc_clears_the_grid_on_the_laptop_panel() {
        let g = Geometry::new(4, (1920.0, 1080.0));
        let grid_left = (1920.0 - (3.0 * 280.0 + 2.0 * 36.0)) / 2.0;
        assert!(
            g.width <= grid_left,
            "fan is {:.0} wide but the leftmost tile starts at {grid_left:.0}",
            g.width
        );
    }

    #[test]
    fn one_member_takes_the_middle_of_the_span() {
        assert_eq!(spread(1, (10.0, 90.0)), vec![50.0]);
        assert_eq!(spread(0, (10.0, 90.0)), Vec::<f64>::new());
        assert_eq!(spread(3, (10.0, 90.0)), vec![10.0, 50.0, 90.0]);
    }

    #[test]
    fn the_easings_start_and_finish_where_they_should() {
        assert!(ease_out_back(0.0).abs() < 1e-9);
        assert!((ease_out_back(1.0) - 1.0).abs() < 1e-9);
        assert!(ease_out_cubic(0.0).abs() < 1e-9);
        assert!((ease_out_cubic(1.0) - 1.0).abs() < 1e-9);
        // The overshoot is the point of ease_out_back.
        assert!(ease_out_back(0.8) > 1.0);
    }
}
