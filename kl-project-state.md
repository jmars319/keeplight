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
  3. Wake, device-change and OpenRGB-restart re-apply; failure notification; tray icon setting (off by default). Tested with the owner again.
  4. Stop keeping (hand back to onboard lighting) and ask-before-dark. Starts with a keyboard hand-back test with the owner.
- Settled for stage 02: choosing what to keep = per-device "Keep this" of what it shows now (colors still set in OpenRGB until stage 03); no tray icon by default, notification only on failure, tray icon optional in Settings; first release 0.2.0 when the keeper is done.

## Active Task State & Known Regressions

- PR 1 merged as #8 (1b174e5, 2026-11-13) after a Codex finding: `fit` now compares speed, brightness, direction and color mode too.
- PR 2 in progress (2026-11-13), branch `feat/keeper-at-sign-in`: `keep::put_back_all` (waits up to 120 s, polls every 3 s, for every kept device), `keeplight --keep` (no window, named mutex, `keeper.log`), the sign-in switch (HKCU Run value `Keeplight`, off by default, `KEEPLIGHT_STARTUP_NAME` for tests), uninstall removes the value and stops the exe. Verified: 53 tests, smoke 7/7 including the switch, one real `--keep` run put back 2 of 2 RAM sticks in 128 ms with all 5 devices still red.
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
