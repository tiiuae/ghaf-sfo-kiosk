// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// Running a button's action and reporting what happened -- to the banner and to
// the journal with the full argv. A kiosk that silently does nothing leaves the
// operator with no desktop, launcher or terminal to diagnose from.
//
// Never wait for the child and never impose a timeout: an `exec` action runs the
// application itself, and `cosmic-settings network` exits when the operator
// closes the window, possibly an hour later. Spawn, report a non-zero exit,
// otherwise stay quiet. stdout/stderr are inherited so they land in the journal
// under the kiosk's own unit.

use gtk::gio;
use gtk::glib;

use crate::config::{Action, AwaitJob, SingleInstance};
use crate::toplevels::{self, Activation};

/// How often to ask whether the job is still registered.
const POLL_SECONDS: u32 = 2;
/// Give up watching after this long and say so. The job is not cancelled -- we
/// simply stop claiming to know, which is better than a banner that never
/// resolves.
const POLL_GIVE_UP_SECONDS: u32 = 15 * 60;

/// How the caller is told what happened.
pub trait Reporter: 'static {
    fn info(&self, message: &str);
    fn error(&self, message: &str);
}

/// One button's "a run is already in flight" flag.
///
/// Without it every press spawns another process, and for a button whose child
/// IS a window that is one window per press -- three stacked cosmic-applet-power
/// windows was the report that prompted this. Cleared when the child exits, or
/// for an awaited GIVC action when the job leaves the registry, so a second
/// press cannot re-queue an install that is still running.
///
/// For a `single_instance` button this also covers the compositor check
/// itself: that check is asynchronous too (a background thread, see
/// `toplevels::check_and_activate_async`), and without `busy` a rapid double
/// press could start two checks that both conclude "not open yet" and launch
/// twice.
#[derive(Clone, Default)]
pub struct Busy(std::rc::Rc<std::cell::Cell<bool>>);

impl Busy {
    pub fn new() -> Self {
        Self::default()
    }
    fn set(&self, v: bool) {
        self.0.set(v);
    }
    fn get(&self) -> bool {
        self.0.get()
    }
}

/// Spawn `argv`, reporting a spawn failure through `reporter` and returning
/// `None`. Shared by every path that ends in actually starting something.
fn spawn_or_report<R: Reporter>(
    argv: &[String],
    label: &str,
    reporter: &R,
) -> Option<gio::Subprocess> {
    let refs: Vec<&std::ffi::OsStr> = argv.iter().map(std::ffi::OsStr::new).collect();
    // NONE: inherit stdout and stderr, so the child's own diagnostics go to
    // the journal rather than into a buffer only we can see.
    match gio::Subprocess::newv(&refs, gio::SubprocessFlags::NONE) {
        Ok(p) => Some(p),
        Err(e) => {
            // Almost always "no such file": a store path that did not make it
            // into the VM, or a typo in the nix module.
            log::error!("button {label:?}: spawn failed: {e}; argv was {argv:?}");
            reporter.error(&format!("{label} could not start: {e}"));
            None
        }
    }
}

/// Spawn `argv` and track it through to completion: clears `busy` when the
/// child exits (after `await_job` finishes watching, if there is one), and on
/// spawn failure too, so a failed launch never leaves a button wedged.
///
/// Shared by the plain launch path and by a `single_instance` button once its
/// compositor check has concluded there is nothing to raise.
fn spawn_and_track<R: Reporter + Clone>(
    argv: Vec<String>,
    label: String,
    reporter: R,
    busy: Busy,
    await_job: Option<AwaitJob>,
) {
    log::info!("button {label:?}: exec {argv:?}");
    reporter.info(&format!("Starting {label}…"));

    let Some(proc) = spawn_or_report(&argv, &label, &reporter) else {
        busy.set(false);
        return;
    };
    busy.set(true);

    proc.wait_check_async(gio::Cancellable::NONE, move |result| match result {
        Ok(()) => {
            // Either it did its job and exited, or the operator closed the
            // window. Both are unremarkable, and a banner here would pop up
            // long after the press that caused it.
            log::info!("button {label:?}: child exited cleanly");
            // For a GIVC job this only means "queued". The work is still
            // running in the other VM, so stay busy until the job is gone.
            if let Some(job) = await_job {
                watch_job(job, label, reporter, busy);
            } else {
                busy.set(false);
            }
        }
        Err(e) => {
            log::error!("button {label:?}: {e}");
            // Keep the banner readable; the journal has the child's own output.
            let msg = e.to_string();
            let brief: String = msg.chars().take(160).collect();
            reporter.error(&format!("{label} failed: {brief}"));
            // A failed run must not leave the button wedged.
            busy.set(false);
        }
    });
}

