//! Who is asking for a change, and the rule every write an agent can trigger
//! passes first: AI tools may read and plan at any time, but change nothing
//! unless the owner has turned on "Let AI tools make changes" (off by
//! default) and the call carries the owner's own request for that change.
//!
//! The switch lives in the app's own settings, under `aiChanges`, and only
//! the app's window turns it on. This module never reads settings: the app
//! passes the switch's current value.

use super::error::{AgentError, Result, codes};

/// Who is asking the app to do something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The app's own window.
    Window,
    /// The app's command line.
    Cli,
    /// An AI tool through the app's MCP server, by the name its client gave.
    Mcp { client: String },
    /// The app's own background work, such as a keeper or a scheduled job,
    /// by a short name the app chooses (`keeper`). It acts on the owner's
    /// settings, never on a model's request.
    Background { name: String },
}

impl Caller {
    /// `gui`, `cli`, `mcp:<client>` or `background:<name>`, as logs and the
    /// journal record it.
    pub fn label(&self) -> String {
        match self {
            Caller::Window => "gui".into(),
            Caller::Cli => "cli".into(),
            Caller::Mcp { client } => format!("mcp:{}", clean_client(client)),
            Caller::Background { name } => format!("background:{}", clean_client(name)),
        }
    }

    pub fn is_ai(&self) -> bool {
        matches!(self, Caller::Mcp { .. })
    }
}

/// A client name as a short, printable label: what an MCP client says about
/// itself is not trusted to be either.
fn clean_client(name: &str) -> String {
    let cleaned: String =
        name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ')).take(64).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() { "unknown".into() } else { cleaned.into() }
}

/// Proof that a write passed [`authorize_write`]: the plan store's `take`
/// and the journal need one, so no apply path can skip the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorized {
    caller: Caller,
    request: Option<String>,
}

impl Authorized {
    pub fn caller(&self) -> &Caller {
        &self.caller
    }

    /// The owner's request in their own words, trimmed. Always set for an AI
    /// tool; optional for the window and the command line.
    pub fn request(&self) -> Option<&str> {
        self.request.as_deref()
    }
}

/// Checks that `caller` may make a change, before anything is read or
/// claimed.
///
/// - The window, the command line and the app's own background work pass
///   (the command line's own `--yes` rule is the app's to check).
/// - An AI tool is refused with `ai.changes_off` while the switch is off, and
///   with `ai.request_missing` without a request.
pub fn authorize_write(caller: &Caller, ai_changes_on: bool, request: Option<&str>) -> Result<Authorized> {
    let request = request.map(str::trim).filter(|r| !r.is_empty()).map(str::to_string);
    if caller.is_ai() {
        if !ai_changes_on {
            return Err(AgentError::new(
                codes::AI_CHANGES_OFF,
                "Changes from AI tools are turned off in this app's Settings. Nothing was changed.",
            ));
        }
        if request.is_none() {
            return Err(AgentError::new(
                codes::AI_REQUEST_MISSING,
                "Pass the user's own request for this change as `request`. AI tools change nothing on their own \
                 initiative. Nothing was changed.",
            ));
        }
    }
    Ok(Authorized { caller: caller.clone(), request })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_writes_are_off_until_switched_on_and_asked_for() {
        let ai = Caller::Mcp { client: "claude-code".into() };
        assert_eq!(authorize_write(&ai, false, Some("turn it off")).unwrap_err().code, codes::AI_CHANGES_OFF);
        assert_eq!(authorize_write(&ai, true, Some("  ")).unwrap_err().code, codes::AI_REQUEST_MISSING);
        assert_eq!(authorize_write(&ai, true, Some(" turn it off ")).unwrap().request(), Some("turn it off"));
        assert_eq!(authorize_write(&Caller::Cli, false, None).unwrap().request(), None);
        assert_eq!(authorize_write(&Caller::Window, false, None).unwrap().caller(), &Caller::Window);
        let keeper = Caller::Background { name: "keeper".into() };
        assert!(!keeper.is_ai());
        assert_eq!(authorize_write(&keeper, false, None).unwrap().caller().label(), "background:keeper");
    }

    #[test]
    fn labels_name_the_caller_and_never_carry_control_characters() {
        assert_eq!(Caller::Window.label(), "gui");
        assert_eq!(Caller::Cli.label(), "cli");
        assert_eq!(Caller::Mcp { client: "Claude Code".into() }.label(), "mcp:Claude Code");
        assert_eq!(Caller::Mcp { client: "x\n\u{1b}[31m".into() }.label(), "mcp:x31m");
        assert_eq!(Caller::Mcp { client: "\n".into() }.label(), "mcp:unknown");
    }
}
