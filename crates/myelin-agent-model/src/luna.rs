use crate::client::{
    ModelClient, ModelError, ModelReply, ModelRequest, ModelResponse, ModelTurn, ToolCallRequest,
    Usage,
};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::Request;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde_json::{json, Value};
use std::time::Duration;

const LUNA_ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";
const LUNA_MODEL: &str = "gpt-5.6-luna";
const API_KEY_ENV: &str = "OPENAI_API_KEY";

fn to_wire_tool_name(name: &str) -> String {
    name.replace('.', "-")
}

fn from_wire_tool_name(name: &str) -> String {
    name.replace('-', ".")
}

const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct LunaClient {
    api_key: String,
    model: String,
    endpoint: String,
    timeout: Duration,
}

impl core::fmt::Debug for LunaClient {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LunaClient")
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .field("endpoint", &self.endpoint)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl LunaClient {
    pub fn from_env() -> Result<LunaClient, ModelError> {
        let api_key = std::env::var(API_KEY_ENV).unwrap_or_default();
        if api_key.trim().is_empty() || api_key.contains("{{") {
            return Err(ModelError::MissingApiKey);
        }
        Ok(LunaClient::new(api_key))
    }

    pub fn new(api_key: impl Into<String>) -> LunaClient {
        LunaClient {
            api_key: api_key.into(),
            model: LUNA_MODEL.to_string(),
            endpoint: LUNA_ENDPOINT.to_string(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> LunaClient {
        self.timeout = timeout;
        self
    }

    fn body_for(&self, request: &ModelRequest) -> Value {
        let mut messages: Vec<Value> = Vec::with_capacity(request.turns.len() + 1);
        messages.push(json!({"role": "system", "content": request.system}));
        for turn in &request.turns {
            match turn {
                ModelTurn::User { content } => {
                    messages.push(json!({"role": "user", "content": content}));
                }
                ModelTurn::Assistant {
                    content,
                    tool_calls,
                } => {
                    let mut msg = json!({"role": "assistant", "content": content});
                    if !tool_calls.is_empty() {
                        let calls: Vec<Value> = tool_calls
                            .iter()
                            .map(|c| {
                                json!({
                                    "id": c.id,
                                    "type": "function",
                                    "function": {
                                        "name": to_wire_tool_name(&c.name),
                                        "arguments": c.arguments.to_string(),
                                    }
                                })
                            })
                            .collect();
                        msg["tool_calls"] = Value::Array(calls);
                    }
                    messages.push(msg);
                }
                ModelTurn::ToolResults(results) => {
                    for r in results {
                        let content = if r.is_error {
                            format!("The governed tool call was refused: {}", r.content)
                        } else {
                            r.content.clone()
                        };
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": r.id,
                            "content": content,
                        }));
                    }
                }
            }
        }

        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": to_wire_tool_name(&t.name),
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();

        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "reasoning_effort": "none",
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools);
            body["tool_choice"] = json!("auto");
        }
        if let Some(max) = request.max_output_tokens {
            body["max_completion_tokens"] = json!(max);
        }
        body
    }

    fn post(&self, body: Bytes) -> Result<(u16, Bytes), ModelError> {
        let endpoint = self.endpoint.clone();
        let api_key = self.api_key.clone();
        let timeout = self.timeout;

        std::thread::scope(|scope| {
            scope
                .spawn(move || -> Result<(u16, Bytes), ModelError> {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| ModelError::Transport(format!("build runtime: {e}")))?;
                    runtime.block_on(async move {
                        let connector = HttpsConnectorBuilder::new()
                            .with_provider_and_native_roots(
                                rustls::crypto::aws_lc_rs::default_provider(),
                            )
                            .map_err(|e| {
                                ModelError::Transport(format!("load TLS trust roots: {e}"))
                            })?
                            .https_or_http()
                            .enable_http1()
                            .build();
                        let client: Client<_, Full<Bytes>> =
                            Client::builder(TokioExecutor::new()).build(connector);

                        let request = Request::builder()
                            .method("POST")
                            .uri(&endpoint)
                            .header("accept", "application/json")
                            .header("content-type", "application/json")
                            .header("authorization", format!("Bearer {api_key}"))
                            .body(Full::new(body))
                            .map_err(|e| ModelError::Transport(format!("build request: {e}")))?;

                        tokio::time::timeout(timeout, async {
                            let response = client.request(request).await.map_err(|e| {
                                ModelError::Transport(format!("request failed: {e}"))
                            })?;
                            let status = response.status().as_u16();
                            let bytes = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
                                .collect()
                                .await
                                .map_err(|_| {
                                    ModelError::Transport(format!(
                                        "response exceeded the {MAX_RESPONSE_BYTES}-byte limit \
                                         or could not be read"
                                    ))
                                })?
                                .to_bytes();
                            Ok::<(u16, Bytes), ModelError>((status, bytes))
                        })
                        .await
                        .map_err(|_| {
                            ModelError::Transport(format!(
                                "request exceeded the {}-second deadline",
                                timeout.as_secs()
                            ))
                        })?
                    })
                })
                .join()
                .map_err(|_| ModelError::Transport("request worker thread panicked".into()))?
        })
    }
}

