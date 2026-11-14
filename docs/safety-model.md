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

Changing a light, only when you press **Put back** on a device you kept or **Keep it as before** on a device you stopped keeping, or, while the sign-in switch is on, for every kept device when the keeper starts, after the PC wakes, when OpenRGB says its device list changed, and when OpenRGB comes back after a restart:

| Request | Why |
| --- | --- |
| `RGBCONTROLLER_UPDATEMODE` (1101) | Switches that one device to the mode you kept, with its kept settings. |
| `RGBCONTROLLER_UPDATELEDS` (1050) | Sets that one device's lights to the colors you kept, when its mode colors each light itself. |

Put back sends these only to the device you pressed it on, and only that device's own kept setup. The keeper does the same for each kept device in turn, and nothing for any device that isn't kept. It re-sends even when OpenRGB's record already matches, because after sleep that record can be wrong.

While running, the keeper also holds a second connection that sends only `REQUEST_PROTOCOL_VERSION` and `SET_CLIENT_NAME`, then just listens for OpenRGB's own `DEVICE_LIST_UPDATED` (100) notices. It never sends a request that changes a light (`a_device_list_change_becomes_a_trigger`). It sends nothing when the device no longer fits what was kept, for example when it reports a different number of lights. Afterwards it reads the device back. That read comes from OpenRGB's record of the device, so it shows that OpenRGB took the request, not that the hardware shows it: RAM that loses power falls back to its own effect without OpenRGB noticing until it scans again.

**Stop keeping** removes a device from the setup and remembers what it was kept as. **Keep it as before** keeps that remembered setup again and puts it back, exactly like Put back. The keeper never puts back a remembered setup, only kept ones (`keeping_anew_forgets_the_earlier_setup_and_the_keeper_never_uses_it`).

When a device is removed, if it is in Direct mode, which shows nothing unless a program keeps sending colors, and it has a built-in mode named "Onboard" (the SteelSeries keyboard on this PC does), Keeplight sends it one `RGBCONTROLLER_UPDATEMODE` switching it to that mode, with the mode's settings exactly as OpenRGB reported them, and reads it back. Every other device is sent nothing and keeps showing what it shows until it loses power. If the read-back still shows another mode, the window says so (`stopping_hands_a_direct_keyboard_back_to_its_own_lighting`, `a_keyboard_that_ignores_the_hand_back_is_reported`).

Keeplight never sends `RGBCONTROLLER_SAVEMODE`, which writes into a device's own memory, and never loads, saves or deletes OpenRGB profiles. On 2026-10-23 loading a saved OpenRGB profile turned a keyboard's lights off, because a profile holds every device, including ones nobody chose a color for.

Tests against a fake server hold this, including `put_back_all_restores_every_kept_device_and_nothing_else` for the keeper: reading sends only the read requests (`reads_every_device_and_negotiates_down`), writes reach only the named device (`writes_reach_only_the_named_device`, `putting_back_sends_only_that_device_its_own_setup`), Keep sends no write at all (`keep_now_and_put_back_now_work_by_name_and_location`), and a device that changed gets nothing (`a_changed_device_is_skipped_without_sending_anything`).

## Written by the app

- `%LOCALAPPDATA%\Keeplight\setup.json`, or `setup.json` in `KEEPLIGHT_DATA` when that is set: the devices you pressed **Keep this** on, each with its product name, location, light count, mode and colors, plus what each device you stopped keeping was kept as. Written through a temporary file and a rename. A setup Keeplight can't read is never saved over; the window says so and turns Keep and Put back off.
- `plans\<16 hex digits>.json`: plans from the command line and AI tools, deleted when applied or once expired. `journal.jsonl` and `logs\keeplight-YYYY-MM-DD.jsonl`: see "Changes from AI tools" above. `settings.json` also holds `aiChanges`.
- `setup.lock` beside it: an empty file the window and the keeper lock while they change the setup or send a kept setup, so a device you stopped keeping can't be put back by a keeper run that read the setup earlier.
- `keeper.log` beside it: one line each time the keeper runs, saying how many kept devices it put back, plus a line for each that didn't hold. Trimmed to its last 400 lines once it passes 800.
- `downloads\` beside it: pinned installers you chose to download, kept after installing so a reinstall needs no second download.
- `settings.json` beside it: Keeplight's preferences, today only whether the keeper shows its icon in the notification area (off by default). A missing or unreadable file means the defaults.
- The startup entry `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `Keeplight`, holding `"<path to keeplight.exe>" --keep`. Written only when you turn on **Put kept lights back when you sign in**, and removed when you turn it off or uninstall. `KEEPLIGHT_STARTUP_NAME` renames the value, which the smoke test uses so it never touches the real one.

