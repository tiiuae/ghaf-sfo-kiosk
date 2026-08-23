// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// The contract between two repositories that bump independently: ghaf-sfo-laptop
// generates this file, the kiosk reads it. `version` matters -- a field added on
// the nix side and read by an older binary shows up as "the button does nothing".
//
// One bad button must not take the kiosk down: a malformed action becomes
// `Action::Unsupported`, which still renders and says why when pressed. Only a
// malformed file is fatal.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

/// The highest `version` this binary understands.
const SUPPORTED_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub version: u32,
    #[serde(default = "default_title")]
    pub title: String,
    /// Shown at the left of the status bar instead of `title`. Absolute path or
    /// icon name, same rule as a button's `icon`.
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub status_bar: StatusBar,
    #[serde(default)]
    pub layout: Layout,
    pub buttons: Vec<Button>,
}

fn default_title() -> String {
    "SFO".to_owned()
}

#[derive(Debug, Deserialize)]
pub struct StatusBar {
    #[serde(default = "default_clock_format")]
    pub clock_format: String,
    #[serde(default = "yes")]
    pub show_battery: bool,
    #[serde(default = "yes")]
    pub show_network: bool,
}

fn default_clock_format() -> String {
    "%H:%M".to_owned()
}
const fn yes() -> bool {
    true
}

