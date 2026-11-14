//! `keeplight-cli`: Keeplight's command line and its MCP server, as a console
//! program beside `keeplight.exe` (the window, which is a Windows GUI program
//! and so can't give a terminal its output or exit code). Every command is
//! one of the engine's operations (`keeplight_core::agent`), with `--json`
//! printing exactly one JSON object and the fleet's exit codes.

use std::process::ExitCode;

use keeplight_core::agent::{ALL_CODES, Action, Change, Engine, SCHEMA_VERSION};
use keeplight_core::agent_kit::error::{AgentError, ErrorCodeList, codes};
use keeplight_core::agent_kit::gate::Caller;

const USAGE: &str = "\
keeplight-cli devices                    every device, and how it stands against what is kept
keeplight-cli plan <action> <device>     check a change and save it as a plan; changes nothing else
keeplight-cli apply <plan id> --yes      apply a saved plan once
keeplight-cli <action> <device> --yes    plan and apply a change at once
keeplight-cli set-color <device> <color> [--zone <zone>] --yes   set a device, or one zone, to #rrggbb and keep it (also with plan)
keeplight-cli set-color all <color> --yes   every device that can take it, each planned, kept and journaled
keeplight-cli logs [--limit N]           the newest events
keeplight-cli diagnose                   everything for working out a problem, redacted (JSON)
keeplight-cli error-codes                every error code
keeplight-cli mcp                        an MCP server on stdin and stdout (docs/ai.md)

<action> is keep, keep-dark, put-back, stop-keeping, keep-as-before or set-color.
<device> is an id or name from `devices`, such as ram-stick-1.
--json prints one JSON object (the result, or an error report) instead of text.
Exit codes: 0 ok, 2 bad usage, 3 refused, 4 changed meanwhile, 5 busy, 6 failed, 10 unexpected.";

/// One parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Help,
    Version,
    Mcp,
    Devices,
    Plan(Change),
    Apply(String),
    Change(Change),
    Logs(usize),
    Diagnose,
    ErrorCodes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Args {
    command: Command,
    json: bool,
    yes: bool,
}

fn usage(message: impl Into<String>) -> AgentError {
    AgentError::new(codes::USAGE, format!("{} Run `keeplight-cli help` for the commands.", message.into()))
}

fn change(
    action: &str,
    device: Option<&String>,
    color: Option<&String>,
    zone: Option<&String>,
) -> Result<Change, AgentError> {
    let action =
        Action::from_word(action).ok_or_else(|| usage(format!("There is no action \u{201c}{action}\u{201d}.")))?;
    let device = device.ok_or_else(|| usage(format!("Name a device for {}.", action.word())))?;
    if action == Action::SetColor {
        let color = color.ok_or_else(|| usage("Name a color for set-color, such as #0050ff."))?;
        let change = Change::set_color(device.clone(), color.clone());
        return Ok(match zone {
            Some(z) => change.in_zone(z.clone()),
            None => change,
        });
    }
    if zone.is_some() {
        return Err(usage("--zone goes only with set-color."));
    }
    Ok(Change::new(action, device.clone()))
}