## Changes from AI tools and the command line

`keeplight-cli` (the command line, and its MCP server `keeplight-cli mcp`, see `docs/ai.md`) offers the same six changes as the window's buttons: keep, keep dark, put back, stop keeping, keep as before and set a color, one device at a time, and nothing else. A new color goes out as one `UPDATE_MODE` (the device's own mode with the new color, or its Static or Direct mode) and, for a per-light mode, one `UPDATE_LEDS`, then is kept like any other setup. A color for one zone sends every light of that device, with the other zones as they are kept (or show), in a per-light mode; what it replaced is remembered for keep as before. It sends OpenRGB only the requests listed above.

- **Plan, then apply.** Every change from the command line or an AI tool is planned first by the same check that applies it (`agent::check`). A plan records its steps, the setup file's SHA-256 and the device's current mode and colors, expires after 10 minutes and is applied once. Applying refuses when the setup or the device changed since, or when the check now gives different steps.
- **AI changes are off by default.** "Let AI tools make changes" is stored as `aiChanges` in `settings.json` and turned on only from the window. While off, `apply_plan` is refused with `ai.changes_off`; while on, it still needs the user's request in their own words. The command line is gated the same way since agent kit 6 (an AI tool with a shell can run it): a change needs the switch on, `--request "<words>"`, and `--yes`. Keeplight keeps the kit's strict default because its command line reaches everything an AI tool could. The window is never gated.
- **A record.** Every applied plan goes to `journal.jsonl` (time, caller, plan, request, steps), and every change and failure from any front end goes to `logs/keeplight-YYYY-MM-DD.jsonl` (UTC, kept 14 days), with who made it: `gui`, `cli`, `mcp:<app>` or `background:keeper`.
- **No network.** The MCP server speaks only over the stdin and stdout of the app that started it. It opens no port.

## Installing OpenRGB and PawnIO

The one time Keeplight uses the internet, and the one time it asks for admin rights. Owner decisions, 2026-11-14: download on demand rather than bundled, and tested versions only.

- **Only when you ask, only from the window.** When OpenRGB or PawnIO is missing or older than the version Keeplight is tested with, the window offers **Install <name> <version>…**. The button first shows what will happen (the file, its size, the site), and nothing starts until **Download and install**. AI tools and the command line can't install anything, and a newer, untested version is never downgraded.
- **Pinned.** Each installer is listed in `core::installs::TESTED` with its official https link, exact size and SHA-256: OpenRGB 1.0.0's MSI from codeberg.org, the very package installed on JASON-PC; PawnIO 2.2.0's setup from GitHub, whose hash matches GitHub's own and which is signed by its author. A newer version joins the list only after a test on the real PC.
- **Downloaded by Windows' own `curl.exe`** (`%SystemRoot%\System32`, by full path), https only, redirects https only, and at most the pinned size, into `%LOCALAPPDATA%\Keeplight\downloads`. A file that differs in size or hash is deleted and nothing runs. The file is checked again just before it runs.
- **Run with Windows' admin prompt.** The official installer, with its own window (`msiexec /i` for OpenRGB's MSI, PawnIO's setup as it is). Declining the prompt or cancelling installs nothing, and the window says so. The OpenRGB MSI installs OpenRGB's service; Keeplight's keeper puts kept devices back when OpenRGB comes back.

## Asking whether a newer Keeplight is out

