use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct RestrictSet {
    inner: Arc<Mutex<BTreeSet<String>>>,
}

impl RestrictSet {
    pub fn new() -> RestrictSet {
        RestrictSet::default()
    }

    pub fn set(&self, subject_id: &str, on: bool) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            g.insert(subject_id.to_string());
        } else {
            g.remove(subject_id);
        }
    }

    pub fn is_restricted(&self, subject_id: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(subject_id)
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restrict_suppresses_then_re_enables() {
        let s = RestrictSet::new();
        assert!(!s.is_restricted("p-opaque-1"), "not restricted by default");
        s.set("p-opaque-1", true);
        assert!(s.is_restricted("p-opaque-1"), "restrict on → suppressed");
        s.set("p-opaque-1", false);
        assert!(
            !s.is_restricted("p-opaque-1"),
            "restrict off → re-enabled (not deleted)"
        );
    }

    #[test]
    fn restrict_is_idempotent() {
        let s = RestrictSet::new();
        s.set("a", true);
        s.set("a", true);
        assert_eq!(s.len(), 1, "double-set is one entry");
        s.set("b", false);
        assert_eq!(s.len(), 1, "clearing an absent subject is a no-op");
        assert!(!s.is_empty());
    }

    #[test]
    fn len_and_is_empty_track_the_real_cardinality() {
        let s = RestrictSet::new();
        assert_eq!(s.len(), 0, "empty → 0");
        assert!(s.is_empty(), "empty → is_empty");
        s.set("a", true);
        assert_eq!(s.len(), 1, "one entry → 1");
        assert!(!s.is_empty(), "one entry → not empty");
        s.set("b", true);
        assert_eq!(s.len(), 2, "two entries → 2");
    }

    #[test]
    fn shared_handle_sees_the_same_set() {
        let writer = RestrictSet::new();
        let reader = writer.clone();
        writer.set("p-9", true);
        assert!(
            reader.is_restricted("p-9"),
            "the reader sees the holder's restriction"
        );
    }
}
