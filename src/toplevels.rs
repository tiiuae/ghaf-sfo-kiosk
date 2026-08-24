// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// Bringing an already-running singleton's window to the front, instead of just
// saying it is open. GIVC has no concept of a window at all -- it only starts
// and stops units, and its unit numbers are reused slots, not identities, so
// they cannot answer "is this app's window open" either. This talks to
// cosmic-comp directly over Wayland, using the same protocol COSMIC's own
// panel and task switcher use, and treats the compositor as the only source
// of truth for what is actually on screen.
//
// `ext_foreign_toplevel_list_v1`, not `zcosmic_toplevel_info_v1`'s own
// `toplevel`/`app_id`/`title` events: those are deprecated since version 2 in
// favour of exactly this split -- list and identify a toplevel through the
// standard protocol, then call `get_cosmic_toplevel` to get the COSMIC
// extension object for `state` and, via `zcosmic_toplevel_manager_v1`, for
// `activate`/`unset_minimized`.
//
// A fresh connection per attempt, not one kept open for the kiosk's lifetime:
// this only runs on a single_instance button's press, so the cost of a
// roundtrip on a local socket is not worth folding a second event queue into
// the GTK main loop. It does run on a background thread, though -- several
// blocking roundtrips on the GTK thread would freeze the whole kiosk if
// cosmic-comp were ever slow to answer.
//
// Verified against a live device: a plain, un-sandboxed client connecting the
// ordinary way is NOT filtered by cosmic-comp's `client_not_sandboxed` check,
// and DOES receive the flatpak-vm application's window as a normal toplevel.

use crate::protocols::toplevel_info::v1::client::{
    zcosmic_toplevel_handle_v1, zcosmic_toplevel_info_v1,
};
use crate::protocols::toplevel_management::v1::client::zcosmic_toplevel_manager_v1;
use gtk::gio;
use gtk::gio::prelude::CancellableExt;
use gtk::glib;
use std::time::{Duration, Instant};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1, ext_foreign_toplevel_list_v1,
};

/// How long a single_instance launch keeps the button locked, waiting for its
/// window to actually appear, before giving up and reporting the launch
/// timed out. See `wait_for_window_async`.
const LAUNCH_GRACE: Duration = Duration::from_secs(45);
/// How often `wait_for_window_async` re-asks the compositor while waiting.
const LAUNCH_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// How often the GTK-thread side checks whether a background answer has
/// arrived. A `timeout_add_local`, not `idle_add_local`: an idle source
/// re-arms continuously whenever nothing higher-priority is pending, which
/// pegs a core for the whole wait; a low-frequency timeout does not.
const DELIVERY_POLL_INTERVAL: Duration = Duration::from_millis(25);
/// The per-call-site safety margin `compute_then_deliver` waits for its
/// background thread before giving up on it, on top of whatever budget the
/// `compute` closure itself needs. A local Wayland roundtrip normally
/// completes in well under this; it exists for a compositor that accepts the
/// connection but then simply stops answering mid-roundtrip.
/// `EventQueue::roundtrip` has no timeout of its own, so a stall there blocks
/// the background thread forever, and `connect_and_settle`'s own `attempts`
/// cap never gets the chance to run out because the very first stuck call
/// never returns. Without this, that stall wedges the button for the rest of
/// the session. `check_and_activate_async` uses this value directly, since
/// its `compute` is one bounded call; `wait_for_window_async` adds it on top
/// of `LAUNCH_GRACE`, since its `compute` is a whole polling loop that is
/// *meant* to run far longer than a single roundtrip.
const COMPUTE_TIMEOUT: Duration = Duration::from_secs(5);

pub enum Activation {
    /// A toplevel with exactly this `app_id` was found and `activate` sent.
    Activated,
    /// Talked to cosmic-comp fine, but nothing matched `app_id`.
    NotFound,
    /// Could not even ask -- no compositor connection, or the protocol is not
    /// advertised. Carries a one-line reason for the log.
    Unavailable(String),
}