impl Default for StatusBar {
    fn default() -> Self {
        Self {
            clock_format: default_clock_format(),
            show_battery: true,
            show_network: true,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Layout {
    #[serde(default = "default_columns")]
    pub columns: u32,
}

const fn default_columns() -> u32 {
    3
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            columns: default_columns(),
        }
    }
}

// No ExitButton: leaving the kiosk is a keybinding, not a widget -- see ui.rs.
// An `exit` object from an older producer is an unknown field, so serde ignores
// it and the contract stays at version 1.

#[derive(Debug, Deserialize)]
pub struct Button {
    /// Stable key. Appears in the journal and as a CSS class; never shown to the
    /// operator, so it stays put when the label changes.
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    /// Tint for this button's icon, as `#rrggbb`. Only meaningful for a
    /// symbolic icon, which GTK recolours to the widget's `color`; a full-colour
    /// icon or a file path ignores it.
    ///
    /// A product decision, so it arrives in config rather than living in
    /// style.rs -- which also means changing it needs no rebuild of this binary.
    #[serde(default)]
    pub icon_color: Option<String>,
    /// Render this button in the named menu rather than the grid. The value is
    /// another button's `id`, whose action kind must be `menu`.
    ///
    /// Flat, not a `children` list on the trigger: ghaf-sfo-laptop's
    /// `checks.kiosk-buttons-name-real-apps` walks this array flat, and nesting
    /// would drop menu members out of the check that proves a GIVC button names
    /// something real.
    #[serde(default)]
    pub menu: Option<String>,
    #[serde(default)]
    pub action: RawAction,
}

/// Deliberately permissive: `kind` plus whatever else came along. Unknown fields
/// are kept rather than rejected, so a newer nix module adding a field to an
/// existing action kind does not brick an older binary.
#[derive(Debug, Default, Deserialize)]
pub struct RawAction {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub vm: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Absolute path to givc-cli plus its connection arguments, already
    /// resolved by the nix module. A store path on purpose: givc-cli is only on
    /// $PATH in gui-vm on debug images.
    #[serde(default)]
    pub givc_cli: Vec<String>,
    /// This button runs a job that ends, so wait for it rather than reporting
    /// success the moment givc-cli queues it. Absent in older configs, hence
    /// the default.
    #[serde(default)]
    pub await_completion: bool,
    /// This button launches something that stays open indefinitely. A repeat
    /// press asks cosmic-comp directly whether a toplevel with `window_app_id`
    /// already exists rather than starting a second one -- see that field.
    /// GIVC cannot answer this itself: it only ever tracks a launch as a
    /// generic, numbered `run-flatpak-app@N` unit, and that number is a
    /// reused slot, not an identity -- closing one app and opening another can
    /// hand the second the first's old number. Mutually exclusive with
    /// `await_completion`: one is for a job that ends, the other for one that
    /// does not.
    #[serde(default)]
    pub single_instance: bool,
    /// This launcher's actual Wayland `app_id`, exactly as cosmic-comp
    /// reports it -- required whenever `single_instance` is set, since it is
    /// the only thing that answers "is this app's window already open".
    /// Not derivable from `app` or `args`: a flatpak's Wayland `app_id` is set
    /// by its own toolkit and is routinely unrelated to its GIVC/flatpak
    /// name -- Barq's, for instance, is "BARQ Ground Control Station", not
    /// "org.barq.Barq". Matched exactly, not as a substring: "Plan" must not
    /// raise "Mission Planner".
    #[serde(default)]
    pub window_app_id: Option<String>,
}

/// How to tell that a GIVC job has finished. Absent for a launcher button.
#[derive(Debug, Clone)]
pub struct AwaitJob {
    /// `<givc_cli…> query --as-json`.
    ///
    /// Deliberately without `--by-name`: givc accepts that argument and ignores
    /// it — `query()` in its client takes `_by_name` and calls `query_list()` —
    /// so a filtered request quietly returns everything. Passing a filter that
    /// does nothing would read as working. We match here instead.
    pub query_argv: Vec<String>,
    /// Registry entries are matched against this.
    pub app: String,
}

/// How to ask whether a previously-launched singleton is still running.
///
/// No GIVC query here at all, unlike `AwaitJob`: GIVC's `run-flatpak-app@N`
/// unit numbers are reused slots, not identities, so asking it "is `app`
/// running" cannot be answered soundly for this case -- see
/// `RawAction::single_instance`. The compositor is asked directly instead,
/// at press time, in `actions::bring_to_front_or_launch`.
#[derive(Debug, Clone)]
pub struct SingleInstance {
    /// See `RawAction::window_app_id`. Required: without it there is nothing
    /// to ask the compositor.
    pub window_app_id: String,
}

#[derive(Debug, Clone)]
pub enum Action {
    /// Run a command locally in the gui-vm. argv form only: there is no shell,
    /// so there is no quoting to get wrong and nothing to inject into.
    Exec { argv: Vec<String> },
    /// Start a declared GIVC application in another VM. Already fully resolved
    /// to an argv by the nix module. `target` is "<app> in <vm>", for the log:
    /// the button's own label says what the operator wanted, `target` says
    /// where it actually went, and when a button silently does nothing those
    /// are the two facts you need side by side.
    ///
    /// `await_job` is set only for a button that runs a job which ENDS.
    /// `givc-cli start app` returns 0 as soon as the unit is queued, so without
    /// it a button reports success before the work begins — and a launcher must
    /// never be awaited, because its unit stays active for as long as the
    /// application runs.
    Givc {
        argv: Vec<String>,
        target: String,
        await_job: Option<AwaitJob>,
        /// Set only for a launcher whose repeat presses must not stack a
        /// second window. Mutually exclusive with `await_job` by construction
        /// -- see `single_instance` on `RawAction`.
        single_instance: Option<SingleInstance>,
    },
    /// Not a command: a menu trigger. Renders in a screen corner and fans the
    /// buttons naming it out along an arc.
    ///
    /// Layout, not product -- it says where buttons go, never what they mean,
    /// which is why the application may interpret it.
    Menu,
    /// Parsed, but not executable. Still renders; says why when pressed.
    Unsupported { reason: String },
}

/// `givc-cli start …` returns as soon as the unit is queued, so a button that runs
/// a job which ENDS needs something to poll. Shared by both GIVC kinds; the string
/// matched against the registry is the application id for `app`, the unit name for
/// `service`.
fn await_job_for(raw: &RawAction, name: &str) -> Option<AwaitJob> {
    raw.await_completion.then(|| {
        let mut query_argv = raw.givc_cli.clone();
        query_argv.extend(["query".to_owned(), "--as-json".to_owned()]);
        AwaitJob {
            query_argv,
            app: name.to_owned(),
        }
    })
}

impl Action {
    fn from_raw(raw: &RawAction) -> Self {
        match raw.kind.as_str() {
            "exec" => {
                if raw.argv.is_empty() {
                    Self::Unsupported {
                        reason: "action kind \"exec\" has an empty argv".to_owned(),
                    }
                } else {
                    Self::Exec {
                        argv: raw.argv.clone(),
                    }
                }
            }
            "givc-app" => {
                let (Some(vm), Some(app)) = (raw.vm.as_ref(), raw.app.as_ref()) else {
                    return Self::Unsupported {
                        reason: "action kind \"givc-app\" needs both \"vm\" and \"app\"".to_owned(),
                    };
                };
                if raw.givc_cli.is_empty() {
                    return Self::Unsupported {
                        reason: "action kind \"givc-app\" has an empty \"givc_cli\"".to_owned(),
                    };
                }
                // One is for a job that ends, the other for one that does not;
                // a button cannot be both.
                if raw.await_completion && raw.single_instance {
                    return Self::Unsupported {
                        reason: "action kind \"givc-app\" cannot set both \"await_completion\" \
                                 and \"single_instance\""
                            .to_owned(),
                    };
                }
                // Without it there is nothing to ask the compositor -- see
                // SingleInstance's own doc comment. Matched here, not
                // unwrapped in a helper elsewhere: every other malformed
                // config in this file degrades to Unsupported rather than
                // panicking, and a `window_app_id` obtained any way other
                // than this match arm proving it Some would be one too.
                let single_instance = match (raw.single_instance, raw.window_app_id.as_ref()) {
                    (true, Some(window_app_id)) => Some(SingleInstance {
                        window_app_id: window_app_id.clone(),
                    }),
                    (true, None) => {
                        return Self::Unsupported {
                            reason: "action kind \"givc-app\" sets \"single_instance\" without \
                                     \"window_app_id\", which is required to tell whether the \
                                     window is already open"
                                .to_owned(),
                        };
                    }
                    (false, _) => None,
                };
                let mut argv = raw.givc_cli.clone();
                argv.extend(["start".to_owned(), "app".to_owned()]);
                argv.extend(["--vm".to_owned(), vm.clone()]);
                argv.push(app.clone());
                if !raw.args.is_empty() {
                    argv.push("--".to_owned());
                    argv.extend(raw.args.iter().cloned());
                }
                Self::Givc {
                    argv,
                    target: format!("{app} in {vm}"),
                    await_job: await_job_for(raw, app),
                    single_instance,
                }
            }
            // A systemd user unit exposed through givc.appvm.capabilities.services.
            // A separate givc-cli subcommand, not a variant of `start app`.
            "givc-service" => {
                let (Some(vm), Some(unit)) = (raw.vm.as_ref(), raw.app.as_ref()) else {
                    return Self::Unsupported {
                        reason: "action kind \"givc-service\" needs both \"vm\" and \"app\""
                            .to_owned(),
                    };
                };
                if raw.givc_cli.is_empty() {
                    return Self::Unsupported {
                        reason: "action kind \"givc-service\" has an empty \"givc_cli\"".to_owned(),
                    };
                }
                // `start service --vm <VM> <SERVICENAME>` has no `--` form, so args
                // would be dropped by the CLI rather than rejected.
                if !raw.args.is_empty() {
                    return Self::Unsupported {
                        reason: "action kind \"givc-service\" takes no args".to_owned(),
                    };
                }
                // A service has no window to avoid duplicating; that concept
                // only applies to a launcher.
                if raw.single_instance {
                    return Self::Unsupported {
                        reason: "action kind \"givc-service\" does not support \"single_instance\""
                            .to_owned(),
                    };
                }
                let mut argv = raw.givc_cli.clone();
                argv.extend(["start".to_owned(), "service".to_owned()]);
                argv.extend(["--vm".to_owned(), vm.clone()]);
                argv.push(unit.clone());
                Self::Givc {
                    argv,
                    target: format!("{unit} in {vm}"),
                    await_job: await_job_for(raw, unit),
                    single_instance: None,
                }
            }
            // Permissive about the other fields: the nix module already asserts
            // a trigger carries no exec, vm, app or args, and refusing here too
            // would dim the one button whose failure hides all its members.
            "menu" => Self::Menu,
            "" => Self::Unsupported {
                reason: "no action configured".to_owned(),
            },
            other => Self::Unsupported {
                reason: format!("unknown action kind {other:?}"),
            },
        }
    }
}

/// A button with its action already resolved.
pub struct ResolvedButton {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    /// Validated `#rrggbb`, or None. Anything malformed was dropped at load with
    /// a warning, so this is safe to paste into a stylesheet.
    pub icon_color: Option<String>,
    pub action: Action,
}

/// `#rrggbb`, lowercase or upper, nothing else.
///
/// This value is interpolated into a stylesheet, so it is validated rather than
/// trusted: a stray `}` would silently break every rule after it, and the symptom
/// would be a differently-wrong colour somewhere else entirely.
fn valid_hex_colour(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// A trigger and its buttons, in config order -- also their order on the arc,
/// nearest the corner first.
pub struct Menu {
    pub trigger: ResolvedButton,
    pub items: Vec<ResolvedButton>,
}

pub struct Kiosk {
    pub title: String,
    pub logo: Option<String>,
    pub status_bar: StatusBar,
    pub layout: Layout,
    /// Grid buttons: everything that is not a trigger and not in a menu.
    pub buttons: Vec<ResolvedButton>,
    /// The menus, in config order.
    pub menus: Vec<Menu>,
}

pub fn load(path: &Path) -> Result<Kiosk> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading kiosk config {}", path.display()))?;
    let cfg: Config = serde_json::from_str(&text)
        .with_context(|| format!("parsing kiosk config {}", path.display()))?;

    if cfg.version > SUPPORTED_VERSION {
        bail!(
            "kiosk config {} declares version {}, but this build understands at most {}. \
             The nix module and the kiosk binary are out of step -- bump the kiosk.",
            path.display(),
            cfg.version,
            SUPPORTED_VERSION
        );
    }
    if cfg.buttons.is_empty() {
        bail!(
            "kiosk config {} declares no buttons. Refusing to start: a kiosk with \
             nothing on it hides the desktop and offers no way to do anything.",
            path.display()
        );
    }

    let resolved = cfg
        .buttons
        .iter()
        .map(|b| {
            let action = Action::from_raw(&b.action);
            if let Action::Unsupported { reason } = &action {
                log::warn!("button {:?}: {}", b.id, reason);
            }
            // Dropped, not fatal: a bad colour is a cosmetic mistake, and
            // refusing to start over one would hide every working button behind
            // it. Same call as one_bad_button_does_not_kill_the_others.
            let icon_color = b.icon_color.as_ref().and_then(|c| {
                if valid_hex_colour(c) {
                    Some(c.clone())
                } else {
                    log::warn!(
                        "button {:?}: ignoring icon_color {:?}, expected #rrggbb",
                        b.id,
                        c
                    );
                    None
                }
            });
            (
                b.menu.clone(),
                ResolvedButton {
                    id: b.id.clone(),
                    label: b.label.clone(),
                    description: b.description.clone(),
                    icon: b.icon.clone(),
                    icon_color,
                    action,
                },
            )
        })
        .collect();

    let (buttons, menus) = partition(resolved);

    if buttons.is_empty() {
        // Not fatal: a corner that fans out still offers the operator something,
        // which is where the fatal line is drawn. ghaf-sfo-laptop asserts
        // against it, so reaching here means a hand-written config.
        log::warn!(
            "every button is inside a menu; the kiosk will show a bare corner \
             trigger and no grid"
        );
    }

    Ok(Kiosk {
        title: cfg.title,
        logo: cfg.logo,
        status_bar: cfg.status_bar,
        layout: cfg.layout,
        buttons,
        menus,
    })
}

/// Split the resolved buttons into the grid and the menus.
///
/// Forgiving like a malformed action: a `menu` naming something that is not a
/// trigger puts its button back in the grid rather than losing it. A visible
/// button is better evidence of a misconfiguration than an absent one.
fn partition(
    resolved: Vec<(Option<String>, ResolvedButton)>,
) -> (Vec<ResolvedButton>, Vec<Menu>) {
    // Triggers first, so a member may be declared before the menu it names --
    // the nix module sorts by `order`, which need not put a trigger first.
    let mut menus: Vec<Menu> = Vec::new();
    let mut members: Vec<(Option<String>, ResolvedButton)> = Vec::new();

    for (menu, button) in resolved {
        if matches!(button.action, Action::Menu) {
            if menu.is_some() {
                // Menus do not nest: an arc from a point already on an arc has
                // nowhere to go that is still in the corner.
                log::warn!(
                    "button {:?} is a menu trigger, so its own \"menu\" is ignored",
                    button.id
                );
            }
            menus.push(Menu {
                trigger: button,
                items: Vec::new(),
            });
        } else {
            members.push((menu, button));
        }
    }

    let mut buttons = Vec::new();
    for (menu, button) in members {
        let Some(want) = menu else {
            buttons.push(button);
            continue;
        };
        match menus.iter_mut().find(|m| m.trigger.id == want) {
            Some(m) => m.items.push(button),
            None => {
                log::warn!(
                    "button {:?} names menu {:?}, which is not a button with action kind \
                     \"menu\"; rendering it in the grid instead",
                    button.id,
                    want
                );
                buttons.push(button);
            }
        }
    }

    // A trigger with nothing behind it does nothing when pressed, which reads
    // as a broken kiosk. Exit used to count as a member; it is gone, so a menu
    // needs a real button of its own.
    menus.retain(|m| {
        let keep = !m.items.is_empty();
        if !keep {
            log::warn!(
                "menu {:?} has no members; not rendering its trigger",
                m.trigger.id
            );
        }
        keep
    });

    (buttons, menus)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each call gets its own file: cargo runs tests in parallel, and a shared
    /// path means one test deletes the file another is still reading.
    fn parse(json: &str) -> Result<Kiosk> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);

