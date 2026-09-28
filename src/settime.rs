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
//! and, when the machine will restart, says so, takes the destructive styling
//! and goes dead for a moment so a double tap cannot reach it. On confirm the
//! kiosk writes `YYYY-MM-DD HH:MM` to net-vm's socket itself; a correction that
//! restarts goes straight into the restart screen (shutdown.rs).
//!
//! **The restart verdict is the host's.** It reboots on the clock's distance
//! from its boot anchor, not on the size of the gesture, so the card asks
//! `verdict_host:verdict_port` rather than working it out. Falls back to the
//! gesture, marked "unconfirmed", when the host cannot be reached.

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

/// How long the card waits after the last stepper tap before asking the host.
/// Long enough that holding `+` is one query, short enough not to be noticed.
const ASK_DEBOUNCE_MS: u32 = 250;

/// Read cap on the socket reply: the helper answers `ok` or a short
/// `error: ...` line, and the verdict socket two short lines.
const REPLY_CAP: usize = 256;

/// The host's clock: displacement from this boot's anchor, and its threshold.
/// `None` until it answers -- the card then says "unconfirmed" rather than
/// inventing a verdict.
type Standing = Rc<Cell<Option<(i64, i64)>>>;

/// The host's answer for ONE target: (target, restarts, resulting displacement).
/// Keyed by the target: never show an answer to a question since changed.
type Verdict = Rc<RefCell<Option<(String, bool, i64)>>>;

/// Deferred, because the asker calls `recompute` and `recompute` calls it.
type AskVerdict = Rc<RefCell<Box<dyn Fn(&str)>>>;

/// `restart`/`no-restart` plus `displacement_after=` from a targeted reply.
fn parse_verdict(reply: &str) -> Option<(bool, i64)> {
    let restarts = match reply.split_whitespace().next()? {
        "restart" => true,
        "no-restart" => false,
        _ => return None,
    };
    let after = reply
        .split_whitespace()
        .find_map(|t| t.strip_prefix("displacement_after="))?
        .parse()
        .ok()?;
    Some((restarts, after))
}

/// `displacement_now=` and `threshold=` from an untargeted reply. Both or
/// neither: half an answer would be shown in the words used for a real verdict.
/// A non-positive threshold would warn about every correction, so it is refused.
fn parse_standing(reply: &str) -> Option<(i64, i64)> {
    let mut displacement = None;
    let mut threshold = None;
    for token in reply.split_whitespace() {
        if let Some(v) = token.strip_prefix("displacement_now=") {
            displacement = v.parse().ok();
        } else if let Some(v) = token.strip_prefix("threshold=") {
            threshold = v.parse().ok().filter(|t: &i64| *t > 0);
        }
    }
    Some((displacement?, threshold?))
}

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