/// Deliver a value computed on a background thread back to a closure on the
/// GTK thread that spawned it, without requiring that closure to be `Send`.
///
/// Not `MainContext::invoke`: that requires the callback itself to be `Send`,
/// and callers here routinely close over `Busy`/`Reporter`, both `Rc`-based
/// and deliberately not `Send` -- this is a GTK app with everything on one
/// thread, and forcing those types to be thread-safe just to satisfy a
/// callback signature would be the wrong fix. Only `compute` runs on the
/// background thread; its result (required to be `Send`) crosses back over an
/// ordinary channel. `compute` gets a `&gio::Cancellable`, already cancelled
/// by the time `on_disconnected` fires, so a `compute` that checks it between
/// steps of its own can stop early instead of running out its full duration
/// after the caller has already moved on -- see `wait_for_window_async`.
///
/// A disconnected channel -- `compute` panicked before sending -- still calls
/// `on_result`, via `on_disconnected`: every caller has a `Busy` flag waiting
/// on this closing, and dropping it silently would wedge that button for the
/// rest of the session.
///
/// A channel that stays merely EMPTY forever gets the same treatment once
/// `timeout` passes -- a caller-supplied deadline, not one constant shared by
/// every caller: `compute` is not cancellable mid-roundtrip -- it typically
/// blocks inside `EventQueue::roundtrip`, which has no timeout of its own --
/// so a background thread that hangs stays leaked for the life of the
/// process. What this guards is the GTK thread: without a deadline here, the
/// poll loop below waits on that leaked thread's result forever, and the
/// `Busy` flag riding on `on_result` never clears.
fn compute_then_deliver<T, F>(
    timeout: Duration,
    compute: impl FnOnce(&gio::Cancellable) -> T + Send + 'static,
    on_disconnected: impl FnOnce() -> T + 'static,
    on_result: F,
) where
    T: Send + 'static,
    F: FnOnce(T) + 'static,
{
    let cancellable = gio::Cancellable::new();
    let (tx, rx) = std::sync::mpsc::channel::<T>();
    let cancellable_for_thread = cancellable.clone();
    std::thread::spawn(move || {
        let _ = tx.send(compute(&cancellable_for_thread));
    });

    let deadline = Instant::now() + timeout;
    let mut on_result = Some(on_result);
    let mut on_disconnected = Some(on_disconnected);
    glib::timeout_add_local(DELIVERY_POLL_INTERVAL, move || match rx.try_recv() {
        Ok(value) => {
            if let Some(f) = on_result.take() {
                f(value);
            }
            glib::ControlFlow::Break
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            if Instant::now() < deadline {
                return glib::ControlFlow::Continue;
            }
            cancellable.cancel();
            if let (Some(f), Some(default)) = (on_result.take(), on_disconnected.take()) {
                f(default());
            }
            glib::ControlFlow::Break
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            if let (Some(f), Some(default)) = (on_result.take(), on_disconnected.take()) {
                f(default());
            }
            glib::ControlFlow::Break
        }
    });
}

/// Ask cosmic-comp whether a toplevel with this exact `app_id` exists and, if
/// so, raise it -- without blocking the caller's thread. `on_result` runs
/// back on the GLib main loop, same thread it was called from.
pub fn check_and_activate_async<F>(app_id: String, on_result: F)
where
    F: FnOnce(Activation) + 'static,
{
    compute_then_deliver(
        COMPUTE_TIMEOUT,
        move |_cancellable: &gio::Cancellable| activate_by_app_id(&app_id),
        || Activation::Unavailable("the check thread ended without answering".to_owned()),
        on_result,
    );
}

