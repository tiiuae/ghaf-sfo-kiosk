// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0

//! The operator's clock-set card.
//!
//! An in-window overlay, not a dialog, for the reason ui.rs gives: any other
//! `gtk::Window` becomes an xdg_toplevel floating above the kiosk and listed in
//! alt-tab. This replaces the old yad script (`sfo-set-time` in
//! tiiuae/ghaf-sfo-laptop): the kiosk now owns the whole flow.
//!
//! Shaped like `confirm::Confirm`: a widget plus one never-drawn
//! `gtk::ToggleButton` carrying open/closed, so shared.rs keeps the card in
//! lockstep across outputs with the same path it uses for a fan or a card.
//!
//! A `gtk::Calendar` for the date, then two steppers -- hour / minute -- for the
//! time: the calendar makes a month/year jump one gesture, the steppers keep big
//! touch targets for the common case of nudging the clock on a device with no
//! other reference. One primary button that always shows the size of the change
//! and, at or above `reboot_threshold_sec`, says the machine will restart, takes
//! the destructive styling and goes dead for a moment so a double tap cannot
//! reach it. On confirm the kiosk writes `YYYY-MM-DD HH:MM` to net-vm's socket
//! itself; a large correction goes straight into the restart screen
//! (shutdown.rs).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gio;
use gtk::gio::prelude::{
    IOStreamExt, InputStreamExtManual, OutputStreamExtManual, SocketClientExt,
};
use gtk::glib;
use gtk::prelude::*;

use crate::actions::Reporter;
use crate::shared::{Broadcast, Shared};

/// The primary button stays dead this long after it flips into "& restart", so
/// a correction that will reboot cannot be reached by a double tap. Matches
/// `confirm`'s arming window.
const ARM_MS: u32 = 600;

/// Read cap on the socket reply: the helper answers `ok` or a short
/// `error: ...` line.
const REPLY_CAP: usize = 256;

#[derive(Clone)]
pub struct SetTime {
    /// Add this to the overlay ABOVE every confirm card.
    pub widget: gtk::Overlay,
    /// Open/closed, never drawn -- the state cell shared.rs drives.
    state: gtk::ToggleButton,
    /// The primary button, so a failed send can re-enable it.
    set_btn: gtk::Button,
}

impl SetTime {
    pub fn open(&self) {
        self.state.set_active(true);
    }
    pub fn close(&self) {
        self.state.set_active(false);
    }
    pub fn is_open(&self) -> bool {
        self.state.is_active()
    }
    pub fn set_open(&self, open: bool) {
        self.state.set_active(open);
    }
    pub fn connect_toggled<F: Fn(bool) + 'static>(&self, f: F) {
        self.state.connect_toggled(move |t| f(t.is_active()));
    }
    pub fn same_as(&self, other: &SetTime) -> bool {
        self.state == other.state
    }
}

/// `d h min` from a second count, omitting zero units, always at least minutes.
fn human(delta: i64) -> String {
    if delta < 60 {
        return "under a minute".to_owned();
    }
    let d = delta / 86400;
    let h = (delta % 86400) / 3600;
    let m = (delta % 3600) / 60;
    let mut out = String::new();
    if d > 0 {
        out.push_str(&format!("{d} d "));
    }
    if h > 0 {
        out.push_str(&format!("{h} h "));
    }
    out.push_str(&format!("{m} min"));
    out
}

/// One `−  [ value ]  +  / caption` control.
struct Stepper {
    root: gtk::Box,
    value: Rc<Cell<i32>>,
    readout: gtk::Label,
    fmt: Rc<dyn Fn(i32) -> String>,
}

impl Stepper {
    fn set(&self, v: i32) {
        self.value.set(v);
        self.readout.set_text(&(self.fmt)(v));
    }
    fn get(&self) -> i32 {
        self.value.get()
    }
}

/// `notify` is called after any `−`/`+`; `wrap` rolls over the ends, otherwise
/// they clamp.
fn stepper(
    caption: &str,
    lo: i32,
    hi: i32,
    wrap: bool,
    fmt: Rc<dyn Fn(i32) -> String>,
    notify: Rc<RefCell<Box<dyn Fn()>>>,
) -> Stepper {
    let value = Rc::new(Cell::new(lo));
    let readout = gtk::Label::new(Some(&fmt(lo)));
    readout.add_css_class("kiosk-settime-value");

    let bump = {
        let value = value.clone();
        let readout = readout.clone();
        let fmt = fmt.clone();
        move |by: i32| {
            let next = if wrap {
                let span = hi - lo + 1;
                (value.get() - lo + by).rem_euclid(span) + lo
            } else {
                (value.get() + by).clamp(lo, hi)
            };
            value.set(next);
            readout.set_text(&fmt(next));
            (notify.borrow())();
        }
    };

    let down = gtk::Button::with_label("\u{2212}"); // real minus sign
    down.add_css_class("kiosk-settime-step");
    down.connect_clicked({
        let bump = bump.clone();
        move |_| bump(-1)
    });
    let up = gtk::Button::with_label("+");
    up.add_css_class("kiosk-settime-step");
    up.connect_clicked({
        let bump = bump.clone();
        move |_| bump(1)
    });

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_halign(gtk::Align::Center);
    row.append(&down);
    row.append(&readout);
    row.append(&up);

    let cap = gtk::Label::new(Some(caption));
    cap.add_css_class("kiosk-settime-caption");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_halign(gtk::Align::Center);
    root.append(&row);
    root.append(&cap);

    Stepper {
        root,
        value,
        readout,
        fmt,
    }
}

