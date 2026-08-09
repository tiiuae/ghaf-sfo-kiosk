// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// The one place the kiosk talks back to the operator.
//
// An in-window revealer rather than a dialog, because NEVER call .present() on
// anything but the layer-shell window: any other gtk::Window becomes an
// xdg_toplevel, floating above the kiosk and appearing in alt-tab. That rule is
// also why there is no libadwaita here.
//
// Its own module so that both the button grid and the radial menu can report
// through it without ui.rs and radial.rs having to reach into each other.

use gtk::prelude::*;

use crate::actions::Reporter;

#[derive(Clone)]
pub struct Banner {
    pub revealer: gtk::Revealer,
    label: gtk::Label,
}

impl Banner {
    pub fn new() -> Self {
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
