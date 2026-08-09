// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// The corner menu: a trigger in the bottom-left corner that fans its members out
// along a quarter arc bounded by the left and bottom edges.
//
// Split in two on purpose. `Geometry` is arithmetic over f64 with no GTK in it,
// so the two properties that decide whether this looks right -- every member's
// box on screen, no two icon circles touching -- are unit tests rather than
// something you check by looking at one laptop. The widget half below is the
// part that genuinely needs a compositor, and it is kept thin.
//
// Positions are animated by moving children of a gtk::Fixed from a tick
// callback. Not GSK transforms and not a custom LayoutManager: both would need
// glib subclassing for an effect two lines of arithmetic already produce.

use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::actions;
use crate::banner::Banner;
use crate::config::{Action, ExitButton, Menu};

// ── Geometry ────────────────────────────────────────────────────────────────

/// Diameter of the trigger, and of each member's icon circle.
pub const TRIGGER_DIAMETER: f64 = 72.0;
pub const ICON_DIAMETER: f64 = 72.0;

/// A member is a button the size of its whole box -- circle plus label -- so the
/// touch target is the whole box while only the circle is drawn. The circle sits
/// at the top of the box, which is why placement is by circle centre and not by
/// box centre.
pub const ITEM_W: f64 = 132.0;
pub const ITEM_H: f64 = 104.0;

/// Gap from the fan's own edges to the trigger, and the smallest gap any box is
/// allowed to have.
const CORNER_MARGIN: f64 = 28.0;
const EDGE_INSET: f64 = 8.0;

/// Closest two icon circles may come. Circles, not boxes: boxes are mostly empty
/// space around a centred label, and demanding they never overlap would push the
/// radius past anything that fits on a small output.
const MIN_ICON_GAP: f64 = 12.0;

/// What the fan wants, and the share of the smaller monitor dimension it will
/// settle for -- a projector at 1280x720 should not get the laptop's fan.
const PREFERRED_RADIUS: f64 = 280.0;
const RADIUS_FRACTION: f64 = 0.35;

/// Degrees anticlockwise from the bottom edge. When exit is a member it gets the
/// top of the arc to itself: it is the one press in this menu we cannot afford
/// the operator to make by accident, and the 28-degree gap below it is the
/// mitigation, not decoration.
const MEMBER_ARC_WITH_EXIT: (f64, f64) = (10.0, 62.0);
const MEMBER_ARC_ALONE: (f64, f64) = (10.0, 90.0);
const EXIT_ANGLE: f64 = 90.0;

/// Where everything goes, in the fan's own coordinate space. The fan is placed
/// flush into the bottom-left corner of the surface, so these are also screen
/// coordinates offset by the surface height.
pub struct Geometry {
    pub width: f64,
    pub height: f64,
    pub radius: f64,
    /// Centre of the trigger.
    pub trigger: (f64, f64),
    /// Centre of each member's icon circle, nearest the corner first.
    pub items: Vec<(f64, f64)>,
    /// Centre of exit's icon circle, when exit is a member of this menu.
    pub exit: Option<(f64, f64)>,
}

impl Geometry {
    pub fn new(members: usize, with_exit: bool, monitor: (f64, f64)) -> Self {
        let arc = if with_exit {
            MEMBER_ARC_WITH_EXIT
        } else {
            MEMBER_ARC_ALONE
        };
        let member_angles = spread(members, arc);

        let mut all = member_angles.clone();
        if with_exit {
            all.push(EXIT_ANGLE);
        }
        let radius = radius_for(monitor, &all);

        // Inset far enough that the widest box still clears the left edge when
        // it sits at the top of the arc, directly above the trigger.
        let tx = (CORNER_MARGIN + TRIGGER_DIAMETER / 2.0).max(ITEM_W / 2.0 + EDGE_INSET);
        // ... and high enough that the topmost circle's box clears the top.
        let ty = radius + ICON_DIAMETER / 2.0 + EDGE_INSET;

        let place = |deg: f64| {
            let rad = deg.to_radians();
            (tx + radius * rad.cos(), ty - radius * rad.sin())
        };
        let items: Vec<(f64, f64)> = member_angles.iter().copied().map(place).collect();
        let exit = with_exit.then(|| place(EXIT_ANGLE));

        // Size the fan around what it actually contains, so nothing is clipped
        // and nothing outside it is covered by dead space.
        let corners = items.iter().chain(exit.iter());
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
            exit,
        }
    }
}

/// `n` angles across `[lo, hi]`. One member takes the middle of the span rather
/// than an end of it, which is the only reading that is not arbitrary.
fn spread(n: usize, (lo, hi): (f64, f64)) -> Vec<f64> {
    match n {
        0 => Vec::new(),
        1 => vec![(lo + hi) / 2.0],
        _ => (0..n)
            .map(|i| lo + (hi - lo) * i as f64 / (n - 1) as f64)
            .collect(),
    }
}

