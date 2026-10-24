# Contributing

Bug reports and pull requests are welcome. Keeplight drives OpenRGB and never talks to hardware itself, so most of the work is in reading what OpenRGB reports carefully, showing it plainly, and changing only the lights you chose.

Keeplight is built for one PC (see the README), so a report about hardware it has not been tested on is useful, but may not be fixed quickly.

## Before you start

- For anything bigger than a fix, open an issue first so we can agree on the approach.
- Read the [safety model](docs/safety-model.md). Changes must keep its promises:
  - send OpenRGB only the requests the safety model lists, and update it in the same pull request as any new one;
  - change only lights the user has set, and never load, save or delete OpenRGB profiles;
  - write only what the safety model lists;
  - connect to nothing but the OpenRGB SDK server;
  - need no admin rights.
- Design decisions are recorded in [docs/adr](docs/adr). The SDK client in `crates/core/src/sdk` is Keeplight's own, written from OpenRGB's protocol description; do not add `openrgb2` or copy code from it, since it is GPL-2.0-only ([ADR 0001](docs/adr/0001-build-on-openrgb.md)).
- The look comes from the Vaexil theme in `crates/app/ui/theme.slint`, which is copied unchanged from Modlist Launcher ([ADR 0002](docs/adr/0002-rust-slint-vaexil-theme.md)). Use its components and `Theme` tokens rather than new colours, and do not edit that file here; change it in Modlist Launcher and copy it again.

## Working on it

You need the Rust toolchain (stable, MSVC), Visual Studio Build Tools, and OpenRGB with its SDK server on. Set `KEEPLIGHT_OPENRGB` to another `host:port` to run against a test server instead of real lights.

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p keeplight
```

CI runs all of these on every pull request, plus `cargo deny check`. `cargo deny` bans HTTP and TLS crates, tokio, and the GPL-2.0-only OpenRGB crates, and rejects yanked or vulnerable ones. Both CI checks must pass before a pull request can merge.

- Keep each pull request to one change.
- Add tests for behaviour that can be tested. Platform-neutral logic belongs in `crates/core`, where it is unit tested; the SDK client is tested against a fake server.
- For UI changes:
  - check dark mode, and light mode once the setting exists;
  - check keyboard use: Tab, Space/Enter;
  - check that controls and device rows have screen-reader labels.
- Update the README or docs when behaviour changes.

## License

By contributing you agree that your work is licensed under [GPL-3.0-only](LICENSE), like the rest of the project.
