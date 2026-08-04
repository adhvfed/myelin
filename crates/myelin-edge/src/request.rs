use crate::error::EdgeError;
use crate::sse::SseSubscription;
use serde_json::Value;

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
            .and_then(|h| h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer ")))
            .map(str::trim)
    }

    pub fn basic_password(&self) -> Option<String> {
        use base64::Engine as _;
        let raw = self.header("authorization")?;
        let b64 = raw
            .strip_prefix("Basic ")
            .or_else(|| raw.strip_prefix("basic "))?
            .trim();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .ok()?;
        let creds = String::from_utf8(decoded).ok()?;
        let (_user, pass) = creds.split_once(':')?;
        if pass.is_empty() {
            return None;
        }
        Some(pass.to_string())
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
            return Err(EdgeError::BadRequest("empty request body (expected JSON)".into()));
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

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> EdgeResponse {
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