fn two_digit() -> Rc<dyn Fn(i32) -> String> {
    Rc::new(|v| format!("{v:02}"))
}

/// Build one output's clock-set card. Hidden until `open`.
pub fn build(host: &str, port: u16, threshold_sec: u32, shared: &Shared) -> SetTime {
    let host = host.to_owned();
    let reporter = shared.reporter();

    let sheet = gtk::Overlay::new();
    sheet.set_visible(false);

    let dim = gtk::Box::new(gtk::Orientation::Vertical, 0);
    dim.add_css_class("kiosk-scrim");
    dim.add_css_class("kiosk-settime");
    dim.set_can_target(false);
    sheet.set_child(Some(&dim));

    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("kiosk-settime-card");
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Center);

    let heading = gtk::Label::new(Some("Set date and time"));
    heading.add_css_class("kiosk-settime-heading");
    card.append(&heading);

    // Shared so the calendar and every stepper's `−`/`+` trigger the same
    // recompute. Starts a no-op because the controls exist before `recompute`.
    let notify: Rc<RefCell<Box<dyn Fn()>>> = Rc::new(RefCell::new(Box::new(|| {})));

    // Date: a real month grid. gtk::Calendar's `month` is 0-based; every read
    // below adds one to match the wire format and glib::DateTime.
    let calendar = gtk::Calendar::new();
    calendar.add_css_class("kiosk-settime-calendar");
    // `day-selected` covers a day tap and a programmatic `select_day`;
    // `notify::month` / `notify::year` cover the header arrows, which page the
    // grid without touching the day.
    calendar.connect_day_selected({
        let notify = notify.clone();
        move |_| (notify.borrow())()
    });
    for prop in ["month", "year"] {
        calendar.connect_notify_local(Some(prop), {
            let notify = notify.clone();
            move |_, _| (notify.borrow())()
        });
    }

    let hour = Rc::new(stepper("hour", 0, 23, true, two_digit(), notify.clone()));
    let minute = Rc::new(stepper("minute", 0, 59, true, two_digit(), notify.clone()));

    let time_col = gtk::Box::new(gtk::Orientation::Vertical, 18);
    time_col.set_halign(gtk::Align::Center);
    for s in [&hour, &minute] {
        time_col.append(&s.root);
    }

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 26);
    controls.add_css_class("kiosk-settime-controls");
    controls.set_halign(gtk::Align::Center);
    controls.append(&calendar);
    controls.append(&time_col);
    card.append(&controls);

    let summary = gtk::Label::new(None);
    summary.add_css_class("kiosk-settime-summary");
    summary.set_justify(gtk::Justification::Center);
    summary.set_wrap(true);
    card.append(&summary);

    let set_btn = gtk::Button::with_label("Set");
    set_btn.add_css_class("kiosk-settime-set");

    // Bumped every time the button enters "& restart", so an arm timer for a
    // state the card has left does nothing -- same shape as confirm's counter.
    let generation = Rc::new(Cell::new(0u64));

    let recompute: Rc<dyn Fn()> = {
        let calendar = calendar.clone();
        let (hour, minute) = (hour.clone(), minute.clone());
        let summary = summary.clone();
        let set_btn = set_btn.clone();
        let generation = generation.clone();
        Rc::new(move || {
            let Ok(now) = glib::DateTime::now_local() else {
                return;
            };
            let Ok(target) = glib::DateTime::from_local(
                calendar.year(),
                calendar.month() + 1,
                calendar.day(),
                hour.get(),
                minute.get(),
                0.0,
            ) else {
                return;
            };
            let delta = target.to_unix() - now.to_unix();
            let dir = if delta >= 0 { "forward" } else { "backward" };
            let mag = delta.abs();
            let restarts = mag >= i64::from(threshold_sec);

            if mag < 60 {
                summary.set_text("No change.");
            } else if restarts {
                summary.set_text(&format!(
                    "{} {dir} \u{2014} the system will restart to apply it.",
                    human(mag)
                ));
            } else {
                summary.set_text(&format!("{} {dir} \u{2014} no restart.", human(mag)));
            }

            if restarts {
                set_btn.set_label("Set & restart");
                set_btn.add_css_class("kiosk-settime-set-restart");
                let mine = generation.get().wrapping_add(1);
                generation.set(mine);
                set_btn.set_sensitive(false);
                glib::timeout_add_local_once(
                    std::time::Duration::from_millis(u64::from(ARM_MS)),
                    {
                        let set_btn = set_btn.clone();
                        let generation = generation.clone();
                        move || {
                            if generation.get() == mine {
                                set_btn.set_sensitive(true);
                            }
                        }
                    },
                );
            } else {
                set_btn.set_label("Set");
                set_btn.remove_css_class("kiosk-settime-set-restart");
                set_btn.set_sensitive(true);
            }
        })
    };
    *notify.borrow_mut() = Box::new({
        let recompute = recompute.clone();
        move || recompute()
    });

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    actions.add_css_class("kiosk-settime-actions");
    actions.set_halign(gtk::Align::Center);
    actions.set_homogeneous(true);

    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("kiosk-settime-cancel");
    actions.append(&cancel);
    actions.append(&set_btn);
    card.append(&actions);

    sheet.add_overlay(&card);

    let settime = SetTime {
        widget: sheet.clone(),
        state: gtk::ToggleButton::new(),
        set_btn: set_btn.clone(),
    };

    settime.state.connect_toggled({
        let sheet = sheet.clone();
        let dim = dim.clone();
        let recompute = recompute.clone();
        let calendar = calendar.clone();
        let (hour, minute) = (hour.clone(), minute.clone());
        move |t| {
            if t.is_active() {
                // Every open starts from the current clock, so the card shows
                // "No change" until the operator moves something.
                if let Ok(now) = glib::DateTime::now_local() {
                    calendar.select_day(&now);
                    hour.set(now.hour());
                    minute.set(now.minute());
                }
                recompute();
                sheet.set_visible(true);
                dim.add_css_class("kiosk-scrim-open");
                dim.set_can_target(true);
            } else {
                dim.remove_css_class("kiosk-scrim-open");
                dim.set_can_target(false);
                sheet.set_visible(false);
            }
        }
    });

    {
        let me = settime.clone();
        cancel.connect_clicked(move |_| me.close());
    }
    {
        let me = settime.clone();
        let click = gtk::GestureClick::new();
        click.connect_pressed(move |_, _, _, _| me.close());
        dim.add_controller(click);
    }
    {
        let me = settime.clone();
        let shared = shared.clone();
        let reporter = reporter.clone();
        let host = host.clone();
        let calendar = calendar.clone();
        let (hour, minute) = (hour, minute);
        set_btn.connect_clicked(move |btn| {
            if !me.is_open() {
                return;
            }
            let want = format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                calendar.year(),
                calendar.month() + 1,
                calendar.day(),
                hour.get(),
                minute.get()
            );
            let restarts = btn.has_css_class("kiosk-settime-set-restart");
            log::info!("set-time: sending {want:?} to {host}:{port} (restarts={restarts})");
            btn.set_sensitive(false);
            send(
                &host,
                port,
                &want,
                shared.clone(),
                reporter.clone(),
                me.clone(),
                restarts,
            );
        });
    }

    settime
}

