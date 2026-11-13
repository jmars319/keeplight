# Keeplight project state

Plan artifact: https://claude.ai/artifact/PisWk9vuT3sDiHuyRxWktu

Agent-facing working state. The plan page is the owner-facing view; this file holds the rules and details around it.

## Core Project Architecture & Stack

- Rust workspace, edition 2024, GPL-3.0-only. `crates/core` (`keeplight-core`): the engine every front end calls. `crates/app` (`keeplight`): the Slint window, which only passes requests to core and shows results.
- Core modules: `sdk` (own sync OpenRGB SDK client, protocol 3; `openrgb2` is banned, GPL-2.0-only), `sdk::wire` (framing, `Reader`/`Writer`, `update_mode`/`update_leds` payloads), `device` (model, full `Mode` fields so modes can be sent back unchanged), `keep` (setup file, capture, fit, put back, `keep_now`/`put_back_now`), `testing` (fake SDK server and sample devices; `cfg(test)` or feature `testing`).
- Keeper logic stays in core, not window code: owner-approved design request (2026-11-13, via the Mac coordinating session). Keeplight is the planned pilot for the fleet's agent-interface kit (Vaexil Mod Manager ADR 0003). Do not start CLI, MCP, plan/apply or error-code work until the owner says so.
- Setup file: `%LOCALAPPDATA%\Keeplight\setup.json` (`KEEPLIGHT_DATA` overrides the folder). Format version 1; a newer or unreadable file is never saved over.
- Env for tests: `KEEPLIGHT_OPENRGB` (SDK address), `KEEPLIGHT_DATA` (setup folder).
- Theme: `crates/app/ui/theme.slint` is a byte copy of Modlist Launcher's (c04e8d0). Fixes land there first.

## Current Structural Milestones / Active Feature Goals

- Stage 02, the keeper, in 4 PRs (owner go 2026-11-13):
  1. Write requests, setup file, Keep this / Put back, click smoke test. Branch `feat/keeper-write-and-setup`.
  2. Keeper mode: same exe, no window, per-user startup entry, apply at start and read back. Then sleep and restart tests with the owner.
  3. Split in two (2026-11-13). 3a: wake, device-change and OpenRGB-restart re-apply, keeper stays resident, switch starts/stops it; branch `feat/keeper-reapply`. 3b: failure notification and tray icon setting (off by default). Then the owner's sleep test.
  4. Stop keeping (hand back to onboard lighting) and ask-before-dark. Starts with a keyboard hand-back test with the owner.
- Settled for stage 02: choosing what to keep = per-device "Keep this" of what it shows now (colors still set in OpenRGB until stage 03); no tray icon by default, notification only on failure, tray icon optional in Settings; first release deferred: owner said 2026-11-13 to hold off on releases (no code signing yet), so stage 02 ends without a release and the keeper reaches JASON-PC by local installs from main (`scripts/package.ps1`, then the setup exe with `/VERYSILENT`).

## Active Task State & Known Regressions

