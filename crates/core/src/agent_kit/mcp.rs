//! Helpers for the app's MCP server (part M), built on `rmcp` with its stdio
//! transport only. The server runs only while an AI app runs it, speaks over
//! that app's stdin and stdout, opens no port and makes no network call, so
//! the Loopback Server Guard has nothing to guard here. Where the app's work
//! lives behind its own loopback server, the MCP process calls that server
//! like any other client, and the server keeps its guard and the AI gate.
//!
//! The app writes its own tool router (`#[tool_router]`, one `#[tool]` per
//! operation, read tools annotated read-only, write tools destructive) and
//! uses these helpers for what every app does the same way. The kit's test
//! harness (`tests/mcp.rs` beside the reference copy) is a complete example.

use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData as McpError, Peer, RoleServer, ServerHandler, ServiceExt};
use serde::Serialize;

use super::error::{ErrorCode, ErrorCodeList, Result};
use super::gate::Caller;

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

/// The server's identity, its tools and its instructions.
pub fn server_config(name: &str, version: &str, instructions: String) -> ServerConfig {
    ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        .with_server_info(Implementation::new(name, version))
        .with_instructions(instructions)
}

/// Serves MCP on stdin and stdout until the client disconnects. The only
/// async runtime in the app, kept to this edge.
pub fn serve_stdio<S: ServerHandler>(server: S) -> std::result::Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|e| format!("Couldn't start the MCP server: {e}"))?;
    runtime.block_on(async {
        let service =
            server.serve(rmcp::transport::stdio()).await.map_err(|e| format!("The MCP client didn't connect: {e}"))?;
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
}
