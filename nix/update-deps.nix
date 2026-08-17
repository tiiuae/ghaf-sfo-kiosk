# SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
# SPDX-License-Identifier: Apache-2.0
#
# This repository has two lock files and only one of them has a robot behind it:
# dependabot bumps Cargo.toml/Cargo.lock weekly, and has no Nix flake ecosystem,
# so flake.lock is ours to move by hand (see .github/dependabot.yml). This is the
# one command that moves both, and the only thing that rewrites the requirements
# in Cargo.toml rather than just the lock under them.
#
# The sibling ghafpkgs repo ships an update-deps that walks packages/ and
# dispatches per ecosystem. There is exactly one crate and one flake here, so
# this is the same interface -- `update-deps`, `update-deps --upgrade` -- with
# the discovery pass dropped.
{
  writeShellApplication,
  lib,
  cargo,
  cargo-edit,
  rustc,
  git,
  nix,
}:

writeShellApplication {
  name = "update-deps";

  meta = {
    description = "Update this repository's Cargo and Nix flake dependency locks";
    license = lib.licenses.asl20;
    platforms = lib.platforms.linux;
    mainProgram = "update-deps";
  };

  runtimeInputs = [
    cargo
    # Provides `cargo upgrade`, the only thing here that edits Cargo.toml. cargo
    # itself never touches a version requirement, only the lock beneath it.
    cargo-edit
    # cargo shells out to `rustc -vV` for host/target info before resolving.
    rustc
    git
    nix
  ];

  text = ''
    set -euo pipefail

    UPGRADE=false

    usage() {
      echo "Usage: update-deps [OPTIONS]"
      echo ""
      echo "Move this repository's dependency locks forward, in that order:"
      echo ""
      echo "  flake.lock   always -- dependabot has no Nix flake ecosystem, so"
      echo "               nothing else in the repo moves it. First, because the"
      echo "               pin supplies the rustc and the C libraries the crates"
      echo "               below resolve against."
      echo "  Cargo.toml   only with --upgrade"
      echo "  Cargo.lock   always"
      echo ""
      echo "OPTIONS:"
      echo "  -u, --upgrade    Also raise the version requirements in Cargo.toml to the"
      echo "                   latest release, across semver-incompatible bumps."
      echo "                   Potentially breaking; review and test the result."
      echo "  -h, --help       Show this message"
      echo ""
      echo "Examples:"
      echo "  update-deps              # locks only, every requirement left alone (safe)"
      echo "  update-deps --upgrade    # + latest major versions (potentially breaking)"
      echo ""
    }

    while [[ $# -gt 0 ]]; do
      case $1 in
        -u | --upgrade)
          UPGRADE=true
          shift
          ;;
        -h | --help)
          usage
          exit 0
          ;;
        *)
          echo "update-deps: unknown option: $1" >&2
          echo "Try 'update-deps --help'." >&2
          exit 1
          ;;
      esac
    done

    # Only colour a terminal; this gets run from CI too, where the escapes are
    # just noise in the log.
    if [[ -t 1 ]]; then
      RED=$'\033[0;31m'
      GREEN=$'\033[0;32m'
      YELLOW=$'\033[1;33m'
      BLUE=$'\033[0;34m'
      NC=$'\033[0m'
    else
      RED=""
      GREEN=""
      YELLOW=""
      BLUE=""
      NC=""
    fi

    log() {
      local color=$1
      shift
      printf '%s[update-deps]%s %s\n' "$color" "$NC" "$*"
    }

    IN_GIT=true
    repo_root=$(git rev-parse --show-toplevel 2>/dev/null) || IN_GIT=false
    [[ "$IN_GIT" == true ]] || repo_root=$PWD

    cd "$repo_root"

    if [[ ! -f Cargo.toml || ! -f flake.nix ]]; then
      log "$RED" "$repo_root has no Cargo.toml and flake.nix side by side"
      log "$RED" "run this from inside a ghaf-sfo-kiosk checkout"
      exit 1
    fi

    if [[ "$IN_GIT" == false ]]; then
      log "$YELLOW" "not a git checkout; working in $repo_root"
    fi

    nix --extra-experimental-features 'nix-command flakes' flake update

    if [[ "$UPGRADE" == true ]]; then
      log "$YELLOW" "upgrade mode: version requirements in Cargo.toml will be rewritten"

      # --incompatible is the whole point of this mode. Without it cargo-upgrade
      # only moves a requirement that the latest release no longer satisfies,
      # which is a subset of what `cargo update` does on its own.
      #
      # --pinned is deliberately NOT passed. cargo-upgrade honours rust-version
      # in Cargo.toml, so "latest" to it means "latest release that still builds
      # on the declared MSRV" -- and a requirement already ahead of that gets
      # reported as pinned. Adding --pinned would rewrite those *downwards*.
      #
      # --verbose prints the old req / latest / new req table. Without it a run
      # that changed nothing looks the same as a run that had nothing to change.
      log "$BLUE" "cargo upgrade --incompatible --verbose"
      cargo upgrade --incompatible --verbose
    fi

    log "$BLUE" "cargo update"
    cargo update

    if [[ "$IN_GIT" == true ]]; then
      echo ""
      git --no-pager diff --stat -- Cargo.toml Cargo.lock flake.lock || true
      echo ""
    fi

    log "$GREEN" "done -- review the diff, then run the full gate:"
    log "$GREEN" "  nix flake check --all-systems --keep-going -L"
    if [[ "$UPGRADE" == true ]]; then
      log "$YELLOW" "upgrade mode was used: expect breakage from major bumps, and read"
      log "$YELLOW" "the changelogs for anything that moved a major version"
    else
      log "$BLUE" "tip: --upgrade also raises the requirements in Cargo.toml"
    fi
  '';
}
