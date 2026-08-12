use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, Uri};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use myelin_agent::{ToolCall, ToolDef, ToolResult};
use myelin_agent_service::{ToolExecError, ToolExecutionContext, ToolExecutor};
use serde_json::{json, Value};

const REQUEST_DEADLINE: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_RESULT_TEXT_BYTES: usize = 256 * 1024;

type JsonHttpClient = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;

#[derive(Clone)]
pub struct EdgeMcpToolExecutor {
    client: JsonHttpClient,
    base_url: String,
    runtime: tokio::runtime::Handle,
}

impl core::fmt::Debug for EdgeMcpToolExecutor {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("EdgeMcpToolExecutor")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl EdgeMcpToolExecutor {
    pub fn new(
        base_url: impl Into<String>,
        runtime: tokio::runtime::Handle,
    ) -> Result<Self, ToolExecError> {
        let base_url = validate_base_url(base_url.into())?;
        let connector = HttpsConnectorBuilder::new()
            .with_provider_and_native_roots(rustls::crypto::aws_lc_rs::default_provider())
            .map_err(|error| failed(format!("load native TLS trust roots: {error}")))?
            .https_or_http()
            .enable_http1()
            .build();
        Ok(Self {
            client: Client::builder(TokioExecutor::new()).build(connector),
            base_url,
            runtime,
        })
    }

    fn request(
        &self,
        context: &ToolExecutionContext<'_>,
        definition: &ToolDef,
        call: &ToolCall,
    ) -> Result<(Uri, Bytes), ToolExecError> {
        let run_id = uuid::Uuid::parse_str(context.run_id)
            .map_err(|_| failed("hosted tool execution requires a UUID run id"))?;
        if run_id.to_string() != context.run_id {
            return Err(failed(
                "hosted tool execution requires a canonical UUID run id",
            ));
        }
        if call.id.0.is_empty() || call.id.0.len() > 512 {
            return Err(failed(
                "model tool call id is outside its 1..=512 byte bound",
            ));
        }
        let name = definition.canonical_name();
        if call.name.0 != name && call.name != definition.name {
            return Err(failed(
                "model tool call does not match its governed definition",
            ));
        }
        let idempotency_key = stable_idempotency_key(context, call, definition)?;
        let body = json!({
            "jsonrpc": "2.0",
            "id": call.id.0,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": call.arguments,
                "_meta": {
                    "com.myelin/idempotencyKey": idempotency_key,
                },
            },
        });
        let body = serde_json::to_vec(&body)
            .map(Bytes::from)
            .map_err(|error| failed(format!("serialize governed MCP request: {error}")))?;
        let uri = format!("{}/v1/agent-runs/{run_id}/mcp", self.base_url)
            .parse()
            .map_err(|error| failed(format!("build governed MCP endpoint: {error}")))?;
        Ok((uri, body))
    }

    fn post(&self, uri: Uri, bearer: &str, body: Bytes) -> Result<(u16, Bytes), ToolExecError> {
        let client = self.client.clone();
        crate::bridge(&self.runtime, async move {
            let request = Request::builder()
                .method("POST")
                .uri(uri)
                .header("accept", "application/json")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {bearer}"))
                .header("x-myelin-token-scheme", "agent")
                .body(Full::new(body))
                .map_err(|error| failed(format!("build governed MCP request: {error}")))?;
            let response = tokio::time::timeout(REQUEST_DEADLINE, client.request(request))
                .await
                .map_err(|_| failed("governed MCP request exceeded its 30-second deadline"))?
                .map_err(|error| failed(format!("governed MCP request failed: {error}")))?;
            let status = response.status().as_u16();
            let bytes = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
                .collect()
                .await
                .map_err(|error| failed(format!("read governed MCP response: {error}")))?
                .to_bytes();
            Ok((status, bytes))
        })
    }
}

impl ToolExecutor for EdgeMcpToolExecutor {
    fn execute(
        &self,
        context: &ToolExecutionContext<'_>,
        definition: &ToolDef,
        call: &ToolCall,
    ) -> Result<ToolResult, ToolExecError> {
        let (uri, body) = self.request(context, definition, call)?;
        let (status, response) = self.post(uri, &context.run_token.token, body)?;
        if !(200..300).contains(&status) {
            return Err(failed(format!(
                "governed MCP boundary returned HTTP {status}"
            )));
        }
        parse_tool_result(&response, &call.id.0)
    }
}

