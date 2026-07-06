//! # The edge error model — the `{error:{message, code?}}` envelope + typed error→status mapping
//!
//! This is the API convention every subsystem follows for failures. The frontend canon
//! (`planning/system-reviews/2026-06-26/10-frontend-component-patterns.md` §5) parses the
//! `{error:{message}}` envelope (its `GatewayError` extracts `error.message`), so the SHAPE is the
//! consumer contract. The typed [`EdgeError`] maps to the HTTP status set (401/403/404/409/422/4xx,
//! 5xx) and — critically — its [`EdgeError::client_message`] **NEVER leaks an internal detail or
//! PII**: an `Internal` error surfaces a generic message and an `Unauthorized`/`Forbidden` is uniform
//! (a forged vs expired vs revoked token are indistinguishable to the client — the security posture).
//! The detailed reason is retained in the variant for SERVER-side logging only, never in the body.

use serde_json::{json, Value};

/// A typed edge error with a fixed HTTP status + a stable machine `code`. The String payload is the
/// SERVER-side detail (for logs/audit) — it is never the client message for the fail-closed variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EdgeError {
    /// 400 — a malformed request (bad JSON, an unsupported method). The detail IS client-safe (it
    /// describes the request shape, not an internal secret).
    BadRequest(String),
    /// 401 — authentication failed or was absent. The client message is UNIFORM ("authentication
    /// required") regardless of the internal cause (forged / expired / revoked / missing) — never
    /// leak which (an oracle for an attacker). The detail is kept for the server log only.
    Unauthorized(String),
    /// 403 — authenticated but not authorized (incl. a cross-tenant IDOR rejected at the edge). The
    /// client message is uniform ("forbidden") — never confirm the target resource/tenant exists.
    Forbidden(String),
    /// 404 — no such route/resource. Detail is client-safe (a route name).
    NotFound(String),
    /// 409 — a conflict (e.g. a CAS/precondition failure). Detail is client-safe.
    Conflict(String),
    /// 422 — a well-formed but semantically invalid request. Detail is client-safe.
    Unprocessable(String),
    /// 413 — the request body exceeded the front-door size ceiling (R0.5 / DELTA N3). Rejected at the
    /// edge BEFORE the whole body is buffered, so a single oversize POST cannot exhaust host RAM.
    /// Detail is client-safe (it names the byte ceiling, not an internal secret).
    PayloadTooLarge(String),
    /// 503 — a dependency is unavailable, OR an auth mode is configured-deferred and refuses LOUDLY
    /// (refuse-not-mock — e.g. human login until JWKS/trust-anchors land). Detail is client-safe.
    Unavailable(String),
    /// 500 — an internal error. The detail is retained for the server log; the CLIENT message is the
    /// generic "internal error" (never leak a stack/SQL/internal string — the no-PII-leak floor).
    Internal(String),
}

impl EdgeError {
    /// The HTTP status code this error maps to.
    pub fn status(&self) -> u16 {
        match self {
            EdgeError::BadRequest(_) => 400,
            EdgeError::Unauthorized(_) => 401,
            EdgeError::Forbidden(_) => 403,
            EdgeError::NotFound(_) => 404,
            EdgeError::Conflict(_) => 409,
            EdgeError::Unprocessable(_) => 422,
            EdgeError::PayloadTooLarge(_) => 413,
            EdgeError::Unavailable(_) => 503,
            EdgeError::Internal(_) => 500,
        }
    }

    /// The stable machine-readable `code` (the optional `error.code` the canon may key on).
    pub fn code(&self) -> &'static str {
        match self {
            EdgeError::BadRequest(_) => "bad_request",
            EdgeError::Unauthorized(_) => "unauthorized",
            EdgeError::Forbidden(_) => "forbidden",
            EdgeError::NotFound(_) => "not_found",
            EdgeError::Conflict(_) => "conflict",
            EdgeError::Unprocessable(_) => "unprocessable",
            EdgeError::PayloadTooLarge(_) => "payload_too_large",
            EdgeError::Unavailable(_) => "unavailable",
            EdgeError::Internal(_) => "internal",
        }
    }

    /// **The client-safe message — NEVER leaks an internal detail or PII.** For the fail-closed
    /// variants (`Unauthorized`/`Forbidden`/`Internal`) the message is GENERIC + uniform, so a
    /// forged/expired/revoked token (all 401) or a present/absent resource (all 403/404) cannot be
    /// distinguished by the client. The descriptive variants pass their (already client-safe) detail.
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
            | EdgeError::Unavailable(m) => m.clone(),
        }
    }

    /// The server-side detail (for logging/audit) — the full reason, never sent to the client.
    pub fn detail(&self) -> &str {
        match self {
            EdgeError::BadRequest(m)
            | EdgeError::Unauthorized(m)
            | EdgeError::Forbidden(m)
            | EdgeError::NotFound(m)
            | EdgeError::Conflict(m)
            | EdgeError::Unprocessable(m)
            | EdgeError::PayloadTooLarge(m)
            | EdgeError::Unavailable(m)
            | EdgeError::Internal(m) => m,
        }
    }

    /// The `{error:{message, code}}` JSON envelope the frontend canon parses (its `GatewayError`
    /// extracts `error.message`). The `message` is the client-safe message; `code` is the stable
    /// machine code.
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

/// Map an identity-layer [`myelin_identity::AuthzError`] surfaced from a NON-authentication site
/// (e.g. a handler calling `check`/a store op) to an [`EdgeError`]. NOTE: at the AUTHENTICATION
/// boundary itself, the gateway collapses ANY failure to [`EdgeError::Unauthorized`] (uniform 401 —
/// never leak forged-vs-expired-vs-revoked); this mapping is for the dispatched-handler path.
pub fn map_authz_error(e: myelin_identity::AuthzError) -> EdgeError {
    use myelin_identity::AuthzError;
    match e {
        AuthzError::BadRequest(m) => EdgeError::BadRequest(m),
        // A fail-closed authorization decision at a handler is a 403 (the principal authenticated but
        // the action was denied), with a uniform client message.
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
        assert_eq!(EdgeError::Unavailable("x".into()).status(), 503);
        assert_eq!(EdgeError::Internal("x".into()).status(), 500);
        assert_eq!(EdgeError::Unauthorized("x".into()).code(), "unauthorized");
    }

    /// The envelope shape matches the canon: `error.message` is present + a string; the fail-closed
    /// variants do NOT leak their internal detail.
    #[test]
    fn envelope_matches_canon_and_never_leaks_internal_detail() {
        let e = EdgeError::Internal("postgres: relation \"secret\" does not exist".into());
        let env = e.envelope();
        assert_eq!(
            env["error"]["message"], "internal error",
            "the 500 client message is generic — never the internal detail"
        );
        assert_eq!(env["error"]["code"], "internal");
        assert!(
            !env.to_string().contains("postgres"),
            "the internal detail must NOT appear in the client envelope"
        );

        // A 401 is uniform regardless of the (forged/expired/revoked) cause.
        let forged = EdgeError::Unauthorized("signature verification failed (forged)".into());
        let revoked = EdgeError::Unauthorized("token jti revoked (durable S7)".into());
        assert_eq!(forged.envelope(), revoked.envelope(), "401 is an oracle-free uniform message");
        assert_eq!(forged.envelope()["error"]["message"], "authentication required");
    }
}
