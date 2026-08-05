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

use crate::config::Action;

/// How the caller is told what happened.
pub trait Reporter: 'static {
    fn info(&self, message: &str);
    fn error(&self, message: &str);
}

/// Run `action`, reporting progress and outcome through `reporter`.
pub fn dispatch<R: Reporter + Clone>(action: &Action, label: &str, reporter: &R) {
    let argv: Vec<String> = match action {
        Action::Exec { argv } => argv.clone(),
        Action::Givc { argv, target } => {
            log::info!("button {label:?} targets {target}");
            argv.clone()
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

    let reporter = reporter.clone();
    let label = label.to_owned();
    proc.wait_check_async(gio::Cancellable::NONE, move |result| match result {
        Ok(()) => {
            // Either it did its job and exited, or the operator closed the
            // window. Both are unremarkable, and a banner here would pop up
            // long after the press that caused it.
            log::info!("button {label:?}: child exited cleanly");
        }
        Err(e) => {
            log::error!("button {label:?}: {e}");
            // Keep the banner readable; the journal has the child's own output.
            let msg = e.to_string();
            let brief: String = msg.chars().take(160).collect();
            reporter.error(&format!("{label} failed: {brief}"));
        }
    });
}