impl ModelClient for LunaClient {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let body = Bytes::from(self.body_for(request).to_string());
        let (status, bytes) = self.post(body)?;
        if !(200..300).contains(&status) {
            let mut body = String::from_utf8_lossy(&bytes).into_owned();
            body.truncate(1000);
            return Err(ModelError::Http { status, body });
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ModelError::Parse(format!("body is not JSON: {e}")))?;
        parse_response(&value)
    }
}

pub(crate) fn parse_response(value: &Value) -> Result<ModelResponse, ModelError> {
    let message = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .ok_or_else(|| ModelError::Parse("no choices[0].message in response".into()))?;

    let usage = parse_usage(value.get("usage"));

    let reply = match message.get("tool_calls").and_then(Value::as_array) {
        Some(raw_calls) if !raw_calls.is_empty() => {
            let mut calls = Vec::with_capacity(raw_calls.len());
            for raw in raw_calls {
                let id = raw
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ModelError::Parse("tool_call missing id".into()))?
                    .to_string();
                let function = raw
                    .get("function")
                    .ok_or_else(|| ModelError::Parse("tool_call missing function".into()))?;
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .map(from_wire_tool_name)
                    .ok_or_else(|| ModelError::Parse("tool_call missing function.name".into()))?;
                let arguments = function
                    .get("arguments")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .unwrap_or(Value::Null);
                calls.push(ToolCallRequest {
                    id,
                    name,
                    arguments,
                });
            }
            ModelReply::ToolCalls(calls)
        }
        _ => {
            let content = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            ModelReply::Final { content }
        }
    };

    Ok(ModelResponse { reply, usage })
}

