# Safety model

Keeplight's promise is that it only changes lights you have set, and only to what you set. This page lists everything the app sends and touches. It is updated in the same pull request as any change to that list.

## Sent to OpenRGB

Keeplight connects to OpenRGB's SDK server at `127.0.0.1:6742`, or the `host:port` in `KEEPLIGHT_OPENRGB`.

Reading, which happens every few seconds while the window is open:

| Request | Why |
| --- | --- |
| `REQUEST_PROTOCOL_VERSION` (40) | Agree on a protocol version. Keeplight decodes up to version 3. |
| `SET_CLIENT_NAME` (50) | Shows up as "Keeplight" in OpenRGB's client list. |
| `REQUEST_CONTROLLER_COUNT` (0) | How many devices there are. |
| `REQUEST_CONTROLLER_DATA` (1) | Each device's name, modes, zones and current colors. |

Changing a light, only when you press **Put back** on a device you kept:

| Request | Why |
| --- | --- |
| `RGBCONTROLLER_UPDATEMODE` (1101) | Switches that one device to the mode you kept, with its kept settings. |
| `RGBCONTROLLER_UPDATELEDS` (1050) | Sets that one device's lights to the colors you kept, when its mode colors each light itself. |

Put back sends these only to the device you pressed it on, and only that device's own kept setup. It sends nothing when the device no longer fits what was kept, for example when it reports a different number of lights. Afterwards it reads the device back. That read comes from OpenRGB's record of the device, so it shows that OpenRGB took the request, not that the hardware shows it: RAM that loses power falls back to its own effect without OpenRGB noticing until it scans again.

Keeplight never sends `RGBCONTROLLER_SAVEMODE`, which writes into a device's own memory, and never loads, saves or deletes OpenRGB profiles. On 2026-10-23 loading a saved OpenRGB profile turned a keyboard's lights off, because a profile holds every device, including ones nobody chose a color for.

Tests against a fake server hold this: reading sends only the read requests (`reads_every_device_and_negotiates_down`), writes reach only the named device (`writes_reach_only_the_named_device`, `putting_back_sends_only_that_device_its_own_setup`), Keep sends no write at all (`keep_now_and_put_back_now_work_by_name_and_location`), and a device that changed gets nothing (`a_changed_device_is_skipped_without_sending_anything`).

## Written by the app

- `%LOCALAPPDATA%\Keeplight\setup.json`, or `setup.json` in `KEEPLIGHT_DATA` when that is set: the devices you pressed **Keep this** on, each with its product name, location, light count, mode and colors. Written through a temporary file and a rename. A setup Keeplight can't read is never saved over; the window says so and turns Keep and Put back off.

## Processes

- None are started.

## Not done, by design

- No network access beyond the SDK connection above. CI's dependency policy bans HTTP and TLS crates.
- No telemetry or accounts.
- No admin rights. The app runs as the signed-in user.
- No direct hardware access. Every read and write goes through OpenRGB.
- No device is kept while all its lights are off, so Keep can't save a dark device.