fn parse(raw: &[String]) -> Result<Args, AgentError> {
    let json = raw.iter().any(|a| a == "--json");
    let yes = raw.iter().any(|a| a == "--yes");
    let mut words: Vec<String> = Vec::new();
    let mut limit = 50;
    let mut zone: Option<String> = None;
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--json" | "--yes" => {}
            "--limit" => {
                i += 1;
                limit = raw.get(i).and_then(|n| n.parse().ok()).ok_or_else(|| usage("--limit needs a number."))?;
            }
            "--zone" => {
                i += 1;
                zone = Some(raw.get(i).cloned().ok_or_else(|| usage("--zone needs a zone name or id."))?);
            }
            "--help" | "-h" | "/?" => words.insert(0, "help".into()),
            "--version" | "-V" => words.insert(0, "version".into()),
            flag if flag.starts_with("--") => return Err(usage(format!("There is no option {flag}."))),
            word => words.push(word.to_string()),
        }
        i += 1;
    }
    let first = words.first().map(String::as_str).unwrap_or("help");
    let command = match first {
        "help" => Command::Help,
        "version" => Command::Version,
        "mcp" => Command::Mcp,
        "devices" => Command::Devices,
        "plan" => {
            let action = words.get(1).ok_or_else(|| usage("Name an action to plan."))?;
            Command::Plan(change(action, words.get(2), words.get(3), zone.as_ref())?)
        }
        "apply" => Command::Apply(words.get(1).cloned().ok_or_else(|| usage("Name the plan id to apply."))?),
        "logs" => Command::Logs(limit.min(500)),
        "diagnose" => Command::Diagnose,
        "error-codes" => Command::ErrorCodes,
        action => Command::Change(change(action, words.get(1), words.get(2), zone.as_ref())?),
    };
    let changes = matches!(command, Command::Apply(_) | Command::Change(_));
    if changes && !yes {
        return Err(usage("This changes your lights or what Keeplight keeps: add --yes. `plan` shows it first."));
    }
    Ok(Args { command, json, yes })
}

/// Prints `value` as JSON, or `text` for people.
fn print(json: bool, value: &impl serde::Serialize, text: impl FnOnce() -> String) {
    if json {
        println!("{}", serde_json::to_string_pretty(value).expect("results serialize"));
    } else {
        println!("{}", text());
    }
}

