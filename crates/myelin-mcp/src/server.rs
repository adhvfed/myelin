use std::io::{BufRead, Read, Write};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::governance::{
    CallOutcome, GovernedRouter, ReadAuditOutcome, ReadAuthorization, ReadRefusalCategory,
};
use crate::protocol::{
    error_response, parse_request, success, RpcError, AUTHORIZATION_REFUSED, GOVERNANCE_NOT_WIRED,
    INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND,
};
use crate::registry::ToolRegistry;
use myelin_agent::EffectKind;
use myelin_agent_service::validate_tool_arguments;
use myelin_events::Timestamp;
use myelin_identity::Principal;

pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

pub type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirectReadError {
    InvalidInput(String),
    Denied,
    NotFound,
    Unavailable,
}

pub trait DirectReadExecutor: Send + Sync {
    fn execute(
        &self,
        principal: &Principal,
        authority: &ReadAuthorization,
        tool: &str,
        arguments: &Value,
    ) -> Result<Value, DirectReadError>;
}

pub struct McpServer {
    registry: ToolRegistry,
    router: Option<GovernedRouter>,
    read_executor: Option<Arc<dyn DirectReadExecutor>>,
    clock: Clock,
}

impl McpServer {
    pub fn new_catalogue_only() -> McpServer {
        McpServer {
            registry: ToolRegistry::platform()
                .expect("the built-in MCP catalogue must remain valid"),
            router: None,
            read_executor: None,
            clock: Arc::new(system_now),
        }
    }

    pub fn with_router(registry: ToolRegistry, router: GovernedRouter) -> McpServer {
        McpServer::with_router_and_clock(registry, router, Arc::new(system_now))
    }

    pub fn with_router_and_clock(
        registry: ToolRegistry,
        router: GovernedRouter,
        clock: Clock,
    ) -> McpServer {
        McpServer {
            registry,
            router: Some(router),
            read_executor: None,
            clock,
        }
    }

    pub fn with_router_and_reads(
        registry: ToolRegistry,
        router: GovernedRouter,
        read_executor: Arc<dyn DirectReadExecutor>,
    ) -> McpServer {
        McpServer::with_router_reads_and_clock(
            registry,
            router,
            read_executor,
            Arc::new(system_now),
        )
    }

    pub fn with_router_reads_and_clock(
        registry: ToolRegistry,
        router: GovernedRouter,
        read_executor: Arc<dyn DirectReadExecutor>,
        clock: Clock,
    ) -> McpServer {
        McpServer {
            registry,
            router: Some(router),
            read_executor: Some(read_executor),
            clock,
        }
    }

    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    pub fn router(&self) -> Option<&GovernedRouter> {
        self.router.as_ref()
    }

    pub fn handle_line(&self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let req = match parse_request(line) {
            Ok(r) => r,
            Err((id, err)) => return Some(write_value(&error_response(id, err))),
        };
        if req.is_notification {
            return None;
        }
        let response = match req.method.as_str() {
            "initialize" => success(req.id, self.initialize_result()),
            "tools/list" => match self.tools_list() {
                Ok(result) => success(req.id, result),
                Err(err) => error_response(req.id, err),
            },
            "tools/call" => match self.tools_call(&req.params) {
                Ok(result) => success(req.id, result),
                Err(err) => error_response(req.id, err),
            },
            _ => error_response(
                req.id,
                RpcError::new(
                    METHOD_NOT_FOUND,
                    format!("method not found: {}", req.method),
                ),
            ),
        };
        Some(write_value(&response))
    }

