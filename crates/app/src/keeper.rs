//! `keeplight --keep`: the keeper, with no window. Started at sign-in by the
//! startup entry (or by the window when the switch is turned on), it puts
//! back every kept device, then stays running and does it again whenever the
//! PC wakes, OpenRGB's device list changes or OpenRGB comes back after a
//! restart. The work is `keeplight_core::keeper`; this module wires it to
//! Windows and writes the log.

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use keeplight_core::agent_kit::events::LogLevel;
use keeplight_core::keep::{ActionError, Outcome, Run};
use keeplight_core::keeper::{self, Notices, Timing, Trigger};

pub fn run() -> i32 {
    let Some(_only) = crate::platform::SingleInstance::acquire(&crate::platform::keeper_name()) else {
        return 0;
    };
    let address = crate::platform::openrgb_address();
    let setup_path = crate::platform::setup_path();
    // At sign-in OpenRGB's device scan can lag behind the desktop.
    let timing = Timing { wait: Duration::from_secs(120), ..Timing::default() };

    let (tx, rx) = mpsc::channel();
    let _ = tx.send(Trigger::Start);
    let stop = keeper::stop_flag();
    {
        let (tx, stop, address) = (tx.clone(), stop.clone(), address.clone());
        std::thread::spawn(move || keeper::watch(&address, &tx, &stop, timing));
    }
    let wake = crate::platform::on_wake(tx.clone());
    let tray = crate::tray::start(tx.clone(), crate::platform::settings_path());
    if let Err(e) = &tray {
        let _ = crate::platform::append_log(&[format!(
            "{}  Couldn't set up notifications, so problems are only written here: {e}",
            crate::platform::local_timestamp()
        )]);
    }
    let tray = tray.ok();
    let mut notices = Notices::default();
    crate::platform::keeper_stop::listen({
        let stop = stop.clone();
        move || {
            stop.store(true, Ordering::Relaxed);
            let _ = crate::platform::append_log(&[format!(
                "{}  Stopped: the sign-in switch was turned off.",
                crate::platform::local_timestamp()
            )]);
            std::process::exit(0);
        }
    });
    // The keeper runs only while the sign-in switch is on. Checked after the
    // stop listener exists: the window removes the entry before it signals
    // stop, so a switch turned off while this keeper was starting is caught
    // here or by the listener, never missed by both.
    if crate::platform::startup::current().is_none() {
        let _ = crate::platform::append_log(&[format!(
            "{}  Didn't start: the sign-in switch is off.",
            crate::platform::local_timestamp()
        )]);
        return 0;
    }
    if !wake {
        let _ = crate::platform::append_log(&[format!(
            "{}  Windows didn't accept the wake notification, so kept devices are put back at sign-in and \
             when OpenRGB's devices change, but not after sleep.",
            crate::platform::local_timestamp()
        )]);
    }
    drop(tx);

    // The shared event log (logs/keeplight-*.jsonl), beside keeper.log, so
    // `get_logs` and `diagnose` show the keeper's runs too.
    let events = keeplight_core::agent::Engine::new(keeplight_core::paths::Paths::from_env(), address.clone());
    let caller = keeplight_core::agent_kit::gate::Caller::Background { name: "keeper".into() };
    keeper::run(&address, &setup_path, &rx, timing, |why, result| {
        // Nothing kept and nothing wrong: nothing worth a log line.
        if !matches!(result, Ok(run) if run.reports.is_empty()) {
            let lines = log_lines(&crate::platform::local_timestamp(), why, result);
            let _ = crate::platform::append_log(&lines);
            let problems = keeper::problems(result);
            let level = if problems.is_empty() { LogLevel::Info } else { LogLevel::Warn };
            let summary = lines[0].split_once("  ").map_or(lines[0].as_str(), |(_, rest)| rest);
            let message = std::iter::once(summary.to_string()).chain(problems).collect::<Vec<_>>().join(" ");
            events.log(level, "keeper.put_back", &caller, None, None, &message, None);
        }
        // Tell you once when something didn't hold, not on every wake after.
        if let (Some(tray), Some(notice)) = (&tray, notices.next(keeper::problems(result))) {
            tray.notify(notice);
        }
    });
    0
}

/// The log entry: one summary line, then an indented line for each device
/// that didn't hold.
pub fn log_lines(when: &str, why: Trigger, result: &Result<Run, ActionError>) -> Vec<String> {
    let reason = why.reason();
    let run = match result {
        Ok(run) => run,
        Err(e) => return vec![format!("{when}  Put back nothing {reason}: {e}")],
    };
    let total = run.reports.len();
    let held = run.held();
    let mut summary =
        format!("{when}  Put back {held} of {total} kept device{} {reason}", if total == 1 { "" } else { "s" });
    let waited = run.waited.as_secs();
    if waited > 0 {
        summary.push_str(&format!(", after waiting {waited} s for OpenRGB"));
    }
    summary.push('.');
    let mut lines = vec![summary];
    for r in run.reports.iter().filter(|r| r.result != Ok(Outcome::Held)) {
        let why = match &r.result {
            Ok(Outcome::NotHeld) => "sent, but it reads back as something else".to_string(),
            Ok(Outcome::Skipped(reason)) => format!("not sent: {reason}"),
            Ok(Outcome::Held) => unreachable!(),
            Err(reason) => format!("not sent: {reason}"),
        };
        lines.push(format!("    {} at {}: {why}", r.name, r.location));
    }
    lines
}

#[cfg(test)]
mod tests {
    use keeplight_core::keep::Report;

    use super::*;

    fn report(location: &str, result: Result<Outcome, String>) -> Report {
        Report { name: "ENE DRAM".into(), location: location.into(), result }
    }

    #[test]
    fn a_clean_run_is_one_line_with_its_reason() {
        let run = Run {
            reports: vec![report("0x71", Ok(Outcome::Held)), report("0x73", Ok(Outcome::Held))],
            waited: Duration::ZERO,
        };
        assert_eq!(
            log_lines("2026-11-13 21:00:00", Trigger::Wake, &Ok(run)),
            ["2026-11-13 21:00:00  Put back 2 of 2 kept devices after the PC woke."]
        );
    }

    #[test]
    fn devices_that_did_not_hold_get_their_own_lines() {
        let run = Run {
            reports: vec![
                report("0x71", Ok(Outcome::Held)),
                report("0x73", Err("OpenRGB doesn't list it".into())),
                report("0x75", Ok(Outcome::NotHeld)),
            ],
            waited: Duration::from_secs(9),
        };
        let lines = log_lines("T", Trigger::Start, &Ok(run));
        assert_eq!(lines[0], "T  Put back 1 of 3 kept devices when the keeper started, after waiting 9 s for OpenRGB.");
        assert_eq!(lines[1], "    ENE DRAM at 0x73: not sent: OpenRGB doesn't list it");
        assert_eq!(lines[2], "    ENE DRAM at 0x75: sent, but it reads back as something else");
    }

    #[test]
    fn a_failed_run_says_why() {
        let lines = log_lines("T", Trigger::OpenRgbBack, &Err(ActionError::NotFound));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("T  Put back nothing after OpenRGB came back: "));
    }
}
