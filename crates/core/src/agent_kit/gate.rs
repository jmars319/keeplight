//! Who is asking for a change, and the rule every write an agent can trigger
//! passes first: AI tools may read and plan at any time, but change nothing
//! unless the owner has turned on "Let AI tools make changes" (off by
//! default) and the call carries the owner's own request for that change.
//!
//! The command line counts as an AI tool by default ([`CliPolicy`]): any AI
//! tool with a shell can run it, so the switch would mean nothing otherwise
//! (owner decision, 2026-11-14, from Atlas's review).
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

/// How the gate treats the app's command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CliPolicy {
    /// The default (owner decision, 2026-11-14): any AI tool with a shell can
    /// run the app's command line, so a command-line write needs the switch
    /// and a request (`--request "..."`), exactly like an MCP call.
    #[default]
    LikeAi,
    /// The command line passes like the window. Only for an app whose command
    /// line can't reach anything an AI tool shouldn't change; say why in the
    /// app's safety model.
    Trusted,
}

/// Checks that `caller` may make a change, before anything is read or
/// claimed, with the default [`CliPolicy::LikeAi`].
///
/// - The window and the app's own background work pass.
/// - An AI tool through MCP, and the command line, are refused with
///   `ai.changes_off` while the switch is off, and with `ai.request_missing`
///   without a request. The command line's own `--yes` rule is still the
///   app's to check.
pub fn authorize_write(caller: &Caller, ai_changes_on: bool, request: Option<&str>) -> Result<Authorized> {
    authorize_write_with(CliPolicy::LikeAi, caller, ai_changes_on, request)
}

/// [`authorize_write`] with an explicit command-line policy.
pub fn authorize_write_with(
    cli: CliPolicy,
    caller: &Caller,
    ai_changes_on: bool,
    request: Option<&str>,
) -> Result<Authorized> {
    let request = request.map(str::trim).filter(|r| !r.is_empty()).map(str::to_string);
    let gated = caller.is_ai() || (*caller == Caller::Cli && cli == CliPolicy::LikeAi);
    if gated {
        if !ai_changes_on {
            return Err(AgentError::new(
                codes::AI_CHANGES_OFF,
                "Changes from AI tools and the command line are turned off in this app's Settings. Nothing was \
                 changed.",
            ));
        }
        if request.is_none() {
            let how = if *caller == Caller::Cli { "--request \"...\"" } else { "`request`" };
            return Err(AgentError::new(
                codes::AI_REQUEST_MISSING,
                format!(
                    "Pass the user's own request for this change as {how}. AI tools change nothing on their own \
                     initiative. Nothing was changed."
                ),
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
        assert_eq!(authorize_write(&Caller::Cli, false, Some("x")).unwrap_err().code, codes::AI_CHANGES_OFF);
        assert_eq!(authorize_write(&Caller::Cli, true, None).unwrap_err().code, codes::AI_REQUEST_MISSING);
        assert!(authorize_write(&Caller::Cli, true, None).unwrap_err().message.contains("--request"));
        assert_eq!(authorize_write(&Caller::Cli, true, Some("turn it off")).unwrap().request(), Some("turn it off"));
        let trusted = authorize_write_with(CliPolicy::Trusted, &Caller::Cli, false, None).unwrap();
        assert_eq!(trusted.request(), None);
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
