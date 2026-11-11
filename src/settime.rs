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
//! Everything else -- the target, the host's answers, a Set in flight -- lives
//! in one `Model` per button, so every screen shows the same card.
//!
//! A `gtk::Calendar` for the date, then two steppers -- hour / minute -- for the
//! time: the calendar makes a month/year jump one gesture, the steppers keep big
//! touch targets for the common case of nudging the clock on a device with no
//! other reference. One primary button that always shows the size of the change
//! and, when the machine will restart, says so, takes the destructive styling
//! and goes dead for a moment so a double tap cannot reach it. On confirm the
//! kiosk writes to net-vm's socket itself (see `wire_line`); a correction that
//! restarts goes straight into the restart screen (shutdown.rs).
//!
//! **The restart verdict is the host's.** It reboots on the clock's distance
//! from its boot anchor, not on the size of the gesture, so the card asks
//! `verdict_host:verdict_port` rather than working it out, again every
//! `REFRESH_SECS` while open and once more on Set. Set waits for that answer;
//! only when the host cannot be reached does it fall back to the gesture,
//! marked "unconfirmed".

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gio;
use gtk::gio::prelude::{
    CancellableExt, IOStreamExt, InputStreamExtManual, OutputStreamExtManual, SocketClientExt,
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

/// While open, the card refreshes this often: for a fixed target the difference
/// and the host's answer both drift as the clock runs.
const REFRESH_SECS: u32 = 10;

/// Set waits on the verdict, so a stalled host must not leave the button dead.
const ASK_TIMEOUT_SECS: u32 = 3;

/// net-vm restarts chronyd before it answers, so this is looser than an ask.
const SEND_TIMEOUT_SECS: u32 = 15;

/// Read cap on the socket reply: the helper answers `ok` or a short
/// `error: ...` line, and the verdict socket two short lines.
const REPLY_CAP: usize = 256;

/// Prefix of a targeted verdict request. The bare target is what net-vm's
/// socket acts on; this one it refuses, so a misrouted probe cannot set the clock.
const PROBE_VERB: &str = "would-set";

/// `None` on any failure: callers fall back rather than report. The second is
/// the host's `epoch=`, never without the first.
type OnVerdict = Box<dyn FnOnce(Option<(bool, Option<i64>)>, Option<i64>)>;

/// `restart`/`no-restart`, plus `displacement_after=` when present: the first
/// token alone answers the question; the number only words it.
fn parse_verdict(reply: &str) -> Option<(bool, Option<i64>)> {
    let restarts = match reply.split_whitespace().next()? {
        "restart" => true,
        "no-restart" => false,
        _ => return None,
    };
    let after = reply
        .split_whitespace()
        .find_map(|t| t.strip_prefix("displacement_after="))
        .and_then(|v| v.parse().ok());
    Some((restarts, after))
}

/// The host's reading of the target (tiiuae/ghaf-sfo-laptop#65). An older
/// host sends none.
fn parse_epoch(reply: &str) -> Option<i64> {
    reply
        .split_whitespace()
        .find_map(|t| t.strip_prefix("epoch="))
        .and_then(|v| v.parse().ok())
        .filter(|e: &i64| *e > 0)
}

/// What net-vm is sent: `@<epoch>` only when host and card read the target as
/// the same instant, so a zone wrong on either side can never do worse than
/// the string. Otherwise the string, which every net-vm accepts.
fn wire_line(want: &str, host_epoch: Option<i64>, card_epoch: Option<i64>) -> String {
    match host_epoch.filter(|e| Some(*e) == card_epoch) {
        Some(e) => format!("@{e}\n"),
        None => format!("{want}\n"),
    }
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

/// One output's copy of a card, as the model sees it.
#[derive(Clone)]
struct View {
    sheet: gtk::Overlay,
    recompute: Rc<dyn Fn()>,
    show_target: Rc<dyn Fn(&glib::DateTime)>,
    /// Put the controls back to now, without a recompute.
    reset: Rc<dyn Fn()>,
}

/// One Set Time button's state, shared by its card on every output (shared.rs):
/// one question to the host per opening, one cached answer, one Set in flight,
/// one target on every screen.
#[derive(Default)]
pub struct Model {
    open: Cell<bool>,
    /// Bumped on every open, so a reply or tick from an earlier opening is dropped.
    opened: Cell<u64>,
    /// The host's clock: displacement from this boot's anchor, and its threshold.
    standing: Cell<Option<(i64, i64)>>,
    /// The host's answer for ONE target: (target, restarts, resulting displacement).
    /// Keyed by the target: never show an answer to a question since changed.
    verdict: RefCell<Option<(String, bool, Option<i64>)>>,
    /// The last target the host could not answer: Set may go ahead on the estimate.
    unanswered: RefCell<Option<String>>,
    /// Set by the refresh tick: the cached answer is for an earlier clock.
    aged: Cell<bool>,
    /// The click whose confirm is in flight, 0 for none. Tagged, so a confirm
    /// cancelled by a close can only clear its own.
    confirming: Cell<u64>,
    clicks: Cell<u64>,
    /// A send is in flight. Survives close and reopen: it cannot be cancelled.
    sending: Cell<bool>,
    /// The operator moved something this opening; until then the target tracks now.
    touched: Cell<bool>,
    /// Set while the controls are put back to now, so that is not a gesture.
    resetting: Cell<bool>,
    /// The operator's target, for a copy of the card that opens after them.
    target: RefCell<Option<glib::DateTime>>,
    /// Debounce counter: only the last request in a burst is asked.
    pending: Cell<u64>,
    /// Cancelled on close, so no question outlives its opening.
    cancel: RefCell<Option<gio::Cancellable>>,
    /// Set while one output's target is copied to the others, so it is not echoed.
    mirroring: Cell<bool>,
    views: RefCell<Vec<View>>,
}

impl Model {
    /// Redraw every output. Cloned out first: a recompute can re-enter.
    fn redraw(&self) {
        let views: Vec<View> = self.views.borrow().clone();
        for v in &views {
            (v.recompute)();
        }
    }

    fn mirror(&self, from: &gtk::Overlay, target: &glib::DateTime) {
        if self.mirroring.replace(true) {
            return;
        }
        let views: Vec<View> = self.views.borrow().clone();
        for v in views.iter().filter(|v| &v.sheet != from) {
            (v.show_target)(target);
        }
        self.mirroring.set(false);
    }

    /// No output's Set may fire.
    fn busy(&self) -> bool {
        self.confirming.get() != 0 || self.sending.get()
    }

    /// Every output back to now, then redrawn.
    fn reset_all(&self) {
        let views: Vec<View> = self.views.borrow().clone();
        self.resetting.set(true);
        for v in &views {
            (v.reset)();
        }
        self.resetting.set(false);
        self.redraw();
    }

    fn current(&self, opening: u64) -> bool {
        self.open.get() && self.opened.get() == opening
    }

    fn cancellable(&self) -> Option<gio::Cancellable> {
        self.cancel.borrow().clone()
    }

    /// Store the host's answer for `want`, or note that it had none. A cached
    /// answer the host could not renew is no longer the host's.
    fn record(&self, want: String, answer: Option<(bool, Option<i64>)>) {
        self.aged.set(false);
        match answer {
            Some((restarts, after)) => {
                self.unanswered.replace(None);
                self.verdict.replace(Some((want, restarts, after)));
            }
            None => {
                let mut verdict = self.verdict.borrow_mut();
                if verdict.as_ref().is_some_and(|(t, _, _)| *t == want) {
                    *verdict = None;
                }
                drop(verdict);
                self.unanswered.replace(Some(want));
            }
        }
    }

    /// Drop the views of destroyed surfaces -- see `Shared::prune`.
    pub fn prune(&self) {
        self.views.borrow_mut().retain(|v| v.sheet.root().is_some());
    }
}

#[derive(Clone)]
pub struct SetTime {
    /// Add this to the overlay ABOVE every confirm card.
    pub widget: gtk::Overlay,
    /// Open/closed, never drawn -- the state cell shared.rs drives.
    state: gtk::ToggleButton,
    model: Rc<Model>,
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
    fn ready(&self) {
        self.model.sending.set(false);
        self.model.redraw();
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

/// The same target as an instant in this card's zone, to cross-check the host's.
fn card_epoch(calendar: &gtk::Calendar, hour: i32, minute: i32) -> Option<i64> {
    glib::DateTime::from_local(
        calendar.year(),
        calendar.month() + 1,
        calendar.day(),
        hour,
        minute,
        0.0,
    )
    .ok()
    .map(|d| d.to_unix())
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

/// Debounced: one query per pause, not per tap, whichever output it came from.
/// Each request supersedes the pending one, so holding `+` costs a single query.
fn schedule_ask(model: &Rc<Model>, host: &str, port: u16, want: &str) {
    let mine = model.pending.get().wrapping_add(1);
    model.pending.set(mine);
    let opening = model.opened.get();
    let (model, host, want) = (model.clone(), host.to_owned(), want.to_owned());
    glib::timeout_add_local_once(
        std::time::Duration::from_millis(u64::from(ASK_DEBOUNCE_MS)),
        move || {
            if model.pending.get() != mine || !model.current(opening) {
                return;
            }
            let cancel = model.cancellable();
            let asked = want.clone();
            ask_verdict(
                &host,
                port,
                &asked,
                cancel,
                Box::new(move |answer, _epoch| {
                    // Superseded: a newer request is on its way and will redraw.
                    if !model.current(opening) || model.pending.get() != mine {
                        return;
                    }
                    model.record(want, answer);
                    model.redraw();
                }),
            );
        },
    );
}

/// Build one output's clock-set card. Hidden until `open`.
pub fn build(
    id: &str,
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
    let model = shared.settime_model(id);
    // This output's button only: dead while arming, or while there is nothing
    // to set or the host's answer is still on its way.
    let arming = Rc::new(Cell::new(false));
    let blocked = Rc::new(Cell::new(true));
    // Created first: recompute asks the host only for a card that is open.
    let state = gtk::ToggleButton::new();

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
    set_btn.set_sensitive(false);

    let sync_btn: Rc<dyn Fn()> = {
        let (set_btn, model, arming, blocked) = (
            set_btn.clone(),
            model.clone(),
            arming.clone(),
            blocked.clone(),
        );
        Rc::new(move || set_btn.set_sensitive(!model.busy() && !arming.get() && !blocked.get()))
    };

    // Bumped every time the button enters or leaves "& restart", so an arm timer
    // for a state the card has left does nothing -- same shape as confirm's counter.
    let generation = Rc::new(Cell::new(0u64));

    let recompute: Rc<dyn Fn()> = {
        let calendar = calendar.clone();
        let (hour, minute) = (hour.clone(), minute.clone());
        let summary = summary.clone();
        let set_btn = set_btn.clone();
        let generation = generation.clone();
        let model = model.clone();
        let (arming, blocked, sync_btn) = (arming.clone(), blocked.clone(), sync_btn.clone());
        let verdict_host = verdict_host.clone();
        let (state, controls) = (state.clone(), controls.clone());
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
            // Untouched means "now", even between a minute rollover and the tick.
            let no_change = mag < 60 || !model.touched.get();

            // Ask the host about THIS target rather than composing an answer:
            // `delta` is gui-vm's clock and zone, `displacement` is the host's.
            let want = wire_target(&calendar, hour.get(), minute.get());
            let known = model.standing.get();
            let threshold = known.map_or(i64::from(threshold_sec), |(_, t)| t);

            let answered = model
                .verdict
                .borrow()
                .as_ref()
                .filter(|(t, _, _)| *t == want)
                .map(|(_, r, a)| (*r, *a));
            let unanswerable = model.unanswered.borrow().as_deref() == Some(want.as_str());

            // Until it answers, an estimate -- flagged as such below.
            let estimate = known.map(|(displacement, _)| displacement + delta);
            let restarts = !no_change
                && answered.map_or_else(
                    || estimate.map_or(mag >= threshold, |a| a.abs() >= threshold),
                    |(r, _)| r,
                );
            let confirmed = answered.is_some();

            // Aged: re-ask, but keep the old answer up until the new one lands.
            // A target the host could not answer waits for the next tick. A
            // closed copy (a screen plugged in mid-opening) never asks.
            if state.is_active()
                && !no_change
                && (model.aged.get() || (!confirmed && !unanswerable))
            {
                schedule_ask(&model, &verdict_host, verdict_port, &want);
            }

            if no_change {
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
                } else if let Some(after) =
                    answered.and_then(|(_, a)| a).filter(|_| mag < threshold)
                {
                    // Sub-threshold gesture that still restarts. Unexplained
                    // that reads as a bug, so name the accumulation.
                    format!(
                        "that leaves the clock {} from where this boot started, \
                         so the system will restart.",
                        human(after.abs())
                    )
                } else {
                    "the system will restart to apply it.".to_owned()
                };
                summary.set_text(&format!("{} {dir} \u{2014} {tail}", human(mag)));
            }

            // Set only on an answer, or once the host has failed to give one.
            blocked.set(no_change || (!confirmed && !unanswerable));
            // Frozen while Set is in flight, so what is sent is what is shown.
            controls.set_sensitive(!model.busy());

            // Arm only on entry: the refresh recomputes every tick.
            if restarts && !set_btn.has_css_class("kiosk-settime-set-restart") {
                set_btn.set_label("Set & restart");
                set_btn.add_css_class("kiosk-settime-set-restart");
                let mine = generation.get().wrapping_add(1);
                generation.set(mine);
                arming.set(true);
                glib::timeout_add_local_once(
                    std::time::Duration::from_millis(u64::from(ARM_MS)),
                    {
                        let (generation, arming, sync_btn) =
                            (generation.clone(), arming.clone(), sync_btn.clone());
                        move || {
                            if generation.get() == mine {
                                arming.set(false);
                                sync_btn();
                            }
                        }
                    },
                );
            } else if !restarts && set_btn.has_css_class("kiosk-settime-set-restart") {
                set_btn.set_label("Set");
                set_btn.remove_css_class("kiosk-settime-set-restart");
                generation.set(generation.get().wrapping_add(1));
                arming.set(false);
            }
            sync_btn();
        })
    };
    *notify.borrow_mut() = Box::new({
        let (recompute, model, sheet, calendar) = (
            recompute.clone(),
            model.clone(),
            sheet.clone(),
            calendar.clone(),
        );
        let (hour, minute) = (hour.clone(), minute.clone());
        move || {
            // A reset or a copy recomputes once the date AND time are in; here
            // it would see the new date with the old time.
            if model.resetting.get() || model.mirroring.get() {
                return;
            }
            model.touched.set(true);
            recompute();
            if let Ok(target) = glib::DateTime::from_local(
                calendar.year(),
                calendar.month() + 1,
                calendar.day(),
                hour.get(),
                minute.get(),
                0.0,
            ) {
                model.target.replace(Some(target.clone()));
                model.mirror(&sheet, &target);
            }
        }
    });

    let show_target: Rc<dyn Fn(&glib::DateTime)> = {
        let (calendar, recompute, model) = (calendar.clone(), recompute.clone(), model.clone());
        let (hour, minute) = (hour.clone(), minute.clone());
        Rc::new(move |target: &glib::DateTime| {
            // Copying, not choosing: never echoed back or taken for a gesture.
            let was = model.mirroring.replace(true);
            calendar.select_day(target);
            hour.set(target.hour());
            minute.set(target.minute());
            model.mirroring.set(was);
            recompute();
        })
    };
    let reset: Rc<dyn Fn()> = {
        let calendar = calendar.clone();
        let (hour, minute) = (hour.clone(), minute.clone());
        Rc::new(move || {
            if let Ok(now) = glib::DateTime::now_local() {
                calendar.select_day(&now);
                hour.set(now.hour());
                minute.set(now.minute());
            }
        })
    };
    model.views.borrow_mut().push(View {
        sheet: sheet.clone(),
        recompute: recompute.clone(),
        show_target: show_target.clone(),
        reset,
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
        state,
        model: model.clone(),
    };

    settime.state.connect_toggled({
        let sheet = sheet.clone();
        let dim = dim.clone();
        let recompute = recompute.clone();
        let model = model.clone();
        let verdict_host = verdict_host.clone();
        move |t| {
            if t.is_active() {
                // The first output to open does the per-opening work; its
                // peers open from inside this same toggle (shared.rs). A send
                // still in flight from an earlier opening is left alone.
                if !model.open.replace(true) {
                    let mine = model.opened.get().wrapping_add(1);
                    model.opened.set(mine);
                    model.standing.set(None);
                    model.verdict.replace(None);
                    model.unanswered.replace(None);
                    model.aged.set(false);
                    model.touched.set(false);
                    model.target.replace(None);
                    model.cancel.replace(Some(gio::Cancellable::new()));
                    // Every output starts from the same "now", so the card
                    // shows "No change" until the operator moves something.
                    model.reset_all();
                    glib::timeout_add_seconds_local(REFRESH_SECS, {
                        let model = model.clone();
                        move || {
                            if !model.current(mine) {
                                return glib::ControlFlow::Break;
                            }
                            // Untouched, "now" moves on: never a stale backward step.
                            if model.touched.get() {
                                model.aged.set(true);
                                model.redraw();
                            } else {
                                model.reset_all();
                            }
                            glib::ControlFlow::Continue
                        }
                    });
                    // Standing is asked once per opening: it does not drift.
                    ask_standing(&verdict_host, verdict_port, model.clone(), mine);
                } else {
                    // A copy opening into a card already up (a screen plugged in
                    // since): show what the others show, never its own stale target.
                    let target = model.target.borrow().clone();
                    match target.filter(|_| model.touched.get()) {
                        Some(t) => show_target(&t),
                        None => model.reset_all(),
                    }
                }
                recompute();
                sheet.set_visible(true);
                dim.add_css_class("kiosk-scrim-open");
                dim.set_can_target(true);
            } else {
                if model.open.replace(false) {
                    if let Some(c) = model.cancel.take() {
                        c.cancel();
                    }
                }
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
            let model = me.model.clone();
            if !me.is_open() || model.busy() {
                return;
            }
            let want = wire_target(&calendar, hour.get(), minute.get());
            let card_epoch = card_epoch(&calendar, hour.get(), minute.get());
            let shown = btn.has_css_class("kiosk-settime-set-restart");
            let opening = model.opened.get();
            let click = model.clicks.get().wrapping_add(1).max(1);
            model.clicks.set(click);
            model.confirming.set(click);
            model.redraw();
            // Ask again now: the button may be up to a tick old.
            let (me, shared, reporter, host) =
                (me.clone(), shared.clone(), reporter.clone(), host.clone());
            let asked = want.clone();
            ask_verdict(
                &verdict_host,
                verdict_port,
                &asked,
                model.cancellable(),
                Box::new(move |answer, epoch| {
                    if model.confirming.get() == click {
                        model.confirming.set(0);
                    }
                    // Closed or reopened since: never send. Redrawn, since a
                    // reopening may have drawn Set dead while this was in flight.
                    if !model.current(opening) {
                        model.redraw();
                        return;
                    }
                    model.record(want.clone(), answer);
                    // Unreachable host: what the button showed.
                    let restarts = answer.map_or(shown, |(r, _)| r);
                    if restarts != shown {
                        log::info!("set-time: {want:?} now restart={restarts}; not sending");
                        model.redraw();
                        return;
                    }
                    model.sending.set(true);
                    let line = wire_line(&want, epoch, card_epoch);
                    if epoch.is_some() && !line.starts_with('@') {
                        log::warn!(
                            "set-time: host read {want:?} as {epoch:?}, this card as {card_epoch:?}; sending the string"
                        );
                    }
                    log::info!(
                        "set-time: sending {:?} for {want:?} to {host}:{port} (restarts={restarts})",
                        line.trim_end()
                    );
                    send(&host, port, line, shared, reporter, me, restarts);
                }),
            );
        });
    }

    settime
}

/// Ask the host what setting the clock to `want` would do.
///
/// The host parses `want` in its own zone against its own clock, so neither
/// gui-vm's zone nor its clock enters the answer. Composing the two was the bug.
fn ask_verdict(
    host: &str,
    port: u16,
    want: &str,
    cancel: Option<gio::Cancellable>,
    on_reply: OnVerdict,
) {
    let client = gio::SocketClient::new();
    client.set_timeout(ASK_TIMEOUT_SECS);
    let line = format!("{PROBE_VERB} {want}\n");
    let want = want.to_owned();
    let first = cancel.clone();
    client.connect_to_host_async(host, port, first.as_ref(), move |res| {
        let conn = match res {
            Ok(c) => c,
            Err(e) => {
                log::warn!("set-time: no clock verdict from the host: {e}");
                on_reply(None, None);
                return;
            }
        };
        let write = cancel.clone();
        conn.output_stream().write_all_async(
            line.into_bytes(),
            glib::Priority::DEFAULT,
            write.as_ref(),
            move |res| {
                if let Err((_, e)) = res {
                    log::warn!("set-time: could not ask for the clock verdict: {e}");
                    on_reply(None, None);
                    return;
                }
                let istream = conn.input_stream();
                istream.read_all_async(
                    vec![0u8; REPLY_CAP],
                    glib::Priority::DEFAULT,
                    cancel.as_ref(),
                    move |res| {
                        // Moved in so the connection outlives the read.
                        let _conn = conn;
                        let reply = match res {
                            Ok((buf, n, _)) => String::from_utf8_lossy(&buf[..n]).into_owned(),
                            Err((_, e)) => {
                                log::warn!("set-time: no reply to the clock verdict: {e}");
                                on_reply(None, None);
                                return;
                            }
                        };
                        let answer = parse_verdict(&reply);
                        let epoch = answer.and_then(|_| parse_epoch(&reply));
                        match answer {
                            Some((restarts, after)) => log::info!(
                                "set-time: host says {want:?} -> restart={restarts} after={after:?} epoch={epoch:?}"
                            ),
                            None => log::warn!("set-time: unusable verdict {:?}", reply.trim()),
                        }
                        on_reply(answer, epoch);
                    },
                );
            },
        );
    });
}

/// Ask the host where its clock stands -- an empty request, for the threshold
/// and the opening estimate. Failure is log-only: the card reads "unconfirmed",
/// and a banner for a label's wording would be noise.
fn ask_standing(host: &str, port: u16, model: Rc<Model>, opening: u64) {
    let client = gio::SocketClient::new();
    client.set_timeout(ASK_TIMEOUT_SECS);
    let cancel = model.cancellable();
    let first = cancel.clone();
    client.connect_to_host_async(host, port, first.as_ref(), move |res| {
        let conn = match res {
            Ok(c) => c,
            Err(e) => {
                log::warn!("set-time: no clock verdict from the host: {e}");
                return;
            }
        };
        let write = cancel.clone();
        conn.output_stream().write_all_async(
            b"\n".to_vec(),
            glib::Priority::DEFAULT,
            write.as_ref(),
            move |res| {
                if let Err((_, e)) = res {
                    log::warn!("set-time: could not ask for the clock verdict: {e}");
                    return;
                }
                let istream = conn.input_stream();
                istream.read_all_async(
                    vec![0u8; REPLY_CAP],
                    glib::Priority::DEFAULT,
                    cancel.as_ref(),
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
                                if model.current(opening) {
                                    model.standing.set(Some(v));
                                    model.redraw();
                                }
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

/// Send `line` (see `wire_line`) to the clock socket and act on the one-line
/// reply. All async on the GTK loop; `conn` is carried through every stage so
/// the stream stays open. Bounded by a timeout but never cancelled: once
/// written, the clock may be set, and only the reply can say whether to show
/// the restart screen.
fn send(
    host: &str,
    port: u16,
    line: String,
    shared: Shared,
    reporter: Broadcast,
    card: SetTime,
    restarts: bool,
) {
    let client = gio::SocketClient::new();
    client.set_timeout(SEND_TIMEOUT_SECS);
    client.connect_to_host_async(host, port, gio::Cancellable::NONE, move |res| {
        let conn = match res {
            Ok(c) => c,
            Err(e) => {
                reporter.error(&format!("Could not reach the clock service: {e}"));
                card.ready();
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
                    card.ready();
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
                                card.ready();
                                return;
                            }
                        };
                        match reply.as_str() {
                            "ok" => {
                                card.ready();
                                card.close();
                                if restarts {
                                    crate::shutdown::request_restart(&shared);
                                } else {
                                    reporter.info("Clock set.");
                                }
                            }
                            other => {
                                reporter.error(&format!("Could not set the clock: {other}"));
                                card.ready();
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

    #[test]
    fn parses_the_hosts_targeted_reply() {
        assert_eq!(parse_verdict(REPLY), Some((false, Some(-1500))));
        assert_eq!(
            parse_verdict(
                "restart\ndisplacement_now=1200 step=900 displacement_after=2100 threshold=1800\n"
            ),
            Some((true, Some(2100)))
        );
    }

    /// The first token is the answer; the number only words it.
    #[test]
    fn a_verdict_stands_without_its_detail() {
        assert_eq!(parse_verdict("no-restart\n"), Some((false, None)));
        assert_eq!(parse_verdict("restart"), Some((true, None)));
        assert_eq!(
            parse_verdict("restart\ndisplacement_after=x"),
            Some((true, None))
        );
    }

    /// A cached answer the host could not renew is no longer the host's; a
    /// failure about another target leaves it alone.
    #[test]
    fn a_failed_reask_withdraws_only_its_own_answer() {
        let m = Model::default();
        m.aged.set(true);
        m.record("2026-11-02 15:00".into(), Some((false, Some(-1500))));
        assert!(!m.aged.get());
        assert!(m.unanswered.borrow().is_none());

        m.record("2026-11-02 14:00".into(), None);
        assert!(m.verdict.borrow().is_some());
        assert_eq!(m.unanswered.borrow().as_deref(), Some("2026-11-02 14:00"));

        m.record("2026-11-02 15:00".into(), None);
        assert!(m.verdict.borrow().is_none());
        assert_eq!(m.unanswered.borrow().as_deref(), Some("2026-11-02 15:00"));

        m.record("2026-11-02 15:00".into(), Some((true, None)));
        assert_eq!(
            *m.verdict.borrow(),
            Some(("2026-11-02 15:00".to_owned(), true, None))
        );
        assert!(m.unanswered.borrow().is_none());
    }

    /// A host with the epoch (tiiuae/ghaf-sfo-laptop#65) appends two tokens;
    /// every reading of the reply that predates them must be unchanged.
    const REPLY_WITH_EPOCH: &str = "no-restart\n\
        displacement_now=1200 step=-2700 displacement_after=-1500 threshold=1800 \
        epoch=1794384000 zone=Asia/Dubai\n";

    #[test]
    fn the_epoch_tokens_change_nothing_else() {
        assert_eq!(parse_verdict(REPLY_WITH_EPOCH), parse_verdict(REPLY));
        assert_eq!(parse_standing(REPLY_WITH_EPOCH), parse_standing(REPLY));
    }

    #[test]
    fn sends_the_hosts_epoch_when_it_gave_one() {
        let epoch = parse_epoch(REPLY_WITH_EPOCH);
        assert_eq!(epoch, Some(1794384000));
        assert_eq!(
            wire_line("2026-11-11 12:00", epoch, Some(1794384000)),
            "@1794384000\n"
        );
    }

    /// Host and card disagree on the instant (a zone wrong on one side): today's string.
    #[test]
    fn falls_back_to_the_string_when_host_and_card_disagree() {
        let want = "2026-11-11 12:00";
        assert_eq!(
            wire_line(want, Some(1794384000), Some(1794369600)),
            "2026-11-11 12:00\n"
        );
        assert_eq!(
            wire_line(want, Some(1794384000), None),
            "2026-11-11 12:00\n"
        );
    }

    /// An older host, or a garbled number: the string, exactly as before.
    #[test]
    fn falls_back_to_the_string_without_an_epoch() {
        for reply in [
            REPLY,
            "restart\nepoch=x",
            "restart\nepoch=-5",
            "restart\nepoch=0",
        ] {
            assert_eq!(parse_epoch(reply), None, "{reply:?}");
        }
        assert_eq!(
            wire_line("2026-11-11 12:00", None, Some(1794384000)),
            "2026-11-11 12:00\n"
        );
    }

    /// A send cannot be cancelled, so a close and reopen must not free Set.
    #[test]
    fn a_send_in_flight_keeps_set_busy() {
        let m = Model::default();
        assert!(!m.busy());
        m.sending.set(true);
        assert!(m.busy());
        m.sending.set(false);
        m.confirming.set(3);
        assert!(m.busy());
    }

    /// Anything that is not one of the two words is no answer at all: a refusal
    /// must not read as "no restart".
    #[test]
    fn refuses_anything_but_the_two_words() {
        assert_eq!(parse_verdict(""), None);
        assert_eq!(parse_verdict("error: no boot anchor\n"), None);
        assert_eq!(parse_verdict("Restart\n"), None);
        assert_eq!(parse_verdict("displacement_after=10 restart"), None);
    }
}
