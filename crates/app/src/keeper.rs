//! `keeplight --keep`: the keeper, with no window. Run at sign-in by the
//! startup entry, it puts back every kept device through
//! `keeplight_core::keep::put_back_all`, writes what happened to the log and
//! exits. Re-applying after sleep and on device changes comes in a later
//! version.

use std::time::Duration;

use keeplight_core::keep::{ActionError, Outcome, Run};

/// How long to wait at sign-in for OpenRGB to start and list every kept
/// device. On this PC the service answers within seconds, but its device scan
/// can lag behind the desktop.
const WAIT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_secs(3);

pub fn run() -> i32 {
    let Some(_only) = crate::platform::SingleInstance::acquire("Local\\Keeplight.Keeper") else {
        return 0;
    };
    let address = crate::platform::openrgb_address();
    let result = keeplight_core::keep::put_back_all(&address, &crate::platform::setup_path(), WAIT, POLL);
    // Nothing kept and nothing wrong: nothing worth a log line.
    if matches!(&result, Ok(run) if run.reports.is_empty()) {
        return 0;
    }
    let _ = crate::platform::append_log(&log_lines(&crate::platform::local_timestamp(), &result));
    0
}

/// The log entry: one summary line, then an indented line for each device
/// that didn't hold.
pub fn log_lines(when: &str, result: &Result<Run, ActionError>) -> Vec<String> {
    let run = match result {
        Ok(run) => run,
        Err(e) => return vec![format!("{when}  Put back nothing: {e}")],
    };
    let total = run.reports.len();
    let held = run.held();
    let mut summary = format!("{when}  Put back {held} of {total} kept device{}", if total == 1 { "" } else { "s" });
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
    fn a_clean_run_is_one_line() {
        let run = Run {
            reports: vec![report("0x71", Ok(Outcome::Held)), report("0x73", Ok(Outcome::Held))],
            waited: Duration::ZERO,
        };
        assert_eq!(log_lines("2026-11-13 21:00:00", &Ok(run)), ["2026-11-13 21:00:00  Put back 2 of 2 kept devices."]);
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
        let lines = log_lines("T", &Ok(run));
        assert_eq!(lines[0], "T  Put back 1 of 3 kept devices, after waiting 9 s for OpenRGB.");
        assert_eq!(lines[1], "    ENE DRAM at 0x73: not sent: OpenRGB doesn't list it");
        assert_eq!(lines[2], "    ENE DRAM at 0x75: sent, but it reads back as something else");
    }

    #[test]
    fn a_failed_run_says_why() {
        let lines = log_lines("T", &Err(ActionError::NotFound));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("T  Put back nothing: "));
    }
}
