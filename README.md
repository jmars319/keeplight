# Keeplight

Plan artifact: https://claude.ai/artifact/PisWk9vuT3sDiHuyRxWktu

Keeplight shows every light in the PC, lets you set each one with a click, and puts your colors back on its own after a restart, a sleep or a power cut.

It runs on top of [OpenRGB](https://openrgb.org), which does all the hardware work, and talks to it over OpenRGB's SDK port on `127.0.0.1`. See [ADR 0001](docs/adr/0001-build-on-openrgb.md).

Keeplight is an independent project. It is not affiliated with OpenRGB or any hardware vendor. It is built and tested on one PC: an ASUS TUF Gaming Z590-Plus WiFi with Mushkin Redline Lumina RAM, an EVGA RTX 3080 Ti FTW3 Ultra and a SteelSeries Apex Pro TKL keyboard. Other hardware works as far as OpenRGB supports it, but is untested.

## Status

Early development, nothing released yet. This first version is read-only:

- **Lists every device OpenRGB reports**, by plain name ("RAM stick 2", "Graphics card"), with the product name, light count, current effect and zones.
- **Shows each device's current color** as a swatch, read back from the device.
- **Flags devices that cannot store a color themselves**, which go back to their own lighting whenever they lose power.
- **Changes nothing.** It sends OpenRGB only questions, never a request that changes a light, and writes no file.

Setting colors, keeping them applied after sleep and restarts, and the case map come in later stages on the plan page.

## Build from source

You need the Rust toolchain (stable, MSVC), Visual Studio Build Tools, and OpenRGB running with its SDK server on (the OpenRGB service has it on).

```powershell
cargo test --workspace
cargo run -p keeplight
```

Set `KEEPLIGHT_OPENRGB` to another `host:port` to point Keeplight at a different SDK server, such as a test one.

`scripts/package.ps1` builds the portable zip and, when Inno Setup 6 is installed, the per-user installer into `dist/`. `scripts/generate-icon.ps1` redraws the icon in `assets/`.

## Safety

[docs/safety-model.md](docs/safety-model.md) lists every request Keeplight sends to OpenRGB. It writes no file.

## License

GPL-3.0-only. The embedded Geist fonts are under the SIL Open Font License 1.1 (`assets/fonts/OFL.txt`).