/// Run `action`, reporting progress and outcome through `reporter`.
pub fn dispatch<R: Reporter + Clone>(action: &Action, label: &str, reporter: &R, busy: &Busy) {
    let mut await_job: Option<AwaitJob> = None;
    let (argv, single_instance): (Vec<String>, Option<SingleInstance>) = match action {
        Action::Exec { argv, single_instance } => (argv.clone(), single_instance.clone()),
        Action::Givc {
            argv,
            target,
            await_job: job,
            single_instance,
        } => {
            log::info!("button {label:?} targets {target}");
            await_job = job.clone();
            (argv.clone(), single_instance.clone())
        }
        // The fan handles a trigger's presses, so reaching here means it was
        // rendered in the grid -- which config::partition never does. Log rather
        // than panic; the operator keeps a working kiosk.
        Action::Menu => {
            log::warn!("button {label:?} is a menu trigger but was pressed as an ordinary button");
            return;
        }
        Action::Unsupported { reason } => {
            // Not an error to shout about — it is a configuration problem the
            // operator can do nothing about. Say precisely what is wrong so
            // whoever reads the photo of the screen knows where to look.
            log::warn!("button {label:?} pressed but not configured: {reason}");
            reporter.error(&format!("{label} is not configured: {reason}"));
            return;
        }
    };

    // A press while the last run -- or the last compositor check, below -- is
    // still going is a press the operator has already made.
    if busy.get() {
        log::info!("button {label:?} pressed while its previous run is still going; ignoring");
        reporter.info(&format!("{label} is already open"));
        return;
    }

    if let Some(si) = single_instance {
        bring_to_front_or_launch(si, argv, label.to_owned(), reporter.clone(), busy.clone());
        return;
    }

    spawn_and_track(
        argv,
        label.to_owned(),
        reporter.clone(),
        busy.clone(),
        await_job,
    );
}

/// A `single_instance` launcher's press: ask cosmic-comp directly whether its
/// window already exists, and either raise it or launch fresh -- never both.
///
/// GIVC cannot answer "is this app's window open": its `run-flatpak-app@N`
/// unit numbers are reused slots, not identities, so a launcher whose number
/// happens to have been recycled from a *different*, since-closed launcher
/// would otherwise be reported as already running. Asking the compositor
/// sidesteps that entirely -- it is the one thing that actually knows what is
/// on screen, keyed on `window_app_id` rather than a unit number.
///
/// Runs the check on a background thread (see
/// `toplevels::check_and_activate_async`) so a slow or unreachable
/// compositor cannot freeze the kiosk; `busy` covers the gap.
fn bring_to_front_or_launch<R: Reporter + Clone>(
    si: SingleInstance,
    argv: Vec<String>,
    label: String,
    reporter: R,
    busy: Busy,
) {
    busy.set(true);
    let app_id = si.window_app_id;
    let argv_exits_quickly = si.argv_exits_quickly;
    toplevels::check_and_activate_async(app_id.clone(), move |result| {
        if should_launch(&result) {
            if let Activation::Unavailable(reason) = &result {
                // Fails open: one compositor call failing must not
                // permanently refuse to launch this button. The cost is a
                // possible duplicate window, same trade-off the rest of
                // this module makes elsewhere.
                log::warn!(
                    "button {label:?}: could not check for an existing window ({reason}); \
                     launching anyway"
                );
            } else {
                log::info!("button {label:?}: no matching window; launching");
            }
            launch_singleton(argv, label, reporter, busy, app_id, argv_exits_quickly);
        } else {
            log::info!("button {label:?}: brought its window to the front");
            busy.set(false);
        }
    });
}

/// Whether `bring_to_front_or_launch` should launch after this `Activation`.
/// Pulled out on its own because it is the one piece of that function with a
/// meaningful branch to get wrong, and unlike the rest -- which needs a live
/// compositor -- this needs nothing but the enum.
fn should_launch(result: &Activation) -> bool {
    !matches!(result, Activation::Activated)
}

