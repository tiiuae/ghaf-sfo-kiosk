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
    #[serde(default)]
    pub status_bar: StatusBar,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub exit: ExitButton,
    /// Utility controls anchored to the edges of the surface rather than placed
    /// in the grid. Several may share a position; they become a row, in this
    /// order. Empty is allowed, and is what every config produced before this
    /// key existed contains.
    #[serde(default)]
    pub corners: Vec<Corner>,
    pub buttons: Vec<Button>,
}

/// A control anchored to an edge of the surface rather than placed in the grid.
///
/// Edge controls are for utilities; the grid is for applications. They are drawn
/// as dashed circles for exactly that reason -- the difference has to be legible
/// before the label is read.
#[derive(Debug, Deserialize)]
pub struct Corner {
    /// Where it sits: see `Position`. A string rather than a serde enum so that
    /// an unrecognised value can be reported and skipped instead of making the
    /// whole file unparseable.
    pub position: String,
    pub label: String,
    #[serde(default)]
    pub icon: Option<String>,
    /// Rendered size in pixels, overriding the default.
    ///
    /// Optical, not geometric: a solid glyph fills its box and a few thin
    /// strokes do not, so two icons at the same pixel size look like different
    /// sizes. Which icons need the adjustment is a property of the icon set the
    /// product chose, so it is set here rather than guessed at in the binary.
    #[serde(default)]
    pub icon_size: Option<u32>,
    #[serde(default)]
    pub action: RawAction,
}

/// Where along the bottom edge a control sits. More than one control may share a
/// position; they are laid out as a row, in config order.
///
/// Position is policy, not decoration. The two CORNERS are the hardest places on
/// a touchscreen to hit by accident and still easy to reach with a thumb, so they
/// suit things you need but rarely want. `Center` is the opposite: the
/// easiest place on the screen to hit, which is right for something used
/// constantly and wrong for anything you would regret.
///
/// The top edge is deliberately absent. Nothing has needed it, and it is one
/// line to add when something does -- at which point these variants grow an
/// edge in their names, which is why they do not carry one now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Left,
    Center,
    Right,
}

impl Position {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "bottom-left" => Some(Self::Left),
            "bottom-center" => Some(Self::Center),
            "bottom-right" => Some(Self::Right),
            _ => None,
        }
    }
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

#[derive(Debug, Deserialize)]
pub struct ExitButton {
    #[serde(default = "default_exit_label")]
    pub label: String,
    #[serde(default = "default_exit_icon")]
    pub icon: String,
}

fn default_exit_label() -> String {
    "Exit".to_owned()
}
fn default_exit_icon() -> String {
    "window-close-symbolic".to_owned()
}

impl Default for ExitButton {
    fn default() -> Self {
        Self {
            label: default_exit_label(),
            icon: default_exit_icon(),
        }
    }
}

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
    /// Rendered size in pixels, overriding the default.
    ///
    /// Optical, not geometric: a solid glyph fills its box and a few thin
    /// strokes do not, so two icons at the same pixel size look like different
    /// sizes. Which icons need the adjustment is a property of the icon set the
    /// product chose, so it is set here rather than guessed at in the binary.
    #[serde(default)]
    pub icon_size: Option<u32>,
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
    },
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
                let mut argv = raw.givc_cli.clone();
                argv.extend(["start".to_owned(), "service".to_owned()]);
                argv.extend(["--vm".to_owned(), vm.clone()]);
                argv.push(unit.clone());
                Self::Givc {
                    argv,
                    target: format!("{unit} in {vm}"),
                    await_job: await_job_for(raw, unit),
                }
            }
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
    pub icon_size: Option<u32>,
    pub action: Action,
}

/// An edge control with its position and action already resolved.
pub struct ResolvedCorner {
    pub position: Position,
    pub label: String,
    pub icon: Option<String>,
    pub icon_size: Option<u32>,
    pub action: Action,
}

pub struct Kiosk {
    pub title: String,
    pub status_bar: StatusBar,
    pub layout: Layout,
    pub exit: ExitButton,
    pub corners: Vec<ResolvedCorner>,
    pub buttons: Vec<ResolvedButton>,
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

    let buttons = cfg
        .buttons
        .iter()
        .map(|b| {
            let action = Action::from_raw(&b.action);
            if let Action::Unsupported { reason } = &action {
                log::warn!("button {:?}: {}", b.id, reason);
            }
            ResolvedButton {
                id: b.id.clone(),
                label: b.label.clone(),
                description: b.description.clone(),
                icon: b.icon.clone(),
                icon_size: b.icon_size,
                action,
            }
        })
        .collect();

    // An unplaceable control is the one problem that has to remove it: there is
    // nowhere to draw it. Every other problem keeps the control and reports
    // itself when pressed, so the operator sees something that explains itself
    // rather than an edge that is mysteriously empty.
    let corners = cfg
        .corners
        .iter()
        .filter_map(|c| {
            let Some(position) = Position::parse(&c.position) else {
                log::warn!(
                    "edge control {:?}: unknown position {:?}; skipping it",
                    c.label,
                    c.position
                );
                return None;
            };
            let action = Action::from_raw(&c.action);
            if let Action::Unsupported { reason } = &action {
                log::warn!("edge control {:?}: {}", c.label, reason);
            }
            Some(ResolvedCorner {
                position,
                label: c.label.clone(),
                icon: c.icon.clone(),
                icon_size: c.icon_size,
                action,
            })
        })
        .collect();

    Ok(Kiosk {
        title: cfg.title,
        status_bar: cfg.status_bar,
        layout: cfg.layout,
        exit: cfg.exit,
        corners,
        buttons,
    })
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

        // Three in the grid and three on the edge, and the split is the design:
        // the grid is what an operator works in, the edge is the machine. A
        // button reappearing in the grid means that line has moved.
        assert_eq!(k.buttons.len(), 3, "three applications in the grid");
        assert_eq!(
            k.corners.iter().map(|c| c.position).collect::<Vec<_>>(),
            vec![Position::Left, Position::Center, Position::Right],
            "the edge controls run left, centre, right along the bottom"
        );
        for c in &k.corners {
            assert!(
                !matches!(c.action, Action::Unsupported { .. }),
                "edge control {:?} did not resolve",
                c.label
            );
        }
        for b in &k.buttons {
            assert!(
                !matches!(b.action, Action::Unsupported { .. }),
                "button {:?} did not resolve",
                b.id
            );
        }
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
    fn an_edge_control_with_an_unknown_position_is_dropped_not_fatal() {
        let k = parse(
            r#"{"version":1,"buttons":[{"id":"a","label":"A","action":{"kind":"exec","argv":["true"]}}],
                "corners":[{"position":"middle","label":"Nowhere"},
                           {"position":"bottom-left","label":"Here","action":{"kind":"exec","argv":["true"]}}]}"#,
        )
        .expect("an unplaceable control must not fail the whole file");
        assert_eq!(k.corners.len(), 1);
        assert_eq!(k.corners[0].position, Position::Left);
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
}