    pub fn run(&self, mut reader: impl BufRead, mut writer: impl Write) -> std::io::Result<()> {
        let result = (|| {
            loop {
                let frame = match read_frame(&mut reader)? {
                    Frame::Eof => break,
                    Frame::Payload(frame) => frame,
                    Frame::Oversized => {
                        let error = error_response(
                            Value::Null,
                            RpcError::new(
                                INVALID_REQUEST,
                                format!("JSON-RPC frame exceeds {MAX_FRAME_BYTES} bytes"),
                            ),
                        );
                        write_response(&mut writer, &write_value(&error))?;
                        continue;
                    }
                    Frame::InvalidUtf8 => {
                        let error = error_response(
                            Value::Null,
                            RpcError::new(INVALID_REQUEST, "JSON-RPC frame is not valid UTF-8"),
                        );
                        write_response(&mut writer, &write_value(&error))?;
                        continue;
                    }
                };
                if let Some(resp) = self.handle_line(&frame) {
                    write_response(&mut writer, &resp)?;
                }
                if self.router.as_ref().is_some_and(GovernedRouter::is_fatal) {
                    return Err(std::io::Error::other(
                        "governed MCP session reached an indeterminate mutation outcome",
                    ));
                }
            }
            Ok(())
        })();

        self.teardown();
        result
    }

    fn initialize_result(&self) -> Value {
        json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "myelin-mcp", "version": env!("CARGO_PKG_VERSION") }
        })
    }

    fn tools_list(&self) -> Result<Value, RpcError> {
        let Some(router) = &self.router else {
            return Ok(self.registry.list_result());
        };
        if router.is_fatal() {
            return Err(RpcError::new(
                AUTHORIZATION_REFUSED,
                "governed session is terminal after an indeterminate mutation outcome",
            ));
        }
        let permitted = router
            .permitted_tool_names(&self.registry, &(self.clock)())
            .map_err(|reason| RpcError::new(AUTHORIZATION_REFUSED, reason))?;
        Ok(self.registry.list_result_for(&permitted))
    }

    fn tools_call(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "tools/call requires a string `name`"))?;
        let args = params.get("arguments").cloned().unwrap_or(json!({}));

        let tool = self
            .registry
            .resolve(name)
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, format!("unknown tool: {name}")))?
            .clone();

        validate_tool_arguments(tool.definition(), &args)
            .map_err(|reason| RpcError::new(INVALID_PARAMS, reason))?;

        let router = self.router.as_ref().ok_or_else(|| {
            RpcError::new(
                GOVERNANCE_NOT_WIRED,
                "governed routing not wired into this server instance: a tools/call must act under \
                 a per-run token and then follow its ToolDef route; the minter and execution \
                 adapters are injected by the production composition root, not the catalogue-only \
                 protocol shell.",
            )
        })?;
        if router.is_fatal() {
            return Err(RpcError::new(
                GOVERNANCE_NOT_WIRED,
                "governed session is terminal after an indeterminate mutation outcome",
            ));
        }

        match tool.effect_kind() {
            EffectKind::Read => {
                let executor = self.read_executor.as_ref().ok_or_else(|| {
                    RpcError::new(
                        GOVERNANCE_NOT_WIRED,
                        "direct read routing is not wired into this server instance",
                    )
                })?;
                let authorization = match router.authorize_read(&tool, &args, &(self.clock)()) {
                    Ok(authorization) => authorization,
                    Err(outcome) => return Ok(call_result_json(name, &outcome)),
                };
                let mut result =
                    executor.execute(&router.principal().agent, &authorization, name, &args);
                let audit_outcome = read_audit_outcome(&result);
                if router
                    .complete_read(&authorization, audit_outcome, &(self.clock)())
                    .is_err()
                {
                    result = Err(DirectReadError::Unavailable);
                }
                return Ok(read_result_json(name, authorization.jti(), result));
            }
            EffectKind::Compute => {
                return Err(RpcError::new(
                    GOVERNANCE_NOT_WIRED,
                    "sandbox compute routing is not wired into this MCP server",
                ))
            }
            EffectKind::Mutate | EffectKind::External => {}
        }

        let presented_gate_id = params
            .get("approval")
            .and_then(|a| a.get("gateId"))
            .and_then(Value::as_str);

        let now = (self.clock)();
        let idempotency_key = params
            .get("_meta")
            .and_then(|meta| meta.get("com.myelin/idempotencyKey"))
            .and_then(Value::as_str)
            .filter(|key| {
                !key.is_empty()
                    && key.len() <= 256
                    && key.bytes().all(|byte| byte.is_ascii_graphic())
            })
            .ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    "mutating tools/call requires a printable caller-stable `_meta[\"com.myelin/idempotencyKey\"]` of at most 256 bytes",
                )
            })?;

        let outcome = router.call(&tool, &args, idempotency_key, &now, presented_gate_id);
        Ok(call_result_json(name, &outcome))
    }

    pub fn teardown(&self) {
        if let Some(router) = &self.router {
            router.teardown(&(self.clock)());
        }
    }
}

