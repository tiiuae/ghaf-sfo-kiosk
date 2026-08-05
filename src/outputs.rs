// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// One full kiosk per output -- a "primary only" kiosk would leave a second
// screen showing bare wallpaper, the state the kiosk exists to prevent.
//
// Reconciled against gdk's monitor list rather than the compositor's `closed`
// event: gtk4-layer-shell defaults `respect_close` to false, so GTK swallows it.

use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::config::Kiosk;

/// Create surfaces for the current outputs and keep them in step with hotplug.
pub fn manage(app: &gtk::Application, kiosk: Rc<Kiosk>) {
    let Some(display) = gtk::gdk::Display::default() else {
        log::error!("no GDK display; cannot create a surface");
        return;
    };

    // Keyed by the monitor object itself: gdk::Monitor has no id that survives a
    // replug, but object identity is exactly what the ListModel hands us.
    let windows: Rc<RefCell<Vec<(gtk::gdk::Monitor, gtk::ApplicationWindow)>>> =
        Rc::new(RefCell::new(Vec::new()));

    let monitors = display.monitors();
    reconcile(app, &kiosk, &monitors, &windows);

    let app2 = app.clone();
    let windows2 = windows.clone();
    let kiosk2 = kiosk.clone();
    monitors.connect_items_changed(move |model, _pos, _removed, _added| {
        log::info!("output set changed; reconciling kiosk surfaces");
        reconcile(&app2, &kiosk2, model, &windows2);
    });
}

fn reconcile(
    app: &gtk::Application,
    kiosk: &Rc<Kiosk>,
    model: &gtk::gio::ListModel,
    windows: &Rc<RefCell<Vec<(gtk::gdk::Monitor, gtk::ApplicationWindow)>>>,
) {
    let mut live: Vec<gtk::gdk::Monitor> = Vec::new();
    for i in 0..model.n_items() {
        if let Some(m) = model.item(i).and_downcast::<gtk::gdk::Monitor>() {
            live.push(m);
        }
    }

    let mut held = windows.borrow_mut();

    // Drop surfaces whose output is gone.
    held.retain(|(monitor, window)| {
        let still_there = live.iter().any(|m| m == monitor) && monitor.is_valid();
        if !still_there {
            log::info!("output removed; destroying its kiosk surface");
            window.destroy();
        }
        still_there
    });

    // Add surfaces for new outputs.
    for monitor in live {
        if held.iter().any(|(m, _)| *m == monitor) {
            continue;
        }
        log::info!(
            "output added ({}); creating a kiosk surface",
            monitor.connector().unwrap_or_else(|| "unknown".into())
        );
        let content = crate::ui::build(kiosk, app);
        let window = crate::surface::build(app, &monitor, &content);
        window.present();
        held.push((monitor, window));
    }

    if held.is_empty() {
        log::warn!("no outputs; the kiosk has no surface");
    }
}