/// The output decides how big the fan would like to be; the member count decides
/// how small it is allowed to be.
///
/// Clamping down to a share of the output and stopping there is the trap: five
/// members on a 1280x720 projector would then draw their circles through each
/// other. A fan that is large for its screen is worse-looking than one whose
/// icons overlap is unusable, so the count wins.
fn radius_for(monitor: (f64, f64), angles: &[f64]) -> f64 {
    let smaller = monitor.0.min(monitor.1);
    let base = if smaller > 0.0 {
        PREFERRED_RADIUS.min(RADIUS_FRACTION * smaller)
    } else {
        PREFERRED_RADIUS
    };

    let tightest = angles
        .windows(2)
        .map(|w| w[1] - w[0])
        .fold(f64::INFINITY, f64::min);
    if !tightest.is_finite() || tightest <= 0.0 {
        return base;
    }
    // chord = 2 r sin(delta / 2)
    let required = (ICON_DIAMETER + MIN_ICON_GAP) / (2.0 * (tightest.to_radians() / 2.0).sin());
    base.max(required)
}

// ── The widget ──────────────────────────────────────────────────────────────

/// How long one member takes to travel, and how far apart their departures are.
const DURATION_MS: f64 = 220.0;
const STAGGER_MS: f64 = 40.0;

pub struct Fan {
    /// Add this to the overlay ABOVE the scrim.
    pub widget: gtk::Fixed,
    trigger: gtk::ToggleButton,
}

impl Fan {
    /// Close the fan from anywhere. Everything routes through the trigger's
    /// `toggled` signal, so there is exactly one path that opens or closes it.
    pub fn close(&self) {
        self.trigger.set_active(false);
    }

    pub fn is_open(&self) -> bool {
        self.trigger.is_active()
    }
}

/// Build one menu's corner trigger and its arc.
///
/// `exit` is `Some` only when the config placed the exit button in *this* menu;
/// otherwise ui.rs keeps it as the small button in the opposite corner.
pub fn build(
    menu: &Menu,
    exit: Option<&ExitButton>,
    monitor: (f64, f64),
    app: &gtk::Application,
    banner: &Banner,
    scrim: &gtk::Widget,
) -> Fan {
    let geom = Geometry::new(menu.items.len(), exit.is_some(), monitor);
    // The radius is derived from two things that are not both obvious on a
    // device -- the output size and the member count -- so say what came out.
    log::info!(
        "menu {:?}: {} member(s){}, radius {:.0} on a {:.0}x{:.0} output",
        menu.trigger.id,
        menu.items.len(),
        if exit.is_some() { " plus exit" } else { "" },
        geom.radius,
        monitor.0,
        monitor.1
    );

    let fixed = gtk::Fixed::new();
    fixed.add_css_class("kiosk-radial");
    fixed.set_halign(gtk::Align::Start);
    fixed.set_valign(gtk::Align::End);
    fixed.set_size_request(geom.width.ceil() as i32, geom.height.ceil() as i32);

    // A ToggleButton, so open/closed state and its `:checked` styling come from
    // GTK. The cog deliberately does NOT become an X when open: exit now sits on
    // the same arc, and two X's a thumb's width apart is the mistake this whole
    // layout is arranged to avoid.
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

    // Box position from a circle centre: the circle is at the top of the box.
    let box_at = |c: (f64, f64)| (c.0 - ITEM_W / 2.0, c.1 - ICON_DIAMETER / 2.0);
    // Where a member rests before it has fanned out: under the trigger.
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
        // Same treatment as the grid: an unrunnable button still renders,
        // dimmed, and says why when pressed.
        if matches!(item.action, Action::Unsupported { .. }) {
            button.add_css_class("kiosk-button-unconfigured");
        }

        let action = item.action.clone();
        let name = item.label.clone();
        let reporter = banner.clone();
        let trig = trigger.clone();
        button.connect_clicked(move |_| {
            // Close FIRST. An application launched from here must never come up
            // behind an open fan.
            trig.set_active(false);
            actions::dispatch(&action, &name, &reporter);
        });

        fixed.put(&button, collapsed.0, collapsed.1);
        button.set_visible(false);
        targets.push(box_at(geom.items[i]));
        sats.push(button);
    }

    if let (Some(spec), Some(centre)) = (exit, geom.exit) {
        let button = satellite(Some(&spec.icon), &spec.label, Some(&spec.label));
        // Muted, and its own class: it is the only member that ends the kiosk.
        button.add_css_class("kiosk-radial-exit");

        let app = app.clone();
        button.connect_clicked(move |_| {
            // Quit cleanly. The systemd unit's ExecStopPost is what restores the
            // COSMIC panel and shortcuts -- doing it here instead would not
            // survive a crash, which is the case that matters.
            log::info!("exit button pressed; quitting");
            app.quit();
        });

        fixed.put(&button, collapsed.0, collapsed.1);
        button.set_visible(false);
        targets.push(box_at(centre));
        sats.push(button);
    }

    let sats = Rc::new(sats);
    let targets = Rc::new(targets);
    // Bumped on every toggle. An animation whose generation is stale stops,
    // which is how a fan closed halfway through opening does the right thing.
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

            // Unrealized, so there is no frame clock to drive anything: snap.
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
                    // Closing runs the stagger backwards, so the fan folds from
                    // its far end rather than collapsing from the corner out.
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
                        // Out of the focus chain and out of hit-testing. An
                        // opacity-0 Fixed child is still both.
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
                // On an idle, not here. The members were hidden a few lines
                // ago and have not been through a layout pass yet; grabbing
                // focus on a widget that is not mappable yet fails silently,
                // and the symptom is Tab starting from nowhere.
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
/// The button rather than the circle is the hit target on purpose -- 132x104
/// instead of 72x72 -- because this is a corner control on a machine that may
/// have a touchscreen, and nothing here falls back to hover.
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
    text.set_max_width_chars(14);
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

