//! Typed errors with stable codes (part C). A code is a public contract: once
//! released it keeps its meaning and is never reused. Each carries a plain
//! message for people, a hint saying what to do, whether retrying can work,
//! and the exit code the app's command line uses for it.
//!
//! The kit's own codes are in [`codes`]. An app declares its own the same
//! way, as `ErrorCode` constants, and lists every code it can return (its own
//! and the kit's) for `list_error_codes` and its tests.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a failure maps onto the command line's exit code. The numbers are
/// the same in every app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Exit {
    /// 2: a command line or call that can't be read, or a change without `--yes`.
    Usage,
    /// 3: refused by a rule. Nothing was changed.
    Refused,
    /// 4: something the change depends on changed meanwhile.
    ChangedMeanwhile,
    /// 5: busy: another process, a job or the thing being changed is running.
    Busy,
    /// 6: an operation ran and reported failure.
    Failed,
    /// 10: unexpected, such as a file that could not be read or written.
    Unexpected,
}

impl Exit {
    pub fn code(self) -> i32 {
        match self {
            Exit::Usage => 2,
            Exit::Refused => 3,
            Exit::ChangedMeanwhile => 4,
            Exit::Busy => 5,
            Exit::Failed => 6,
            Exit::Unexpected => 10,
        }
    }
}

/// One stable error code, declared as a constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErrorCode {
    /// Dotted lowercase, such as `plan.expired` or `record.not_found`.
    pub code: &'static str,
    /// What the code means, for the list of codes.
    pub meaning: &'static str,
    /// What to do about it, in a sentence.
    pub hint: &'static str,
    /// Whether trying the same thing again later can work without changing
    /// anything first.
    pub retryable: bool,
    pub exit: Exit,
}

impl ErrorCode {
    pub const fn new(
        code: &'static str,
        meaning: &'static str,
        hint: &'static str,
        retryable: bool,
        exit: Exit,
    ) -> ErrorCode {
        ErrorCode { code, meaning, hint, retryable, exit }
    }

    pub fn exit_code(self) -> i32 {
        self.exit.code()
    }
}

/// The kit's own codes. Apps return these as they are and do not redefine them.
pub mod codes {
    use super::{ErrorCode, Exit};

    pub const USAGE: ErrorCode = ErrorCode::new(
        "usage.invalid",
        "A command or tool call that can't be read, or a change without --yes.",
        "Check the command's help or the tool's input schema. Changes from the command line need --yes.",
        false,
        Exit::Usage,
    );
    pub const PLAN_NOT_FOUND: ErrorCode = ErrorCode::new(
        "plan.not_found",
        "No plan has that id, or it was already applied.",
        "Plan the change again and apply the plan id it gives.",
        false,
        Exit::Refused,
    );
    pub const PLAN_EXPIRED: ErrorCode = ErrorCode::new(
        "plan.expired",
        "The plan is more than 10 minutes old.",
        "Plan the change again and apply the new plan.",
        false,
        Exit::Refused,
    );
    pub const PLAN_OTHER_TARGET: ErrorCode = ErrorCode::new(
        "plan.other_target",
        "The plan was made for something else than the call names.",
        "Apply the plan for the target it was made for, or plan the change again.",
        false,
        Exit::Refused,
    );
    pub const CHANGED_MEANWHILE: ErrorCode = ErrorCode::new(
        "plan.changed_meanwhile",
        "Something the plan depends on changed after the plan was made.",
        "Read the state again, plan the change again, and apply the new plan.",
        true,
        Exit::ChangedMeanwhile,
    );
    pub const AI_CHANGES_OFF: ErrorCode = ErrorCode::new(
        "ai.changes_off",
        "AI tools may read and plan, but changes from them are turned off in the app's Settings.",
        "Tell the user. They can turn on \"Let AI tools make changes\" in the app's Settings, or make the change themselves.",
        false,
        Exit::Refused,
    );
    pub const AI_REQUEST_MISSING: ErrorCode = ErrorCode::new(
        "ai.request_missing",
        "An AI tool asked for a change without the user's request for it.",
        "Pass the user's own words asking for this change as `request`. AI tools change nothing on their own initiative.",
        false,
        Exit::Usage,
    );
    pub const STORE_UNREADABLE: ErrorCode = ErrorCode::new(
        "store.unreadable",
        "One of the app's own files (settings, plans, records) can't be read.",
        "Fix or move aside the named file; the app won't overwrite a file it can't read.",
        false,
        Exit::Refused,
    );
    pub const SAVE_FAILED: ErrorCode = ErrorCode::new(
        "save.failed",
        "A file could not be written.",
        "Check that the folder exists, has space and can be written to, then try again.",
        false,
        Exit::Unexpected,
    );
    pub const IO: ErrorCode = ErrorCode::new(
        "io",
        "Something else went wrong reading or writing a file.",
        "Check the named file and folder, then try again.",
        false,
        Exit::Unexpected,
    );

    /// The kit's codes followed by the app's own: an app's full list, which
    /// keeps working when the kit adds a code.
    pub fn with(app: &[ErrorCode]) -> Vec<ErrorCode> {
        ALL.iter().chain(app).copied().collect()
    }

