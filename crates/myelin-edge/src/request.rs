use crate::error::EdgeError;
use crate::sse::SseSubscription;
use serde_json::Value;

pub(crate) fn require_empty_json_object(
    bytes: &[u8],
    operation: &str,
    max_bytes: usize,
) -> Result<(), EdgeError> {
    if bytes.len() > max_bytes {
        return Err(EdgeError::PayloadTooLarge(format!(
            "{operation} request body exceeds {max_bytes} bytes"
        )));
    }
    if bytes.is_empty() {
        return Err(EdgeError::BadRequest(
            "empty request body (expected an empty JSON object)".into(),
        ));
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(object)) if object.is_empty() => Ok(()),
        Ok(_) => Err(EdgeError::BadRequest(format!(
            "invalid {operation} body: expected an empty JSON object"
        ))),
        Err(error) => Err(EdgeError::BadRequest(format!(
            "invalid {operation} body: {error}"
        ))),
    }
}

pub struct EdgeRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl EdgeRequest {
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

    pub fn header(&self, name: &str) -> Option<&str> {
        let n = name.to_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, v)| v.as_str())
    }

    pub fn bearer(&self) -> Option<&str> {
        self.header("authorization")
            .and_then(|h| {
                h.strip_prefix("Bearer ")
                    .or_else(|| h.strip_prefix("bearer "))
            })
            .map(str::trim)
    }

    /// Derive the bounded, storage-safe nonce used by mutation stores from the public retry key.
    /// The verified principal and route scope independent callers and operations; the raw key and
    /// principal never enter durable rows or logs.
    pub fn stable_idempotency_nonce(&self, principal_id: &str) -> Result<String, EdgeError> {
        let key = self.header("idempotency-key").ok_or_else(|| {
            EdgeError::BadRequest("mutation requires an `Idempotency-Key` header".into())
        })?;
        if key.is_empty() || key.len() > 128 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(EdgeError::BadRequest(
                "`Idempotency-Key` must be 1-128 ASCII-graphic bytes without spaces".into(),
            ));
        }
        let mut digest = blake3::Hasher::new();
        for part in [
            b"myelin.edge.request-idempotency.v1".as_slice(),
            principal_id.as_bytes(),
            self.method.as_bytes(),
            self.path.as_bytes(),
            key.as_bytes(),
        ] {
            digest.update(&(part.len() as u64).to_be_bytes());
            digest.update(part);
        }
        Ok(format!("request-v1-{}", digest.finalize().to_hex()))
    }

    pub fn basic_credentials(&self) -> Option<(String, String)> {
        use base64::Engine as _;
        let raw = self.header("authorization")?;
        let b64 = raw
            .strip_prefix("Basic ")
            .or_else(|| raw.strip_prefix("basic "))?
            .trim();
        let decoded = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
        let creds = String::from_utf8(decoded).ok()?;
        let (user, pass) = creds.split_once(':')?;
        if user.is_empty() || pass.is_empty() {
            return None;
        }
        Some((user.to_string(), pass.to_string()))
    }

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

    pub fn json_body(&self) -> Result<Value, EdgeError> {
        if self.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "empty request body (expected JSON)".into(),
            ));
        }
        serde_json::from_slice(&self.body)
            .map_err(|e| EdgeError::BadRequest(format!("malformed JSON body: {e}")))
    }
}

pub enum EdgeResponse {
    Bytes {
        status: u16,
        content_type: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    Sse {
        headers: Vec<(String, String)>,
        sub: SseSubscription,
        expires_at_unix: i64,
    },
}

impl EdgeResponse {
    pub fn json(status: u16, value: &Value) -> EdgeResponse {
        EdgeResponse::Bytes {
            status,
            content_type: "application/json".to_string(),
            headers: Vec::new(),
            body: serde_json::to_vec(value).unwrap_or_default(),
        }
    }

    pub fn error(e: &EdgeError) -> EdgeResponse {
        EdgeResponse::json(e.status(), &e.envelope())
    }

    pub fn sse(sub: SseSubscription, expires_at_unix: i64) -> EdgeResponse {
        EdgeResponse::Sse {
            headers: Vec::new(),
            sub,
            expires_at_unix,
        }
    }