        let dir = std::env::temp_dir().join(format!("kiosk-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!(
            "kiosk-{}.json",
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&p, json).unwrap();
        let r = load(&p);
        let _ = std::fs::remove_file(&p);
        r
    }

    #[test]
    fn the_shipped_example_parses_with_every_action_supported() {
        let example = include_str!("../examples/sfo.json");
        let dir = std::env::temp_dir().join("kiosk-test-example");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("sfo.json");
        std::fs::write(&p, example).unwrap();
        let k = load(&p).expect("examples/sfo.json must parse");

        // The SPLIT, not a bare count: a regression sweeping every button into
        // the menu, or none of them, still totals seven.
        assert_eq!(
            k.buttons.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(),
            ["plan", "launch", "clear"],
            "the mission workflow stays in the grid"
        );
        assert_eq!(k.menus.len(), 1, "one corner menu");
        assert_eq!(k.menus[0].trigger.id, "settings");
        assert_eq!(
            k.menus[0]
                .items
                .iter()
                .map(|b| b.id.as_str())
                .collect::<Vec<_>>(),
            ["network", "update", "power"],
            "arc order is config order, nearest the corner first"
        );
        for b in k.buttons.iter().chain(&k.menus[0].items) {
            assert!(
                !matches!(b.action, Action::Unsupported { .. }),
                "button {:?} did not resolve",
                b.id
            );
        }
    }

