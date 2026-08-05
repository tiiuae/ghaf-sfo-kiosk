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
    Givc { argv: Vec<String>, target: String },
    /// Parsed, but not executable. Still renders; says why when pressed.
    Unsupported { reason: String },
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
    pub action: Action,
}

pub struct Kiosk {
    pub title: String,
    pub status_bar: StatusBar,
    pub layout: Layout,
    pub exit: ExitButton,
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
                action,
            }
        })
        .collect();

    Ok(Kiosk {
        title: cfg.title,
        status_bar: cfg.status_bar,
        layout: cfg.layout,
        exit: cfg.exit,
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
        assert_eq!(k.buttons.len(), 5, "the SFO product has five buttons");
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
}
