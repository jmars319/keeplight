# ADR 0001: Build on OpenRGB, with Keeplight's own SDK client

Date: 2026-10-24
Status: accepted

## Context

Keeplight exists because the RAM on JASON-PC kept going back to rainbow. That turned out to have two causes: Armoury Crate and OpenRGB were both controlling the same lights, and the RAM's ENE chips cannot store a color, so they reset to their built-in rainbow on every power loss, sleep included.

Every lighting device speaks its own protocol, mostly reverse-engineered, and the RAM needs a kernel driver to reach. OpenRGB already supports all five devices on JASON-PC and runs there as a service with the PawnIO driver.

Two ways to build on it were considered on 2026-10-23:

- **Fork OpenRGB.** One install and no network port. But every device fix the OpenRGB community ships would then have to be merged by hand, into a large C++ and Qt codebase whose UI Keeplight would replace anyway.
- **Use OpenRGB unmodified** and drive it through its SDK server, a documented TCP protocol on `127.0.0.1:6742`.

The plan named `openrgb2` (0.4.1, 2026-09-05) as the Rust client, since it is the only one that speaks protocol 6. On 2026-10-24 its crates.io metadata showed it is **GPL-2.0-only**. GPL-2.0-only code cannot be combined with this GPL-3.0-only app. It also requires the tokio async runtime, which a small Slint app does not otherwise need.

## Decision

- Keeplight uses OpenRGB unmodified and talks to it only through the SDK server. A later stage bundles a pinned, tested OpenRGB build with the installer.
- Keeplight has its own small, synchronous SDK client in `crates/core/src/sdk`, written from OpenRGB's protocol description. No code is taken from `openrgb2` or its predecessor `openrgb`, and `deny.toml` bans both crates, along with tokio.
- The client negotiates protocol version 3 at most. Version 3 carries everything Keeplight needs: device type, names, modes and their flags, zones and current colors. The layout was checked against OpenRGB 1.0 on JASON-PC on 2026-10-23.
- Keeplight never uses OpenRGB profiles. Loading one applies every device it holds, and OpenRGB 1.0 rewrites the profile file with every device's current state on each load. Both behaviors were seen on 2026-10-23.

## Consequences

- Device support improves whenever OpenRGB is updated, with no merge work.
- Keeplight depends on OpenRGB's SDK server running. The window says so plainly when it is not.
- The client is small, but it is ours to maintain. If OpenRGB changes the protocol 3 layout, Keeplight stops reading devices until the client is updated. The parser fails with an error rather than misreading, and the version is negotiated, so the server keeps sending the layout Keeplight asked for.
- If the SDK cannot do something Keeplight needs, the fallback is a small patch to the bundled OpenRGB, not a fork of the whole program.

## Amendment, 2026-11-13: tokio for the MCP server only

Keeplight is the pilot for the fleet's agent-interface kit (`docs/agent-interfaces.md` in agentic-instructions), whose MCP server is built on `rmcp` and needs tokio. The owner decided on 2026-11-13 to lift the tokio ban for that edge only. `deny.toml` now allows tokio only as a dependency of `rmcp` (with its `tokio-stream` and `tokio-util`) and `keeplight-core`, where `mcp_server::serve` runs a single-thread runtime for `keeplight-cli mcp`. The OpenRGB client, the window and the keeper stay synchronous and never start a runtime. The bans on HTTP and TLS crates are unchanged, and `axum` joined them. See `docs/ai.md`.
