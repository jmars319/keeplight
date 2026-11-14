//! Keeplight's MCP server, `keeplight-cli mcp`: the engine's operations as
//! tools, over stdio only (no port, no network). Shaped like the kit's
//! example server (jamarq-agent-kit `rust/tests/mcp.rs`): read tools and the
//! plan tool are read-only, `apply_plan` is destructive, and failures are
//! `ErrorReport`s marked `isError`. The tokio runtime lives only here, inside
//! `agent_kit::mcp::serve_stdio`.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ServerConfig};
use rmcp::{ErrorData as McpError, Peer, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::agent::{ALL_CODES, Change, Engine, SCHEMA_VERSION};
use crate::agent_kit::mcp;

const ABOUT: &str = "Keeplight keeps the lights in this PC the way the user set them. OpenRGB does the hardware \
work; Keeplight keeps a setup per device and puts it back at sign-in, after sleep and when OpenRGB's devices change.

- get_devices lists every device with an id, its mode and color, whether Keeplight keeps it (kept, differs, \
changed or notKept), what it was kept as, and what probably lights it.
- A change is one action on one device: keep (what it shows now), keepDark (only when every light is off), putBack, \
stopKeeping (a Direct device is handed back to its own Onboard lighting), keepAsBefore (goes back to the setup \
before the last stopKeeping or setColor; on a kept device the two swap) or setColor with a color as #rrggbb (keeps \
the device's effect in the new color, or switches to Static or Direct; the color is kept). For a whole-PC color, \
plan one setColor per device.
- What get_devices reads comes from OpenRGB's record. After sleep, RAM can show its own rainbow while the record \
still says the kept color; putBack fixes that.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ApplyArgs {
    /// The planId from plan_change.
    plan_id: String,
    /// What the user asked for, in their own words.
    request: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct LogArgs {
    /// How many of the newest events to return; 50 when left out, at most 500.
    limit: Option<usize>,
}

#[derive(Clone)]
pub struct KeeplightServer {
    tool_router: ToolRouter<Self>,
    engine: Arc<Engine>,
}

#[tool_router(router = tool_router)]
impl KeeplightServer {
    pub fn new(engine: Engine) -> Self {
        Self { tool_router: Self::tool_router(), engine: Arc::new(engine) }
    }

    #[tool(
        description = "Every error code: what it means, what to do, whether retrying can work.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn list_error_codes(&self) -> Result<CallToolResult, McpError> {
        mcp::error_codes(SCHEMA_VERSION, &ALL_CODES)
    }

    #[tool(
        description = "Every lighting device OpenRGB reports: id, name, mode, color, whether Keeplight keeps it and \
                       as what, and what probably lights it. Also whether AI changes are allowed.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn get_devices(&self) -> Result<CallToolResult, McpError> {
        mcp::tool_result(self.engine.devices(), SCHEMA_VERSION)
    }

    #[tool(
        description = "The newest events: every change, who made it (gui, cli or mcp:<app>), and every failure with \
                       its code.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn get_logs(&self, Parameters(a): Parameters<LogArgs>) -> Result<CallToolResult, McpError> {
        let events = self.engine.recent_events(a.limit.unwrap_or(50).min(500));
        mcp::tool_result(Ok(serde_json::json!({ "schemaVersion": SCHEMA_VERSION, "events": events })), SCHEMA_VERSION)
    }

    #[tool(
        description = "Everything for working out a problem at once, redacted: OpenRGB, the devices, the setup in \
                       counts, the settings, the keeper's last runs and the newest events. Start here for \"what \
                       went wrong\".",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn diagnose(&self) -> Result<CallToolResult, McpError> {
        mcp::tool_result(Ok(self.engine.diagnose(serde_json::json!({ "frontEnd": "mcp" }))), SCHEMA_VERSION)
    }

    #[tool(
        description = "Checks one change the user asked for (an action on a device) and returns a plan: its steps \
                       in plain words and a planId. Changes nothing else.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn plan_change(
        &self,
        Parameters(change): Parameters<Change>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        mcp::tool_result(self.engine.plan(&mcp::caller(&peer), change), SCHEMA_VERSION)
    }

    #[tool(
        description = "Applies a plan from plan_change, once, if nothing it depends on changed since. Only when the \
                       user asked for this change; pass their request. Refused (ai.changes_off) unless the user \
                       allows changes from AI tools in Keeplight's Settings.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn apply_plan(
        &self,
        Parameters(a): Parameters<ApplyArgs>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        mcp::tool_result(self.engine.apply(&mcp::caller(&peer), &a.plan_id, Some(&a.request)), SCHEMA_VERSION)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for KeeplightServer {
    fn get_info(&self) -> ServerConfig {
        mcp::server_config("keeplight", env!("CARGO_PKG_VERSION"), mcp::instructions(ABOUT))
    }
}