/// Send `want` to the clock socket and act on the one-line reply. All async on
/// the GTK loop; `conn` is carried through every stage so the stream stays open.
fn send(
    host: &str,
    port: u16,
    want: &str,
    shared: Shared,
    reporter: Broadcast,
    card: SetTime,
    restarts: bool,
) {
    let client = gio::SocketClient::new();
    let line = format!("{want}\n");
    client.connect_to_host_async(host, port, gio::Cancellable::NONE, move |res| {
        let conn = match res {
            Ok(c) => c,
            Err(e) => {
                reporter.error(&format!("Could not reach the clock service: {e}"));
                card.set_btn.set_sensitive(true);
                return;
            }
        };
        conn.output_stream().write_all_async(
            line.into_bytes(),
            glib::Priority::DEFAULT,
            gio::Cancellable::NONE,
            move |res| {
                if let Err((_, e)) = res {
                    reporter.error(&format!("Could not send the time: {e}"));
                    card.set_btn.set_sensitive(true);
                    return;
                }
                let istream = conn.input_stream();
                istream.read_all_async(
                    vec![0u8; REPLY_CAP],
                    glib::Priority::DEFAULT,
                    gio::Cancellable::NONE,
                    move |res| {
                        // Moved in so the connection outlives the read.
                        let _conn = conn;
                        let reply = match res {
                            Ok((buf, n, _)) => String::from_utf8_lossy(&buf[..n]).trim().to_owned(),
                            Err((_, e)) => {
                                reporter.error(&format!("No reply from the clock service: {e}"));
                                card.set_btn.set_sensitive(true);
                                return;
                            }
                        };
                        match reply.as_str() {
                            "ok" => {
                                card.close();
                                if restarts {
                                    crate::shutdown::request_restart(&shared);
                                } else {
                                    reporter.info("Clock set.");
                                }
                            }
                            other => {
                                reporter.error(&format!("Could not set the clock: {other}"));
                                card.set_btn.set_sensitive(true);
                            }
                        }
                    },
                );
            },
        );
    });
}
