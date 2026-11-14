# Using Keeplight from an AI app

Keeplight has an MCP server, `keeplight-cli mcp`. Through it, an AI app such as Claude Code, Gemini CLI or Codex CLI can read your lights and, when you ask, keep or put back a device, as structured tool calls. It doesn't need screen control. The server is `keeplight-cli.exe`, which is installed beside `keeplight.exe`. It runs only while the AI app runs it, speaks over that app's stdin and stdout, and makes no network connection.

Keeplight follows the fleet's agent-interface standard (`docs/agent-interfaces.md` in agentic-instructions) with a byte-identical copy of the JAMARQ agent kit in `crates/core/src/agent_kit/`.

## Connect an app

Settings in Keeplight shows these lines with the real path filled in. Run the line for your app once, in a terminal:

```
claude mcp add --scope user keeplight -- "C:\path\to\keeplight-cli.exe" mcp
gemini mcp add --scope user keeplight "C:\path\to\keeplight-cli.exe" mcp
codex mcp add keeplight -- "C:\path\to\keeplight-cli.exe" mcp
```

Each line is that app's own command for adding a server, and it changes only that app's settings. Keeplight doesn't edit other apps' settings. `--scope user` makes the server available in every project. For another MCP client, the server is the command `keeplight-cli.exe` with the single argument `mcp`, over stdio.

## What an AI app can do

| Tool | What it does | Changes anything |
| --- | --- | --- |
| `get_devices` | Every device: id, name, mode, color, whether it's dark or on its own lighting, whether Keeplight keeps it and as what, what it was kept as before, and what probably lights it. | No |
| `get_logs` | The newest events: every change, who made it (`gui`, `cli`, `mcp:<app>` or `keeper`), and every failure with its code. | No |
| `diagnose` | All of it at once for working out a problem, with your user folder written as `~`. | No |
| `list_error_codes` | Every error code, what it means and what to do. | No |
| `plan_change` | Checks one change and returns a plan: its steps in plain words and a `planId`. | No (it saves the plan for 10 minutes) |
| `apply_plan` | Applies a plan once, if neither the setup nor the device changed since. | Yes |

A change is one action on one device:

| Action | What it does |
| --- | --- |
| `keep` | Keep what the device shows now. Changes no light. |
| `keepDark` | Keep a device whose every light is off, so it's turned back off after sleep. Only when it is dark. |
| `putBack` | Send a kept device its kept setup. |
| `stopKeeping` | Stop keeping it. A device in Direct mode with an Onboard mode is handed back to that, so it isn't left dark. |
| `keepAsBefore` | Go back to the setup it had before the last `stopKeeping` or `setColor`, and send it that. On a kept device the two swap, so doing it again redoes the change. |
| `setColor` | Set the device to `color` (`#rrggbb`) and keep it. A mode that takes colors keeps its effect in the new color (red Chase Fade becomes blue Chase Fade); a mode that doesn't, such as Onboard or a rainbow, switches to Static, or Direct. Black is refused: use `keepDark`. |

For one color on the whole PC, plan a `setColor` for each device. Nothing an AI app can do is irreversible: every action can be undone by another, and a new color by `keepAsBefore`.

Results are the same JSON as `keeplight-cli --json`, in the shapes under [`schema/`](../schema/). On failure a tool returns an error result (`isError`) holding an `ErrorReport`: a stable `code`, a `message`, a `hint` and whether retrying can work.

## Your control

- **Off until you turn it on.** "Let AI tools make changes" in Keeplight's window is off by default, and only the window can turn it on. While it's off, AI tools can read and plan, but `apply_plan` is refused with `ai.changes_off`.
- **Only what you ask for.** The server tells the model to change nothing on its own initiative and to show you a plan's steps first. `apply_plan` is marked destructive, so your AI app asks you before it runs, and it needs your request in your own words.
- **A record.** Every applied plan is appended to `journal.jsonl` in `%LOCALAPPDATA%\Keeplight`, with who applied it, your request and each step. Every change and failure, from any front end, is in `logs\keeplight-YYYY-MM-DD.jsonl` there, kept 14 days.
- **Nothing else.** The server has no tool for files, commands or the network, and none that sets a color, switches the keeper or changes a setting.

## The command line

Everything above is also available without MCP:

```
keeplight-cli devices
keeplight-cli plan put-back ram-stick-1
keeplight-cli apply <plan id> --yes
keeplight-cli put-back ram-stick-1 --yes
keeplight-cli logs --limit 20
keeplight-cli diagnose
```

`--json` prints one JSON object (the result, or an error report). A change needs `--yes`. Exit codes: 0 ok, 2 bad usage, 3 refused by a rule, 4 changed meanwhile, 5 busy, 6 failed, 10 unexpected. `keeplight-cli` is a console program so a terminal gets its output and exit code; `keeplight.exe` is the window.
