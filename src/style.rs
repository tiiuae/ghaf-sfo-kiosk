// SPDX-FileCopyrightText: 2026 TII (SSRC) and the Ghaf contributors
// SPDX-License-Identifier: Apache-2.0
//
// Styling, inline rather than a GResource: one less build step, one less thing
// that can be stale on the device.
//
// Sizes the arithmetic in radial.rs depends on -- the 72px circles, the 132x104
// boxes -- are set with set_size_request, not here. Duplicating them here would
// drift silently.

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
.kiosk-clock { font-size: 17px; font-weight: bold; }

/* ── the grid ────────────────────────────────────────────────────────────── */
.kiosk-grid { padding: 40px; }
.kiosk-button {
    min-width: 280px;
    min-height: 220px;
    padding: 24px;
    border-radius: 18px;
    background-color: #1d2530;
    border: 1px solid #2b3542;
}
.kiosk-button:hover {
    background-color: #26313f;
    border-color: #46596f;
}
.kiosk-button:active {
    background-color: #303d4d;
    border-color: #6b8299;
}
.kiosk-button:focus { border-color: #6ea8ff; }
.kiosk-button-label { font-size: 22px; font-weight: bold; }
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
    border: 1px solid #2b3542;
    color: #aab6c4;
}
.kiosk-radial-trigger:hover {
    background-color: #26313f;
    border-color: #46596f;
    color: #e8ecf1;
}
/* Open state. The cog never becomes an X: exit sits on the same arc. */
.kiosk-radial-trigger:checked {
    background-color: #33445a;
    border-color: #6ea8ff;
    color: #ffffff;
}

/* The button is the whole 132x104 box; only the circle inside it is drawn. */
.kiosk-radial-item {
    padding: 0;
    background: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
}
.kiosk-radial-item-icon {
    border-radius: 36px;
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

/*
 * Exit is the only member that ends the kiosk, so it stays muted until reached
 * for. Its distance up the arc is the other half; see MEMBER_ARC_WITH_EXIT.
 */
.kiosk-radial-exit { opacity: 0.55; }
.kiosk-radial-exit:hover, .kiosk-radial-exit:focus { opacity: 1.0; }
.kiosk-radial-exit:hover .kiosk-radial-item-icon {
    background-color: #3a2530;
    border-color: #7a4a58;
}

/* ── the corner exit, used when no menu claimed it ───────────────────────── */
.kiosk-exit {
    margin: 14px;
    min-width: 34px;
    min-height: 34px;
    padding: 4px;
    border-radius: 17px;
    opacity: 0.35;
    background-color: transparent;
}
.kiosk-exit:hover { opacity: 1.0; background-color: #3a2530; }

/* ── the banner ──────────────────────────────────────────────────────────── */
.kiosk-banner { padding: 0; }
.kiosk-banner label { padding: 12px 20px; font-size: 15px; }
.kiosk-banner-info label { background-color: #1b3048; }
.kiosk-banner-error label { background-color: #4a1f26; color: #ffd9dd; }
";