fn validate_base_url(base_url: String) -> Result<String, ToolExecError> {
    let base_url = base_url.trim_end_matches('/').to_string();
    let uri: Uri = base_url
        .parse()
        .map_err(|error| failed(format!("invalid Edge base URL: {error}")))?;
    let scheme = uri
        .scheme_str()
        .ok_or_else(|| failed("Edge base URL has no scheme"))?;
    let authority = uri
        .authority()
        .ok_or_else(|| failed("Edge base URL has no host"))?;
    if authority.as_str().contains('@') {
        return Err(failed("Edge base URL must not contain user information"));
    }
    if uri
        .path_and_query()
        .is_some_and(|path| path.as_str() != "/")
    {
        return Err(failed("Edge base URL must not contain a path or query"));
    }
    if scheme != "https" && !(scheme == "http" && is_loopback_host(authority.host())) {
        return Err(failed(
            "Edge base URL must use HTTPS (or loopback HTTP for development)",
        ));
    }
    Ok(base_url)
}

fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn stable_idempotency_key(
    context: &ToolExecutionContext<'_>,
    call: &ToolCall,
    definition: &ToolDef,
) -> Result<String, ToolExecError> {
    if context.effect_key.is_empty() || context.effect_key.len() > 1024 {
        return Err(failed(
            "hosted tool effect key is outside its 1..=1024 byte bound",
        ));
    }
    let request_hash = crate::durable_tool::tool_request_hash(definition, call)?;
    let mut digest = blake3::Hasher::new();
    for part in [
        context.run_id.as_bytes(),
        context.effect_key.as_bytes(),
        request_hash.as_bytes(),
    ] {
        digest.update(&(part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    Ok(format!(
        "hosted:{}:{}",
        context.run_id,
        digest.finalize().to_hex()
    ))
}

fn parse_tool_result(response: &[u8], expected_call_id: &str) -> Result<ToolResult, ToolExecError> {
    let envelope: Value = serde_json::from_slice(response)
        .map_err(|error| failed(format!("decode governed MCP response: {error}")))?;
    if envelope.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || envelope.get("id").and_then(Value::as_str) != Some(expected_call_id)
    {
        return Err(failed(
            "governed MCP response is not bound to the requested tool call",
        ));
    }
    if let Some(error) = envelope.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("governed MCP request was refused");
        return Err(failed(message));
    }
    let result = envelope
        .get("result")
        .ok_or_else(|| failed("governed MCP response contains neither result nor error"))?;
    if let Some(gate_id) = result
        .get("_meta")
        .and_then(|meta| meta.get("gateId"))
        .and_then(Value::as_str)
    {
        if gate_id.is_empty()
            || gate_id.len() > 256
            || !gate_id.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(failed(
                "governed MCP response contains an invalid approval gate ID",
            ));
        }
        return Err(ToolExecError::ApprovalRequired {
            gate_id: gate_id.to_string(),
        });
    }
    let mut text = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|content| content.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|content| content.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        text = serde_json::to_string(result)
            .map_err(|error| failed(format!("serialize governed MCP result: {error}")))?;
    }
    if text.len() > MAX_RESULT_TEXT_BYTES {
        return Err(failed(
            "governed MCP tool result exceeds its 256 KiB model bound",
        ));
    }
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Ok(ToolResult::Refused { refused: text });
    }
    Ok(ToolResult::Succeeded(text))
}

