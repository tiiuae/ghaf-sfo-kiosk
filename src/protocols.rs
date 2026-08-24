// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// Generated bindings for the two COSMIC Wayland protocol extensions
// toplevels.rs needs, from the XML vendored in ../protocols/. Not a
// dependency on the `cosmic-protocols` crate: that pulls it from git, which
// is invisible to flake.lock (so an outdated, GPL-3.0-only pin shipped here
// once, unnoticed by any check in this repo or in ghaf-sfo-laptop's), and
// the crate itself is only ever these two files plus wayland-scanner --
// already a direct dependency of every wayland-client consumer.
//
// `workspace` is vendored too, unused on its own, purely because
// cosmic-toplevel-info-unstable-v1.xml declares `workspace_enter` /
// `workspace_leave` events referencing `zcosmic_workspace_handle_v1`, and
// wayland-scanner generates a full `Event` enum covering the whole protocol
// regardless of which version a client actually binds. `deprecated-since="3"`
// on those two is advisory, not a version gate -- it does not stop the
// compositor sending them to a client bound at version 2, which is what
// toplevels.rs does (`version.min(2)` on its zcosmic_toplevel_info_v1 bind).
// The actually version-gated pair is their `since="3"` replacement,
// `ext_workspace_enter`/`ext_workspace_leave`: those genuinely can never
// reach a v2-bound client. Either way nothing here is dead by protocol
// mechanics alone -- toplevels.rs's Dispatch impl for
// zcosmic_toplevel_handle_v1 discards every event outright (the whole event
// parameter is bound to `_`, not matched at all), and its
// ext_foreign_toplevel_handle_v1 counterpart discards the ones it doesn't
// use, workspace ones included, via a catch-all `_ => {}` arm -- either way
// the type still has to exist for the generated code to compile but nothing
// in this crate ever reads it.
// Trimming them out of the vendored XML instead is NOT safe: Wayland opcodes
// are assigned by an event's *position* in the file, not an explicit id, so
// deleting one from the middle would silently renumber every event after it
// and desync from what the real compositor sends.

#![allow(missing_docs, clippy::all)]

pub mod workspace {
    pub mod v1 {
        pub mod client {
            use wayland_client;
            use wayland_client::protocol::*;

            pub mod __interfaces {
                use wayland_client::protocol::__interfaces::*;
                wayland_scanner::generate_interfaces!(
                    "./protocols/cosmic-workspace-unstable-v1.xml"
                );
            }
            use self::__interfaces::*;

            wayland_scanner::generate_client_code!("./protocols/cosmic-workspace-unstable-v1.xml");
        }
    }
}

pub mod toplevel_info {
    pub mod v1 {
        pub mod client {
            use crate::protocols::workspace::v1::client::*;
            use wayland_client;
            use wayland_client::protocol::*;
            use wayland_protocols::ext::foreign_toplevel_list::v1::client::*;
            use wayland_protocols::ext::workspace::v1::client::*;

            pub mod __interfaces {
                use crate::protocols::workspace::v1::client::__interfaces::*;
                use wayland_client::protocol::__interfaces::*;
                use wayland_protocols::ext::foreign_toplevel_list::v1::client::__interfaces::*;
                use wayland_protocols::ext::workspace::v1::client::__interfaces::*;
                wayland_scanner::generate_interfaces!(
                    "./protocols/cosmic-toplevel-info-unstable-v1.xml"
                );
            }
            use self::__interfaces::*;

            wayland_scanner::generate_client_code!(
                "./protocols/cosmic-toplevel-info-unstable-v1.xml"
            );
        }
    }
}

pub mod toplevel_management {
    pub mod v1 {
        pub mod client {
            use crate::protocols::toplevel_info::v1::client::*;
            use crate::protocols::workspace::v1::client::*;
            use wayland_client;
            use wayland_client::protocol::*;
            use wayland_protocols::ext::workspace::v1::client::*;

            pub mod __interfaces {
                use crate::protocols::toplevel_info::v1::client::__interfaces::*;
                use crate::protocols::workspace::v1::client::__interfaces::*;
                use wayland_client::protocol::__interfaces::*;
                use wayland_protocols::ext::workspace::v1::client::__interfaces::*;
                wayland_scanner::generate_interfaces!(
                    "./protocols/cosmic-toplevel-management-unstable-v1.xml"
                );
            }
            use self::__interfaces::*;

            wayland_scanner::generate_client_code!(
                "./protocols/cosmic-toplevel-management-unstable-v1.xml"
            );
        }
    }
}
