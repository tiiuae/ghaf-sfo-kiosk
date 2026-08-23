// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// State that every output's kiosk surface shares.
//
// `ui::build` runs once per output, so two screens get two independent widget
// trees -- and, before this, two independent menu states and two banners: the
// fan opened on one screen only, and "Starting..." showed only where it was
// pressed.
//
// Mirroring the outputs in the compositor instead was tried on hardware and
// rejected; it kills the laptop touchscreen. docs/display.md in
// tiiuae/ghaf-sfo-laptop has the measurements. So surfaces stay independent and
// only STATE is shared -- geometry stays per-output, as it should.

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

    /// What every button and menu item must report through, so a message lands
    /// on every screen and not just the one that was touched.
    pub fn reporter(&self) -> Broadcast {
        self.reporter.clone()
    }

    pub fn register_banner(&self, banner: Banner) {
        self.reporter.push(banner);
    }

    /// Drop registrations whose surface has been destroyed.
    ///
    /// Unplugging a screen destroys its window (outputs.rs); without this, every
    /// replug grows both lists and banners are "shown" on dead surfaces. No root
    /// is GTK's cheapest liveness test.
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
            // Cloned out of the RefCell first: setting a peer re-enters this
            // callback for that fan, which borrows the same list.
            let peers: Vec<(String, Fan)> = fans.borrow().clone();
            for (peer_id, peer) in &peers {
                if peer_id != &id || peer.same_as(&me) {
                    continue;
                }
                // Terminates: GTK emits `toggled` only on a real change.
                if peer.is_open() != open {
                    peer.set_open(open);
                }
            }
        });
    }
}