fn failed(reason: impl Into<String>) -> ToolExecError {
    ToolExecError::Failed(reason.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent::{EffectKind, ToolCallId, ToolName};

    fn read_call() -> (ToolDef, ToolCall) {
        (
            ToolDef {
                name: ToolName("read_run".into()),
                subsystem: "ci".into(),
                version: 1,
                input_schema: r#"{"type":"object"}"#.into(),
                required_caps: vec!["run.view".into()],
                effect_kind: EffectKind::Read,
                side_effecting: false,
                requires_approval: false,
                exposed_over_mcp: true,
            },
            ToolCall {
                id: ToolCallId("model-call-1".into()),
                name: ToolName("ci.read_run".into()),
                arguments: json!({"run_id": "run-to-read"}),
            },
        )
    }

    fn context(effect_key: &'static str) -> ToolExecutionContext<'static> {
        let token = Box::leak(Box::new(myelin_flow::RunTokenHandle {
            token: "secret-test-token".into(),
            jti: "test-jti".into(),
            ttl_secs: 60,
        }));
        ToolExecutionContext {
            run_id: "01234567-89ab-cdef-0123-456789abcdef",
            run_token: token,
            effect_key,
        }
    }

    #[test]
    fn bearer_transport_refuses_cleartext_beyond_loopback() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let handle = runtime.handle().clone();
        assert!(EdgeMcpToolExecutor::new("https://edge.example.test", handle.clone()).is_ok());
        assert!(EdgeMcpToolExecutor::new("http://127.0.0.1:8080", handle.clone()).is_ok());
        assert!(EdgeMcpToolExecutor::new("http://edge.example.test", handle.clone()).is_err());
        assert!(
            EdgeMcpToolExecutor::new("https://user@edge.example.test", handle.clone()).is_err()
        );
        assert!(EdgeMcpToolExecutor::new("https://edge.example.test/path", handle).is_err());
    }

    #[test]
    fn mutation_retry_key_is_stable_bounded_and_request_specific() {
        let (definition, call) = read_call();
        let base_context = context("model-turn/0/tool/0");
        let first = stable_idempotency_key(&base_context, &call, &definition).unwrap();
        let replay = stable_idempotency_key(&base_context, &call, &definition).unwrap();
        let mut changed = call.clone();
        changed.arguments = json!({"run_id": "another-run"});
        let different = stable_idempotency_key(&base_context, &changed, &definition).unwrap();

        assert_eq!(first, replay);
        assert_ne!(first, different);
        assert!(first.len() <= 256);
        assert!(first.bytes().all(|byte| byte.is_ascii_graphic()));

        let mut renamed = call.clone();
        renamed.id.0 = "provider-chose-another-id".into();
        assert_eq!(
            first,
            stable_idempotency_key(&base_context, &renamed, &definition).unwrap(),
            "provider correlation IDs do not define durable effects",
        );
        assert_ne!(
            first,
            stable_idempotency_key(&context("model-turn/0/tool/1"), &call, &definition,).unwrap(),
            "two calls with the same arguments remain distinct logical effects",
        );
    }

    #[test]
    fn governed_denials_return_to_the_model_as_tool_results() {
        let response = json!({
            "jsonrpc": "2.0",
            "id": "call-1",
            "result": {
                "content": [{"type": "text", "text": "`ci.read_run` was denied."}],
                "isError": true,
                "_meta": {"reason": "denied"},
            },
        });
        let result = parse_tool_result(response.to_string().as_bytes(), "call-1").unwrap();
        assert!(result.is_refused());
        assert!(result.content().contains("denied"));
    }

    #[test]
    fn governed_approval_gates_interrupt_the_host_before_another_model_turn() {
        let response = json!({
            "jsonrpc": "2.0",
            "id": "call-1",
            "result": {
                "content": [{"type": "text", "text": "The effect is waiting for approval."}],
                "isError": false,
                "_meta": {"gateId": "gate-merge-42"},
            },
        });

        assert_eq!(
            parse_tool_result(response.to_string().as_bytes(), "call-1"),
            Err(ToolExecError::ApprovalRequired {
                gate_id: "gate-merge-42".into(),
            })
        );
    }

    #[test]
    fn a_response_for_another_call_is_never_misbound() {
        let response = json!({
            "jsonrpc": "2.0",
            "id": "another-call",
            "result": {"content": [{"type": "text", "text": "wrong"}]},
        });
        let error = parse_tool_result(response.to_string().as_bytes(), "expected-call")
            .expect_err("a response for another call must fail closed");
        assert!(error.to_string().contains("not bound"));
    }
}
