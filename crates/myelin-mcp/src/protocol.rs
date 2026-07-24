//! # `protocol` — the hand-built JSON-RPC 2.0 framing (MCP-over-stdio).
//!
//! MCP is **JSON-RPC 2.0** (`initialize` / `tools/list` / `tools/call`) over a stdio transport whose
//! framing is **newline-delimited JSON** (one complete JSON-RPC message per line; a message never
//! contains an embedded newline — `serde_json::to_string` guarantees this). There is **no MCP/rmcp
//! crate in `Cargo.lock`**, so this is hand-built on `serde_json` (the minimal-deps ethos — do not
//! pull an unvetted MCP SDK).
//!
//! **Total over malformed input (the prompt's no-panic rule):** a bad line → a JSON-RPC error
//! response, never a panic. `parse_request` never panics; every helper returns a `Value` the caller
//! can write back verbatim.

use serde_json::{json, Value};

/// The JSON-RPC protocol version string (the only value the `jsonrpc` field may take).
pub const JSONRPC_VERSION: &str = "2.0";

// ── The standard JSON-RPC 2.0 error codes (spec §5.1) + the MCP/governance app codes. ──────────────

/// `-32700` — invalid JSON was received (the line did not parse as JSON).
pub const PARSE_ERROR: i64 = -32700;
/// `-32600` — the JSON is not a valid JSON-RPC Request object.
pub const INVALID_REQUEST: i64 = -32600;
/// `-32601` — the method does not exist / is not supported.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// `-32602` — invalid method parameters (e.g. an unknown tool name on `tools/call`).
pub const INVALID_PARAMS: i64 = -32602;
/// `-32603` — an internal JSON-RPC error.
pub const INTERNAL_ERROR: i64 = -32603;
/// `-32004` — APP: governed routing is not wired into this server instance (no `GovernedRouter`).
/// The catalogue-only shell returns this for `tools/call`; the per-run minter and effect-kind
/// execution adapters are injected by the production composition root.
pub const GOVERNANCE_NOT_WIRED: i64 = -32004;

/// A parsed JSON-RPC request (or notification). `id` is `Null` for a notification (no response is
/// written for one). `params` defaults to `Null` when absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The request id, echoed back on the response. `Value::Null` ⇒ a notification (no id field, or
    /// an explicit null) — the server writes NO response for it.
    pub id: Value,
    /// The invoked method (`initialize` / `tools/list` / `tools/call` / `notifications/...`).
    pub method: String,
    /// The method params (an object/array/null per JSON-RPC).
    pub params: Value,
    /// Whether this is a notification (the `id` member was ABSENT). A notification gets no response
    /// even on error (JSON-RPC §4.1).
    pub is_notification: bool,
}

/// A structured JSON-RPC error (the `error` member of a response).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcError {
    /// The error code (a standard JSON-RPC code or an app code above).
    pub code: i64,
    /// A short human-readable message.
    pub message: String,
    /// Optional structured detail (attribution / outcome metadata).
    pub data: Option<Value>,
}

impl RpcError {
    /// A code+message error with no data.
    pub fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError { code, message: message.into(), data: None }
    }

    /// Attach structured `data` to the error (e.g. the run-token jti / outcome).
    pub fn with_data(mut self, data: Value) -> RpcError {
        self.data = Some(data);
        self
    }
}

/// **Parse one line into a [`Request`] — TOTAL (never panics).** A line that is not valid JSON, or is
/// valid JSON but not a well-formed JSON-RPC request (no string `method`), yields the appropriate
/// [`RpcError`] (with the `id` recovered from the body when possible, else `Null`).
pub fn parse_request(line: &str) -> Result<Request, (Value, RpcError)> {
    // (1) JSON parse. A non-JSON line → -32700 Parse error, id null (we cannot recover an id).
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Err((Value::Null, RpcError::new(PARSE_ERROR, format!("parse error: {e}"))));
        }
    };

    // The id is echoed even on most errors; recover it best-effort (absent ⇒ Null/notification).
    let id_present = value.get("id").is_some();
    let id = value.get("id").cloned().unwrap_or(Value::Null);

    // (2) The `jsonrpc` member must be exactly "2.0" if present (we are lenient: absent is tolerated
    //     for robustness, a wrong value is an Invalid Request).
    if let Some(v) = value.get("jsonrpc") {
        if v.as_str() != Some(JSONRPC_VERSION) {
            return Err((id, RpcError::new(INVALID_REQUEST, "jsonrpc version must be \"2.0\"")));
        }
    }

    // (3) The `method` member must be a string.
    let method = match value.get("method").and_then(Value::as_str) {
        Some(m) => m.to_string(),
        None => return Err((id, RpcError::new(INVALID_REQUEST, "missing or non-string `method`"))),
    };

    let params = value.get("params").cloned().unwrap_or(Value::Null);

    Ok(Request { id, method, params, is_notification: !id_present, })
}

/// Build a JSON-RPC success response `Value` (echoing `id`).
pub fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": JSONRPC_VERSION, "id": id, "result": result })
}

/// Build a JSON-RPC error response `Value` (echoing `id`).
pub fn error_response(id: Value, err: RpcError) -> Value {
    let mut e = json!({ "code": err.code, "message": err.message });
    if let Some(data) = err.data {
        e["data"] = data;
    }
    json!({ "jsonrpc": JSONRPC_VERSION, "id": id, "error": e })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_well_formed_request() {
        let r = parse_request(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#)
            .expect("parses");
        assert_eq!(r.method, "tools/list");
        assert_eq!(r.id, json!(1));
        assert!(!r.is_notification);
    }

    #[test]
    fn a_notification_has_no_id() {
        let r = parse_request(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .expect("parses");
        assert!(r.is_notification);
        assert_eq!(r.id, Value::Null);
    }

    #[test]
    fn malformed_json_is_a_parse_error_not_a_panic() {
        let (id, err) = parse_request("{not json").unwrap_err();
        assert_eq!(id, Value::Null);
        assert_eq!(err.code, PARSE_ERROR);
    }

    #[test]
    fn missing_method_is_invalid_request_with_recovered_id() {
        let (id, err) = parse_request(r#"{"jsonrpc":"2.0","id":7}"#).unwrap_err();
        assert_eq!(id, json!(7), "the id is recovered so the peer can correlate the error");
        assert_eq!(err.code, INVALID_REQUEST);
    }

    #[test]
    fn wrong_jsonrpc_version_is_invalid_request() {
        let (_id, err) = parse_request(r#"{"jsonrpc":"1.0","id":1,"method":"x"}"#).unwrap_err();
        assert_eq!(err.code, INVALID_REQUEST);
    }
}
