//! Who changed what: every applied plan, from any front end, is appended to
//! `journal.jsonl` in the app's data folder, one JSON object per line, with
//! the caller and the owner's request it was made for.
//!
//! The journal is a record, not a lock: failing to write it never undoes or
//! blocks a change already made, but the apply result must say it failed.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::{AgentError, ErrorBody};
use super::gate::Authorized;
use super::{append_line, now_ms};

/// The journal's file name in the data folder.
pub const FILE: &str = "journal.jsonl";

/// One applied plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    pub ts_ms: u64,
    /// `gui`, `cli` or `mcp:<client>`.
    pub caller: String,
    pub target: String,
    pub plan_id: String,
    /// The owner's request in their own words. Always set for an AI tool.
    pub request: Option<String>,
    /// The steps applied, in order, as the app describes them.
    pub steps: serde_json::Value,
    /// Set when a later step failed: its error. The steps before it stay applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<ErrorBody>,
}

/// Appends one applied plan to `dir/journal.jsonl`.
pub fn append(
    dir: &Path,
    write: &Authorized,
    target: &str,
    plan_id: &str,
    steps: &impl Serialize,
    failed: Option<&AgentError>,
) -> std::io::Result<()> {
    let entry = JournalEntry {
        ts_ms: now_ms(),
        caller: write.caller().label(),
        target: target.to_string(),
        plan_id: plan_id.to_string(),
        request: write.request().map(str::to_string),
        steps: serde_json::to_value(steps).expect("journal steps serialize"),
        failed: failed.map(AgentError::body),
    };
    append_line(&dir.join(FILE), &serde_json::to_string(&entry).expect("a journal entry serializes"))
}

/// The journal's entries, oldest first. Lines that can't be read are skipped.
pub fn read(dir: &Path) -> Vec<JournalEntry> {
    std::fs::read_to_string(dir.join(FILE))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::TestDir;
    use super::super::error::codes;
    use super::super::gate::{Caller, authorize_write};
    use super::*;

    #[test]
    fn records_the_caller_the_request_and_a_failure() {
        let tmp = TestDir::new("journal");
        let ai = authorize_write(&Caller::Mcp { client: "codex".into() }, true, Some("turn off every mod")).unwrap();
        append(&tmp.0, &ai, "game-a", "0123456789abcdef", &["mod B off"], None).unwrap();
        let failed = AgentError::new(codes::SAVE_FAILED, "disk full");
        let cli = authorize_write(&Caller::Cli, false, None).unwrap();
        append(&tmp.0, &cli, "game-a", "fedcba9876543210", &["x", "y"], Some(&failed)).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(tmp.0.join(FILE))
            .and_then(|mut f| std::io::Write::write_all(&mut f, b"not json\n"))
            .unwrap();
        let entries = read(&tmp.0);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].caller, "mcp:codex");
        assert_eq!(entries[0].request.as_deref(), Some("turn off every mod"));
        assert_eq!(entries[1].failed.as_ref().unwrap().code, "save.failed");
        assert_eq!(entries[1].steps, serde_json::json!(["x", "y"]));
    }
}