enum Frame {
    Eof,
    Payload(String),
    Oversized,
    InvalidUtf8,
}

fn read_frame(reader: &mut impl BufRead) -> std::io::Result<Frame> {
    let mut bytes = Vec::with_capacity(4096);
    let read = reader
        .take((MAX_FRAME_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if read == 0 {
        return Ok(Frame::Eof);
    }

    let ended = bytes.last() == Some(&b'\n');
    let payload_len = bytes.len().saturating_sub(usize::from(ended));
    if payload_len > MAX_FRAME_BYTES || !ended && bytes.len() > MAX_FRAME_BYTES {
        if !ended {
            drain_through_newline(reader)?;
        }
        return Ok(Frame::Oversized);
    }

    if ended {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    match String::from_utf8(bytes) {
        Ok(frame) => Ok(Frame::Payload(frame)),
        Err(_) => Ok(Frame::InvalidUtf8),
    }
}

fn drain_through_newline(reader: &mut impl BufRead) -> std::io::Result<()> {
    loop {
        let buffered = reader.fill_buf()?;
        if buffered.is_empty() {
            return Ok(());
        }
        let consumed = buffered
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffered.len(), |position| position + 1);
        let reached_newline = buffered.get(consumed.saturating_sub(1)) == Some(&b'\n');
        reader.consume(consumed);
        if reached_newline {
            return Ok(());
        }
    }
}

fn write_response(writer: &mut impl Write, response: &str) -> std::io::Result<()> {
    writeln!(writer, "{response}")?;
    writer.flush()
}

fn call_result_json(tool: &str, outcome: &CallOutcome) -> Value {
    let (text, is_error) = match outcome {
        CallOutcome::Applied {
            event_id, resource, ..
        } => match resource {
            Some(resource) => (
                format!(
                    "`{tool}` applied through EffectApi (event {event_id}); resource {}.",
                    resource.artifact_ref.0
                ),
                false,
            ),
            None => (
                format!("`{tool}` applied through EffectApi (event {event_id})."),
                false,
            ),
        },
        CallOutcome::Gated { gate_id, .. } => (
            format!("`{tool}` is withheld pending HITL approval (gate {gate_id}); not applied."),
            false,
        ),
        CallOutcome::Denied { reason, .. } => (format!("`{tool}` denied: {reason}"), true),
        CallOutcome::Indeterminate { reason, .. } => (
            format!("`{tool}` has an indeterminate outcome: {reason}"),
            true,
        ),
    };
    let mut meta = json!({ "runToken": outcome.jti(), "tool": tool });
    match outcome {
        CallOutcome::Applied { event_id, .. } => meta["eventId"] = json!(event_id),
        CallOutcome::Gated { gate_id, .. } => meta["gateId"] = json!(gate_id),
        CallOutcome::Denied { reason, .. } => meta["reason"] = json!(reason),
        CallOutcome::Indeterminate { reason, .. } => {
            meta["reason"] = json!(reason);
            meta["fatal"] = json!(true);
        }
    }
    let mut result = json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": is_error,
        "_meta": meta
    });
    if let CallOutcome::Applied {
        event_id, resource, ..
    } = outcome
    {
        result["structuredContent"] = match resource {
            Some(resource) => json!({
                "data": resource.data,
                "ref": resource.artifact_ref.0,
                "event_id": event_id,
            }),
            None => json!({ "event_id": event_id }),
        };
    }
    result
}