/// Poll the compositor for a toplevel matching `app_id`, calling
/// `on_result(true)` as soon as one appears, or `on_result(false)` once
/// `LAUNCH_GRACE` elapses with none found. For the gap between "givc-cli
/// queued the launch" and "the window actually exists" -- during which the
/// old unit-number check would already see the app as running, but the
/// compositor genuinely has nothing to raise yet.
pub fn wait_for_window_async<F>(app_id: String, on_result: F)
where
    F: FnOnce(bool) + 'static,
{
    compute_then_deliver(
        // The outer guard on top of LAUNCH_GRACE, not LAUNCH_GRACE itself --
        // this only fires if the loop below is ALSO stuck inside a single
        // roundtrip past its own deadline. Using COMPUTE_TIMEOUT alone here
        // was the bug: a poll loop that is *meant* to run for 45s was being
        // cut off at 5s by a timeout meant for a single bounded call.
        LAUNCH_GRACE + COMPUTE_TIMEOUT,
        move |cancellable: &gio::Cancellable| {
            let deadline = Instant::now() + LAUNCH_GRACE;
            loop {
                if cancellable.is_cancelled() {
                    return false;
                }
                match find_toplevel_status(&app_id) {
                    ToplevelStatus::Found => return true,
                    ToplevelStatus::NotFound => {}
                }
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(LAUNCH_POLL_INTERVAL);
            }
        },
        // Couldn't tell -- stop waiting rather than lock the button forever.
        || false,
        on_result,
    );
}

enum ToplevelStatus {
    Found,
    /// Also covers "could not tell" -- `wait_for_window_async`'s poll loop
    /// treats both alike, retrying regardless of which one it was.
    NotFound,
}

#[derive(Default)]
struct AppData {
    toplevel_info: Option<zcosmic_toplevel_info_v1::ZcosmicToplevelInfoV1>,
    toplevel_manager: Option<zcosmic_toplevel_manager_v1::ZcosmicToplevelManagerV1>,
    /// Kept only to know whether the compositor advertised the protocol at
    /// all -- nothing here ever calls a method on it. Without this,
    /// `toplevels` stays empty both when the protocol is missing and when it
    /// is present with genuinely nothing open, and those two are not the same
    /// answer: the caller needs to tell "confirmed empty" from "never asked".
    toplevel_list: Option<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1>,
    seat: Option<wl_seat::WlSeat>,
    toplevels: Vec<Toplevel>,
}

struct Toplevel {
    foreign: ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    cosmic: zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
    app_id: Option<String>,
    /// Set once this toplevel's own `done` fires -- see `connect_and_settle`
    /// for why this, and not `zcosmic_toplevel_info_v1::done`, is what is
    /// waited on.
    done: bool,
}