/// The one async runtime in Keeplight: a single thread, with timers.
fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| format!("Couldn't start the MCP server: {e}"))
}

/// Serves MCP on stdin and stdout until the client disconnects.
///
/// This is the kit's `mcp::serve_stdio` with timers turned on. The kit's
/// version (KIT_VERSION 1) builds its runtime without them, and rmcp uses a
/// timer when the client disconnects, so every session ended in a panic;
/// the kit's own tests miss it because `#[tokio::test]` turns timers on.
/// Reported to jamarq-agent-kit; go back to `mcp::serve_stdio` once the kit
/// is fixed and copied again.
pub fn serve(engine: Engine) -> Result<(), String> {
    use rmcp::ServiceExt;
    runtime()?.block_on(async {
        let service = KeeplightServer::new(engine)
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| format!("The MCP client didn't connect: {e}"))?;
        service.waiting().await.map(|_| ()).map_err(|e| format!("The MCP server stopped: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use rmcp::ServiceExt;
    use rmcp::model::CallToolRequestParams;

    use super::*;
    use crate::agent_kit::journal;
    use crate::paths::Paths;
    use crate::testing::{self, FakeServer};

    async fn call(
        client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
        name: &'static str,
        args: serde_json::Value,
    ) -> CallToolResult {
        let mut params = CallToolRequestParams::new(name);
        params.arguments = args.as_object().cloned();
        client.call_tool(params).await.unwrap()
    }

    fn error_code(r: &CallToolResult) -> String {
        let text = serde_json::to_value(&r.content).unwrap()[0]["text"].as_str().unwrap().to_string();
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["error"]["code"].as_str().unwrap().to_string()
    }

    /// A client disconnecting ends the session cleanly on the runtime `serve`
    /// uses. Not a `#[tokio::test]`, which turns timers on by itself and hid
    /// the kit's missing `enable_time`.
    #[test]
    fn a_client_disconnecting_ends_the_session_cleanly() {
        let data = std::env::temp_dir().join(format!("keeplight-mcp-end-{}", std::process::id()));
        let engine = Engine::new(Paths { data: data.clone() }, "127.0.0.1:9");
        runtime().unwrap().block_on(async {
            let (server_io, client_io) = tokio::io::duplex(64 * 1024);
            let server = tokio::spawn(async move {
                KeeplightServer::new(engine).serve(server_io).await.unwrap().waiting().await.map(|_| ())
            });
            let client = ().serve(client_io).await.unwrap();
            client.cancel().await.unwrap();
            assert!(server.await.is_ok(), "the server task ended without panicking");
        });
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tools_are_annotated_and_ai_changes_need_the_switch_and_a_request() {
        let data = std::env::temp_dir().join(format!("keeplight-mcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let mut rainbow = testing::ram("SMBus 0x71");
        rainbow.active_mode = Some(1);
        let server = FakeServer::start(vec![rainbow]);
        let address = server.address.clone();
        let engine = || Engine::new(Paths { data: data.clone() }, address.clone());
        // Keep the stick as red Chase Fade from the window, then let it lose
        // power: OpenRGB's record goes back to its rainbow.
        server.state.lock().unwrap().devices[0].active_mode = Some(2);
        engine()
            .change_now(&crate::agent_kit::gate::Caller::Window, Change::new(crate::agent::Action::Keep, "ram-stick"))
            .unwrap();
        server.state.lock().unwrap().devices[0].active_mode = Some(1);

        for ai_changes_on in [false, true] {
            engine().settings();
            if ai_changes_on {
                engine().set_ai_changes(true).unwrap();
            }
            let (server_io, client_io) = tokio::io::duplex(64 * 1024);
            let mcp_server = KeeplightServer::new(engine());
            tokio::spawn(async move { mcp_server.serve(server_io).await.unwrap().waiting().await });
            let client = ().serve(client_io).await.unwrap();

            let tools = client.list_all_tools().await.unwrap();
            let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
            assert_eq!(names.len(), 6, "{names:?}");
            for t in &tools {
                let a = t.annotations.as_ref().unwrap();
                let writes = t.name == "apply_plan";
                assert_eq!(a.read_only_hint, Some(!writes), "{}", t.name);
                assert_eq!(a.destructive_hint, Some(writes), "{}", t.name);
            }
            let info = client.peer_info().unwrap();
            let instructions = info.instructions.as_deref().unwrap();
            assert!(instructions.starts_with("Keeplight keeps the lights"));
            assert!(instructions.contains("Change nothing on your own initiative"));

            let codes = call(&client, "list_error_codes", serde_json::json!({})).await;
            assert_eq!(codes.structured_content.unwrap()["codes"].as_array().unwrap().len(), ALL_CODES.len());

            let devices = call(&client, "get_devices", serde_json::json!({})).await;
            let devices = devices.structured_content.unwrap();
            assert_eq!(devices["devices"][0]["id"], "ram-stick");
            assert_eq!(devices["devices"][0]["kept"], "differs");
            assert_eq!(devices["aiChanges"], ai_changes_on);

            let missing =
                call(&client, "plan_change", serde_json::json!({ "action": "putBack", "device": "zzz" })).await;
            assert_eq!(error_code(&missing), "device.not_found");

            let plan =
                call(&client, "plan_change", serde_json::json!({ "action": "putBack", "device": "RAM stick" })).await;
            let plan_id = plan.structured_content.unwrap()["planId"].as_str().unwrap().to_string();
            let apply = |request: &'static str| {
                call(&client, "apply_plan", serde_json::json!({ "planId": plan_id.clone(), "request": request }))
            };

            if !ai_changes_on {
                let refused = apply("put the RAM back").await;
                assert_eq!(refused.is_error, Some(true));
                assert_eq!(error_code(&refused), "ai.changes_off");
                assert_eq!(server.device(0).active_mode_name(), Some("Rainbow"), "nothing was sent");
            } else {
                assert_eq!(error_code(&apply("   ").await), "ai.request_missing");
                let done = apply("put the RAM back").await;
                assert_eq!(done.is_error, Some(false), "{:?}", done.content);
                assert_eq!(server.device(0).active_mode_name(), Some("Chase Fade"));
                assert_eq!(error_code(&apply("put the RAM back").await), "plan.not_found");
                let entries = journal::read(&data);
                assert_eq!(entries.len(), 1);
                assert!(entries[0].caller.starts_with("mcp:"));
                assert_eq!(entries[0].request.as_deref(), Some("put the RAM back"));
                let logs = call(&client, "get_logs", serde_json::json!({ "limit": 20 })).await;
                let logs = logs.structured_content.unwrap();
                assert!(logs["events"].as_array().unwrap().iter().any(|e| e["event"] == "device.put_back"));
                let diagnosis = call(&client, "diagnose", serde_json::json!({})).await;
                assert_eq!(diagnosis.structured_content.unwrap()["sections"]["setup"]["kept"], 1);
            }
            client.cancel().await.unwrap();
        }
        let _ = std::fs::remove_dir_all(&data);
    }
}