/// A small overshoot at the end of the travel. It is what makes this read as a
/// fan opening rather than four buttons appearing.
fn ease_out_back(p: f64) -> f64 {
    const C1: f64 = 1.70158;
    const C3: f64 = C1 + 1.0;
    let q = p - 1.0;
    1.0 + C3 * q * q * q + C1 * q * q
}

/// No overshoot on the way back in: an overshoot on close reads as a bounce off
/// the corner.
fn ease_out_cubic(p: f64) -> f64 {
    let q = 1.0 - p;
    1.0 - q * q * q
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The laptop's own panel, the taller variant of it, a projector, and a
    /// small 4:3 display.
    const SCREENS: [(f64, f64); 4] = [
        (1920.0, 1080.0),
        (1920.0, 1200.0),
        (1280.0, 720.0),
        (1024.0, 768.0),
    ];

    fn circles(g: &Geometry) -> Vec<(f64, f64)> {
        g.items.iter().chain(g.exit.iter()).copied().collect()
    }

    /// "Bounded by the left vertical edge and the bottom edge" as an assertion
    /// rather than as an impression of one screenshot.
    #[test]
    fn every_member_box_stays_inside_the_fan() {
        for screen in SCREENS {
            for members in 0..=6 {
                for with_exit in [false, true] {
                    let g = Geometry::new(members, with_exit, screen);
                    for (cx, cy) in circles(&g) {
                        let left = cx - ITEM_W / 2.0;
                        let top = cy - ICON_DIAMETER / 2.0;
                        assert!(
                            left >= 0.0 && top >= 0.0,
                            "{members} members, exit={with_exit}, {screen:?}: box at \
                             ({left}, {top}) crosses the left or top edge"
                        );
                        assert!(
                            left + ITEM_W <= g.width && top + ITEM_H <= g.height,
                            "{members} members, exit={with_exit}, {screen:?}: box at \
                             ({left}, {top}) leaves the {}x{} fan",
                            g.width,
                            g.height
                        );
                    }
                }
            }
        }
    }

    /// The property that decides whether the arc looks deliberate or crowded.
    #[test]
    fn no_two_icon_circles_come_closer_than_the_gap() {
        for screen in SCREENS {
            for members in 2..=6 {
                for with_exit in [false, true] {
                    let g = Geometry::new(members, with_exit, screen);
                    let c = circles(&g);
                    for pair in c.windows(2) {
                        let d = (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1);
                        assert!(
                            d >= ICON_DIAMETER + MIN_ICON_GAP - 0.5,
                            "{members} members, exit={with_exit}, {screen:?}: circles \
                             {d} apart, want at least {}",
                            ICON_DIAMETER + MIN_ICON_GAP
                        );
                    }
                }
            }
        }
    }

    /// The mis-tap mitigation, asserted so a later tidy-up of the arc constants
    /// cannot quietly remove it.
    #[test]
    fn exit_is_alone_at_the_top_with_a_wider_gap_than_the_members_have() {
        let g = Geometry::new(3, true, (1920.0, 1080.0));
        let exit = g.exit.expect("exit was asked for");

        // 90 degrees: directly above the trigger, against the left edge.
        assert!((exit.0 - g.trigger.0).abs() < 0.001);
        assert!(exit.1 < g.items.last().unwrap().1, "exit is the topmost");

        let gap = |a: (f64, f64), b: (f64, f64)| (b.0 - a.0).hypot(b.1 - a.1);
        let to_exit = gap(*g.items.last().unwrap(), exit);
        let between = gap(g.items[0], g.items[1]);
        assert!(
            to_exit > between,
            "exit sits {to_exit} from the last member but members are {between} apart"
        );
    }

    /// A projector gets a smaller fan; a crowded arc gets a bigger one whatever
    /// the output says. The second half is the one that stops circles from
    /// overlapping on a small screen.
    #[test]
    fn the_radius_follows_the_output_until_the_member_count_needs_more() {
        let laptop = Geometry::new(3, true, (1920.0, 1080.0));
        assert_eq!(laptop.radius, PREFERRED_RADIUS);

        let projector = Geometry::new(3, true, (1280.0, 720.0));
        assert!(
            projector.radius < laptop.radius,
            "a smaller output should get a smaller fan, got {}",
            projector.radius
        );

        let crowded = Geometry::new(6, true, (1280.0, 720.0));
        assert!(
            crowded.radius > RADIUS_FRACTION * 720.0,
            "six members must outgrow the output's share, got {}",
            crowded.radius
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
