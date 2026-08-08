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
    grid.set_row_spacing(24);
    grid.set_column_spacing(24);
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
            image.set_pixel_size(64);
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
window.kiosk-root {
    background-color: #10141a;
    color: #e8ecf1;
}
.kiosk-statusbar {
    padding: 10px 20px;
    background-color: #161b23;
    border-bottom: 1px solid #232a35;
    font-size: 15px;
}
.kiosk-title { font-weight: bold; letter-spacing: 2px; }
.kiosk-clock { font-size: 17px; font-weight: bold; }
.kiosk-grid { padding: 40px; }
.kiosk-button {
    min-width: 200px;
    min-height: 160px;
    padding: 20px;
    border-radius: 14px;
    background-color: #1d2530;
    border: 1px solid #2b3542;
}
.kiosk-button:hover { background-color: #26313f; }
.kiosk-button:active { background-color: #303d4d; }
.kiosk-button-label { font-size: 19px; font-weight: bold; }
.kiosk-button-unconfigured { opacity: 0.45; }

/* Edge controls: quieter than an application, and no fill -- the dashed ring is
   drawn, so the button itself must not paint a box behind it. */
.kiosk-corner { background: transparent; padding: 0; }
.kiosk-corner-label { color: #8b979e; font-size: 15px; }
.kiosk-corner-icon { color: #b7c1c6; }
.kiosk-corner:hover .kiosk-corner-icon { color: #e9edef; }
.kiosk-exit {
    margin: 14px;
    min-width: 34px;
    min-height: 34px;
    padding: 4px;
    border-radius: 17px;
    opacity: 0.35;
    background-color: transparent;
}
.kiosk-exit:hover { opacity: 1.0; background-color: #3a2530; }
.kiosk-banner { padding: 0; }
.kiosk-banner label { padding: 12px 20px; font-size: 15px; }
.kiosk-banner-info label { background-color: #1b3048; }
.kiosk-banner-error label { background-color: #4a1f26; color: #ffd9dd; }
";
