// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// Styling, inline rather than a GResource: one less build step, one less thing
// that can be stale on the device.
//
// Sizes the arithmetic in radial.rs depends on -- circle diameters, box extents
// -- are set with set_size_request, not here. Duplicating them would drift
// silently. The border-radius below is the one exception and must stay at half
// ICON_DIAMETER.

pub const CSS: &str = "
window.kiosk-root {
    background-color: #10141a;
    color: #e8ecf1;
}
.kiosk-statusbar {
    padding: 10px 20px;
    background-color: #161b23;
    border-bottom: 1px solid #232a35;
    font-size: 15px;
}
.kiosk-title { font-weight: bold; letter-spacing: 2px; }
/* Logo replaces the title; the margin keeps it off the bar's left padding. */
.kiosk-logo { margin-right: 4px; }
.kiosk-clock { font-size: 17px; font-weight: bold; }

/*
 * EVERY rule below that colours a `button` node must also clear
 * `background-image` and `box-shadow`.
 *
 * GTK4's default theme styles a plain button with
 * `background-image: linear-gradient(to top, #f6f5f4 2px, #fbfafa)` -- a light
 * gradient, not a colour. `background-color` paints BEHIND it, so setting the
 * colour alone leaves the button looking exactly as the theme drew it and the
 * kiosk renders light-on-dark. The theme repeats the gradient on :hover,
 * :active and :checked, so each of those needs clearing too.
 */

/* ── the grid ────────────────────────────────────────────────────────────── */
.kiosk-grid { padding: 40px; }
.kiosk-button {
    min-width: 280px;
    min-height: 220px;
    padding: 24px;
    border-radius: 18px;
    background-color: #1d2530;
    background-image: none;
    box-shadow: none;
    border: 1px solid #2b3542;
}
.kiosk-button:hover {
    background-color: #26313f;
    background-image: none;
    border-color: #46596f;
}
.kiosk-button:active {
    background-color: #303d4d;
    background-image: none;
    border-color: #6b8299;
}
.kiosk-button:focus { border-color: #6ea8ff; }
/*
 * `color` here is as load-bearing as `background-image` above, and for the
 * mirror-image reason.
 *
 * Set it on the icon and the label too, not just the button: the theme styles
 * those nodes as well, and inheriting from the button is only reliable where it
 * does not.
 */
.kiosk-button { color: #f2f6fa; }
.kiosk-button-label { font-size: 22px; font-weight: bold; color: #f2f6fa; }
.kiosk-button-icon { color: #f2f6fa; }
.kiosk-button-unconfigured { opacity: 0.45; }

/* ── the radial menu ─────────────────────────────────────────────────────── */
/*
 * The scrim stays mapped and fades by opacity, so the transition is CSS rather
 * than another tick callback. can_target(false) while closed stops an invisible
 * sheet eating every click on the grid.
 */
.kiosk-scrim {
    background-color: rgba(6, 9, 13, 0.66);
    opacity: 0;
    transition: opacity 180ms ease-out;
}
.kiosk-scrim.kiosk-scrim-open { opacity: 1; }

.kiosk-radial { background-color: transparent; }

.kiosk-radial-trigger {
    padding: 0;
    border-radius: 36px;
    background-color: #1d2530;
    background-image: none;
    box-shadow: none;
    border: 1px solid #2b3542;
    color: #aab6c4;
}
.kiosk-radial-trigger:hover {
    background-color: #26313f;
    background-image: none;
    border-color: #46596f;
    color: #e8ecf1;
}
/* Open state. The cog never becomes an X: an X here reads as 'close the menu'. */
.kiosk-radial-trigger:checked {
    background-color: #33445a;
    background-image: none;
    border-color: #6ea8ff;
    color: #ffffff;
}

/* The button is the whole ITEM_W x ITEM_H box; only the circle is drawn. */
.kiosk-radial-item {
    padding: 0;
    background-color: transparent;
    background-image: none;
    border: none;
    box-shadow: none;
}
.kiosk-radial-item:hover, .kiosk-radial-item:active, .kiosk-radial-item:focus {
    background-color: transparent;
    background-image: none;
    box-shadow: none;
}
.kiosk-radial-item-icon {
    border-radius: 32px;
    background-color: #1d2530;
    border: 1px solid #2b3542;
    color: #d4dbe4;
}
.kiosk-radial-item:hover .kiosk-radial-item-icon {
    background-color: #26313f;
    border-color: #46596f;
    color: #ffffff;
}
.kiosk-radial-item:active .kiosk-radial-item-icon { background-color: #303d4d; }
.kiosk-radial-item:focus .kiosk-radial-item-icon { border-color: #6ea8ff; }
.kiosk-radial-item-label {
    font-size: 15px;
    font-weight: bold;
    color: #c2ccd8;
}

/* ── the banner ──────────────────────────────────────────────────────────── */
.kiosk-banner { padding: 0; }
.kiosk-banner label { padding: 12px 20px; font-size: 15px; }
.kiosk-banner-info label { background-color: #1b3048; }
.kiosk-banner-error label { background-color: #4a1f26; color: #ffd9dd; }
";

/// Per-button icon colours, rendered as a stylesheet.
///
/// GTK4 CSS cannot read values from the program, so a colour that arrives in
/// config has to become CSS text. These target the `kiosk-button-<id>` and
/// `kiosk-radial-item-<id>` classes that ui.rs and radial.rs already attach, so
/// nothing new has to be plumbed through the widget tree.
///
/// Values are validated as `#rrggbb` in config.rs before they reach this, which
/// is what makes the interpolation safe.
pub fn per_button_css(kiosk: &crate::config::Kiosk) -> String {
    fn rule(out: &mut String, scope: &str, id: &str, colour: &str) {
        out.push_str(&format!(
            ".{scope}-{id} .{scope}-icon {{ color: {colour}; }}\n"
        ));
    }

    let mut out = String::new();
    for b in &kiosk.buttons {
        if let Some(c) = &b.icon_color {
            rule(&mut out, "kiosk-button", &b.id, c);
        }
    }
    for menu in &kiosk.menus {
        for item in &menu.items {
            if let Some(c) = &item.icon_color {
                rule(&mut out, "kiosk-radial-item", &item.id, c);
            }
        }
    }
    out
}
