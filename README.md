# Keeplight

Plan artifact: https://claude.ai/artifact/PisWk9vuT3sDiHuyRxWktu

Keeplight shows every light in the PC, lets you set each one with a click, and puts your colors back on its own after a restart, a sleep or a power cut.

It runs on top of [OpenRGB](https://openrgb.org), which does all the hardware work, and talks to it over OpenRGB's SDK port on `127.0.0.1`. See [ADR 0001](docs/adr/0001-build-on-openrgb.md).

Keeplight is an independent project. It is not affiliated with OpenRGB or any hardware vendor. It is built and tested on one PC: an ASUS TUF Gaming Z590-Plus WiFi with Mushkin Redline Lumina RAM, an EVGA RTX 3080 Ti FTW3 Ultra and a SteelSeries Apex Pro TKL keyboard. Other hardware works as far as OpenRGB supports it, but is untested.

## Status

Early development, nothing released yet. Stage 02, the keeper, is being built:

- **Lists every device OpenRGB reports**, by plain name ("RAM stick 2", "Graphics card"), with the product name, light count, current effect and zones.
- **Shows each device's current color** as a swatch, read back from the device.
- **Flags devices that cannot store a color themselves**, which go back to their own lighting whenever they lose power.
- **Keep this** saves what a device shows now into Keeplight's own setup file. Set colors in OpenRGB first; a color picker comes in a later stage.
- **Put back** appears when a kept device shows something else, and sends that one device its kept mode and colors.

- **Put kept lights back when you sign in** turns on a startup entry that runs `keeplight --keep`, which puts every kept device back with no window and logs what it did to `%LOCALAPPDATA%\Keeplight\keeper.log`. Off until you turn it on.

Putting kept devices back after sleep and when a device reconnects comes next. See the plan page.

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
