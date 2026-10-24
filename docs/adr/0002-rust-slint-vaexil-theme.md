# ADR 0002: Rust and Slint, laid out like Modlist Launcher, in the Vaexil theme

Date: 2026-10-24
Status: accepted

## Context

JAMARQ's Windows-native desktop apps are Rust and Slint (agentic-instructions ADR-0034, 2026-10-24). Modlist Launcher is the template, and Vaexil Mod Manager followed it the same day. Modlist Launcher's ADR 0001 explains the choice of Slint's software renderer, about 34 MB working set against about 111 MB with OpenGL. Its ADR 0002 records the Vaexil theme.

## Decision

- Same layout as Modlist Launcher and Vaexil Mod Manager: a `core` crate with no UI, an `app` crate with the Slint window, a written safety model, cargo-deny with HTTP and TLS crates banned, and an Inno Setup per-user installer plus a portable zip.
- Slint 1.18 with the `cupertino` style for the standard widgets still in use, and the software renderer.
- `crates/app/ui/theme.slint` is copied unchanged from Modlist Launcher (jmars319/modlist-launcher#13, at de1ec1c), and the Geist fonts are copied from its `assets/fonts`. Theme, Button, ComboBox, Switch and CheckBox are imported from it rather than from std-widgets, because the `cupertino` palette is read-only and tints to the Windows accent instead of cyan.
- **Dark by default** (owner decision 2026-10-24). The window sets `Palette.color-scheme` to dark on start. A setting to follow Windows or use light comes with the settings screen.
- The icon keeps the Vaexil canvas and the mark's blue-to-magenta with Keeplight's own symbol, a lit LED above a strip, so it stays distinct from the other apps in the taskbar (`scripts/generate-icon.ps1`).
- The repaint workaround after restoring from minimized is carried over (`repaint_after_restore`, and `window-background-alt` one step off `window-background`).
- OpenRGB is read on a worker thread, because it can take seconds to answer while it scans for devices. Only the finished view comes back to the window.

## Consequences

- One shared look and one way of working across the Windows apps.
- When the theme changes in Modlist Launcher, copy the file again; do not edit it here.
- The repository is private, unlike its two siblings, so CI on GitHub-hosted runners counts against the account's Actions minutes. Windows minutes count double.
