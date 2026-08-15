use serde_json::{json, Value};

pub const JSONRPC_VERSION: &str = "2.0";

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
pub const AUTHORIZATION_REFUSED: i64 = -32001;
pub const GOVERNANCE_NOT_WIRED: i64 = -32004;
const MAX_REQUEST_ID_BYTES: usize = 512;
const MAX_METHOD_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: Value,
    pub method: String,
    pub params: Value,
    pub is_notification: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> RpcError {
        self.data = Some(data);
        self
    }
}

pub fn parse_request(line: &str) -> Result<Request, (Value, RpcError)> {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Err((
                Value::Null,
                RpcError::new(PARSE_ERROR, format!("parse error: {e}")),
            ));
        }
    };
    let object = value.as_object().ok_or_else(|| {
        (
            Value::Null,
            RpcError::new(INVALID_REQUEST, "JSON-RPC request must be an object"),
        )
    })?;

    let id_present = object.contains_key("id");
    let id = match object.get("id") {
        None => Value::Null,
        Some(id @ (Value::Null | Value::Number(_))) => id.clone(),
        Some(Value::String(id)) if id.len() <= MAX_REQUEST_ID_BYTES => Value::String(id.clone()),
        Some(_) => {
            return Err((
                Value::Null,
                RpcError::new(
                    INVALID_REQUEST,
                    "JSON-RPC `id` must be null, a number, or a string of at most 512 bytes",
                ),
            ))
        }
    };

    if object.get("jsonrpc").and_then(Value::as_str) != Some(JSONRPC_VERSION) {
        return Err((
            id,
            RpcError::new(INVALID_REQUEST, "jsonrpc version must be \"2.0\""),
        ));
    }

    let method = match object.get("method").and_then(Value::as_str) {
        Some(method)
            if !method.is_empty()
                && method.len() <= MAX_METHOD_BYTES
                && method.bytes().all(|byte| byte.is_ascii_graphic()) =>
        {
            method.to_string()
        }
        None => {
            return Err((
                id,
                RpcError::new(INVALID_REQUEST, "missing or non-string `method`"),
            ))
        }
        Some(_) => {
            return Err((
                id,
                RpcError::new(
                    INVALID_REQUEST,
                    "JSON-RPC `method` must be 1..=256 ASCII-graphic bytes",
                ),
            ))
        }
    };

    let params = match object.get("params") {
        None => Value::Null,
        Some(params @ (Value::Array(_) | Value::Object(_))) => params.clone(),
        Some(_) => {
            return Err((
                id,
                RpcError::new(
                    INVALID_REQUEST,
                    "JSON-RPC `params`, when present, must be an object or array",
                ),
            ))
        }
    };

    Ok(Request {
        id,
        method,
        params,
        is_notification: !id_present,
    })
}

pub fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": JSONRPC_VERSION, "id": id, "result": result })
}

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
        assert_eq!(
            id,
            json!(7),
            "the id is recovered so the peer can correlate the error"
        );
        assert_eq!(err.code, INVALID_REQUEST);
    }

    #[test]
    fn wrong_jsonrpc_version_is_invalid_request() {
        let (_id, err) = parse_request(r#"{"jsonrpc":"1.0","id":1,"method":"x"}"#).unwrap_err();
        assert_eq!(err.code, INVALID_REQUEST);
    }

    #[test]
    fn version_is_required_and_the_top_level_must_be_an_object() {
        for request in [r#"{"id":1,"method":"tools/list"}"#, "[]", "null"] {
            let (id, error) = parse_request(request).unwrap_err();
            assert_eq!(error.code, INVALID_REQUEST);
            if request.starts_with('{') {
                assert_eq!(id, json!(1), "a valid correlation id is retained");
            } else {
                assert_eq!(id, Value::Null);
            }
        }
    }

    #[test]
    fn request_id_has_one_bounded_json_rpc_interpretation() {
        for id in [
            json!(true),
            json!([]),
            json!({"nested": 1}),
            json!("x".repeat(513)),
        ] {
            let request = json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"});
            let (response_id, error) = parse_request(&request.to_string()).unwrap_err();
            assert_eq!(response_id, Value::Null, "an invalid id is never reflected");
            assert_eq!(error.code, INVALID_REQUEST);
        }
    }

    #[test]
    fn params_are_structured_or_omitted() {
        for params in [json!(null), json!(false), json!("arguments")] {
            let request =
                json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": params});
            let (id, error) = parse_request(&request.to_string()).unwrap_err();
            assert_eq!(id, json!(7));
            assert_eq!(error.code, INVALID_REQUEST);
        }
    }
}
