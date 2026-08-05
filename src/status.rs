// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// Status-bar data sources, all on the gui-VM's SYSTEM bus. UPower is enabled in
// gui-vm and the VM really has a battery. NetworkManager is NOT enabled there --
// it lives in net-vm -- but ghaf republishes it onto gui-vm's system bus over a
// GIVC socket proxy, so a local org.freedesktop.NetworkManager is what we talk
// to, exactly as COSMIC's own applet does.
//
// Poll `cached_property` on a glib timeout rather than `g-properties-changed`:
// gio's handler is bound `Send + Sync + 'static` and GTK widgets are neither, so
// the handler could not be written without marshalling through a channel.
// DBusProxy keeps the cache current anyway.
//
// Only NetworkManager's ROOT object. Walking to devices and access points for an
// SSID is the thing most likely to work on a bench and fail on the device.

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

/// UPower's `State` enumeration, of which we need two.
const UPOWER_STATE_CHARGING: u32 = 1;
const UPOWER_STATE_FULLY_CHARGED: u32 = 4;

/// NetworkManager's `NM_STATE_*`.
const NM_STATE_CONNECTED_GLOBAL: u32 = 70;
const NM_STATE_CONNECTED_SITE: u32 = 60;
const NM_STATE_CONNECTED_LOCAL: u32 = 50;
const NM_STATE_CONNECTING: u32 = 40;

/// How often the bus-backed indicators refresh. The QEMU battery device probes
/// at 20 s intervals anyway, so anything faster only costs wakeups.
const POLL_SECONDS: u32 = 5;

pub struct Clock {
    pub label: gtk::Label,
    format: String,
}

impl Clock {
    pub fn new(format: &str) -> Self {
        let me = Self {
            label: gtk::Label::new(None),
            format: format.to_owned(),
        };
        me.label.add_css_class("kiosk-clock");
        me.tick();
        me
    }

    fn tick(&self) {
        // glib's own formatter: no chrono, and it follows /etc/localtime, which
        // ghaf keeps in step across VMs with `givc-cli set-timezone`.
        if let Ok(now) = glib::DateTime::now_local() {
            if let Ok(text) = now.format(&self.format) {
                self.label.set_text(&text);
            }
        }
    }

    pub fn start(self: std::rc::Rc<Self>) {
        glib::timeout_add_seconds_local(1, move || {
            self.tick();
            glib::ControlFlow::Continue
        });
    }
}

/// An icon + label pair that degrades to "unavailable" rather than vanishing.
#[derive(Clone)]
pub struct Indicator {
    pub container: gtk::Box,
    icon: gtk::Image,
    label: gtk::Label,
}

impl Indicator {
    fn new(css: &str) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        container.add_css_class(css);
        let icon = gtk::Image::new();
        let label = gtk::Label::new(None);
        container.append(&icon);
        container.append(&label);
        Self {
            container,
            icon,
            label,
        }
    }

    fn set(&self, icon_name: &str, text: &str) {
        self.icon.set_icon_name(Some(icon_name));
        self.label.set_text(text);
    }

    fn set_unavailable(&self, what: &str) {
        self.set("dialog-question-symbolic", &format!("{what}: unavailable"));
    }
}

fn system_proxy(name: &str, path: &str, iface: &str) -> Option<gio::DBusProxy> {
    match gio::DBusProxy::for_bus_sync(
        gio::BusType::System,
        gio::DBusProxyFlags::NONE,
        None,
        name,
        path,
        iface,
        gio::Cancellable::NONE,
    ) {
        Ok(p) => Some(p),
        Err(e) => {
            log::warn!("D-Bus proxy {name} {path}: {e}");
            None
        }
    }
}

