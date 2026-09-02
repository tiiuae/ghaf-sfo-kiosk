// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// ghaf-sfo-kiosk -- a config-driven kiosk shell for Ghaf-based SFO laptops.
//
// A wlr-layer-shell surface on the BOTTOM layer, so it behaves as the desktop;
// see surface.rs. It knows nothing about "plan" or "launch" -- it renders the
// buttons in its config file. Adding a sixth is a change in ghaf-sfo-laptop.

mod actions;
mod banner;
mod config;
mod confirm;
mod outputs;
mod protocols;
mod radial;
mod shared;
mod status;
mod style;
mod surface;
mod toplevels;
mod ui;

use clap::Parser;
use gtk::prelude::*;

/// Serializes any test that pumps `glib::MainContext::default()`.
///
/// `glib::timeout_add_local` always posts to that one context, process-wide,
/// with no way to give it an isolated one -- so two tests that both drive it
/// concurrently (`cargo test` runs tests on separate threads) can race
/// `g_main_context_acquire` and panic with "already acquired by another
/// thread". Poisoning is ignored: one test panicking for an unrelated reason
/// must not cascade-fail the other lock holder.
#[cfg(test)]
pub(crate) static GLOBAL_MAIN_CONTEXT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Exit code used when the compositor will not give us layer-shell. Distinct
/// from a generic failure so the unit's status is self-explanatory.
const EXIT_NO_LAYER_SHELL: i32 = 3;

#[derive(Parser, Debug)]
#[command(about, version)]
struct Args {
    /// The generated kiosk configuration.
    #[arg(
        short,
        long,
        env = "GHAF_SFO_KIOSK_CONFIG",
        default_value = "/etc/sfo-kiosk/config.json"
    )]
    config: std::path::PathBuf,

    /// Log level: error, warn, info, debug, trace.
    #[arg(long, env = "GHAF_SFO_KIOSK_LOG", default_value = "info")]
    log_level: String,

    /// Parse the configuration, report what it contains, and exit without
    /// opening a surface.
    ///
    /// For a human holding a generated config and asking "would the kiosk
    /// accept this, and how many buttons would actually work?". It is
    /// deliberately NOT wired into ghaf-sfo-laptop's `nix flake check`: that
    /// would put a Rust compile into a check set whose whole contract is to
    /// stay cheap enough to run on a stock CI runner. The structural checks
    /// there (kiosk-buttons-name-real-apps, kiosk-config-follows-enable)
    /// validate the same config in nix, without building anything.
    #[arg(long)]
    check: bool,
}

fn main() -> std::process::ExitCode {
    let args = Args::parse();
    env_logger::Builder::new()
        .parse_filters(&args.log_level)
        .format_timestamp(None) // the journal already timestamps
        .init();

    let kiosk = match config::load(&args.config) {
        Ok(k) => k,
        Err(e) => {
            // Fatal on purpose. A kiosk that starts with no buttons hides the
            // desktop and offers nothing -- failing loudly into the journal and
            // letting the desktop show through is strictly better.
            log::error!("{e:#}");
            return std::process::ExitCode::FAILURE;
        }
    };

    if args.check {
        // Grid and menus separately: a button in the wrong one of the two is on
        // screen either way, so a bare total answers nothing.
        let in_menus: usize = kiosk.menus.iter().map(|m| m.items.len()).sum();
        let unconfigured = kiosk
            .buttons
            .iter()
            .chain(kiosk.menus.iter().flat_map(|m| &m.items))
            .filter(|b| matches!(b.action, config::Action::Unsupported { .. }))
            .count();
        // Which buttons ask first is a product decision someone has to be able
        // to check against a generated config, without a display.
        let confirmed = kiosk
            .buttons
            .iter()
            .chain(kiosk.menus.iter().flat_map(|m| &m.items))
            .filter(|b| b.confirm.is_some())
            .count();
        println!(
            "ok: {} button(s) in the grid, {} in {} menu(s), {} unconfigured, {} confirmed",
            kiosk.buttons.len(),
            in_menus,
            kiosk.menus.len(),
            unconfigured,
            confirmed,
        );
        return std::process::ExitCode::SUCCESS;
    }

    // gtk::init before touching anything Wayland: is_supported() needs a display
    // connection to answer.
    if let Err(e) = gtk::init() {
        log::error!("could not initialise GTK: {e}");
        return std::process::ExitCode::FAILURE;
    }

    // The guard. Without it, a Wayland security context, an X11 fallback or a
    // bad library link order silently produces an ordinary window with a
    // titlebar -- in the alt-tab list, closable with Alt+F4 -- which looks like
    // the kiosk half-working and costs a day to diagnose on the device.
    if !surface::supported() {
        log::error!(
            "the compositor is not offering zwlr_layer_shell_v1 \
             (protocol_version={}). Refusing to start as an ordinary window. \
             Check that this is running natively in gui-vm and not through waypipe: \
             cosmic-comp hides layer-shell from any client carrying a Wayland \
             security context.",
            surface::protocol_version()
        );
        return std::process::ExitCode::from(u8::try_from(EXIT_NO_LAYER_SHELL).unwrap_or(1));
    }
    log::info!(
        "layer-shell available, protocol version {}",
        surface::protocol_version()
    );

    let app = gtk::Application::builder()
        .application_id("ae.tii.ghaf.SfoKiosk")
        // The kiosk owns the session; a second instance would fight the first
        // for the surface, so let GTK enforce singleton behaviour.
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let kiosk = std::rc::Rc::new(kiosk);
    app.connect_activate(move |app| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(style::CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            // Added second and at the same priority, so it overrides the
            // defaults above by order rather than by specificity. Empty when no
            // button asks for a colour, which costs nothing.
            let per_button = style::per_button_css(&kiosk);
            if !per_button.is_empty() {
                let overrides = gtk::CssProvider::new();
                overrides.load_from_string(&per_button);
                gtk::style_context_add_provider_for_display(
                    &display,
                    &overrides,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
        }
        outputs::manage(app, kiosk.clone());
    });

    // No SIGTERM handler on purpose.
    //
    // glib 0.22 does not expose unix_signal_add, and we do not need it: the
    // COSMIC panel and shortcuts are restored by the systemd unit's
    // ExecStopPost, which runs however the main process died -- clean exit,
    // SIGTERM, or SIGKILL. Putting the restore in a Rust signal handler would
    // cover strictly fewer cases while looking like it covered more.
    //
    // run_with_args with an empty list: clap has already consumed argv, and GTK
    // would otherwise reject our own flags.
    let status = app.run_with_args::<&str>(&[]);
    log::info!("exiting");
    if status == gtk::glib::ExitCode::SUCCESS {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
