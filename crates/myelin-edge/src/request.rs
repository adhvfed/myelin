//! # The transport-agnostic request/response carriers
//!
//! [`EdgeRequest`]/[`EdgeResponse`] are the gateway's I/O at the SEAM — so the request lifecycle
//! (authenticate → resolve → scope → authorize → dispatch → respond) is REAL and unit-testable
//! WITHOUT a socket, and the hyper listener ([`crate::server`]) is a thin adapter that converts
//! `hyper::Request`↔`EdgeRequest` and `EdgeResponse`↔`hyper::Response`. The gateway is TOTAL over a
//! malformed request: every accessor is checked, no slice/parse can panic.

use crate::error::EdgeError;
use crate::sse::SseSubscription;
use serde_json::Value;

/// An inbound request at the edge seam. Header names are stored lowercased (HTTP header names are
/// case-insensitive) so lookups are stable. `body` is the raw bytes (the gateway parses JSON itself,
/// loudly, so a malformed body is a clean 400 — never a panic).
pub struct EdgeRequest {
    /// The HTTP method as an uppercase string (`GET`/`POST`/…).
    pub method: String,
    /// The request path (`/v1/whoami`).
    pub path: String,
    /// The raw query string (without the leading `?`).
    pub query: String,
    /// The headers, names lowercased.
    pub headers: Vec<(String, String)>,
    /// The raw request body.
    pub body: Vec<u8>,
}

impl EdgeRequest {
    /// Build a request, lowercasing the header names (case-insensitive lookups).
    pub fn new(
        method: impl Into<String>,
        path: impl Into<String>,
        query: impl Into<String>,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> EdgeRequest {
        EdgeRequest {
            method: method.into().to_uppercase(),
            path: path.into(),
            query: query.into(),
            headers: headers
                .into_iter()
                .map(|(k, v)| (k.to_lowercase(), v))
                .collect(),
            body,
        }
    }

    /// A header value by (case-insensitive) name.
    pub fn header(&self, name: &str) -> Option<&str> {
        let n = name.to_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, v)| v.as_str())
    }

    /// The `Authorization: Bearer <token>` value (the raw credential material), if present.
    pub fn bearer(&self) -> Option<&str> {
        self.header("authorization")
            .and_then(|h| h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer ")))
            .map(str::trim)
    }

    /// A cookie value by name (parses the `Cookie` header — total over a malformed header).
    pub fn cookie(&self, name: &str) -> Option<String> {
        let raw = self.header("cookie")?;
        for pair in raw.split(';') {
            let pair = pair.trim();
            if let Some((k, v)) = pair.split_once('=') {
                if k.trim() == name {
                    return Some(v.trim().to_string());
                }
            }
        }
        None
    }

    /// A query parameter by key (no percent-decoding on this floor — the convention proof needs the
    /// shape, not a full URL codec; the codec is a thin later layer). Total over a malformed query.
    pub fn query_param(&self, key: &str) -> Option<String> {
        for pair in self.query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == key {
                    return Some(v.to_string());
                }
            }
        }
        None
    }

    /// Parse the body as a JSON object — a malformed body is a LOUD `BadRequest` (never a panic, and
    /// never coerced into an empty object).
    pub fn json_body(&self) -> Result<Value, EdgeError> {
        if self.body.is_empty() {
            return Err(EdgeError::BadRequest("empty request body (expected JSON)".into()));
        }
        serde_json::from_slice(&self.body)
            .map_err(|e| EdgeError::BadRequest(format!("malformed JSON body: {e}")))
    }
}

/// The gateway's response at the seam — either a finished byte body (the JSON view-model/error
/// envelope) or a live SSE stream (the real-time convention). The hyper adapter renders both.
pub enum EdgeResponse {
    /// A finished response (status + headers + body bytes), e.g. a JSON view-model or the error
    /// envelope.
    Bytes {
        /// The HTTP status.
        status: u16,
        /// The `Content-Type`.
        content_type: String,
        /// Extra headers (e.g. `Set-Cookie`).
        headers: Vec<(String, String)>,
        /// The body bytes.
        body: Vec<u8>,
    },
    /// A live Server-Sent-Events stream (the real-time convention).
    Sse {
        /// Extra headers.
        headers: Vec<(String, String)>,
        /// The subscription the server streams frames from.
        sub: SseSubscription,
    },
}

impl EdgeResponse {
    /// A JSON response (`application/json`) with `status` and `value` as the body.
    pub fn json(status: u16, value: &Value) -> EdgeResponse {
        EdgeResponse::Bytes {
            status,
            content_type: "application/json".to_string(),
            headers: Vec::new(),
            body: serde_json::to_vec(value).unwrap_or_default(),
        }
    }

    /// The `{error:{message,code?}}` envelope response for an [`EdgeError`].
    pub fn error(e: &EdgeError) -> EdgeResponse {
        EdgeResponse::json(e.status(), &e.envelope())
    }

    /// A live SSE response over `sub`.
    pub fn sse(sub: SseSubscription) -> EdgeResponse {
        EdgeResponse::Sse { headers: Vec::new(), sub }
    }

    /// Add a header (builder form) — works on either variant.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> EdgeResponse {
        match &mut self {
            EdgeResponse::Bytes { headers, .. } | EdgeResponse::Sse { headers, .. } => {
                headers.push((name.into(), value.into()));
            }
        }
        self
    }

    /// The status code (for the seam-level tests). An SSE response is a 200.
    pub fn status(&self) -> u16 {
        match self {
            EdgeResponse::Bytes { status, .. } => *status,
            EdgeResponse::Sse { .. } => 200,
        }
    }

    /// The JSON body, if this is a `Bytes` response carrying JSON (for the seam-level tests).
    pub fn json_body(&self) -> Option<Value> {
        match self {
            EdgeResponse::Bytes { body, .. } => serde_json::from_slice(body).ok(),
            EdgeResponse::Sse { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_lookup_is_case_insensitive_and_bearer_parses() {
        let req = EdgeRequest::new(
            "get",
            "/v1/whoami",
            "",
            vec![("Authorization".into(), "Bearer abc.def".into())],
            Vec::new(),
        );
        assert_eq!(req.method, "GET");
        assert_eq!(req.header("AUTHORIZATION"), Some("Bearer abc.def"));
        assert_eq!(req.bearer(), Some("abc.def"));
    }

    #[test]
    fn cookie_and_query_parsing_is_total() {
        let req = EdgeRequest::new(
            "GET",
            "/v1/x",
            "limit=10&cursor=zz",
            vec![("cookie".into(), "a=1; myelin_session=SID; b=2".into())],
            Vec::new(),
        );
        assert_eq!(req.cookie("myelin_session"), Some("SID".to_string()));
        assert_eq!(req.cookie("missing"), None);
        assert_eq!(req.query_param("limit"), Some("10".to_string()));
        assert_eq!(req.query_param("cursor"), Some("zz".to_string()));
        // a malformed cookie/query does not panic.
        let bad = EdgeRequest::new("GET", "/", "&&=&x", vec![("cookie".into(), ";;= ;".into())], vec![]);
        assert_eq!(bad.cookie("x"), None);
        assert_eq!(bad.query_param("x"), None);
    }

    #[test]
    fn json_body_is_loud_on_malformed() {
        let bad = EdgeRequest::new("POST", "/", "", vec![], b"{not json".to_vec());
        assert!(matches!(bad.json_body(), Err(EdgeError::BadRequest(_))));
        let empty = EdgeRequest::new("POST", "/", "", vec![], vec![]);
        assert!(matches!(empty.json_body(), Err(EdgeError::BadRequest(_))));
    }
}
