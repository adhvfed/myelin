use serde_json::{json, Value};

pub const JSONRPC_VERSION: &str = "2.0";

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
pub const AUTHORIZATION_REFUSED: i64 = -32001;
pub const GOVERNANCE_NOT_WIRED: i64 = -32004;

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

    let id_present = value.get("id").is_some();
    let id = value.get("id").cloned().unwrap_or(Value::Null);

    if let Some(v) = value.get("jsonrpc") {
        if v.as_str() != Some(JSONRPC_VERSION) {
            return Err((
                id,
                RpcError::new(INVALID_REQUEST, "jsonrpc version must be \"2.0\""),
            ));
        }
    }

    let method = match value.get("method").and_then(Value::as_str) {
        Some(m) => m.to_string(),
        None => {
            return Err((
                id,
                RpcError::new(INVALID_REQUEST, "missing or non-string `method`"),
            ))
        }
    };

    let params = value.get("params").cloned().unwrap_or(Value::Null);

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
}