    pub fn with_header(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> EdgeResponse {
        match &mut self {
            EdgeResponse::Bytes { headers, .. } | EdgeResponse::Sse { headers, .. } => {
                headers.push((name.into(), value.into()));
            }
        }
        self
    }

    pub fn status(&self) -> u16 {
        match self {
            EdgeResponse::Bytes { status, .. } => *status,
            EdgeResponse::Sse { .. } => 200,
        }
    }

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
    fn empty_mutations_accept_exactly_an_empty_json_object() {
        assert!(require_empty_json_object(br#"{}"#, "lifecycle", 128).is_ok());
        assert!(require_empty_json_object(br#" { } "#, "lifecycle", 128).is_ok());
        assert!(require_empty_json_object(br#"{"ttl":30}"#, "lifecycle", 128).is_err());
        assert!(require_empty_json_object(b"null", "lifecycle", 128).is_err());
        assert!(require_empty_json_object(b"", "lifecycle", 128).is_err());
        assert!(matches!(
            require_empty_json_object(br#"{}"#, "lifecycle", 1),
            Err(EdgeError::PayloadTooLarge(_))
        ));
    }

    #[test]
    fn basic_credentials_preserve_the_scheme_selecting_username() {
        use base64::Engine as _;
        let encoded =
            base64::engine::general_purpose::STANDARD.encode("myelin-session:opaque-session-token");
        let request = EdgeRequest::new(
            "GET",
            "/acme/eu/repo.git/info/refs",
            "",
            vec![("Authorization".into(), format!("Basic {encoded}"))],
            vec![],
        );
        assert_eq!(
            request.basic_credentials(),
            Some(("myelin-session".into(), "opaque-session-token".into()))
        );
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
        let bad = EdgeRequest::new(
            "GET",
            "/",
            "&&=&x",
            vec![("cookie".into(), ";;= ;".into())],
            vec![],
        );
        assert_eq!(bad.cookie("x"), None);
        assert_eq!(bad.query_param("x"), None);
    }

    #[test]
    fn retry_keys_derive_stable_storage_safe_nonces_without_retaining_raw_material() {
        let request = EdgeRequest::new(
            "POST",
            "/v1/example",
            "",
            vec![("Idempotency-Key".into(), "retry/key:42".into())],
            Vec::new(),
        );
        let nonce = request.stable_idempotency_nonce("svc:agent").unwrap();
        assert_eq!(
            nonce,
            request.stable_idempotency_nonce("svc:agent").unwrap()
        );
        assert!(nonce.starts_with("request-v1-"));
        assert_eq!(nonce.len(), 75);
        assert!(!nonce.contains("retry/key:42"));
        assert_ne!(
            nonce,
            request.stable_idempotency_nonce("svc:other").unwrap(),
            "one caller cannot collide with another caller's retry namespace"
        );
        let another_route = EdgeRequest::new(
            "POST",
            "/v1/another-example",
            "",
            vec![("Idempotency-Key".into(), "retry/key:42".into())],
            Vec::new(),
        );
        assert_ne!(
            nonce,
            another_route.stable_idempotency_nonce("svc:agent").unwrap(),
            "the same public key remains local to one operation route"
        );

        for key in ["", "contains space", "ø", &"x".repeat(129)] {
            let request = EdgeRequest::new(
                "POST",
                "/v1/example",
                "",
                vec![("idempotency-key".into(), key.into())],
                Vec::new(),
            );
            assert_eq!(
                request
                    .stable_idempotency_nonce("svc:agent")
                    .unwrap_err()
                    .status(),
                400
            );
        }
    }

    #[test]
    fn json_body_is_loud_on_malformed() {
        let bad = EdgeRequest::new("POST", "/", "", vec![], b"{not json".to_vec());
        assert!(matches!(bad.json_body(), Err(EdgeError::BadRequest(_))));
        let empty = EdgeRequest::new("POST", "/", "", vec![], vec![]);
        assert!(matches!(empty.json_body(), Err(EdgeError::BadRequest(_))));
    }
}