/// A `single_instance` launcher's actual launch, once the compositor check
/// found nothing to raise. Keeps `busy` set until the window it just started
/// actually appears (or a grace period elapses) -- not merely until `argv`
/// exits, because what that means differs by action kind. Clearing `busy`
/// too early reopens the exact bug `single_instance` exists to close: a
/// second press in the gap sees no window either, via the same check, and
/// starts a second copy -- reachable in the few seconds right after every
/// single press, which is exactly when an operator is likely to press again
/// because nothing is on screen yet.
fn launch_singleton<R: Reporter + Clone>(
    argv: Vec<String>,
    label: String,
    reporter: R,
    busy: Busy,
    app_id: String,
    argv_exits_quickly: bool,
) {
    log::info!("button {label:?}: exec {argv:?}");
    reporter.info(&format!("Starting {label}…"));

    let Some(proc) = spawn_or_report(&argv, &label, &reporter) else {
        busy.set(false);
        return;
    };

    let watch_window = {
        let label = label.clone();
        let busy = busy.clone();
        let reporter = reporter.clone();
        move || {
            log::info!("button {label:?}: child queued; waiting for its window");
            toplevels::wait_for_window_async(app_id, move |appeared| {
                if appeared {
                    log::info!("button {label:?}: window appeared");
                } else {
                    log::warn!(
                        "button {label:?}: no window appeared within the grace period; \
                         unlocking anyway"
                    );
                    // Otherwise "Starting {label}…" from above is the last
                    // thing the operator ever sees -- nothing supersedes it,
                    // so a slow or failed launch reads as one that is still
                    // in progress forever.
                    reporter.info(&format!("{label} is taking a while — check the logs"));
                }
                busy.set(false);
            });
        }
    };

    if argv_exits_quickly {
        // givc-app: argv is givc-cli, a proxy that exits once the unit is
        // queued in flatpak-vm, well before its surface is forwarded and
        // mapped -- waiting for it first is instant, and it is also the one
        // chance to report a launch request that failed outright (e.g.
        // flatpak-vm unreachable) before polling for a window that was never
        // going to appear.
        proc.wait_check_async(gio::Cancellable::NONE, move |result| match result {
            Ok(()) => watch_window(),
            Err(e) => {
                log::error!("button {label:?}: {e}");
                let msg = e.to_string();
                let brief: String = msg.chars().take(160).collect();
                reporter.error(&format!("{label} failed: {brief}"));
                busy.set(false);
            }
        });
    } else {
        // exec: argv IS the application, and does not exit until the
        // operator closes it -- waiting for that first would mean busy never
        // clears while the window is merely minimized. Poll for the window
        // immediately instead. Still watch the process, but only to log an
        // unexpectedly early exit; it does not gate anything, since
        // wait_for_window_async's own grace period is what unlocks the
        // button if the window never appears at all.
        proc.wait_check_async(gio::Cancellable::NONE, move |result| {
            if let Err(e) = result {
                log::warn!("button {label:?}: launch process exited abnormally: {e}");
            }
        });
        watch_window();
    }
}

/// Poll the GIVC registry until `job.app` is no longer listed, then say so.
///
/// Completion only, deliberately. givc's JSON reply carries `VMStatus`, whose
/// entire vocabulary is Running / PoweredOff / Paused -- there is no failure
/// state, so success cannot be distinguished from failure here. The only thing
/// that knows is `get-status`, which prints a Rust `Debug` struct with no
/// `--as-json`; scraping that would give a banner that lies the day upstream
/// reformats it. Better to report honestly what we can observe and leave the
/// detail to the journal.
/// `busy` is cleared on EVERY terminal branch, including the ones where we give
/// up. A button left busy is a button that never works again.
fn watch_job<R: Reporter + Clone>(job: AwaitJob, label: String, reporter: R, busy: Busy) {
    let mut waited = 0u32;
    glib::timeout_add_seconds_local(POLL_SECONDS, move || {
        waited += POLL_SECONDS;

        match still_running(&job) {
            Some(true) if waited < POLL_GIVE_UP_SECONDS => glib::ControlFlow::Continue,
            Some(true) => {
                log::warn!("button {label:?}: still running after {waited}s; no longer watching");
                reporter.info(&format!("{label} is still running — check the logs"));
                busy.set(false);
                glib::ControlFlow::Break
            }
            Some(false) => {
                log::info!("button {label:?}: job finished after {waited}s");
                reporter.info(&format!("{label} finished"));
                busy.set(false);
                glib::ControlFlow::Break
            }
            // Could not ask. Saying nothing is worse than saying we lost track:
            // the operator is left watching a banner that never resolves.
            None => {
                log::error!("button {label:?}: cannot query the GIVC registry; stopped watching");
                reporter.error(&format!("{label}: lost track of the job — check the logs"));
                busy.set(false);
                glib::ControlFlow::Break
            }
        }
    });
}

/// `Some(true)` still registered, `Some(false)` gone, `None` could not tell.
fn still_running(job: &AwaitJob) -> Option<bool> {
    let out = std::process::Command::new(job.query_argv.first()?)
        .args(&job.query_argv[1..])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    // Match on either field: `name` and `description` are documented upstream as
    // "VM name" and "App name, some details", and which one carries a GIVC
    // application's unit name is not something to guess at.
    let listed = parsed.as_array()?.iter().any(|e| {
        ["name", "description"]
            .iter()
            .filter_map(|k| e.get(*k)?.as_str())
            .any(|v| v.contains(&job.app))
    });
    Some(listed)
}

#[cfg(test)]
mod tests {
    use super::{should_launch, Activation};

    #[test]
    fn a_window_already_on_screen_is_not_launched_again() {
        assert!(!should_launch(&Activation::Activated));
    }

    #[test]
    fn no_matching_window_is_launched() {
        assert!(should_launch(&Activation::NotFound));
    }

    #[test]
    fn an_unreachable_compositor_fails_open_into_launching() {
        assert!(should_launch(&Activation::Unavailable(
            "no Wayland connection".to_owned()
        )));
    }
}