impl Dispatch<zcosmic_toplevel_info_v1::ZcosmicToplevelInfoV1, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &zcosmic_toplevel_info_v1::ZcosmicToplevelInfoV1,
        _: zcosmic_toplevel_info_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Its own `toplevel` event is the deprecated v1 path -- toplevels are
        // discovered via ext_foreign_toplevel_list_v1 instead, and this
        // object exists only so get_cosmic_toplevel has something to call.
        //
        // Its `done` is NOT a "this client's initial sync is complete"
        // signal, confirmed against cosmic-comp's own source
        // (src/wayland/protocols/toplevel_info.rs): the compositor only
        // sends it on a global dirty-to-clean transition across ALL
        // clients' state, not on a fresh bind -- so a client that connects
        // while the desktop happens to be quiet can wait on it forever.
        // `ext_foreign_toplevel_handle_v1::done`, tracked per toplevel
        // below, does not have this problem.
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for AppData {
    fn event(
        app_data: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<AppData>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match &*interface {
            // get_cosmic_toplevel and the non-deprecated `done` event both
            // need version 2. Binding higher than the compositor advertises
            // is a protocol error that kills the connection outright, so
            // clamp to what it actually offers -- and skip entirely below
            // that, leaving `toplevel_info` None, which the caller already
            // treats as Unavailable.
            "zcosmic_toplevel_info_v1" if version >= 2 => {
                app_data.toplevel_info = Some(
                    registry.bind::<zcosmic_toplevel_info_v1::ZcosmicToplevelInfoV1, _, _>(
                        name,
                        version.min(2),
                        qh,
                        (),
                    ),
                );
            }
            "zcosmic_toplevel_manager_v1" => {
                app_data.toplevel_manager = Some(
                    registry.bind::<zcosmic_toplevel_manager_v1::ZcosmicToplevelManagerV1, _, _>(
                        name,
                        1,
                        qh,
                        (),
                    ),
                );
            }
            "ext_foreign_toplevel_list_v1" => {
                app_data.toplevel_list = Some(
                    registry.bind::<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, _, _>(
                        name,
                        1,
                        qh,
                        (),
                    ),
                );
            }
            "wl_seat" => {
                app_data.seat = Some(registry.bind::<wl_seat::WlSeat, _, _>(name, 1, qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, ()> for AppData {
    fn event(
        app_data: &mut Self,
        _: &ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<AppData>,
    ) {
        let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event else {
            return;
        };
        // toplevel_info is bound in the same registry burst as this list, so
        // it is already Some by the time the compositor starts sending real
        // toplevels (which needs a further roundtrip to request and flush) --
        // unless the compositor only advertised version 1, in which case it
        // is deliberately left None and there is nothing to pair this with.
        let Some(info) = app_data.toplevel_info.as_ref() else {
            return;
        };
        let cosmic = info.get_cosmic_toplevel(&toplevel, qh, ());
        app_data.toplevels.push(Toplevel {
            foreign: toplevel,
            cosmic,
            app_id: None,
            done: false,
        });
    }

    wayland_client::event_created_child!(
        AppData,
        ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
        [
            ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ()),
        ]
    );
}

impl Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ()> for AppData {
    fn event(
        app_data: &mut Self,
        handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                if let Some(t) = app_data.toplevels.iter_mut().find(|t| &t.foreign == handle) {
                    t.app_id = Some(app_id);
                }
            }
            ext_foreign_toplevel_handle_v1::Event::Done => {
                if let Some(t) = app_data.toplevels.iter_mut().find(|t| &t.foreign == handle) {
                    t.done = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
        _: zcosmic_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Its app_id/title events are the deprecated ones -- read from the
        // ext_foreign_toplevel_handle_v1 counterpart instead. Nothing here
        // needs `state`: unset_minimized is sent unconditionally rather than
        // conditioned on it -- see activate_by_app_id.
    }
}

impl Dispatch<zcosmic_toplevel_manager_v1::ZcosmicToplevelManagerV1, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &zcosmic_toplevel_manager_v1::ZcosmicToplevelManagerV1,
        _: zcosmic_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Connect and drive roundtrips until every currently-known toplevel has
/// reported its own `done` (all its properties atomically settled) and no
/// further roundtrip discovered a new one, or `attempts` roundtrips pass
/// without settling, whichever comes first.
///
/// Deliberately not `zcosmic_toplevel_info_v1::done`: confirmed against
/// cosmic-comp's own source that it is not a per-client "initial sync
/// complete" signal -- the compositor only sends it on a global
/// dirty-to-clean transition across every client's state, so a client that
/// connects while the desktop happens to be quiet can wait on it forever.
/// `ext_foreign_toplevel_handle_v1::done` fires whenever that one handle's
/// own properties changed, which they always do on their first send, so it
/// does not share that failure mode. The `attempts` cap only bounds a
/// compositor that keeps answering but never settles -- it does NOT bound a
/// compositor that stops answering mid-roundtrip, since `roundtrip` itself
/// has no timeout and a stall there blocks before `attempts` can run out.
/// That case is caught one layer up, by `compute_then_deliver`'s own
/// wall-clock `COMPUTE_TIMEOUT` on the caller's side of this call.
fn connect_and_settle(
    attempts: u32,
) -> Result<(Connection, wayland_client::EventQueue<AppData>, AppData), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland connection: {e}"))?;
    let display = conn.display();
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();
    let _registry = display.get_registry(&qh, ());

    let mut app_data = AppData::default();
    let mut last_count = 0;
    for round in 0..attempts {
        event_queue
            .roundtrip(&mut app_data)
            .map_err(|e| format!("Wayland roundtrip failed: {e}"))?;
        // Round 0 only sees the registry's globals and issues the binds;
        // nothing has requested toplevels yet, so an empty, "already
        // settled" list here would be premature, not a real answer. Also
        // requires toplevel_list itself: without it, `toplevels` stays empty
        // forever not because nothing is open but because nothing was ever
        // asked, and that must not be reported as a settled, trustworthy
        // empty list -- it would read as "confirmed nothing running" to
        // every caller, when the honest answer is "could not ask".
        let settled = round > 0
            && app_data.toplevel_list.is_some()
            && app_data.toplevels.len() == last_count
            && app_data.toplevels.iter().all(|t| t.done);
        last_count = app_data.toplevels.len();
        if settled {
            return Ok((conn, event_queue, app_data));
        }
    }
    if app_data.toplevel_list.is_none() {
        return Err("ext_foreign_toplevel_list_v1 not advertised".to_owned());
    }
    Err(format!(
        "toplevel list never settled after {attempts} roundtrips"
    ))
}

/// Whether a toplevel with this exact `app_id` currently exists, without
/// touching it. Used only by `wait_for_window_async`'s polling loop, which
/// treats "could not tell" the same as "not found yet" -- see
/// `ToplevelStatus::NotFound`'s own doc comment -- so a connection failure or
/// a too-old compositor here is not distinguished from a genuine absence.
fn find_toplevel_status(app_id: &str) -> ToplevelStatus {
    let (_conn, _event_queue, app_data) = match connect_and_settle(10) {
        Ok(v) => v,
        Err(e) => {
            // Folded into NotFound by design -- see this function's own doc
            // comment -- but the reason is worth keeping somewhere: a poll
            // loop that always fails the same way should be diagnosable from
            // the log instead of just watching a button retry forever.
            log::debug!("find_toplevel_status({app_id}): {e}");
            return ToplevelStatus::NotFound;
        }
    };
    if app_data.toplevel_info.is_none() {
        return ToplevelStatus::NotFound;
    }
    if app_data
        .toplevels
        .iter()
        .any(|t| t.app_id.as_deref() == Some(app_id))
    {
        ToplevelStatus::Found
    } else {
        log::debug!(
            "find_toplevel_status({app_id}): settled with no match; saw {:?}",
            app_data
                .toplevels
                .iter()
                .map(|t| t.app_id.as_deref().unwrap_or("<none>"))
                .collect::<Vec<_>>()
        );
        ToplevelStatus::NotFound
    }
}

/// Ask cosmic-comp to raise and focus the toplevel whose `app_id` exactly
/// equals `app_id`. Exact, not substring: `app_id` is the specific string a
/// product's config names for one application, and a looser match risks
/// raising the wrong one -- "Plan" would otherwise match "Mission Planner".
fn activate_by_app_id(app_id: &str) -> Activation {
    let (_conn, mut event_queue, mut app_data) = match connect_and_settle(10) {
        Ok(v) => v,
        Err(e) => return Activation::Unavailable(e),
    };

    // Checked explicitly rather than folded into "no match found": a
    // compositor that never advertised the protocol at all means "could not
    // ask", not "definitely not open", and the two must not be confused --
    // the caller treats them alike today (both fall through to launching),
    // but that is its call to make, not this function's to hide.
    if app_data.toplevel_info.is_none() {
        return Activation::Unavailable(
            "zcosmic_toplevel_info_v1 not advertised at version 2 or higher".to_owned(),
        );
    }
    let Some(manager) = app_data.toplevel_manager.as_ref() else {
        return Activation::Unavailable("zcosmic_toplevel_manager_v1 not advertised".to_owned());
    };
    let Some(seat) = app_data.seat.as_ref() else {
        return Activation::Unavailable("no wl_seat".to_owned());
    };
    let Some(toplevel) = app_data
        .toplevels
        .iter()
        .find(|t| t.app_id.as_deref() == Some(app_id))
    else {
        // A settled connection reporting no match is not an error, so this
        // stays at debug -- but a caller chasing a false "not found" needs to
        // see what this connection actually saw, not just that it saw
        // nothing matching.
        log::debug!(
            "activate_by_app_id({app_id}): settled with no match; saw {:?}",
            app_data
                .toplevels
                .iter()
                .map(|t| t.app_id.as_deref().unwrap_or("<none>"))
                .collect::<Vec<_>>()
        );
        return Activation::NotFound;
    };

    // unset_minimized is a separate request from activate, not implied by
    // it. Harmless to send unconditionally: a no-op on a window that was
    // never minimized.
    manager.unset_minimized(&toplevel.cosmic);
    manager.activate(&toplevel.cosmic, seat);
    if let Err(e) = event_queue.roundtrip(&mut app_data) {
        return Activation::Unavailable(format!("activate did not flush: {e}"));
    }
    Activation::Activated
}