fn run(args: &Args) -> Result<(), AgentError> {
    let json = args.json;
    match &args.command {
        Command::Help => println!("{USAGE}"),
        Command::Version => println!("keeplight-cli {}", env!("CARGO_PKG_VERSION")),
        Command::Mcp => {
            keeplight_core::mcp_server::serve(Engine::from_env()).map_err(|e| AgentError::new(codes::IO, e))?;
        }
        Command::ErrorCodes => {
            let list = ErrorCodeList::of(SCHEMA_VERSION, &ALL_CODES);
            print(json, &list, || {
                list.codes
                    .iter()
                    .map(|c| format!("{:24} {}  {}", c.code, c.exit_code, c.meaning))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        Command::Devices => {
            let r = Engine::from_env().devices()?;
            print(json, &r, || {
                r.devices
                    .iter()
                    .map(|d| {
                        let shows = d
                            .color
                            .clone()
                            .unwrap_or_else(|| if d.own_lighting { "own lighting".into() } else { "off".into() });
                        let kept = match &d.kept_as {
                            Some(k) => format!(
                                "kept as {k} ({})",
                                serde_json::to_value(&d.kept).unwrap().as_str().unwrap_or("")
                            ),
                            None => match &d.kept_before {
                                Some(b) => format!("not kept (before: {b})"),
                                None => "not kept".into(),
                            },
                        };
                        format!(
                            "{:16} {} · {} · {shows} · {kept} · lit by {}",
                            d.id,
                            d.name,
                            d.mode.as_deref().unwrap_or("?"),
                            d.lit_by
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        Command::Plan(c) => {
            let p = Engine::from_env().plan(&Caller::Cli, c.clone())?;
            print(json, &p, || {
                format!(
                    "{}\nPlan {} (valid 10 minutes). Apply it with: keeplight-cli apply {} --yes",
                    p.payload.steps.join("\n"),
                    p.plan_id,
                    p.plan_id
                )
            });
        }
        Command::Apply(id) => {
            let a = Engine::from_env().apply(&Caller::Cli, id, None)?;
            print(json, &a, || a.result.clone());
        }
        Command::Change(c) if c.action == Action::SetColor && c.device.eq_ignore_ascii_case("all") => {
            if c.zone.is_some() {
                return Err(usage("--zone names one device's zone, so it doesn't go with all."));
            }
            let r = Engine::from_env().set_everything(&Caller::Cli, c.color.as_deref().unwrap_or_default())?;
            print(json, &r, || {
                let mut lines = vec![format!("Set {} of {} devices to {}.", r.set(), r.devices.len(), r.color)];
                lines.extend(r.devices.iter().filter_map(|d| d.problem.as_ref().map(|p| format!("{}: {p}", d.name))));
                lines.join("\n")
            });
            if r.set() == 0 {
                return Err(AgentError::new(
                    keeplight_core::agent::codes::CANT_COLOR,
                    "No device took that color, so nothing was changed.",
                ));
            }
        }
        Command::Change(c) => {
            // Planned, then applied, so the change is journaled like any plan.
            let engine = Engine::from_env();
            let p = engine.plan(&Caller::Cli, c.clone())?;
            let a = engine.apply(&Caller::Cli, &p.plan_id, None)?;
            print(json, &a, || a.result.clone());
        }
        Command::Logs(limit) => {
            let events = Engine::from_env().recent_events(*limit);
            let value = serde_json::json!({ "schemaVersion": SCHEMA_VERSION, "events": events });
            print(json, &value, || {
                events
                    .iter()
                    .map(|e| {
                        format!(
                            "{} {:5} {:24} {:8} {}",
                            keeplight_core::agent_kit::events::timestamp(e.ts_ms),
                            format!("{:?}", e.level).to_lowercase(),
                            e.event,
                            e.caller,
                            e.message
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        Command::Diagnose => {
            let v = Engine::from_env().diagnose(serde_json::json!({ "frontEnd": "cli" }));
            println!("{}", serde_json::to_string_pretty(&v).expect("a diagnosis serializes"));
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let json = raw.iter().any(|a| a == "--json");
    let result = parse(&raw).and_then(|args| run(&args));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&e.report(SCHEMA_VERSION)).expect("errors serialize"));
            } else {
                eprintln!("{e}\n{}", e.code.hint);
            }
            ExitCode::from(e.code.exit_code() as u8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Result<Args, AgentError> {
        parse(&line.split_whitespace().map(str::to_string).collect::<Vec<_>>())
    }

    #[test]
    fn commands_parse() {
        assert_eq!(args("").unwrap().command, Command::Help);
        assert_eq!(args("--version").unwrap().command, Command::Version);
        assert_eq!(args("devices --json").unwrap(), Args { command: Command::Devices, json: true, yes: false });
        assert_eq!(
            args("plan put-back ram-stick-1").unwrap().command,
            Command::Plan(Change::new(Action::PutBack, "ram-stick-1"))
        );
        assert_eq!(args("apply 0123456789abcdef --yes").unwrap().command, Command::Apply("0123456789abcdef".into()));
        assert_eq!(
            args("stop-keeping keyboard --yes --json").unwrap().command,
            Command::Change(Change::new(Action::StopKeeping, "keyboard"))
        );
        assert_eq!(
            args("set-color keyboard #0050ff --yes").unwrap().command,
            Command::Change(Change::set_color("keyboard", "#0050ff"))
        );
        assert_eq!(
            args("plan set-color ram-stick-2 00ff00").unwrap().command,
            Command::Plan(Change::set_color("ram-stick-2", "00ff00"))
        );
        assert_eq!(
            args("set-color motherboard #00ff00 --zone addressable-rgb-header-1 --yes").unwrap().command,
            Command::Change(Change::set_color("motherboard", "#00ff00").in_zone("addressable-rgb-header-1"))
        );
        assert_eq!(args("logs --limit 5").unwrap().command, Command::Logs(5));
        assert_eq!(args("logs --limit 9999").unwrap().command, Command::Logs(500));
        assert_eq!(args("mcp").unwrap().command, Command::Mcp);
    }

    #[test]
    fn changes_need_yes_and_bad_input_is_usage() {
        for line in [
            "keep ram-stick-1",
            "apply 0123456789abcdef",
            "nonsense thing",
            "plan keep",
            "--nope",
            "logs --limit x",
            "set-color keyboard --yes",
            "keep keyboard --zone x --yes",
            "set-color keyboard #ff0000 --zone",
        ] {
            let e = args(line).unwrap_err();
            assert_eq!(e.code, codes::USAGE, "{line}");
            assert_eq!(e.code.exit_code(), 2);
        }
        assert!(args("keep ram-stick-1").unwrap_err().message.contains("--yes"));
        assert!(args("plan keep ram-stick-1").is_ok(), "planning changes nothing, so needs no --yes");
    }
}
