// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// State that every output's kiosk surface shares.
//
// WHY THIS EXISTS. `ui::build` runs once per output (outputs.rs), so a laptop
// with an external screen attached gets two complete, INDEPENDENT widget trees.
// Each was laid out correctly for its own geometry -- and each had its own menu
// state and its own banner. Opening the settings fan on the laptop did nothing
// on the room's screen, and "Starting..." / "Clearing..." appeared only on the
// surface whose button was pressed. For an operator driving the laptop while an
// audience watches the big screen, that is the whole point missed.
//
// The obvious fix is to mirror the outputs in the compositor instead. It was
// tried on hardware and rejected: on cosmic-comp 1.5.0 a mirroring output leaves
// the shell's output set, which KILLS THE LAPTOP TOUCHSCREEN, and unplugging the
// source leaves a stale mirror no command can clear. See docs/display.md in
// tiiuae/ghaf-sfo-laptop for the measurements.
//
// So the surfaces stay independent -- each native, each touchable, unplug
// harmless -- and only the STATE is shared. Geometry stays per-output (the fan
// radius differs between a 2560x1440 and a 1920x1080 screen, and should).

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::actions::Reporter;
use crate::banner::Banner;
use crate::radial::Fan;

/// A `Reporter` that says the same thing on every output.
#[derive(Clone, Default)]
pub struct Broadcast(Rc<RefCell<Vec<Banner>>>);

impl Broadcast {
    fn push(&self, banner: Banner) {
        self.0.borrow_mut().push(banner);
    }
}

impl Reporter for Broadcast {
    fn info(&self, message: &str) {
        // Collected first: a banner's timer callback can re-enter, and holding
        // the borrow across `info` would panic.
        let banners: Vec<Banner> = self.0.borrow().clone();
        for b in &banners {
            b.info(message);
        }
    }
    fn error(&self, message: &str) {
        let banners: Vec<Banner> = self.0.borrow().clone();
        for b in &banners {
            b.error(message);
        }
    }
}

/// Everything the per-output surfaces hold in common.
#[derive(Clone, Default)]
pub struct Shared {
    reporter: Broadcast,
    /// (menu id, fan). Fans with the SAME id live on different outputs and are
    /// kept in lockstep; different ids are different menus and are independent.
    fans: Rc<RefCell<Vec<(String, Fan)>>>,
}

impl Shared {
    pub fn new() -> Self {
        Self::default()
    }

    /// The reporter every button and menu item must use, so a message lands on
    /// every screen rather than only the one that was touched.
    pub fn reporter(&self) -> Broadcast {
        self.reporter.clone()
    }

    pub fn register_banner(&self, banner: Banner) {
        self.reporter.push(banner);
    }

    /// Drop registrations whose surface has been destroyed.
    ///
    /// Unplugging a screen calls `window.destroy()` on its surface (outputs.rs),
    /// which leaves this struct holding widgets belonging to a window that no
    /// longer exists. Without pruning, every replug grows both lists and a
    /// banner is "shown" on a dead surface. A destroyed widget has no root, and
    /// that is the cheapest liveness test GTK offers.
    pub fn prune(&self) {
        self.reporter
            .0
            .borrow_mut()
            .retain(|b| b.revealer.root().is_some());
        self.fans
            .borrow_mut()
            .retain(|(_, f)| f.widget.root().is_some());
    }

    /// Register one output's fan and link it to its peers on other outputs.
    pub fn register_fan(&self, menu_id: &str, fan: &Fan) {
        self.fans
            .borrow_mut()
            .push((menu_id.to_owned(), fan.clone()));

        let fans = self.fans.clone();
        let id = menu_id.to_owned();
        let me = fan.clone();
        fan.connect_toggled(move |open| {
            // Cloned out of the RefCell before touching any peer: setting a
            // peer's state re-enters this callback for THAT fan, which borrows
            // the same list again.
            let peers: Vec<(String, Fan)> = fans.borrow().clone();
            for (peer_id, peer) in &peers {
                if peer_id != &id || peer.same_as(&me) {
                    continue;
                }
                // Terminates: GTK emits `toggled` only on a real change, so once
                // every peer already holds `open` the cascade stops.
                if peer.is_open() != open {
                    peer.set_open(open);
                }
            }
        });
    }
}
