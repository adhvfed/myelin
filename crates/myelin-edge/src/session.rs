//! # The httpOnly-cookie session convention (the web path)
//!
//! The frontend canon §5 expects a **server-side cookie-auth gateway**: the session lives in an
//! **httpOnly cookie**, the gateway carries the Bearer token **server-side**, and **tokens never reach
//! client JS**. This module is that machinery: a [`SessionStore`] maps an opaque session id (the
//! cookie value) → a [`SessionRecord`] holding the server-side credential (scheme + material). The
//! gateway reads the cookie, looks up the record, and authenticates with the carried credential —
//! the client never sees the token.
//!
//! **Session issuance** ([`SessionStore::issue`]) is the primitive a successful human login calls
//! (`POST /v1/auth/login`). Because the human verifier is config-deferred (MR-012 — JWKS/trust-
//! anchors pending), login REFUSES loudly and never reaches `issue` yet; but the issuance + cookie +
//! lookup machinery is REAL and exercised here, so the day the human verifier lands it is a config
//! change, not new plumbing.
//!
//! **Floors named (EI-01 §4):** (1) the in-memory store is the model — the durable session store
//! (Redis/Valkey + PG, like S7) is the named follow-on; the cookie/lookup/issue SEMANTICS are
//! complete now. (2) the session id is generated from a process-monotonic counter + a nanosecond
//! timestamp; a production CSPRNG-backed unguessable id is a named hardening (the SHAPE — opaque id in
//! an httpOnly cookie — does not change).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The session cookie name (httpOnly; carries ONLY the opaque session id, never the token).
pub const SESSION_COOKIE: &str = "myelin_session";

/// The server-side credential a session carries (NEVER exposed to client JS). `scheme`/`material`
/// are the `myelin_identity::Credential` fields the gateway authenticates with on each request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRecord {
    /// The credential scheme the carried token authenticates under (e.g. `pat`).
    pub scheme: String,
    /// The opaque credential material (the capability token) — server-side only.
    pub material: String,
}

/// **The httpOnly-cookie session store.** Maps an opaque session id → the server-side
/// [`SessionRecord`]. Cloneable (one store shared by the gateway). In-memory model; durable backing
/// is the named floor.
#[derive(Clone, Default)]
pub struct SessionStore {
    inner: Arc<Mutex<HashMap<String, SessionRecord>>>,
    counter: Arc<AtomicU64>,
}

impl SessionStore {
    /// A fresh, empty session store.
    pub fn new() -> SessionStore {
        SessionStore::default()
    }

    /// **Issue a session** carrying the server-side credential — the primitive a successful login
    /// calls. Returns the opaque session id to set in the httpOnly cookie. (Reachable via login only
    /// once the human verifier is config-wired; tested directly here as the real issuance path.)
    pub fn issue(&self, scheme: impl Into<String>, material: impl Into<String>) -> String {
        let id = self.fresh_id();
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), SessionRecord { scheme: scheme.into(), material: material.into() });
        id
    }

    /// Look up the server-side credential for a session id (the gateway's cookie→bearer read).
    pub fn get(&self, session_id: &str) -> Option<SessionRecord> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
    }

    /// Remove a session (logout). Idempotent.
    pub fn remove(&self, session_id: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_id);
    }

    /// Render the `Set-Cookie` header value for a session id — `HttpOnly` (no client JS access),
    /// `Path=/`, `SameSite=Strict`, `Secure` (HTTPS-only). This is the canon's "tokens never reach
    /// client JS" made concrete: the cookie carries only the opaque id, and `HttpOnly` keeps even
    /// THAT out of `document.cookie`.
    pub fn set_cookie_header(session_id: &str) -> String {
        format!("{SESSION_COOKIE}={session_id}; HttpOnly; Secure; Path=/; SameSite=Strict")
    }

    /// The `Set-Cookie` value that CLEARS the session cookie (logout).
    pub fn clear_cookie_header() -> String {
        format!("{SESSION_COOKIE}=; HttpOnly; Secure; Path=/; SameSite=Strict; Max-Age=0")
    }

    /// Generate an opaque session id (counter + nanos — the model; a CSPRNG id is the named floor).
    fn fresh_id(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("sess-{nanos:x}-{n:x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_then_get_round_trips_the_server_side_credential() {
        let store = SessionStore::new();
        let sid = store.issue("pat", "v4.public.aaa|bb|cc");
        let rec = store.get(&sid).expect("session present");
        assert_eq!(rec.scheme, "pat");
        assert_eq!(rec.material, "v4.public.aaa|bb|cc");
        // distinct sessions get distinct ids.
        let sid2 = store.issue("pat", "other");
        assert_ne!(sid, sid2);
        // logout removes it.
        store.remove(&sid);
        assert_eq!(store.get(&sid), None);
    }

    #[test]
    fn set_cookie_is_httponly_and_carries_only_the_id() {
        let h = SessionStore::set_cookie_header("sess-1");
        assert!(h.contains("HttpOnly"), "the cookie is httpOnly (no client JS access)");
        assert!(h.contains("SameSite=Strict"));
        assert!(h.starts_with("myelin_session=sess-1"));
        // the cookie value is the opaque id ONLY — no token material in it.
        assert!(!h.contains("v4.public"), "no token material in the cookie");
    }
}
