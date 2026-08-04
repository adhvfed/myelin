use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EdgeError {
    BadRequest(String),
    Unauthorized(String),
    Forbidden(String),
    NotFound(String),
    Conflict(String),
    Unprocessable(String),
    PayloadTooLarge(String),
    RequestTimeout(String),
    Unavailable(String),
    Internal(String),
}

impl EdgeError {
    pub fn status(&self) -> u16 {
        match self {
            EdgeError::BadRequest(_) => 400,
            EdgeError::Unauthorized(_) => 401,
            EdgeError::Forbidden(_) => 403,
            EdgeError::NotFound(_) => 404,
            EdgeError::Conflict(_) => 409,
            EdgeError::Unprocessable(_) => 422,
            EdgeError::PayloadTooLarge(_) => 413,
            EdgeError::RequestTimeout(_) => 408,
            EdgeError::Unavailable(_) => 503,
            EdgeError::Internal(_) => 500,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            EdgeError::BadRequest(_) => "bad_request",
            EdgeError::Unauthorized(_) => "unauthorized",
            EdgeError::Forbidden(_) => "forbidden",
            EdgeError::NotFound(_) => "not_found",
            EdgeError::Conflict(_) => "conflict",
            EdgeError::Unprocessable(_) => "unprocessable",
            EdgeError::PayloadTooLarge(_) => "payload_too_large",
            EdgeError::RequestTimeout(_) => "request_timeout",
            EdgeError::Unavailable(_) => "unavailable",
            EdgeError::Internal(_) => "internal",
        }
    }

    pub fn client_message(&self) -> String {
        match self {
            EdgeError::Unauthorized(_) => "authentication required".to_string(),
            EdgeError::Forbidden(_) => "forbidden".to_string(),
            EdgeError::Internal(_) => "internal error".to_string(),
            EdgeError::BadRequest(m)
            | EdgeError::NotFound(m)
            | EdgeError::Conflict(m)
            | EdgeError::Unprocessable(m)
            | EdgeError::PayloadTooLarge(m)
            | EdgeError::RequestTimeout(m)
            | EdgeError::Unavailable(m) => m.clone(),
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            EdgeError::BadRequest(m)
            | EdgeError::Unauthorized(m)
            | EdgeError::Forbidden(m)
            | EdgeError::NotFound(m)
            | EdgeError::Conflict(m)
            | EdgeError::Unprocessable(m)
            | EdgeError::PayloadTooLarge(m)
            | EdgeError::RequestTimeout(m)
            | EdgeError::Unavailable(m)
            | EdgeError::Internal(m) => m,
        }
    }

    pub fn envelope(&self) -> Value {
        json!({ "error": { "message": self.client_message(), "code": self.code() } })
    }
}

impl core::fmt::Display for EdgeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{} ({}): {}", self.status(), self.code(), self.detail())
    }
}

impl std::error::Error for EdgeError {}

pub fn map_authz_error(e: myelin_identity::AuthzError) -> EdgeError {
    use myelin_identity::AuthzError;
    match e {
        AuthzError::BadRequest(m) => EdgeError::BadRequest(m),
        AuthzError::FailClosed(m) => EdgeError::Forbidden(m),
        AuthzError::Unavailable(m) => EdgeError::Unavailable(m),
        AuthzError::NotYetImplemented(m) => EdgeError::Unavailable(m.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_code_are_the_canonical_mapping() {
        assert_eq!(EdgeError::BadRequest("x".into()).status(), 400);
        assert_eq!(EdgeError::Unauthorized("x".into()).status(), 401);
        assert_eq!(EdgeError::Forbidden("x".into()).status(), 403);
        assert_eq!(EdgeError::NotFound("x".into()).status(), 404);
        assert_eq!(EdgeError::Conflict("x".into()).status(), 409);
        assert_eq!(EdgeError::Unprocessable("x".into()).status(), 422);
        assert_eq!(EdgeError::PayloadTooLarge("x".into()).status(), 413);
        assert_eq!(EdgeError::PayloadTooLarge("x".into()).code(), "payload_too_large");
        assert_eq!(EdgeError::RequestTimeout("x".into()).status(), 408);
        assert_eq!(EdgeError::RequestTimeout("x".into()).code(), "request_timeout");
        assert_eq!(EdgeError::Unavailable("x".into()).status(), 503);
        assert_eq!(EdgeError::Internal("x".into()).status(), 500);
        assert_eq!(EdgeError::Unauthorized("x".into()).code(), "unauthorized");
    }

    #[test]
    fn envelope_matches_canon_and_never_leaks_internal_detail() {
        let e = EdgeError::Internal("postgres: relation \"secret\" does not exist".into());
        let env = e.envelope();
        assert_eq!(
            env["error"]["message"], "internal error",
            "the 500 client message is generic - never the internal detail"
        );
        assert_eq!(env["error"]["code"], "internal");
        assert!(
            !env.to_string().contains("postgres"),
            "the internal detail must NOT appear in the client envelope"
        );

        let forged = EdgeError::Unauthorized("signature verification failed (forged)".into());
        let revoked = EdgeError::Unauthorized("token jti revoked (durable S7)".into());
        assert_eq!(forged.envelope(), revoked.envelope(), "401 is an oracle-free uniform message");
        assert_eq!(forged.envelope()["error"]["message"], "authentication required");
    }
}
