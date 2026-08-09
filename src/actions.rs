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

use crate::config::{Action, AwaitJob};

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

/// Run `action`, reporting progress and outcome through `reporter`.
pub fn dispatch<R: Reporter + Clone>(action: &Action, label: &str, reporter: &R, busy: &Busy) {
    let mut await_job: Option<AwaitJob> = None;
    let argv: Vec<String> = match action {
        Action::Exec { argv } => argv.clone(),
        Action::Givc {
            argv,
            target,
            await_job: job,
        } => {
            log::info!("button {label:?} targets {target}");
            await_job = job.clone();
            argv.clone()
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

    // A press while the last run is still going is a press the operator has
    // already made. Say so rather than stacking another copy.
    if busy.get() {
        log::info!("button {label:?} pressed while its previous run is still going; ignoring");
        reporter.info(&format!("{label} is already open"));
        return;
    }

    log::info!("button {:?}: exec {:?}", label, argv);
    reporter.info(&format!("Starting {label}…"));

    let refs: Vec<&std::ffi::OsStr> = argv.iter().map(std::ffi::OsStr::new).collect();
    // NONE: inherit stdout and stderr, so the child's own diagnostics go to the
    // journal rather than into a buffer only we can see.
    let proc = match gio::Subprocess::newv(&refs, gio::SubprocessFlags::NONE) {
        Ok(p) => p,
        Err(e) => {
            // Almost always "no such file": a store path that did not make it
            // into the VM, or a typo in the nix module.
            log::error!("button {label:?}: spawn failed: {e}; argv was {argv:?}");
            reporter.error(&format!("{label} could not start: {e}"));
            return;
        }
    };

    busy.set(true);

    let reporter = reporter.clone();
    let label = label.to_owned();
    let busy = busy.clone();
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