/// Battery, via UPower's DisplayDevice on the system bus.
pub fn battery() -> Indicator {
    let ind = Indicator::new("kiosk-battery");

    let Some(manager) = system_proxy(
        "org.freedesktop.UPower",
        "/org/freedesktop/UPower",
        "org.freedesktop.UPower",
    ) else {
        ind.set_unavailable("Battery");
        return ind;
    };

    // Returns (o) -- a single object path.
    let device_path = manager
        .call_sync(
            "GetDisplayDevice",
            None,
            gio::DBusCallFlags::NONE,
            2000,
            gio::Cancellable::NONE,
        )
        .ok()
        .and_then(|reply| {
            reply
                .child_value(0)
                .str()
                .map(std::borrow::ToOwned::to_owned)
        });

    let Some(path) = device_path else {
        log::warn!("UPower: could not resolve the display device");
        ind.set_unavailable("Battery");
        return ind;
    };

    let Some(device) = system_proxy(
        "org.freedesktop.UPower",
        &path,
        "org.freedesktop.UPower.Device",
    ) else {
        ind.set_unavailable("Battery");
        return ind;
    };

    let render = |dev: &gio::DBusProxy, ind: &Indicator| {
        let pct = dev
            .cached_property("Percentage")
            .and_then(|v| v.get::<f64>())
            .unwrap_or(-1.0);
        let state = dev
            .cached_property("State")
            .and_then(|v| v.get::<u32>())
            .unwrap_or(0);
        // UPower already picks the right freedesktop icon for level and charge
        // state, so we render its answer instead of reimplementing thresholds.
        let icon = dev
            .cached_property("IconName")
            .and_then(|v| v.get::<String>())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "battery-missing-symbolic".to_owned());

        let suffix = match state {
            UPOWER_STATE_CHARGING => " ⚡",
            UPOWER_STATE_FULLY_CHARGED => " ✓",
            _ => "",
        };
        if pct >= 0.0 {
            ind.set(&icon, &format!("{pct:.0}%{suffix}"));
        } else {
            ind.set(&icon, "--");
        }
    };

    render(&device, &ind);
    let ind2 = ind.clone();
    glib::timeout_add_seconds_local(POLL_SECONDS, move || {
        render(&device, &ind2);
        glib::ControlFlow::Continue
    });
    ind
}

/// Network, via NetworkManager's root object on the system bus.
pub fn network() -> Indicator {
    let ind = Indicator::new("kiosk-network");

    // net-vm may still be booting, or ghaf's dbus-proxy-networkmanager unit may
    // be restarting (it has Restart=always). Neither is fatal and neither may
    // block startup.
    let Some(nm) = system_proxy(
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager",
        "org.freedesktop.NetworkManager",
    ) else {
        ind.set_unavailable("Network");
        return ind;
    };

    let render = |nm: &gio::DBusProxy, ind: &Indicator| {
        let state = nm
            .cached_property("State")
            .and_then(|v| v.get::<u32>())
            .unwrap_or(0);
        let kind = nm
            .cached_property("PrimaryConnectionType")
            .and_then(|v| v.get::<String>())
            .unwrap_or_default();

        let medium = match kind.as_str() {
            "802-11-wireless" => "Wi-Fi",
            "802-3-ethernet" => "Ethernet",
            "wireguard" => "VPN",
            "" => "Network",
            other => other,
        };
        let (icon, text) = match state {
            NM_STATE_CONNECTED_GLOBAL => ("network-wireless-symbolic", medium.to_owned()),
            NM_STATE_CONNECTED_SITE | NM_STATE_CONNECTED_LOCAL => (
                "network-wireless-acquiring-symbolic",
                format!("{medium} · limited"),
            ),
            NM_STATE_CONNECTING => (
                "network-wireless-acquiring-symbolic",
                "Connecting…".to_owned(),
            ),
            _ => ("network-offline-symbolic", "Offline".to_owned()),
        };
        ind.set(icon, &text);
    };

    render(&nm, &ind);
    let ind2 = ind.clone();
    glib::timeout_add_seconds_local(POLL_SECONDS, move || {
        render(&nm, &ind2);
        glib::ControlFlow::Continue
    });
    ind
}
