// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0

//! Ask before acting.
//!
//! An in-window overlay, not a dialog, for the reason ui.rs gives: any other
//! `gtk::Window` becomes an xdg_toplevel floating above the kiosk and listed in
//! alt-tab. So this is a widget in the same `gtk::Overlay` as the grid and the
//! fans, stacked above both.
//!
//! Shaped deliberately like `radial::Fan`: a widget plus one never-drawn
//! `gtk::ToggleButton` that carries open/closed. Routing all state through a
//! single GTK toggle is what already keeps a menu in lockstep across outputs,
//! and reusing that shape means shared.rs needs no new reasoning to keep the
//! same card on both screens.

use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::config;

/// How long the confirming button stays dead after the card opens.
///
/// The point of this feature is that Clear cannot be reached by accident, and
/// position alone does not guarantee that: a double tap is roughly 250-500 ms,
/// and a layout change could one day put a tile where the card is. Being dead
/// for longer than a double tap is a guarantee that survives both.
const ARM_MS: u32 = 600;

#[derive(Clone)]
pub struct Confirm {
    /// Add this to the overlay ABOVE every fan.
    pub widget: gtk::Box,
    /// Open/closed and nothing else. Never in the widget tree, never drawn: it
    /// is a state cell that happens to emit `toggled`, which is what lets a
    /// peer on another output be driven by the same path that drives a fan.
    state: gtk::ToggleButton,
}

impl Confirm {
    /// Ask. A press while the card is already up is not a re-ask: GTK emits
    /// `toggled` only on a real change, so the arming timer is not restarted
    /// and the card does not flicker.
    pub fn open(&self) {
        self.state.set_active(true);
    }

    /// Dismiss without acting. Everything routes through the state toggle, so
    /// there is one path that opens or closes.
    pub fn close(&self) {
        self.state.set_active(false);
    }

    pub fn is_open(&self) -> bool {
        self.state.is_active()
    }

    /// Open or close explicitly. Used to keep the same button's card in step
    /// across outputs -- see shared.rs.
    pub fn set_open(&self, open: bool) {
        self.state.set_active(open);
    }

    /// Notified whenever this card opens or closes, however it was driven.
    pub fn connect_toggled<F: Fn(bool) + 'static>(&self, f: F) {
        self.state.connect_toggled(move |t| f(t.is_active()));
    }

    /// Whether two handles are the same card, so a broadcast can skip its
    /// originator. Compares the underlying GTK object, not the wrapper.
    pub fn same_as(&self, other: &Confirm) -> bool {
        self.state == other.state
    }
}

/// Build one output's confirmation card.
///
/// `on_yes` is the already-bound dispatch for this button. It runs at most once
/// per opening, on the output whose button was actually pressed -- see the
/// guard in the handler below.
pub fn build<F: Fn() + 'static>(spec: &config::Confirm, on_yes: F) -> Confirm {
    // Its own scrim, not the fans'. Two owners of one sheet would need a
    // reference count and a rule about who may undim it, and a fan closing
    // under an open card would undim the wrong thing. A second box with the
    // same class is zero new invariants.
    let sheet = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sheet.add_css_class("kiosk-scrim");
    sheet.add_css_class("kiosk-confirm");
    sheet.set_can_target(false);
    sheet.set_visible(false);

    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("kiosk-confirm-card");
    card.set_halign(gtk::Align::Center);
    // Bottom, not centre: ui.rs centres the grid vertically, so the bottom band
    // is the one region that never holds a tile. A second tap at the button's
    // own coordinates therefore lands on the sheet, which cancels.
    card.set_valign(gtk::Align::End);

    let heading = gtk::Label::new(Some(&spec.message));
    heading.add_css_class("kiosk-confirm-heading");
    heading.set_wrap(true);
    heading.set_justify(gtk::Justification::Center);
    card.append(&heading);

    if let Some(detail) = &spec.detail {
        let body = gtk::Label::new(Some(detail));
        body.add_css_class("kiosk-confirm-body");
        body.set_wrap(true);
        body.set_justify(gtk::Justification::Center);
        card.append(&body);
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    actions.add_css_class("kiosk-confirm-actions");
    actions.set_halign(gtk::Align::Center);
    actions.set_homogeneous(true);

    // Cancel first, so the safe choice is the one under the reading order and
    // the one a left-to-right thumb reaches first.
    let cancel = gtk::Button::with_label(&spec.cancel_label);
    cancel.add_css_class("kiosk-confirm-cancel");
    actions.append(&cancel);

    let yes = gtk::Button::with_label(&spec.confirm_label);
    yes.add_css_class("kiosk-confirm-yes");
    // Armed by the toggle handler below, never at build time: a card that has
    // not been opened must not have a live confirming button.
    yes.set_sensitive(false);
    actions.append(&yes);

    card.append(&actions);
    sheet.append(&card);

    let state = gtk::ToggleButton::new();

    // Bumped per toggle, like radial.rs's: an arming timeout that fires for an
    // opening this card has already left does nothing.
    let generation = Rc::new(Cell::new(0u64));

    state.connect_toggled({
        let sheet = sheet.clone();
        let yes = yes.clone();
        let cancel = cancel.clone();
        let generation = generation.clone();
        move |t| {
            let opening = t.is_active();
            let mine = generation.get().wrapping_add(1);
            generation.set(mine);

            if opening {
                sheet.set_visible(true);
                sheet.add_css_class("kiosk-scrim-open");
                sheet.set_can_target(true);

                yes.set_sensitive(false);
                glib::timeout_add_local_once(
                    std::time::Duration::from_millis(u64::from(ARM_MS)),
                    {
                        let yes = yes.clone();
                        let generation = generation.clone();
                        move || {
                            if generation.get() == mine {
                                yes.set_sensitive(true);
                            }
                        }
                    },
                );

                // On an idle: the card has had no layout pass yet, and
                // grab_focus on a not-yet-mappable widget fails silently.
                let cancel = cancel.clone();
                glib::idle_add_local_once(move || {
                    if cancel.is_visible() {
                        cancel.grab_focus();
                    }
                });
            } else {
                sheet.remove_css_class("kiosk-scrim-open");
                sheet.set_can_target(false);
                // Hidden, not merely transparent: an opacity-0 child is still
                // focusable and still clickable.
                sheet.set_visible(false);
            }
        }
    });

    let confirm = Confirm {
        widget: sheet.clone(),
        state,
    };

    // Tapping the dimmed area cancels, matching the fans -- and dismissal only
    // ever cancels, never confirms, so a stray tap is always the safe outcome.
    {
        let me = confirm.clone();
        let click = gtk::GestureClick::new();
        click.connect_pressed(move |_, _, _, _| me.close());
        sheet.add_controller(click);
    }

    {
        let me = confirm.clone();
        cancel.connect_clicked(move |_| me.close());
    }

    {
        let me = confirm.clone();
        yes.connect_clicked(move |_| {
            // The guard, not decoration. Two outputs mean two Yes buttons; this
            // is what makes the second one a no-op rather than a second
            // irreversible run. `close` broadcasts synchronously, so every peer
            // is shut before `on_yes` is reached, and GTK dispatches input one
            // event at a time.
            if !me.is_open() {
                return;
            }
            me.close();
            on_yes();
        });
    }

    confirm
}
