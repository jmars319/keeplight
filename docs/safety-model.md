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

Changing a light, only when you press **Put back** on a device you kept, or, while the sign-in switch is on, for every kept device when the keeper starts, after the PC wakes, when OpenRGB says its device list changed, and when OpenRGB comes back after a restart:

| Request | Why |
| --- | --- |
| `RGBCONTROLLER_UPDATEMODE` (1101) | Switches that one device to the mode you kept, with its kept settings. |
| `RGBCONTROLLER_UPDATELEDS` (1050) | Sets that one device's lights to the colors you kept, when its mode colors each light itself. |

Put back sends these only to the device you pressed it on, and only that device's own kept setup. The keeper does the same for each kept device in turn, and nothing for any device that isn't kept. It re-sends even when OpenRGB's record already matches, because after sleep that record can be wrong.

While running, the keeper also holds a second connection that sends only `REQUEST_PROTOCOL_VERSION` and `SET_CLIENT_NAME`, then just listens for OpenRGB's own `DEVICE_LIST_UPDATED` (100) notices. It never sends a request that changes a light (`a_device_list_change_becomes_a_trigger`). It sends nothing when the device no longer fits what was kept, for example when it reports a different number of lights. Afterwards it reads the device back. That read comes from OpenRGB's record of the device, so it shows that OpenRGB took the request, not that the hardware shows it: RAM that loses power falls back to its own effect without OpenRGB noticing until it scans again.

**Stop keeping** removes a device from the setup. If that device is in Direct mode, which shows nothing unless a program keeps sending colors, and it has a built-in mode named "Onboard" (the SteelSeries keyboard on this PC does), Keeplight sends it one `RGBCONTROLLER_UPDATEMODE` switching it to that mode, with the mode's settings exactly as OpenRGB reported them, and reads it back. Every other device is sent nothing and keeps showing what it shows until it loses power. If the read-back still shows another mode, the window says so (`stopping_hands_a_direct_keyboard_back_to_its_own_lighting`, `a_keyboard_that_ignores_the_hand_back_is_reported`).

Keeplight never sends `RGBCONTROLLER_SAVEMODE`, which writes into a device's own memory, and never loads, saves or deletes OpenRGB profiles. On 2026-10-23 loading a saved OpenRGB profile turned a keyboard's lights off, because a profile holds every device, including ones nobody chose a color for.

Tests against a fake server hold this, including `put_back_all_restores_every_kept_device_and_nothing_else` for the keeper: reading sends only the read requests (`reads_every_device_and_negotiates_down`), writes reach only the named device (`writes_reach_only_the_named_device`, `putting_back_sends_only_that_device_its_own_setup`), Keep sends no write at all (`keep_now_and_put_back_now_work_by_name_and_location`), and a device that changed gets nothing (`a_changed_device_is_skipped_without_sending_anything`).

## Written by the app

- `%LOCALAPPDATA%\Keeplight\setup.json`, or `setup.json` in `KEEPLIGHT_DATA` when that is set: the devices you pressed **Keep this** on, each with its product name, location, light count, mode and colors. Written through a temporary file and a rename. A setup Keeplight can't read is never saved over; the window says so and turns Keep and Put back off.
- `keeper.log` beside it: one line each time the keeper runs, saying how many kept devices it put back, plus a line for each that didn't hold. Trimmed to its last 400 lines once it passes 800.
- `settings.json` beside it: Keeplight's preferences, today only whether the keeper shows its icon in the notification area (off by default). A missing or unreadable file means the defaults.
- The startup entry `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `Keeplight`, holding `"<path to keeplight.exe>" --keep`. Written only when you turn on **Put kept lights back when you sign in**, and removed when you turn it off or uninstall. `KEEPLIGHT_STARTUP_NAME` renames the value, which the smoke test uses so it never touches the real one.

## Processes

- `keeplight.exe --keep`, the keeper. It has no window and is started by Windows at sign-in when the switch is on, or by the window the moment you turn the switch on. It runs only while the switch is on: once its stop listener is set up, it checks that the startup entry still exists, and exits without changing anything if it doesn't. Turning the switch off removes the entry before it signals stop, so a keeper that is still starting can't miss it. Each time it puts kept devices back, it waits up to 2 minutes for OpenRGB to list them all, then logs the result. It stays running until you sign out:
  - it registers with Windows for the resume-from-sleep notice (`PowerRegisterSuspendResumeNotification`);
  - it listens to OpenRGB as above;
  - triggers that arrive within 3 seconds of each other become one run.
- A named mutex (`Local\Keeplight.Keeper`) lets only one keeper run at a time.
- A named event (`Local\Keeplight.Keeper.Stop`) is how the window stops it when you turn the switch off. Both names follow `KEEPLIGHT_STARTUP_NAME`, so a smoke test never stops your keeper.
- The keeper's icon in the notification area, from a hidden window on its own thread. It is shown while the keeper runs when **Show the keeper's icon in the notification area** is on, and re-checked every 2 seconds. Clicking it opens Keeplight. Its menu has **Open Keeplight** and **Put kept lights back now**, which puts back kept devices exactly as a wake does.
- A Windows notification when a run leaves a kept device not put back, or can't reach OpenRGB. It is shown once when the problems appear or change, not again on every wake while they stay the same. With the icon off, the icon appears only for the notification and goes away when Windows closes it. Nothing is sent anywhere; the notification is local.
- The window starts no process except the keeper, and the keeper's icon starts no process except the window. The uninstaller stops a running `keeplight.exe` before removing it.

## Not done, by design

- No network access beyond the SDK connection above. CI's dependency policy bans HTTP and TLS crates.
- No telemetry or accounts.
- No admin rights. The app runs as the signed-in user.
- No direct hardware access. Every read and write goes through OpenRGB.
- No device is kept dark without asking. A device whose lights are all off, or that is in its "Off" mode, offers **Keep it dark…**, which asks "Keep every light off, even after sleep?" in its row before anything is saved (`keeping_a_dark_device_needs_the_front_end_to_have_asked`).