    /// A trigger runs nothing; it is a place to put buttons.
    #[test]
    fn a_menu_kind_resolves_to_a_trigger_and_takes_its_members() {
        let k = parse(
            r#"{"version":1,"buttons":[
                 {"id":"a","label":"A","action":{"kind":"exec","argv":["true"]}},
                 {"id":"gear","label":"Settings","action":{"kind":"menu"}},
                 {"id":"b","label":"B","menu":"gear","action":{"kind":"exec","argv":["true"]}}]}"#,
        )
        .unwrap();
        assert_eq!(k.buttons.len(), 1);
        assert_eq!(k.buttons[0].id, "a");
        assert_eq!(k.menus.len(), 1);
        assert!(matches!(k.menus[0].trigger.action, Action::Menu));
        assert_eq!(k.menus[0].items.len(), 1);
        assert_eq!(k.menus[0].items[0].id, "b");
    }

    /// The nix module sorts by `order`, and nothing makes a trigger sort ahead
    /// of its own members.
    #[test]
    fn a_member_may_be_declared_before_the_menu_it_names() {
        let k = parse(
            r#"{"version":1,"buttons":[
                 {"id":"b","label":"B","menu":"gear","action":{"kind":"exec","argv":["true"]}},
                 {"id":"gear","label":"Settings","action":{"kind":"menu"}},
                 {"id":"a","label":"A","action":{"kind":"exec","argv":["true"]}}]}"#,
        )
        .unwrap();
        assert_eq!(k.menus.len(), 1);
        assert_eq!(k.menus[0].items.len(), 1, "the member found its menu");
        assert_eq!(k.buttons.len(), 1);
    }

    /// Falling back to the grid, not vanishing. A button the operator can see
    /// and press is better evidence of a misconfiguration than one that is
    /// silently absent.
    #[test]
    fn a_menu_naming_nothing_puts_its_button_back_in_the_grid() {
        let k = parse(
            r#"{"version":1,"buttons":[
                 {"id":"a","label":"A","menu":"nowhere","action":{"kind":"exec","argv":["true"]}},
                 {"id":"b","label":"B","menu":"a","action":{"kind":"exec","argv":["true"]}}]}"#,
        )
        .unwrap();
        // "nowhere" does not exist; "a" exists but is not a trigger. Both land
        // in the grid rather than disappearing.
        assert_eq!(
            k.buttons.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(k.menus.is_empty());
    }

    /// A cog that opens onto nothing reads as a broken kiosk.
    #[test]
    fn a_menu_with_no_members_is_not_rendered() {
        let k = parse(
            r#"{"version":1,"buttons":[
                 {"id":"gear","label":"Settings","action":{"kind":"menu"}},
                 {"id":"a","label":"A","action":{"kind":"exec","argv":["true"]}}]}"#,
        )
        .unwrap();
        assert!(k.menus.is_empty(), "an empty menu drops its trigger");
        assert_eq!(k.buttons.len(), 1, "the trigger is not demoted to the grid");
    }

    /// A menu could once exist holding exit and nothing else. Exit is gone, so
    /// such a trigger is now dropped.
    #[test]
    fn a_menu_that_only_held_exit_is_dropped() {
        let k = parse(
            r#"{"version":1,
                "buttons":[
                  {"id":"gear","label":"Settings","action":{"kind":"menu"}},
                  {"id":"a","label":"A","action":{"kind":"exec","argv":["true"]}}],
                "exit":{"menu":"gear"}}"#,
        )
        .unwrap();
        assert!(k.menus.is_empty(), "a member-less trigger is not rendered");
        assert_eq!(k.buttons.len(), 1, "the trigger is not demoted to the grid");
    }

    /// A logo replaces the title text; absent means the text is kept.
    #[test]
    fn a_logo_is_optional_and_survives_to_the_kiosk() {
        let with = parse(
            r#"{"version":1,"title":"SFO","logo":"/nix/store/x/ghaf-logo-512px.png",
                "buttons":[{"id":"a","label":"A","action":{"kind":"exec","argv":["true"]}}]}"#,
        )
        .unwrap();
        assert_eq!(with.logo.as_deref(), Some("/nix/store/x/ghaf-logo-512px.png"));
        assert_eq!(with.title, "SFO", "the title is kept as the tooltip");

        let without = parse(
            r#"{"version":1,"title":"SFO",
                "buttons":[{"id":"a","label":"A","action":{"kind":"exec","argv":["true"]}}]}"#,
        )
        .unwrap();
        assert_eq!(without.logo, None);
    }

    /// A stale `exit` object is an unknown field, and unknown fields are
    /// ignored -- which is what keeps the contract at version 1.
    #[test]
    fn a_stale_exit_object_is_ignored_rather_than_fatal() {
        let k = parse(
            r#"{"version":1,
                "buttons":[{"id":"a","label":"A","action":{"kind":"exec","argv":["true"]}}],
                "exit":{"label":"Exit kiosk","icon":"application-exit-symbolic","menu":"gear"}}"#,
        )
        .unwrap();
        assert_eq!(k.buttons.len(), 1);
        assert!(k.menus.is_empty());
    }

    /// An arc drawn from a point already on an arc has nowhere to go that is
    /// still in the corner.
    #[test]
    fn menus_do_not_nest() {
        let k = parse(
            r#"{"version":1,"buttons":[
                 {"id":"outer","label":"O","action":{"kind":"menu"}},
                 {"id":"inner","label":"I","menu":"outer","action":{"kind":"menu"}},
                 {"id":"a","label":"A","menu":"inner","action":{"kind":"exec","argv":["true"]}}]}"#,
        )
        .unwrap();
        // `inner` is a trigger, so it never becomes a member of `outer` -- which
        // leaves `outer` with nothing behind it, and an empty menu is dropped.
        assert_eq!(k.menus.len(), 1);
        assert_eq!(k.menus[0].trigger.id, "inner");
        assert_eq!(k.menus[0].items[0].id, "a");
        assert!(
            k.buttons.is_empty(),
            "a trigger is never demoted to the grid"
        );
    }

    /// The version-1 promise, in the direction that matters: a config written
    /// for the radial build, read by a binary that predates it.
    #[test]
    fn an_older_binary_would_still_see_every_button() {
        // Standing in for that binary: `menu` unknown, so ignored; `kind:"menu"`
        // unknown, so one dimmed button. Nothing else changes.
        let k = parse(
            r#"{"version":1,"buttons":[
                 {"id":"gear","label":"Settings","action":{"kind":"future-menu"}},
                 {"id":"a","label":"A","menu":"gear","action":{"kind":"exec","argv":["true"]}}]}"#,
        )
        .unwrap();
        assert_eq!(k.buttons.len(), 2, "both render");
        assert!(matches!(k.buttons[0].action, Action::Unsupported { .. }));
        assert!(matches!(k.buttons[1].action, Action::Exec { .. }));
    }

    #[test]
    fn an_icon_colour_survives_to_the_resolved_button() {
        // r##: a colour literal contains `"#`, which would close an r#"..."# here.
        let k = parse(
            r##"{"version":1,"buttons":[
                 {"id":"plan","label":"Plan","icon":"mark-location-symbolic",
                  "icon_color":"#ff8a80","action":{"kind":"exec","argv":["true"]}}]}"##,
        )
        .unwrap();
        assert_eq!(k.buttons[0].icon_color.as_deref(), Some("#ff8a80"));
    }

    #[test]
    fn a_button_without_a_colour_is_still_fine() {
        // The field is optional; every button predating it must keep working.
        let k = parse(
            r#"{"version":1,"buttons":[
                 {"id":"a","label":"A","action":{"kind":"exec","argv":["true"]}}]}"#,
        )
        .unwrap();
        assert!(k.buttons[0].icon_color.is_none());
    }

    #[test]
    fn a_malformed_colour_is_dropped_rather_than_pasted_into_css() {
        // The value is interpolated into a stylesheet, so anything that could
        // close a rule early has to be refused. Cosmetic, so the button lives.
        for bad in [
            "red",
            "#fff",
            "#gggggg",
            "#ff8a80; } * { color: red",
            "",
            "#ff8a8080",
        ] {
            let json = format!(
                r#"{{"version":1,"buttons":[
                     {{"id":"a","label":"A","icon_color":{},
                       "action":{{"kind":"exec","argv":["true"]}}}}]}}"#,
                serde_json::to_string(bad).unwrap()
            );
            let k = parse(&json).expect("a bad colour must not fail the whole file");
            assert!(
                k.buttons[0].icon_color.is_none(),
                "expected {bad:?} to be rejected"
            );
            assert!(matches!(k.buttons[0].action, Action::Exec { .. }));
        }
    }

    #[test]
    fn only_buttons_that_ask_for_a_colour_produce_a_rule() {
        let k = parse(
            r##"{"version":1,"buttons":[
                 {"id":"plan","label":"Plan","icon_color":"#ff8a80",
                  "action":{"kind":"exec","argv":["true"]}},
                 {"id":"launch","label":"Launch",
                  "action":{"kind":"exec","argv":["true"]}}]}"##,
        )
        .unwrap();
        let css = crate::style::per_button_css(&k);
        assert_eq!(
            css.trim(),
            ".kiosk-button-plan .kiosk-button-icon { color: #ff8a80; }"
        );
    }

    #[test]
    fn a_menu_member_is_coloured_through_its_radial_class() {
        // The grid and the arc are different widgets with different class
        // prefixes; a colour must follow the button to whichever it lands on.
        let k = parse(
            r##"{"version":1,"buttons":[
                 {"id":"gear","label":"Settings","action":{"kind":"menu"}},
                 {"id":"power","label":"Power","menu":"gear","icon_color":"#ffc246",
                  "action":{"kind":"exec","argv":["true"]}}]}"##,
        )
        .unwrap();
        let css = crate::style::per_button_css(&k);
        assert_eq!(
            css.trim(),
            ".kiosk-radial-item-power .kiosk-radial-item-icon { color: #ffc246; }"
        );
    }

    #[test]
    fn one_bad_button_does_not_kill_the_others() {
        let k = parse(
            r#"{"version":1,"buttons":[
                 {"id":"a","label":"A","action":{"kind":"nonsense"}},
                 {"id":"b","label":"B","action":{"kind":"exec","argv":["true"]}}]}"#,
        )
        .expect("a bad button must not fail the whole file");
        assert!(matches!(k.buttons[0].action, Action::Unsupported { .. }));
        assert!(matches!(k.buttons[1].action, Action::Exec { .. }));
    }

    #[test]
    fn givc_argv_is_built_exactly_as_ghaf_launchers_do() {
        let k = parse(
            r#"{"version":1,"buttons":[{"id":"p","label":"P","action":{
                 "kind":"givc-app","vm":"flatpak-vm","app":"run-flatpak-app",
                 "args":["http://org.example.Plan"],
                 "givc_cli":["/nix/store/x-givc-cli/bin/givc-cli","--name","admin-vm"]}}]}"#,
        )
        .unwrap();
        let Action::Givc { argv, .. } = &k.buttons[0].action else {
            panic!("expected a givc action");
        };
        assert_eq!(
            argv,
            &[
                "/nix/store/x-givc-cli/bin/givc-cli",
                "--name",
                "admin-vm",
                "start",
                "app",
                "--vm",
                "flatpak-vm",
                "run-flatpak-app",
                "--",
                "http://org.example.Plan",
            ]
        );
    }

    /// The default is what stops the kiosk hanging: `run-flatpak-app` stays
    /// active for as long as the application runs, so a launcher that got
    /// awaited would spin until the give-up timeout on every press.
    #[test]
    fn a_button_is_not_awaited_unless_it_says_so() {
        let k = parse(
            r#"{"version":1,"buttons":[{"id":"l","label":"L","action":{
                 "kind":"givc-app","vm":"flatpak-vm","app":"run-flatpak-app",
                 "givc_cli":["/nix/store/x-givc-cli/bin/givc-cli"]}}]}"#,
        )
        .unwrap();
        let Action::Givc { await_job, .. } = &k.buttons[0].action else {
            panic!("expected a givc action");
        };
        assert!(
            await_job.is_none(),
            "a button with no await_completion must not be waited on"
        );
    }

    #[test]
    fn awaiting_a_job_queries_the_registry_without_a_filter() {
        let k = parse(
            r#"{"version":1,"buttons":[{"id":"u","label":"U","action":{
                 "kind":"givc-app","vm":"flatpak-vm","app":"sfo-update-apps",
                 "await_completion":true,
                 "givc_cli":["/nix/store/x-givc-cli/bin/givc-cli","--name","admin-vm"]}}]}"#,
        )
        .unwrap();
        let Action::Givc { await_job, .. } = &k.buttons[0].action else {
            panic!("expected a givc action");
        };
        let job = await_job
            .as_ref()
            .expect("await_completion must be honoured");
        assert_eq!(job.app, "sfo-update-apps");
        // No --by-name: givc ignores that argument and returns the whole list,
        // so asking for a filter would look like it worked and quietly not.
        assert_eq!(
            job.query_argv,
            &[
                "/nix/store/x-givc-cli/bin/givc-cli",
                "--name",
                "admin-vm",
                "query",
                "--as-json",
            ]
        );
    }

    /// `givc-cli start service --vm <VM> <SERVICENAME>` -- a different subcommand
    /// with a different argument shape from `start app`, not a variant of it.
    #[test]
    fn a_service_argv_uses_the_service_subcommand() {
        let k = parse(
            r#"{"version":1,"buttons":[{"id":"c","label":"C","action":{
                 "kind":"givc-service","vm":"flatpak-vm","app":"sfo-clear.service",
                 "givc_cli":["/nix/store/x-givc-cli/bin/givc-cli","--name","admin-vm"]}}]}"#,
        )
        .unwrap();
        let Action::Givc { argv, target, .. } = &k.buttons[0].action else {
            panic!("expected a givc action");
        };
        assert_eq!(
            argv,
            &[
                "/nix/store/x-givc-cli/bin/givc-cli",
                "--name",
                "admin-vm",
                "start",
                "service",
                "--vm",
                "flatpak-vm",
                "sfo-clear.service",
            ]
        );
        assert_eq!(target, "sfo-clear.service in flatpak-vm");
    }

    /// `start service` has no `--` form, so args would be dropped by the CLI and
    /// the button would appear to work while doing something else.
    #[test]
    fn a_service_refuses_args_instead_of_dropping_them() {
        let k = parse(
            r#"{"version":1,"buttons":[{"id":"c","label":"C","action":{
                 "kind":"givc-service","vm":"flatpak-vm","app":"sfo-clear.service",
                 "args":["--anything"],
                 "givc_cli":["/nix/store/x-givc-cli/bin/givc-cli"]}}]}"#,
        )
        .unwrap();
        assert!(matches!(k.buttons[0].action, Action::Unsupported { .. }));
    }

    /// The awaited name is the unit, not an application id.
    ///
    /// Unusable against givc as it stands: a service stays in the registry
    /// whether running or not, so `still_running` never goes false. The nix
    /// module refuses the combination; this covers the code path for when givc
    /// can express it.
    #[test]
    fn a_service_can_be_awaited_by_its_unit_name() {
        let k = parse(
            r#"{"version":1,"buttons":[{"id":"u","label":"U","action":{
                 "kind":"givc-service","vm":"flatpak-vm","app":"sfo-update-apps.service",
                 "await_completion":true,
                 "givc_cli":["/nix/store/x-givc-cli/bin/givc-cli","--name","admin-vm"]}}]}"#,
        )
        .unwrap();
        let Action::Givc { await_job, .. } = &k.buttons[0].action else {
            panic!("expected a givc action");
        };
        let job = await_job
            .as_ref()
            .expect("await_completion must be honoured");
        assert_eq!(job.app, "sfo-update-apps.service");
        assert_eq!(
            job.query_argv,
            &[
                "/nix/store/x-givc-cli/bin/givc-cli",
                "--name",
                "admin-vm",
                "query",
                "--as-json",
            ]
        );
    }

    #[test]
    fn a_future_version_is_refused_rather_than_half_understood() {
        assert!(parse(r#"{"version":2,"buttons":[{"id":"a","label":"A"}]}"#).is_err());
    }

    #[test]
    fn an_empty_button_list_is_fatal() {
        assert!(parse(r#"{"version":1,"buttons":[]}"#).is_err());
    }

    #[test]
    fn unknown_fields_on_a_known_kind_are_tolerated() {
        // A newer nix module adding a field must not brick an older binary.
        let k = parse(
            r#"{"version":1,"buttons":[{"id":"a","label":"A","future":true,
                 "action":{"kind":"exec","argv":["true"],"alsoFuture":1}}]}"#,
        )
        .unwrap();
        assert!(matches!(k.buttons[0].action, Action::Exec { .. }));
    }

    #[test]
    fn a_button_is_not_a_singleton_unless_it_says_so() {
        let k = parse(
            r#"{"version":1,"buttons":[{"id":"l","label":"L","action":{
                 "kind":"givc-app","vm":"flatpak-vm","app":"run-flatpak-app",
                 "givc_cli":["/nix/store/x-givc-cli/bin/givc-cli"]}}]}"#,
        )
        .unwrap();
        let Action::Givc {
            single_instance, ..
        } = &k.buttons[0].action
        else {
            panic!("expected a givc action");
        };
        assert!(
            single_instance.is_none(),
            "a button with no single_instance must not be tracked as one"
        );
    }

    /// Without it there is nothing to ask the compositor -- see
    /// SingleInstance's own doc comment.
    #[test]
    fn a_singleton_without_window_app_id_is_unsupported() {
        let k = parse(
            r#"{"version":1,"buttons":[{"id":"l","label":"L","action":{
                 "kind":"givc-app","vm":"flatpak-vm","app":"run-flatpak-app",
                 "single_instance":true,
                 "givc_cli":["/nix/store/x-givc-cli/bin/givc-cli","--name","admin-vm"]}}]}"#,
        )
        .unwrap();
        assert!(matches!(k.buttons[0].action, Action::Unsupported { .. }));
    }

    #[test]
    fn a_launcher_cannot_be_both_awaited_and_a_singleton() {
        // One is for a job that ends, the other for one that does not.
        let k = parse(
            r#"{"version":1,"buttons":[{"id":"l","label":"L","action":{
                 "kind":"givc-app","vm":"flatpak-vm","app":"run-flatpak-app",
                 "await_completion":true,"single_instance":true,
                 "givc_cli":["/nix/store/x-givc-cli/bin/givc-cli"]}}]}"#,
        )
        .unwrap();
        assert!(matches!(k.buttons[0].action, Action::Unsupported { .. }));
    }

    #[test]
    fn window_app_id_survives_to_the_resolved_action() {
        let k = parse(
            r#"{"version":1,"buttons":[{"id":"l","label":"L","action":{
                 "kind":"givc-app","vm":"flatpak-vm","app":"run-flatpak-app",
                 "single_instance":true,"window_app_id":"BARQ Ground Control Station",
                 "givc_cli":["/nix/store/x-givc-cli/bin/givc-cli"]}}]}"#,
        )
        .unwrap();
        let Action::Givc {
            single_instance, ..
        } = &k.buttons[0].action
        else {
            panic!("expected a givc action");
        };
        assert_eq!(
            single_instance.as_ref().unwrap().window_app_id,
            "BARQ Ground Control Station"
        );
    }

    #[test]
    fn a_service_does_not_support_single_instance() {
        // A service has no window to avoid duplicating; only a launcher does.
        let k = parse(
            r#"{"version":1,"buttons":[{"id":"c","label":"C","action":{
                 "kind":"givc-service","vm":"flatpak-vm","app":"sfo-clear.service",
                 "single_instance":true,
                 "givc_cli":["/nix/store/x-givc-cli/bin/givc-cli"]}}]}"#,
        )
        .unwrap();
        assert!(matches!(k.buttons[0].action, Action::Unsupported { .. }));
    }
}
