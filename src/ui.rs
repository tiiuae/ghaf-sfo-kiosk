// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// The widget tree. Binds and emits; never spawns a process or touches D-Bus.
//
// NEVER call .present() on anything but the layer-shell window: any other
// gtk::Window becomes an xdg_toplevel, floating above the kiosk and appearing in
// alt-tab. Hence an in-window banner rather than a dialog, and no libadwaita.

use gtk::prelude::*;

use crate::actions::{self, Reporter};
use crate::config::Kiosk;
use crate::{corner, status};

/// Default optical size for a grid icon; see `Button::icon_size` for why an
/// individual icon may want a different one.
const ICON: i32 = 58;

#[derive(Clone)]
pub struct Banner {
    revealer: gtk::Revealer,
    label: gtk::Label,
}

impl Banner {
    fn new() -> Self {
        let label = gtk::Label::new(None);
        label.set_wrap(true);
        label.set_xalign(0.0);

        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        revealer.set_child(Some(&label));
        revealer.set_reveal_child(false);
        revealer.add_css_class("kiosk-banner");

        Self { revealer, label }
    }

    fn show(&self, text: &str, css: &str, seconds: u32) {
        self.revealer.remove_css_class("kiosk-banner-error");
        self.revealer.remove_css_class("kiosk-banner-info");
        self.revealer.add_css_class(css);
        self.label.set_text(text);
        self.revealer.set_reveal_child(true);

        let revealer = self.revealer.clone();
        let expected = text.to_owned();
        let label = self.label.clone();
        gtk::glib::timeout_add_seconds_local_once(seconds, move || {
            // Only hide if we are still showing the same message, so a newer
            // message is not cut short by an older message's timer.
            if label.text() == expected {
                revealer.set_reveal_child(false);
            }
        });
    }
}

impl Reporter for Banner {
    fn info(&self, message: &str) {
        self.show(message, "kiosk-banner-info", 4);
    }
    fn error(&self, message: &str) {
        // Longer, because the operator may need to read it twice and there is
        // nowhere else for them to look.
        self.show(message, "kiosk-banner-error", 15);
    }
}

/// Build the whole kiosk surface content for one output.
pub fn build(kiosk: &Kiosk, app: &gtk::Application) -> gtk::Widget {
    let banner = Banner::new();

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
    grid.set_row_spacing(32);
    grid.set_column_spacing(44);
    grid.set_homogeneous(true);
    grid.add_css_class("kiosk-grid");

    for spec in &kiosk.buttons {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        content.set_valign(gtk::Align::Center);
        if let Some(icon) = &spec.icon {
            let image = if icon.starts_with('/') {
                gtk::Image::from_file(icon)
            } else {
                gtk::Image::from_icon_name(icon)
            };
            // Optical size: see `Button::icon_size`. The default suits a glyph
            // of average density; a solid mass wants less and thin strokes more.
            image.set_pixel_size(
                spec.icon_size
                    .and_then(|n| i32::try_from(n).ok())
                    .unwrap_or(ICON),
            );
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
        let reporter = banner.clone();
        button.connect_clicked(move |_| actions::dispatch(&action, &name, &reporter));

        grid.append(&button);
    }

    // ── exit ────────────────────────────────────────────────────────────────
    //
    // Deliberately small and in the bottom-right corner: this is a maintenance
    // affordance, not a normal part of the workflow.
    let exit = gtk::Button::new();
    exit.set_icon_name(&kiosk.exit.icon);
    exit.set_tooltip_text(Some(&kiosk.exit.label));
    exit.add_css_class("kiosk-exit");
    exit.set_halign(gtk::Align::End);
    exit.set_valign(gtk::Align::End);
    {
        let app = app.clone();
        exit.connect_clicked(move |_| {
            // Quit cleanly. The systemd unit's ExecStopPost is what restores the
            // COSMIC panel and shortcuts -- doing it here instead would not
            // survive a crash, which is the case that matters.
            log::info!("exit button pressed; quitting");
            app.quit();
        });
    }

    // ── assembly ────────────────────────────────────────────────────────────
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.append(&bar);
    column.append(&banner.revealer);
    grid.set_vexpand(true);
    column.append(&grid);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&column));
    overlay.add_overlay(&exit);

    // Edge controls: one row per occupied position, over the grid rather than
    // inside it. They are anchored to the surface, not to the end of a list of
    // things, and must stay put however many buttons the grid has. Controls
    // sharing a position would otherwise be drawn on top of each other.
    for position in corner::POSITIONS {
        let specs: Vec<_> = kiosk
            .corners
            .iter()
            .filter(|c| c.position == position)
            .collect();
        if specs.is_empty() {
            continue;
        }
        let row = corner::row(position);
        for spec in specs {
            row.append(&corner::build(spec, {
                let action = spec.action.clone();
                let label = spec.label.clone();
                let reporter = banner.clone();
                move || actions::dispatch(&action, &label, &reporter)
            }));
        }
        overlay.add_overlay(&row);
    }

    overlay.upcast()
}

