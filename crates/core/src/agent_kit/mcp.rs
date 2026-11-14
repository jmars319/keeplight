//! Helpers for the app's MCP server (part M), built on `rmcp` with its stdio
//! transport only. The server runs only while an AI app runs it, speaks over
//! that app's stdin and stdout, opens no port and makes no network call, so
//! the Loopback Server Guard has nothing to guard here. Where the app's work
//! lives behind its own loopback server, the MCP process calls that server
//! like any other client, and the server keeps its guard and the AI gate.
//!
//! The runtime [`serve_stdio`] builds has timers and no I/O driver, so the
//! server can't open a socket by construction, and an async HTTP client
//! (reqwest, hyper) panics with "IO is disabled" on it. To call the app's own
//! loopback server, either use a blocking `std::net::TcpStream` request inside
//! `tokio::task::spawn_blocking` (Atlas's way, which keeps the kit's tokio
//! features), or, in an app whose tokio already has `net`, build a runtime
//! with `enable_all()` and pass it to [`serve_on_runtime`].
//!
//! rmcp's `#[tool_router]` and `#[tool_handler]` expand to a bare
//! `Result<_, ErrorData>`, so the module that holds them must not import the
//! kit's one-parameter `Result` alias: write `agent_kit::error::Result<T>` in
//! full there.
//!
//! The app writes its own tool router (`#[tool_router]`, one `#[tool]` per
//! operation, read tools annotated read-only, write tools destructive) and
//! uses these helpers for what every app does the same way. The kit's test
//! harness (`tests/mcp.rs` beside the reference copy) is a complete example.

use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::transport::IntoTransport;
use rmcp::{ErrorData as McpError, Peer, RoleServer, ServerHandler, ServiceExt};
use serde::Serialize;

use super::error::{ErrorCode, ErrorCodeList, Result};
use super::events::{EventLog, LogEvent, LogLevel};
use super::gate::Caller;
use super::now_ms;

/// The rules every app's server tells the model, after its own description.
pub const SHARED_RULES: &str = "\
Rules:
- You may read anything at any time through the read tools. For \"what went wrong\", start with diagnose.
- Change nothing on your own initiative. Plan or apply a change only when the user has asked for that change \
in so many words.
- To change something: call the plan tool, show the user the plan's steps, then call the apply tool with the \
planId and the user's request in their own words. A plan expires after 10 minutes and is used once.
- If a change is refused with ai.changes_off, the user has not allowed changes from AI tools in the app's \
Settings. Tell them; don't look for another way.
- Some actions are never tools: anything that sends, posts, pays, publishes, deploys or can't be undone stays a \
click in the app's window.
- Every error has a stable code, a message, a hint and whether retrying can work. Follow the hint.";

/// The app's description followed by [`SHARED_RULES`].
pub fn instructions(about_the_app: &str) -> String {
    format!("{}\n\n{SHARED_RULES}", about_the_app.trim_end())
}

/// A tool's result: the value as structured content, or, on failure, the
/// same `ErrorReport` the command line prints, as text marked `isError`, so
/// the model can act on it and it never has to match the success schema.
pub fn tool_result<T: Serialize>(r: Result<T>, schema_version: u32) -> std::result::Result<CallToolResult, McpError> {
    Ok(match r {
        Ok(v) => CallToolResult::structured(serde_json::to_value(v).expect("tool results serialize")),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(
            serde_json::to_string_pretty(&e.report(schema_version)).expect("errors serialize"),
        )]),
    })
}

/// The result of the `list_error_codes` tool every app offers.
pub fn error_codes(schema_version: u32, all: &[ErrorCode]) -> std::result::Result<CallToolResult, McpError> {
    tool_result(Ok(ErrorCodeList::of(schema_version, all)), schema_version)
}

/// The caller for a tool call, named by the client's own `clientInfo`.
pub fn caller(peer: &Peer<RoleServer>) -> Caller {
    let client = peer.peer_info().map(|i| i.client_info.name.clone()).filter(|n| !n.is_empty());
    Caller::Mcp { client: client.unwrap_or_else(|| "unknown".into()) }
}

/// The event name [`record_client`] logs.
pub const CLIENT_SEEN: &str = "mcp.client_seen";

/// Records the AI app that just connected (its `clientInfo` name and
/// version) in the event log, so Settings can show "Last used by Claude Code
/// 2.1.325 at 22:51": proof from the AI app itself, which costs no tokens.
/// Call it from the server's `on_initialized`:
///
/// ```ignore
/// async fn on_initialized(&self, context: rmcp::service::NotificationContext<RoleServer>) {
///     agent_kit::mcp::record_client(&context.peer, &self.events);
/// }
/// ```
pub fn record_client(peer: &Peer<RoleServer>, log: &EventLog) {
    let (name, version) =
        peer.peer_info().map(|i| (i.client_info.name.clone(), i.client_info.version.clone())).unwrap_or_default();
    let caller = Caller::Mcp { client: if name.is_empty() { "unknown".into() } else { name } };
    let _ = log.append(&client_event(&caller, &version, now_ms()));
}