/// The wire format the host parses: the one string both the verdict query and
/// the confirm send, so they cannot disagree about which instant is meant.
fn wire_target(calendar: &gtk::Calendar, hour: i32, minute: i32) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        calendar.year(),
        calendar.month() + 1,
        calendar.day(),
        hour,
        minute
    )
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
pub fn build(
    host: &str,
    port: u16,
    threshold_sec: u32,
    verdict_host: &str,
    verdict_port: u16,
    shared: &Shared,
) -> SetTime {
    let host = host.to_owned();
    let verdict_host = verdict_host.to_owned();
    let reporter = shared.reporter();
    let standing: Standing = Rc::new(Cell::new(None));
    let verdict: Verdict = Rc::new(RefCell::new(None));
    // Filled once `recompute` exists, like `notify` below: the asker has to
    // call recompute, and recompute has to call the asker.
    // `|_: &str|` annotated: a bare `|_|` will not infer into `dyn Fn(&str)`.
    let schedule_ask: AskVerdict = Rc::new(RefCell::new(Box::new(|_: &str| {})));

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
        let standing = standing.clone();
        let verdict = verdict.clone();
        let schedule_ask = schedule_ask.clone();
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

            // Ask the host about THIS target rather than composing an answer:
            // `delta` is gui-vm's clock and zone, `displacement` is the host's.
            let want = wire_target(&calendar, hour.get(), minute.get());
            let known = standing.get();
            let threshold = known.map_or(i64::from(threshold_sec), |(_, t)| t);

            let answered = verdict
                .borrow()
                .as_ref()
                .filter(|(t, _, _)| *t == want)
                .map(|(_, r, a)| (*r, *a));

            // Until it answers, an estimate -- flagged as such below.
            let after = answered
                .map(|(_, a)| a)
                .or_else(|| known.map(|(displacement, _)| displacement + delta));
            let restarts = answered
                .map(|(r, _)| r)
                .unwrap_or_else(|| after.map_or(mag >= threshold, |a| a.abs() >= threshold));
            let confirmed = answered.is_some();

            if answered.is_none() && mag >= 60 {
                (schedule_ask.borrow())(&want);
            }

            if mag < 60 {
                summary.set_text("No change.");
            } else {
                let tail = if !confirmed {
                    // Not the host's answer yet. Say so rather than state a
                    // verdict we cannot stand behind.
                    if restarts {
                        "the system will probably restart (unconfirmed).".to_owned()
                    } else {
                        "probably no restart (unconfirmed).".to_owned()
                    }
                } else if !restarts {
                    "no restart.".to_owned()
                } else if mag >= threshold {
                    "the system will restart to apply it.".to_owned()
                } else {
                    // Sub-threshold gesture that still restarts. Unexplained
                    // that reads as a bug, so name the accumulation.
                    format!(
                        "that leaves the clock {} from where this boot started, \
                         so the system will restart.",
                        human(after.unwrap_or(0).abs())
                    )
                };
                summary.set_text(&format!("{} {dir} \u{2014} {tail}", human(mag)));
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

    // Debounced: one query per pause, not per stepper tap. Each tap supersedes
    // the pending one, so holding `+` costs a single query when it stops.
    *schedule_ask.borrow_mut() = Box::new({
        let verdict = verdict.clone();
        let recompute = recompute.clone();
        let verdict_host = verdict_host.clone();
        let pending = Rc::new(Cell::new(0u64));
        move |want: &str| {
            let mine = pending.get().wrapping_add(1);
            pending.set(mine);
            let (want, verdict, recompute, host, pending) = (
                want.to_owned(),
                verdict.clone(),
                recompute.clone(),
                verdict_host.clone(),
                pending.clone(),
            );
            glib::timeout_add_local_once(
                std::time::Duration::from_millis(u64::from(ASK_DEBOUNCE_MS)),
                move || {
                    if pending.get() == mine {
                        ask_verdict(&host, verdict_port, &want, verdict, recompute);
                    }
                },
            );
        }
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
        let standing = standing.clone();
        let verdict = verdict.clone();
        let verdict_host = verdict_host.clone();
        move |t| {
            if t.is_active() {
                // Every open starts from the current clock, so the card shows
                // "No change" until the operator moves something.
                if let Ok(now) = glib::DateTime::now_local() {
                    calendar.select_day(&now);
                    hour.set(now.hour());
                    minute.set(now.minute());
                }
                // Asked once per opening, not per tap: the answer does not go
                // stale while the card is up, and a unit per keystroke on the
                // host would be an absurd price for wording a label.
                standing.set(None);
                verdict.replace(None);
                recompute();
                ask_standing(
                    &verdict_host,
                    verdict_port,
                    standing.clone(),
                    recompute.clone(),
                );
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

/// Ask the host what setting the clock to `want` would do.
///
/// The host parses `want` in its own zone against its own clock, so neither
/// gui-vm's zone nor its clock enters the answer. Composing the two was the bug.
fn ask_verdict(host: &str, port: u16, want: &str, verdict: Verdict, recompute: Rc<dyn Fn()>) {
    let client = gio::SocketClient::new();
    let line = format!("{want}\n");
    let want = want.to_owned();
    client.connect_to_host_async(host, port, gio::Cancellable::NONE, move |res| {
        let Ok(conn) = res else {
            log::warn!("set-time: no clock verdict from the host");
            return;
        };
        conn.output_stream().write_all_async(
            line.into_bytes(),
            glib::Priority::DEFAULT,
            gio::Cancellable::NONE,
            move |res| {
                if res.is_err() {
                    return;
                }
                let istream = conn.input_stream();
                istream.read_all_async(
                    vec![0u8; REPLY_CAP],
                    glib::Priority::DEFAULT,
                    gio::Cancellable::NONE,
                    move |res| {
                        let _conn = conn;
                        let Ok((buf, n, _)) = res else { return };
                        let reply = String::from_utf8_lossy(&buf[..n]).into_owned();
                        match parse_verdict(&reply) {
                            Some((restarts, after)) => {
                                log::info!("set-time: host says {want:?} -> restart={restarts} after={after}s");
                                *verdict.borrow_mut() = Some((want, restarts, after));
                                recompute();
                            }
                            None => log::warn!(
                                "set-time: unusable verdict {:?}",
                                reply.trim()
                            ),
                        }
                    },
                );
            },
        );
    });
}

/// Ask the host where its clock stands -- an empty request, for the threshold
/// and the opening estimate. Failure is log-only: the card reads "unconfirmed",
/// and a banner for a label's wording would be noise.
fn ask_standing(host: &str, port: u16, standing: Standing, recompute: Rc<dyn Fn()>) {
    let client = gio::SocketClient::new();
    client.connect_to_host_async(host, port, gio::Cancellable::NONE, move |res| {
        let conn = match res {
            Ok(c) => c,
            Err(e) => {
                log::warn!("set-time: no clock verdict from the host: {e}");
                return;
            }
        };
        conn.output_stream().write_all_async(
            b"\n".to_vec(),
            glib::Priority::DEFAULT,
            gio::Cancellable::NONE,
            move |res| {
                if let Err((_, e)) = res {
                    log::warn!("set-time: could not ask for the clock verdict: {e}");
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
                            Ok((buf, n, _)) => String::from_utf8_lossy(&buf[..n]).into_owned(),
                            Err((_, e)) => {
                                log::warn!("set-time: no reply to the clock verdict: {e}");
                                return;
                            }
                        };
                        match parse_standing(&reply) {
                            Some(v) => {
                                log::info!(
                                    "set-time: host clock displacement {}s, threshold {}s",
                                    v.0,
                                    v.1
                                );
                                standing.set(Some(v));
                                recompute();
                            }
                            None => {
                                log::warn!("set-time: unparseable clock verdict {:?}", reply.trim())
                            }
                        }
                    },
                );
            },
        );
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Exactly what `sfo-clock-verdict` writes, both lines.
    const REPLY: &str = "no-restart\n\
        displacement_now=1200 step=-2700 displacement_after=-1500 threshold=1800\n";

    #[test]
    fn parses_the_hosts_own_reply() {
        assert_eq!(parse_standing(REPLY), Some((1200, 1800)));
    }

    #[test]
    fn keeps_the_sign_of_a_backward_displacement() {
        assert_eq!(
            parse_standing("restart\ndisplacement_now=-4000 threshold=1800\n"),
            Some((-4000, 1800))
        );
    }

    /// Half an answer must not become a confident verdict: the card words a
    /// host-confirmed restart differently from a guessed one, so a missing
    /// field has to fall back rather than default.
    #[test]
    fn refuses_a_partial_answer() {
        assert_eq!(parse_standing("restart\ndisplacement_now=1200\n"), None);
        assert_eq!(parse_standing("restart\nthreshold=1800\n"), None);
        assert_eq!(parse_standing("displacement_now=x threshold=1800"), None);
        assert_eq!(parse_standing(""), None);
        assert_eq!(parse_standing("error: no boot anchor\n"), None);
    }

    /// A zero or negative threshold would make `|after| >= threshold` true for
    /// every correction, so the card would warn of a restart on a one-minute
    /// nudge. Fall back to the configured value instead.
    #[test]
    fn refuses_a_threshold_that_would_warn_about_everything() {
        assert_eq!(
            parse_standing("no-restart\ndisplacement_now=0 threshold=0"),
            None
        );
        assert_eq!(
            parse_standing("no-restart\ndisplacement_now=0 threshold=-1800"),
            None
        );
        assert_eq!(
            parse_standing("no-restart\ndisplacement_now=0 threshold=1"),
            Some((0, 1))
        );
    }

    /// `displacement_after` also ends in `=<n>`; picking it up instead of
    /// `displacement_now` would silently double-count the operator's step.
    #[test]
    fn does_not_confuse_displacement_after_for_now() {
        let reply = "restart\ndisplacement_now=100 displacement_after=9999 threshold=1800";
        assert_eq!(parse_standing(reply), Some((100, 1800)));
    }
}