- PR 1 merged as #8 (1b174e5, 2026-11-13) after a Codex finding: `fit` now compares speed, brightness, direction and color mode too.
- PR 2 merged as #9 (e15254c, 2026-11-13; Codex found nothing; auto-merge didn't fire on a clean PR, so it was merged by hand). Installed on JASON-PC from main e15254c the same day (`dist\Keeplight-0.1.0-setup.exe`, still version 0.1.0). It had `keep::put_back_all` (waits up to 120 s, polls every 3 s, for every kept device), `keeplight --keep` (no window, named mutex, `keeper.log`), the sign-in switch (HKCU Run value `Keeplight`, off by default, `KEEPLIGHT_STARTUP_NAME` for tests), uninstall removes the value and stops the exe. Verified: 53 tests, smoke 7/7, one real `--keep` run put back 2 of 2 RAM sticks in 128 ms.
- PR 3a is #10 (2026-11-13). Codex found a race: a stop during keeper startup could be lost. Fixed in 6bb2ec5: the keeper checks the startup entry after its stop listener exists, and the window removes the entry before signalling, so `--keep` runs only while the switch is on (manual test runs need a throwaway Run entry under `KEEPLIGHT_STARTUP_NAME`).
- **Stage 02 code is complete (2026-11-13).** All five PRs are merged (#8, #9, #10, #11, #12 at 297a470), and main 297a470 is installed on JASON-PC (still version 0.1.0; releases are on hold). Waiting on the owner: (1) turn on the sign-in switch in the installed app; (2) sleep test, where "Last run" should end "after the PC woke" with the RAM red; (3) restart test; (4) keyboard hand-back test with them watching. After (2) and (3) pass, stage 04 can retire the stopgap task.
- PR 4 merged as #12 (297a470). Codex found two races, fixed in 73d2278: `SetupLock` (`setup.lock`, `File::lock`) is held by keep, stop keeping (through the hand-back), put back and around each send in `put_back_all`, which re-reads the setup under it, so a stopped device can't be restored by an in-flight run (test passed 20 of 20); `Dark::Allow` returns `NoLongerDark` unless the device is still dark, and the row's question shows only while it is. Before that: `keep::is_dark` (an "Off" mode counts), `Kept::capture_dark`, `keep_now(.., Dark)`, `keep::stop_keeping` with `HandBack` (UPDATE_MODE to "Onboard" only when the device is in Direct); window Stop keeping and a row-level "Keep every light off, even after sleep?" confirm; swatch follows mode colors and "Off". Also fixed a #8 message with a run of spaces. Verified: 66 tests, smoke 16/16. **Not done: the real keyboard hand-back test** (October: UPDATEMODE to Onboard was accepted but read back Direct). It changes the owner's keyboard lighting, so it waits for the owner at the PC; if it fails, the window already reports NotTaken.
- PR 3b merged as #11 (c5ec4a0; Codex: keyboard activation of the tray icon, fixed in c652c34 with NIN_KEYSELECT). Branch `feat/keeper-notices` on top of #10: `core::settings` (`settings.json`, `tray_icon`), `keeper::problems` and `Notices` (tell once per new set of problems), `Trigger::Asked`; app `tray.rs` (hidden window, Shell_NotifyIcon v4, temporary icon for a notification when the setting is off, menu Open / Put back now, TaskbarCreated, handlers only `try_borrow`). Verified with `Shell_NotifyIconGetRect` (UI Automation can't see the overflow): icon present for a notification with the setting off, present with it on, gone when off, temporary icon gone within 10 s of the notification. Smoke 11/11.
- Was PR 3a: `core::keeper` (`Trigger`, `watch`, `run` with a 3 s settle), `Client::next_notice`, fake server `announce_device_change`/`drop_clients`; app keeper stays resident, registers `PowerRegisterSuspendResumeNotification`, stop event `<keeper name>.Stop`; mutex and event names follow `KEEPLIGHT_STARTUP_NAME` so smoke never stops the owner's keeper. Verified: 57 tests, smoke 9/9 (switch starts and stops a keeper), fake-server run re-applied after an OpenRGB restart, real run put back 2 of 2 and Windows accepted the wake registration. Not verified: an actual sleep and wake (owner's sleep test).
- Owner's real setup (2026-11-13): both RAM sticks kept as red Chase Fade. The owner's account had no OpenRGB Start menu shortcut; one was added.
- Real-hardware check 2026-11-13: keep + put back of RAM stick 1 (0x71) into a throwaway setup returned `Held` in 6.6 ms, lights unchanged (put back to the same red Chase Fade).
- Finding: the read-back after Put back is OpenRGB's in-memory record, not the chip. It proves OpenRGB took the request, not that the hardware shows it. ENE RAM falls back to rainbow on power loss without OpenRGB noticing until a rescan. PR 2/3 must not treat `Held` as proof the RAM is lit; re-apply on wake regardless.
- Real Chase Fade on the Mushkin ENE sticks reports color mode per-LED (1), not mode-specific; fixtures match.

## Temporary Logic Rules & Constraints

- Touch only devices in the setup; send a device only its own kept mode and colors; skip a device whose light count or modes changed.
- Never send `SAVEMODE` (1102) or any profile request.
- Never keep a device whose lights are all off (Keep refuses; the ask-first dark save comes in PR 4).
- Real-light tests: one device at a time, to its current state, with a throwaway setup file. Never load an OpenRGB profile for testing.
- The stopgap task `\JAMARQ\OpenRGB apply Red profile` stays until stage 04; it and the keeper set the same colors, so overlap is harmless.

## Context Compaction Checklist

- Branch and PR number of the current keeper PR, and its CI state. Codex reviews each PR and `main` requires resolved conversations: answer and resolve its threads. Auto-merge is on.
- Whether the owner has done the sleep/restart tests for PR 2 and PR 3.
- Plan page version and whether it reflects the merged PRs.
- Any new real-hardware finding (date it).