    /// Every kit code, for an app's full list.
    pub const ALL: [ErrorCode; 10] = [
        USAGE,
        PLAN_NOT_FOUND,
        PLAN_EXPIRED,
        PLAN_OTHER_TARGET,
        CHANGED_MEANWHILE,
        AI_CHANGES_OFF,
        AI_REQUEST_MISSING,
        STORE_UNREADABLE,
        SAVE_FAILED,
        IO,
    ];
}

/// An error any front end can show or return.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentError {
    pub code: ErrorCode,
    /// What went wrong and what was (not) changed, for people.
    pub message: String,
    /// Machine-readable context: ids, paths (already relative or redacted),
    /// versions. Never a secret or a file's contents.
    pub details: Option<serde_json::Value>,
}

impl AgentError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> AgentError {
        AgentError { code, message: message.into(), details: None }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> AgentError {
        self.details = Some(details);
        self
    }

    /// The JSON every interface returns instead of a result.
    pub fn report(&self, schema_version: u32) -> ErrorReport {
        ErrorReport { schema_version, error: self.body() }
    }

    pub fn body(&self) -> ErrorBody {
        ErrorBody {
            code: self.code.code.into(),
            message: self.message.clone(),
            hint: self.code.hint.into(),
            retryable: self.code.retryable,
            exit_code: self.code.exit_code(),
            details: self.details.clone(),
        }
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AgentError {}

/// `Result` with [`AgentError`].
pub type Result<T> = std::result::Result<T, AgentError>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErrorBody {
    /// A stable code, such as `plan.expired`.
    pub code: String,
    /// What went wrong and what was (not) changed, for people.
    pub message: String,
    /// What to do about it.
    pub hint: String,
    /// Whether trying again later, without changing anything, can work.
    pub retryable: bool,
    /// The process exit code the command line uses for this error.
    pub exit_code: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// What every interface returns instead of a result when something fails:
/// the command line's `--json` output and an MCP tool's error result alike.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErrorReport {
    /// The app's own schema version.
    pub schema_version: u32,
    pub error: ErrorBody,
}

/// One entry of the list of error codes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErrorCodeInfo {
    pub code: String,
    pub meaning: String,
    pub hint: String,
    pub retryable: bool,
    pub exit_code: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErrorCodeList {
    pub schema_version: u32,
    pub codes: Vec<ErrorCodeInfo>,
}

impl ErrorCodeList {
    pub fn of(schema_version: u32, all: &[ErrorCode]) -> ErrorCodeList {
        ErrorCodeList {
            schema_version,
            codes: all
                .iter()
                .map(|c| ErrorCodeInfo {
                    code: c.code.into(),
                    meaning: c.meaning.into(),
                    hint: c.hint.into(),
                    retryable: c.retryable,
                    exit_code: c.exit_code(),
                })
                .collect(),
        }
    }
}

/// Checks an app's full list of codes: each unique, dotted lowercase, with a
/// meaning and a hint. Call it from the app's own tests.
pub fn check_codes(all: &[ErrorCode]) -> std::result::Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for c in all {
        if !seen.insert(c.code) {
            return Err(format!("{} is listed twice", c.code));
        }
        if c.code.is_empty() || !c.code.chars().all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_') {
            return Err(format!("{} is not dotted lowercase", c.code));
        }
        if c.meaning.trim().is_empty() || c.hint.trim().is_empty() {
            return Err(format!("{} needs a meaning and a hint", c.code));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kit_codes_are_unique_dotted_and_explained() {
        check_codes(&codes::ALL).unwrap();
        let dup = [codes::IO, codes::IO];
        assert!(check_codes(&dup).is_err());
        const BAD: ErrorCode = ErrorCode::new("Bad Code", "m", "h", false, Exit::Refused);
        assert!(check_codes(&[BAD]).is_err());
    }

    #[test]
    fn a_report_carries_code_hint_retry_and_exit() {
        let e = AgentError::new(codes::CHANGED_MEANWHILE, "x.json changed. Nothing was changed.")
            .with_details(serde_json::json!({ "path": "~/x.json" }));
        let v = serde_json::to_value(e.report(3)).unwrap();
        assert_eq!(v["schemaVersion"], 3);
        assert_eq!(v["error"]["code"], "plan.changed_meanwhile");
        assert_eq!(v["error"]["retryable"], true);
        assert_eq!(v["error"]["exitCode"], 4);
        assert_eq!(v["error"]["details"]["path"], "~/x.json");
        let plain = serde_json::to_value(AgentError::new(codes::IO, "disk full").report(1)).unwrap();
        assert!(plain["error"].get("details").is_none());
        assert_eq!(AgentError::new(codes::IO, "disk full").to_string(), "disk full");
        const MINE: ErrorCode = ErrorCode::new("thing.missing", "m", "h", false, Exit::Refused);
        let all = codes::with(&[MINE]);
        assert_eq!(all.len(), codes::ALL.len() + 1);
        assert_eq!(all.last(), Some(&MINE));
        check_codes(&all).unwrap();
        let list = ErrorCodeList::of(1, &codes::ALL);
        assert_eq!(list.codes.len(), codes::ALL.len());
        assert_eq!(codes::USAGE.exit_code(), 2);
    }
}
