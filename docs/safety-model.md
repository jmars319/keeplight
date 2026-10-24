# Safety model

Keeplight's promise is that it only changes lights you have set, and only to what you set. This page lists everything the app sends and touches. It is updated in the same pull request as any change to that list.

## Sent to OpenRGB

Keeplight connects to OpenRGB's SDK server at `127.0.0.1:6742`, or the `host:port` in `KEEPLIGHT_OPENRGB`. In this version it sends only these requests:

| Request | Why |
| --- | --- |
| `REQUEST_PROTOCOL_VERSION` (40) | Agree on a protocol version. Keeplight decodes up to version 3. |
| `SET_CLIENT_NAME` (50) | Shows up as "Keeplight" in OpenRGB's client list. |
| `REQUEST_CONTROLLER_COUNT` (0) | How many devices there are. |
| `REQUEST_CONTROLLER_DATA` (1) | Each device's name, modes, zones and current colors. |

None of these changes a light. The client in `crates/core/src/sdk` has no packet id for any request that does, and a test against a fake server fails if anything else is sent (`reads_every_device_and_negotiates_down`).

Keeplight never loads, saves or deletes OpenRGB profiles. On 2026-10-23 loading a saved OpenRGB profile turned a keyboard's lights off, because a profile holds every device, including ones nobody chose a color for. When a later stage sets colors, it sets them one device at a time, only for devices you have set.

## Written by the app

Nothing, in this version. There is no settings file and no log yet. When a later stage adds one, it goes in `%LOCALAPPDATA%\Keeplight\` and this page says so first.

## Processes

- None are started.

## Not done, by design

- No network access beyond the SDK connection above. CI's dependency policy bans HTTP and TLS crates.
- No telemetry or accounts.
- No admin rights. The app runs as the signed-in user.
- No direct hardware access. Every read goes through OpenRGB.