fn client_event(caller: &Caller, version: &str, ts_ms: u64) -> LogEvent {
    let version: String =
        version.chars().filter(|c| c.is_ascii_alphanumeric() || ".-+".contains(*c)).take(32).collect();
    LogEvent {
        ts_ms,
        level: LogLevel::Info,
        event: CLIENT_SEEN.into(),
        caller: caller.label(),
        target: None,
        subject: None,
        plan: None,
        message: version,
        error: None,
    }
}

/// The AI app that last connected, as [`record_client`] logged it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastClient {
    /// The client's own name, such as `claude-code`.
    pub client: String,
    pub version: String,
    pub ts_ms: u64,
}

/// The newest [`record_client`] entry among the last `scan` events.
pub fn last_client(log: &EventLog, scan: usize) -> Option<LastClient> {
    log.recent(scan).into_iter().rev().find(|e| e.event == CLIENT_SEEN).map(|e| LastClient {
        client: e.caller.strip_prefix("mcp:").unwrap_or(&e.caller).to_string(),
        version: e.message,
        ts_ms: e.ts_ms,
    })
}

/// The server's identity, its tools and its instructions.
pub fn server_config(name: &str, version: &str, instructions: String) -> ServerConfig {
    ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        .with_server_info(Implementation::new(name, version))
        .with_instructions(instructions)
}

/// Serves MCP on stdin and stdout until the client disconnects. The only
/// async runtime in the app, kept to this edge.
pub fn serve_stdio<S: ServerHandler>(server: S) -> std::result::Result<(), String> {
    serve_on(server, rmcp::transport::stdio())
}

/// Serves MCP on `transport` until the client disconnects, on a
/// single-threaded runtime built here. [`serve_stdio`] is this on stdin and
/// stdout; tests pass an in-memory pipe.
pub fn serve_on<S, T, E, A>(server: S, transport: T) -> std::result::Result<(), String>
where
    S: ServerHandler,
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    // Timers on: rmcp uses one while it shuts down, and without them every
    // client disconnect panics (a Windows release build exits 0xC0000409).
    // No I/O driver: the server opens no socket.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| format!("Couldn't start the MCP server: {e}"))?;
    serve_on_runtime(server, transport, &runtime)
}

/// Serves MCP on `transport` until the client disconnects, on a runtime the
/// app built. For an app whose tool handlers need the runtime's I/O driver
/// (an async client for its own loopback server); the runtime must have
/// timers on (`enable_time()` or `enable_all()`).
pub fn serve_on_runtime<S, T, E, A>(
    server: S,
    transport: T,
    runtime: &tokio::runtime::Runtime,
) -> std::result::Result<(), String>
where
    S: ServerHandler,
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    runtime.block_on(async {
        let service = server.serve(transport).await.map_err(|e| format!("The MCP client didn't connect: {e}"))?;
        service.waiting().await.map(|_| ()).map_err(|e| format!("The MCP server stopped: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::super::error::{AgentError, codes};
    use super::*;

    #[test]
    fn failures_are_error_reports_and_successes_are_structured() {
        let ok = tool_result(Ok(serde_json::json!({ "a": 1 })), 4).unwrap();
        assert_eq!(ok.is_error, Some(false));
        assert_eq!(ok.structured_content.unwrap()["a"], 1);
        let err = tool_result::<()>(Err(AgentError::new(codes::AI_CHANGES_OFF, "off")), 4).unwrap();
        assert_eq!(err.is_error, Some(true));
        assert!(err.structured_content.is_none());
        let text = serde_json::to_string(&err.content).unwrap();
        assert!(text.contains("ai.changes_off") && text.contains("schemaVersion"));
        assert!(instructions("About.\n").starts_with("About.\n\nRules:"));
    }

    #[test]
    fn the_last_client_seen_is_read_back_from_the_event_log() {
        let tmp = super::super::TestDir::new("last-client");
        let log = EventLog::new(tmp.0.join("logs"), "app");
        assert_eq!(last_client(&log, 50), None);
        let claude = Caller::Mcp { client: "claude-code".into() };
        log.append(&client_event(&claude, "2.1.325", 1_000)).unwrap();
        log.append(&client_event(&Caller::Mcp { client: "codex".into() }, "0.1\n<x>", 2_000)).unwrap();
        let last = last_client(&log, 50).unwrap();
        assert_eq!((last.client.as_str(), last.version.as_str(), last.ts_ms), ("codex", "0.1x", 2_000));
    }
}
