# Keeplight

Plan artifact: https://claude.ai/artifact/PisWk9vuT3sDiHuyRxWktu

Keeplight shows every light in the PC, lets you set each one with a click, and puts your colors back on its own after a restart, a sleep or a power cut.

It runs on top of [OpenRGB](https://openrgb.org), which does all the hardware work, and talks to it over OpenRGB's SDK port on `127.0.0.1`. See [ADR 0001](docs/adr/0001-build-on-openrgb.md).

Keeplight is an independent project. It is not affiliated with OpenRGB or any hardware vendor. It is built and tested on one PC: an ASUS TUF Gaming Z590-Plus WiFi with Mushkin Redline Lumina RAM, an EVGA RTX 3080 Ti FTW3 Ultra and a SteelSeries Apex Pro TKL keyboard. Other hardware works as far as OpenRGB supports it, but is untested.

## Status

Early development, nothing released yet. Stage 02, the keeper, is being built:

- **Lists every device OpenRGB reports**, by plain name ("RAM stick 2", "Graphics card"), with the product name, light count, current effect and zones.
- **Shows each device's current color** as a swatch, read back from the device. A device running a lighting mode of its own, such as the keyboard's Onboard mode, shows a spectrum instead, because OpenRGB can't see the colors another app or the device itself is setting.
- **Says what probably lights each device** ("Lit by: its own memory (Onboard)", "probably SteelSeries GG, which is running", "Keeplight, which keeps it"), from the mode OpenRGB reports and which lighting apps are running, and warns when another app is running that may change a device Keeplight keeps.
- **Flags devices that cannot store a color themselves**, which go back to their own lighting whenever they lose power.
- **Keep this** saves what a device shows now into Keeplight's own setup file. Set colors in OpenRGB first; a color picker comes in a later stage.
- **Put back** is on every kept device, highlighted when it shows something else, and sends that one device its kept mode and colors. After a power cut the lights can differ while OpenRGB's record still matches, so it's always there.

- **Put kept lights back when you sign in** starts the keeper, `keeplight --keep`, now and at every sign-in. It has no window. It puts every kept device back when it starts, after the PC wakes, when OpenRGB's device list changes and when OpenRGB comes back after a restart, and logs each run to `%LOCALAPPDATA%\Keeplight\keeper.log`. Off until you turn it on; turning it off stops the keeper.

- **A notification** tells you when the keeper couldn't put a kept device back, once per new problem. **Show the keeper's icon in the notification area** (off by default) keeps an icon there whose menu can open Keeplight or put kept lights back now.

- **Keep it as before** brings back what a device was kept as before you stopped keeping it, and puts it back.
- **Stop keeping** removes a device from the setup (remembering it for Keep it as before) and, if it's in Direct mode and has a built-in "Onboard" mode, hands it back to that, so it isn't left dark.
- **Keep it dark…** on a device whose lights are all off asks once before keeping it that way.

Stage 02 is complete with these. Setting colors in Keeplight itself comes in stage 03. See the plan page.

## Build from source

You need the Rust toolchain (stable, MSVC), Visual Studio Build Tools, and OpenRGB running with its SDK server on (the OpenRGB service has it on).

```powershell
cargo test --workspace
cargo run -p keeplight
```

Set `KEEPLIGHT_OPENRGB` to another `host:port` to point Keeplight at a different SDK server, such as a test one, and `KEEPLIGHT_DATA` to another folder for the setup file and log.

`cargo run -p keeplight-core --features testing --example fake-openrgb` starts a fake SDK server with two RAM sticks and a keyboard and prints its address. `scripts/smoke-mouse-clicks.ps1` clicks the real window's buttons against it (CI runs it too).

`scripts/package.ps1` builds the portable zip and, when Inno Setup 6 is installed, the per-user installer into `dist/`. `scripts/generate-icon.ps1` redraws the icon in `assets/`.

Releases: a `v*` tag builds those into a draft GitHub release. Publishing the draft also copies the installer to [jamarq-releases](https://github.com/JAMARQ-Digital-LLC/jamarq-releases) as `keeplight-<version>-x64-setup.exe`, where `keeplight-latest/release.json` names the newest one and its SHA-256 (`.github/workflows/publish-feed.yml`).

## Safety

[docs/safety-model.md](docs/safety-model.md) lists every request Keeplight sends to OpenRGB and the one file it writes. It changes a light only when you press Put back (that device only) or at sign-in when you turned that on (kept devices only).

## License

GPL-3.0-only. The embedded Geist fonts are under the SIL Open Font License 1.1 (`assets/fonts/OFL.txt`).