fn read_result_json(tool: &str, jti: &str, result: Result<Value, DirectReadError>) -> Value {
    let (text, is_error, reason) = match result {
        Ok(value) => (
            serde_json::to_string(&value)
                .unwrap_or_else(|_| "{\"error\":\"serialization refused\"}".into()),
            false,
            None,
        ),
        Err(DirectReadError::InvalidInput(reason)) => (
            format!("`{tool}` refused invalid input: {reason}"),
            true,
            Some("invalid_input"),
        ),
        Err(DirectReadError::Denied) => (format!("`{tool}` was denied."), true, Some("denied")),
        Err(DirectReadError::NotFound) => (
            format!("`{tool}` did not find a visible resource."),
            true,
            Some("not_found"),
        ),
        Err(DirectReadError::Unavailable) => (
            format!("`{tool}` is temporarily unavailable."),
            true,
            Some("unavailable"),
        ),
    };
    let mut meta = json!({ "runToken": jti, "tool": tool });
    if let Some(reason) = reason {
        meta["reason"] = json!(reason);
    }
    json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": is_error,
        "_meta": meta
    })
}

fn read_audit_outcome(result: &Result<Value, DirectReadError>) -> ReadAuditOutcome {
    match result {
        Ok(_) => ReadAuditOutcome::Succeeded,
        Err(DirectReadError::InvalidInput(_)) => {
            ReadAuditOutcome::Refused(ReadRefusalCategory::InvalidInput)
        }
        Err(DirectReadError::Denied) => {
            ReadAuditOutcome::Refused(ReadRefusalCategory::Authorization)
        }
        Err(DirectReadError::NotFound) => ReadAuditOutcome::Refused(ReadRefusalCategory::NotFound),
        Err(DirectReadError::Unavailable) => {
            ReadAuditOutcome::Refused(ReadRefusalCategory::Unavailable)
        }
    }
}

fn write_value(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"internal serialize error"}}"#
            .to_string()
    })
}