- When the window opens, and once a day while it stays open, it asks the shared release feed for `keeplight-latest/release.json` (`core::update::FEED`, on github.com) with Windows' `curl.exe`: https only, at most 64 KB. Nothing about you or this PC is sent beyond the request itself. The keeper never asks.
- A newer version is only shown, with a **Download** button that opens its installer link in your browser. Keeplight never downloads or installs itself. A manifest whose installer isn't in the feed's own Keeplight releases is ignored.
- **Tell me when a new Keeplight is out** (on by default) turns the check off; it's saved as `noUpdateCheck` in `settings.json`.

## Read from Windows

- **The installed versions of OpenRGB and PawnIO,** from Windows' installed programs list (`...\CurrentVersion\Uninstall`, read only), and **whether the OpenRGB service runs**, from the service manager with query rights only. No admin rights. Shown in the window and in `diagnose`, against the versions Keeplight is tested with (`core::installs::TESTED`); only when OpenRGB is on this PC.

- **The names of running programs**, every few seconds while the window is open, to guess what lights each device: a maker's lighting app (SteelSeries GG, Logitech G HUB, Armoury Crate, Corsair iCUE, Razer Synapse, EVGA Precision X1, NZXT CAM, Gigabyte RGB Fusion) or SignalRGB. Only the names are read, needing no admin rights; they stay in memory, are never written or sent, and only decide the "Lit by" line (`core::lit_by`).

## Processes

- `keeplight.exe --keep`, the keeper. It has no window and is started by Windows at sign-in when the switch is on, or by the window the moment you turn the switch on. It runs only while the switch is on: once its stop listener is set up, it checks that the startup entry still exists, and exits without changing anything if it doesn't. Turning the switch off removes the entry before it signals stop, so a keeper that is still starting can't miss it. Each time it puts kept devices back, it waits up to 2 minutes for OpenRGB to list them all, then logs the result. It stays running until you sign out:
  - it registers with Windows for the resume-from-sleep notice (`PowerRegisterSuspendResumeNotification`);
  - it listens to OpenRGB as above;
  - triggers that arrive within 3 seconds of each other become one run.
- A named mutex (`Local\Keeplight.Keeper`) lets only one keeper run at a time.
- A named event (`Local\Keeplight.Keeper.Stop`) is how the window stops it when you turn the switch off. Both names follow `KEEPLIGHT_STARTUP_NAME`, so a smoke test never stops your keeper.
- The keeper's icon in the notification area, from a hidden window on its own thread. It is shown while the keeper runs when **Show the keeper's icon in the notification area** is on, and re-checked every 2 seconds. Clicking it opens Keeplight. Its menu has **Open Keeplight** and **Put kept lights back now**, which puts back kept devices exactly as a wake does.
- A Windows notification when a run leaves a kept device not put back, or can't reach OpenRGB. It is shown once when the problems appear or change, not again on every wake while they stay the same. With the icon off, the icon appears only for the notification and goes away when Windows closes it. Nothing is sent anywhere; the notification is local.
- The window starts no process except the keeper and, when you install OpenRGB or PawnIO, Windows' `curl.exe` and that installer (see "Installing OpenRGB and PawnIO"). The keeper's icon starts no process except the window. The uninstaller stops a running `keeplight.exe` before removing it. The installer, put over an earlier copy, first stops `keeplight-cli.exe` (an AI app's MCP server, which then loses Keeplight's tools until its next session) and `keeplight.exe` (the window and the keeper), since Windows won't replace a running program; it starts the keeper again when its sign-in switch is on.

## Not done, by design

- No network access beyond the SDK connection above, except a pinned installer you asked for (see "Installing OpenRGB and PawnIO") and the daily update check (see "Asking whether a newer Keeplight is out"), both by Windows' own `curl.exe`. CI's dependency policy still bans HTTP and TLS crates.
- No telemetry or accounts.
- No admin rights. The app runs as the signed-in user. The only admin prompt is Windows' own, for an installer you chose to run.
- No direct hardware access. Every read and write goes through OpenRGB.
- No device is kept dark without asking. A device whose lights are all off, or that is in its "Off" mode, offers **Keep it dark…**, which asks "Keep every light off, even after sleep?" in its row before anything is saved (`keeping_a_dark_device_needs_the_front_end_to_have_asked`).
