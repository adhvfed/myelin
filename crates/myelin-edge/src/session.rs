use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const SESSION_COOKIE: &str = "myelin_session";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRecord {
    pub scheme: String,
    pub material: String,
}

#[derive(Clone, Default)]
pub struct SessionStore {
    inner: Arc<Mutex<HashMap<String, SessionRecord>>>,
    counter: Arc<AtomicU64>,
}

impl SessionStore {
    pub fn new() -> SessionStore {
        SessionStore::default()
    }

    pub fn issue(&self, scheme: impl Into<String>, material: impl Into<String>) -> String {
        let id = self.fresh_id();
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), SessionRecord { scheme: scheme.into(), material: material.into() });
        id
    }

    pub fn get(&self, session_id: &str) -> Option<SessionRecord> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
    }

    pub fn remove(&self, session_id: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_id);
    }

    pub fn set_cookie_header(session_id: &str) -> String {
        format!("{SESSION_COOKIE}={session_id}; HttpOnly; Secure; Path=/; SameSite=Strict")
    }

    pub fn clear_cookie_header() -> String {
        format!("{SESSION_COOKIE}=; HttpOnly; Secure; Path=/; SameSite=Strict; Max-Age=0")
    }

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
        let sid2 = store.issue("pat", "other");
        assert_ne!(sid, sid2);
        store.remove(&sid);
        assert_eq!(store.get(&sid), None);
    }

    #[test]
    fn set_cookie_is_httponly_and_carries_only_the_id() {
        let h = SessionStore::set_cookie_header("sess-1");
        assert!(h.contains("HttpOnly"), "the cookie is httpOnly (no client JS access)");
        assert!(h.contains("SameSite=Strict"));
        assert!(h.starts_with("myelin_session=sess-1"));
        assert!(!h.contains("v4.public"), "no token material in the cookie");
    }
}