/// Styling. Inline rather than a GResource so there is one less build step and
/// one less thing that can be stale on the device.
pub const CSS: &str = "
/* One palette, stated once.
 *
 * background #0b0f14 | surface #151c25 | raised #1d2734 | line #253141
 * text #e8eef5 | muted #8194a8 | accent #5ecfb8
 *
 * Dark because the device is used outdoors at night as often as in daylight,
 * and a bright screen in the dark costs an operator their night vision for
 * minutes. High contrast between text and surface, low contrast between
 * surfaces, so the eye goes to the labels and not to the boxes. */

window.kiosk-root {
    background-color: #0b0f14;
    color: #e8eef5;
}

/* ── status bar ──────────────────────────────────────────────────────────── */
.kiosk-statusbar {
    padding: 16px 30px;
    background-color: transparent;
    border-bottom: 1px solid #172029;
}
.kiosk-title {
    color: #8194a8;
    font-size: 13px;
    font-weight: 600;
    letter-spacing: 5px;
}
.kiosk-clock { font-size: 19px; font-weight: 600; color: #e8eef5; }
.kiosk-statusbar image { color: #8194a8; }
.kiosk-statusbar label { color: #8194a8; font-size: 14px; }

/* ── application tiles ───────────────────────────────────────────────────── */
/* Bottom padding, not a margin on the row: the grid is centred in whatever
   space it has, so reserving the edge row's height here lifts the tiles clear
   of it instead of letting the two crowd each other. */
.kiosk-grid { padding: 48px 48px 210px 48px; }
.kiosk-button {
    min-width: 210px;
    min-height: 172px;
    padding: 22px;
    border-radius: 20px;
    /* background-image, not just background-color: the stock theme paints a
       gradient over any colour set here, which is why this used to render as a
       white tile on a dark screen however dark the colour was. */
    background-image: none;
    background-color: #151c25;
    border: 1px solid #253141;
    box-shadow: none;
    color: #e8eef5;
    transition: background-color 130ms ease, border-color 130ms ease;
}
.kiosk-button:hover { background-color: #1d2734; border-color: #35485e; }
.kiosk-button:active { background-color: #223041; }
/* Icons are the same near-white as the labels. The accent is NOT decoration:
   it marks the one thing the operator came to do, and a config picks that out
   by id (`.kiosk-button-<id>`). Colour used on everything signals nothing. */
.kiosk-button image { color: #dfe8f2; }
.kiosk-button-launch image { color: #5ecfb8; }
.kiosk-button-label {
    color: #e8eef5;
    font-size: 19px;
    font-weight: 600;
    letter-spacing: 0.3px;
}

/* An unrunnable control still renders, dimmed. Hiding it would leave the
   operator wondering where it went; disabling it would be indistinguishable
   from a broken kiosk. */
.kiosk-button-unconfigured { opacity: 0.4; }

/* ── edge controls ───────────────────────────────────────────────────────── */
.kiosk-corner {
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    padding: 0;
}
.kiosk-corner-disc {
    background-color: #121924;
    border: 1px solid #253141;
    border-radius: 54px;
    transition: background-color 130ms ease, border-color 130ms ease;
}
.kiosk-corner:hover .kiosk-corner-disc {
    background-color: #1b2531;
    border-color: #35485e;
}
.kiosk-corner-icon { color: #9db0c4; }
.kiosk-corner:hover .kiosk-corner-icon { color: #5ecfb8; }
.kiosk-corner-label {
    color: #8194a8;
    font-size: 14px;
    letter-spacing: 0.6px;
}

/* ── banner ──────────────────────────────────────────────────────────────── */
.kiosk-banner { padding: 0; }
.kiosk-banner label { padding: 14px 26px; font-size: 15px; }
.kiosk-banner-info label { background-color: #12303f; color: #d5ecf7; }
.kiosk-banner-error label { background-color: #3d1c22; color: #ffd9dd; }
";