fn system_now() -> Timestamp {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let now = chrono::DateTime::from_timestamp(secs, 0).unwrap_or_default();
    Timestamp(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_returns_capabilities_and_server_info() {
        let s = McpServer::new_catalogue_only();
        let resp = s
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
            .expect("response");
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["protocolVersion"], json!(MCP_PROTOCOL_VERSION));
        assert!(v["result"]["capabilities"]["tools"].is_object());
        assert_eq!(v["result"]["serverInfo"]["name"], json!("myelin-mcp"));
    }

    #[test]
    fn applied_resources_are_addressable_without_parsing_the_event_id() {
        let result = call_result_json(
            "issues.create",
            &CallOutcome::Applied {
                event_id: "issue.create:internal-event|run-1".into(),
                resource: Some(myelin_agent::EffectResource::new(
                    "myelin://acme/issue/issue/ENG-41",
                    json!({ "id": "internal-id", "key": "ENG-41" }),
                )),
                jti: "run-token-jti".into(),
            },
        );

        assert_eq!(
            result["structuredContent"],
            json!({
                "data": { "id": "internal-id", "key": "ENG-41" },
                "ref": "myelin://acme/issue/issue/ENG-41",
                "event_id": "issue.create:internal-event|run-1",
            })
        );
        assert_eq!(
            result["_meta"]["eventId"],
            "issue.create:internal-event|run-1"
        );
    }

    #[test]
    fn tools_list_exposes_the_complete_shared_mcp_surface() {
        let s = McpServer::new_catalogue_only();
        let resp = s
            .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .expect("resp");
        let v: Value = serde_json::from_str(&resp).unwrap();
        let tools = v["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 19);
        let merge = tools.iter().find(|t| t["name"] == "git.merge").unwrap();
        assert_eq!(merge["annotations"]["requiresApproval"], json!(true));
        assert!(tools.iter().any(|tool| tool["name"] == "ci.read_run"));
        assert!(tools.iter().any(|tool| tool["name"] == "issues.list"));
        assert!(tools.iter().any(|tool| tool["name"] == "issues.create"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "knowledge.read_page"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "knowledge.link_work"));
        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "chat.read_messages"));
        assert!(tools.iter().any(|tool| tool["name"] == "chat.post"));
        assert!(tools.iter().any(|tool| tool["name"] == "git.read_file"));
        assert!(tools.iter().any(|tool| tool["name"] == "git.write_file"));
    }

    #[test]
    fn a_notification_yields_no_response() {
        let s = McpServer::new_catalogue_only();
        assert!(s
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());
    }

    #[test]
    fn malformed_line_is_a_jsonrpc_error_not_a_panic() {
        let s = McpServer::new_catalogue_only();
        let resp = s.handle_line("{ this is not json").expect("error response");
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["error"]["code"], json!(crate::protocol::PARSE_ERROR));
    }

    #[test]
    fn unknown_method_is_method_not_found() {
        let s = McpServer::new_catalogue_only();
        let resp = s
            .handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"frobnicate"}"#)
            .unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["error"]["code"], json!(METHOD_NOT_FOUND));
    }

    #[test]
    fn tools_call_without_a_router_is_honestly_not_wired() {
        let s = McpServer::new_catalogue_only();
        let resp = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"alpha","number":1}}}"#,
            )
            .unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["error"]["code"], json!(GOVERNANCE_NOT_WIRED));
    }

    #[test]
    fn tools_call_refuses_arguments_that_do_not_match_the_advertised_schema() {
        let server = McpServer::new_catalogue_only();
        let response = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"git.merge","arguments":{"repo":"alpha","number":"one"}}}"#,
            )
            .unwrap();
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["error"]["code"], json!(INVALID_PARAMS));
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("field `number` must be of type `integer`"));
    }

    #[test]
    fn tools_call_unknown_tool_is_invalid_params() {
        let s = McpServer::new_catalogue_only();
        let resp = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"git.nope"}}"#,
            )
            .unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["error"]["code"], json!(INVALID_PARAMS));
    }

    #[test]
    fn oversized_frame_is_rejected_and_the_next_frame_is_processed() {
        let s = McpServer::new_catalogue_only();
        let input = format!(
            "{}\n{{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/list\"}}\n",
            "x".repeat(MAX_FRAME_BYTES + 1)
        );
        let mut output = Vec::new();

        s.run(input.as_bytes(), &mut output).unwrap();

        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["error"]["code"], json!(INVALID_REQUEST));
        assert_eq!(responses[1]["id"], json!(9));
        assert!(responses[1]["result"]["tools"].is_array());
    }

    #[test]
    fn invalid_utf8_is_rejected_without_lossy_reinterpretation_and_next_frame_survives() {
        let server = McpServer::new_catalogue_only();
        let mut input = vec![0xff, b'\n'];
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":11,\"method\":\"tools/list\"}\n");
        let mut output = Vec::new();
        server.run(input.as_slice(), &mut output).unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["error"]["code"], json!(INVALID_REQUEST));
        assert!(responses[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("UTF-8"));
        assert_eq!(responses[1]["id"], json!(11));
    }

    #[test]
    fn frame_at_the_limit_is_accepted() {
        let mut frame = r#"{"jsonrpc":"2.0","id":10,"method":"tools/list","padding":""#.to_string();
        frame.push_str(&"x".repeat(MAX_FRAME_BYTES - frame.len() - 2));
        frame.push_str("\"}\n");
        assert_eq!(frame.len(), MAX_FRAME_BYTES + 1);
        let mut output = Vec::new();

        McpServer::new_catalogue_only()
            .run(frame.as_bytes(), &mut output)
            .unwrap();

        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["id"], json!(10));
    }
}
