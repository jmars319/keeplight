//! The event log (part D): one JSON object per line in
//! `<prefix>-<YYYY-MM-DD>.jsonl` in the app's `logs` folder, a file per day
//! (UTC), kept for 14 days. Every change and every failed operation is
//! recorded with who made it (`gui`, `cli` or `mcp:<client>`). Logging is
//! best effort: a line that can't be written never stops the operation it
//! describes. A line never holds a file's contents or a secret.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::append_line;
use super::error::ErrorBody;

/// How many days of event logs are kept.
pub const KEEP_DAYS: u64 = 14;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// How serious a logged event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// One line of the event log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LogEvent {
    /// Milliseconds since 1970 (UTC).
    pub ts_ms: u64,
    pub level: LogLevel,
    /// What happened, dotted, such as `plan.applied` or `sync.failed`.
    pub event: String,
    /// Who made it happen: `gui`, `cli` or `mcp:<client>`.
    pub caller: String,
    /// What it acted on, as the app names targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// The item within the target (a mod, a record, a device), when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// The plan, when the event is part of one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub message: String,
    /// Set when the event is a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

/// `YYYY-MM-DD` (UTC) for a time in milliseconds since 1970.
pub fn date(ts_ms: u64) -> String {
    // Howard Hinnant's days-to-civil algorithm.
    let z = (ts_ms / DAY_MS) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `YYYY-MM-DD HH:MM:SS` (UTC).
pub fn timestamp(ts_ms: u64) -> String {
    let secs = (ts_ms % DAY_MS) / 1000;
    format!("{} {:02}:{:02}:{:02}", date(ts_ms), secs / 3600, secs / 60 % 60, secs % 60)
}

/// The app's event log folder, with its file name prefix (the app's short
/// name, such as `keeplight`).
#[derive(Debug, Clone)]
pub struct EventLog {
    dir: PathBuf,
    prefix: String,
}

impl EventLog {
    pub fn new(dir: impl Into<PathBuf>, prefix: &str) -> EventLog {
        EventLog { dir: dir.into(), prefix: prefix.to_string() }
    }

    fn file_for(&self, ts_ms: u64) -> String {
        format!("{}-{}.jsonl", self.prefix, date(ts_ms))
    }

    /// The log files, oldest first.
    fn files(&self) -> Vec<PathBuf> {
        let start = format!("{}-", self.prefix);
        let mut out: Vec<PathBuf> = std::fs::read_dir(&self.dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with(&start) && n.ends_with(".jsonl"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    pub fn append(&self, event: &LogEvent) -> std::io::Result<()> {
        let line = serde_json::to_string(event).expect("an event serializes");
        append_line(&self.dir.join(self.file_for(event.ts_ms)), &line)
    }

    /// Deletes event logs older than [`KEEP_DAYS`]. Best effort; call it when
    /// the app or its command line starts.
    pub fn prune(&self, now_ms: u64) {
        let oldest = self.file_for(now_ms.saturating_sub(KEEP_DAYS * DAY_MS));
        for f in self.files() {
            if f.file_name().and_then(|n| n.to_str()).is_some_and(|n| n < oldest.as_str()) {
                let _ = std::fs::remove_file(f);
            }
        }
    }

    /// The newest `limit` events, newest last. Lines that can't be read are skipped.
    pub fn recent(&self, limit: usize) -> Vec<LogEvent> {
        let mut out: Vec<LogEvent> = Vec::new();
        for f in self.files().into_iter().rev() {
            let text = std::fs::read_to_string(&f).unwrap_or_default();
            let mut day: Vec<LogEvent> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
            day.append(&mut out);
            out = day;
            if out.len() >= limit {
                break;
            }
        }
        let skip = out.len().saturating_sub(limit);
        out.split_off(skip)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests {
    use super::super::TestDir;
    use super::*;

    fn event(ts_ms: u64, name: &str) -> LogEvent {
        LogEvent {
            ts_ms,
            level: LogLevel::Info,
            event: name.into(),
            caller: "cli".into(),
            target: None,
            subject: None,
            plan: None,
            message: String::new(),
            error: None,
        }
    }

    #[test]
    fn dates_are_utc_calendar_days() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400_000), "2000-02-29");
        assert_eq!(date(1_794_560_376_416), "2026-11-13");
        assert_eq!(timestamp(1_794_560_376_416), "2026-11-13 08:59:36");
    }

    #[test]
    fn appends_by_day_reads_newest_last_and_prunes_old_days() {
        let tmp = TestDir::new("events");
        let log = EventLog::new(tmp.0.join("logs"), "app");
        let now = 1_794_560_376_416;
        for (i, ts) in [now - 20 * DAY_MS, now - 2 * DAY_MS, now - DAY_MS, now].into_iter().enumerate() {
            log.append(&event(ts, &format!("e{i}"))).unwrap();
        }
        log.append(&event(now + 1, "e4")).unwrap();
        std::fs::write(log.dir().join("other-2000-01-01.jsonl"), "").unwrap();
        let names = |v: Vec<LogEvent>| v.into_iter().map(|e| e.event).collect::<Vec<_>>();
        assert_eq!(names(log.recent(3)), ["e2", "e3", "e4"]);
        log.prune(now);
        assert_eq!(names(log.recent(10)), ["e1", "e2", "e3", "e4"]);
        assert!(log.dir().join("other-2000-01-01.jsonl").exists(), "another prefix is left alone");
        let line = serde_json::to_value(event(0, "x")).unwrap();
        assert!(line.get("error").is_none() && line.get("target").is_none());
    }
}