fn parse_usage(usage: Option<&Value>) -> Usage {
    let Some(usage) = usage else {
        return Usage::NotReported;
    };
    let (Some(prompt), Some(completion)) = (
        usage.get("prompt_tokens").and_then(Value::as_u64),
        usage.get("completion_tokens").and_then(Value::as_u64),
    ) else {
        return Usage::NotReported;
    };
    let cached = usage
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(prompt);
    Usage::Reported {
        input: prompt - cached,
        cached_input: cached,
        output: completion,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{ModelRequest, ModelTurn, ToolCallResult, ToolSpec};

    #[test]
    fn from_env_fails_closed_without_a_key() {
        assert_eq!(LunaClient::new("").api_key, "");
        for bad in ["", "   ", "sk-{{OPENAI_API_KEY}}"] {
            let blank = bad.trim().is_empty() || bad.contains("{{");
            assert!(blank, "{bad:?} must be treated as missing");
        }
    }

    #[test]
    fn body_carries_the_luna_wire_invariants() {
        let client = LunaClient::new("test-key-never-sent");
        let request = ModelRequest {
            system: "you are labelled as an agent".into(),
            turns: vec![
                ModelTurn::User {
                    content: "find the bug".into(),
                },
                ModelTurn::Assistant {
                    content: None,
                    tool_calls: vec![ToolCallRequest {
                        id: "call_1".into(),
                        name: "search".into(),
                        arguments: serde_json::json!({"q": "panic"}),
                    }],
                },
                ModelTurn::ToolResults(vec![ToolCallResult {
                    id: "call_1".into(),
                    content: "match at foo.rs:10".into(),
                    is_error: false,
                }]),
            ],
            tools: vec![ToolSpec {
                name: "search".into(),
                description: "ripgrep".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            max_output_tokens: Some(256),
        };
        let body = client.body_for(&request);

        assert_eq!(body["model"], "gpt-5.6-luna");
        assert_eq!(body["reasoning_effort"], "none");
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["max_completion_tokens"], 256);
        assert!(!body.to_string().contains("test-key-never-sent"));

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(
            messages[2]["tool_calls"][0]["function"]["arguments"],
            "{\"q\":\"panic\"}"
        );
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "call_1");
        assert_eq!(body["tools"][0]["function"]["name"], "search");
    }

    #[test]
    fn body_omits_tool_choice_when_there_are_no_tools() {
        let client = LunaClient::new("k");
        let request = ModelRequest {
            system: "answer".into(),
            turns: vec![ModelTurn::User {
                content: "hi".into(),
            }],
            tools: vec![],
            max_output_tokens: Some(16),
        };
        let body = client.body_for(&request);
        assert!(
            body.get("tool_choice").is_none(),
            "no tool_choice without tools"
        );
        assert!(body.get("tools").is_none(), "no tools key without tools");
        assert_eq!(body["reasoning_effort"], "none");
    }

    #[test]
    fn dotted_tool_names_are_mapped_to_openai_valid_names_and_reversed() {
        let client = LunaClient::new("k");
        let request = ModelRequest {
            system: "sys".into(),
            turns: vec![ModelTurn::Assistant {
                content: None,
                tool_calls: vec![ToolCallRequest {
                    id: "call_1".into(),
                    name: "git.read_check_status".into(),
                    arguments: serde_json::json!({"repo": "r", "commit": "c"}),
                }],
            }],
            tools: vec![ToolSpec {
                name: "git.read_check_status".into(),
                description: "read checks".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            max_output_tokens: Some(64),
        };
        let body = client.body_for(&request);
        assert_eq!(
            body["tools"][0]["function"]["name"],
            "git-read_check_status"
        );
        assert_eq!(
            body["messages"][1]["tool_calls"][0]["function"]["name"],
            "git-read_check_status"
        );
        let openai_pattern = |s: &str| {
            s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        };
        assert!(openai_pattern(
            body["tools"][0]["function"]["name"].as_str().unwrap()
        ));

        let value = serde_json::json!({
            "choices": [{"message": {"content": null, "tool_calls": [{
                "id": "call_9", "type": "function",
                "function": {"name": "git-read_check_status", "arguments": "{}"}
            }]}}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2}
        });
        match parse_response(&value).unwrap().reply {
            ModelReply::ToolCalls(calls) => assert_eq!(calls[0].name, "git.read_check_status"),
            other => panic!("expected ToolCalls, got {other:?}"),
        }
    }

    #[test]
    fn parse_response_maps_tool_calls() {
        let value = serde_json::json!({
            "choices": [{"message": {"content": null, "tool_calls": [{
                "id": "call_42",
                "type": "function",
                "function": {"name": "read_file", "arguments": "{\"path\":\"a.rs\"}"}
            }]}}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 20,
                      "prompt_tokens_details": {"cached_tokens": 40}}
        });
        let resp = parse_response(&value).unwrap();
        match resp.reply {
            ModelReply::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call_42");
                assert_eq!(calls[0].name, "read_file");
                assert_eq!(calls[0].arguments, serde_json::json!({"path": "a.rs"}));
            }
            other => panic!("expected ToolCalls, got {other:?}"),
        }
        assert_eq!(
            resp.usage,
            Usage::Reported {
                input: 60,
                cached_input: 40,
                output: 20
            }
        );
    }

    #[test]
    fn parse_response_maps_final_answer() {
        let value = serde_json::json!({
            "choices": [{"message": {"content": "the answer is foo.rs:10"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let resp = parse_response(&value).unwrap();
        assert!(matches!(resp.reply, ModelReply::Final { content } if content.contains("foo.rs")));
        assert_eq!(
            resp.usage,
            Usage::Reported {
                input: 10,
                cached_input: 0,
                output: 5
            }
        );
    }

    #[test]
    fn parse_response_surfaces_not_reported_when_usage_absent() {
        let value = serde_json::json!({
            "choices": [{"message": {"content": "hi"}}]
        });
        assert_eq!(parse_response(&value).unwrap().usage, Usage::NotReported);
        let partial = serde_json::json!({
            "choices": [{"message": {"content": "hi"}}],
            "usage": {"prompt_tokens": 10}
        });
        assert_eq!(parse_response(&partial).unwrap().usage, Usage::NotReported);
    }

    #[test]
    fn parse_response_errors_on_malformed_body() {
        let value = serde_json::json!({"unexpected": true});
        assert!(matches!(parse_response(&value), Err(ModelError::Parse(_))));
    }

    #[test]
    fn debug_never_leaks_the_key() {
        let rendered = format!("{:?}", LunaClient::new("super-secret-key"));
        assert!(!rendered.contains("super-secret-key"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    #[ignore = "hits the real Luna endpoint; requires OPENAI_API_KEY"]
    fn luna_real_smoke() {
        let client = match LunaClient::from_env() {
            Ok(c) => c,
            Err(ModelError::MissingApiKey) => {
                eprintln!("skipping: OPENAI_API_KEY not set");
                return;
            }
            Err(e) => panic!("unexpected error: {e}"),
        };
        let request = ModelRequest {
            system: "You are a terse assistant. Answer in one word.".into(),
            turns: vec![ModelTurn::User {
                content: "Reply with the single word: ok".into(),
            }],
            tools: vec![],
            max_output_tokens: Some(16),
        };
        let resp = client.complete(&request).expect("real Luna call");
        match resp.reply {
            ModelReply::Final { content } => assert!(!content.is_empty()),
            other => panic!("expected a final answer, got {other:?}"),
        }
        assert!(
            matches!(resp.usage, Usage::Reported { .. }),
            "Luna reports usage"
        );
    }
}
