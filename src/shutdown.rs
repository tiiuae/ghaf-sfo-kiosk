// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0

//! The screen the kiosk shows while the machine is on its way down.
//!
//! The trigger is a marker file, not a signal. A host reboot tearing down this
//! microVM does not go through the guest's logind, so `PrepareForShutdown`
//! never fires here -- and even where it does, it fires seconds before the
//! screen blanks, too late to cover the wait before the reboot. So the thing
//! that starts the reboot writes the marker instead: `sfo-set-time` in
//! tiiuae/ghaf-sfo-laptop touches `$XDG_RUNTIME_DIR/sfo-kiosk-restarting` the
//! moment a clock correction that will restart is confirmed, well before the
//! host's own delay. Anything else that wants this screen can touch the same
//! file.
//!
//! The overlay is an in-window widget stacked above everything else in the
//! same `gtk::Overlay` -- never a `gtk::Window`, for the reason ui.rs gives.
//! Once shown it stays: the system is going, there is nothing to go back to.
//! `hide` is only for the marker being removed again.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gio;
use gtk::gio::prelude::{FileExt, FileMonitorExt};
use gtk::prelude::*;

use crate::shared::Shared;

/// Basename under `$XDG_RUNTIME_DIR`. Kept in step with the `sfo-set-time`
/// dialog in tiiuae/ghaf-sfo-laptop by hand -- a mismatch just means the screen
/// does not show, so it is commented on both sides rather than machine-checked.
const MARKER: &str = "sfo-kiosk-restarting";

#[derive(Clone)]
pub struct Restarting {
    /// Add to the overlay ABOVE every card.
    pub widget: gtk::Box,
    spinner: gtk::Spinner,
}

impl Restarting {
    pub fn show(&self) {
        self.spinner.start();
        self.widget.set_visible(true);
    }

    /// Only for the marker being deleted -- a restart that was announced and
    /// then called off. Rare, but a "Restarting" screen over a machine that is
    /// not restarting would be its own kind of wrong.
    pub fn hide(&self) {
        self.widget.set_visible(false);
        self.spinner.stop();
    }
}

/// Build one output's restart screen. Hidden until `show`. `logo` is the same
/// path the status bar uses (the Ghaf mark on SFO), shown above the text.
pub fn build(logo: Option<&str>) -> Restarting {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
    content.set_halign(gtk::Align::Center);
    content.set_valign(gtk::Align::Center);
    content.set_hexpand(true);
    content.set_vexpand(true);

    if let Some(path) = logo {
        let image = crate::ui::icon_image(path);
        image.set_pixel_size(120);
        image.add_css_class("kiosk-restarting-logo");
        content.append(&image);
    }

    let spinner = gtk::Spinner::new();
    spinner.add_css_class("kiosk-restarting-spinner");
    content.append(&spinner);

    let heading = gtk::Label::new(Some("Restarting"));
    heading.add_css_class("kiosk-restarting-heading");
    content.append(&heading);

    let body = gtk::Label::new(Some("Applying the change. This takes about a minute."));
    body.add_css_class("kiosk-restarting-body");
    body.set_justify(gtk::Justification::Center);
    body.set_wrap(true);
    content.append(&body);

    let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
    widget.add_css_class("kiosk-restarting");
    widget.set_visible(false);
    // Eat every press: no reaching a button while the machine is going down.
    widget.set_can_target(true);
    widget.append(&content);

    Restarting { widget, spinner }
}

/// Ask for the restart screen: raise it now and drop the marker so a kiosk
/// crash-restart in the window that follows comes back showing it. Called by
/// settime.rs after a restart-bound correction; the marker also lets anything
/// else (a GIVC poweroff hook, a technician script) trigger the same screen.
pub fn request_restart(shared: &Shared) {
    shared.show_restarting();
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let path = std::path::Path::new(&dir).join(MARKER);
        if let Err(e) = std::fs::File::create(&path) {
            log::warn!("could not write the restart marker {}: {e}", path.display());
        }
    }
}

/// Watch the marker file and drive every output's restart screen from it.
///
/// Call once for the whole process. The `FileMonitor` is parked in `Shared` so
/// it lives as long as the surfaces it drives.
pub fn watch(shared: &Shared) {
    let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
        log::warn!("no XDG_RUNTIME_DIR; the restart screen will not be shown");
        return;
    };
    let path = std::path::Path::new(&runtime_dir).join(MARKER);

    // A marker already there means a restart was requested before this process
    // was up (a kiosk crash-restart inside the window). Honour it now.
    if path.exists() {
        log::info!("restart marker already present at startup; showing the restart screen");
        shared.show_restarting();
    }

    let file = gio::File::for_path(&path);
    let monitor = match file.monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
        Ok(m) => m,
        Err(e) => {
            log::warn!(
                "cannot watch {}: {e}; the restart screen will not be shown",
                path.display()
            );
            return;
        }
    };

    let on_change = shared.clone();
    monitor.connect_changed(move |_monitor, _file, _other, event| match event {
        // Created only, not ChangesDoneHint: one `create` write emits both, and
        // show_restarting is idempotent but the double log line is noise.
        gio::FileMonitorEvent::Created => {
            log::info!("restart marker appeared; showing the restart screen");
            on_change.show_restarting();
        }
        gio::FileMonitorEvent::Deleted => {
            log::info!("restart marker removed; hiding the restart screen");
            on_change.hide_restarting();
        }
        _ => {}
    });

    shared.hold_restart_monitor(monitor);
}

/// The parked `FileMonitor`. Held, never read: `watch` needs it to outlive its
/// own scope, and there is exactly one for the whole run.
#[derive(Clone, Default)]
pub struct MonitorHold(Rc<RefCell<Option<gio::FileMonitor>>>);

impl MonitorHold {
    pub fn set(&self, monitor: gio::FileMonitor) {
        *self.0.borrow_mut() = Some(monitor);
    }
}
