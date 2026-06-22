//! The **restrict-suppression set** (REF-P15 / P-164; §4.6 / GDPR Art. 18/21; GA-D7).
//!
//! **Architecture:** reference-graph.md §4.6 ("the `restrict` suppression keeps a restricted subject's
//! references out of indexing/agent-use/analytics"). When a DSR `restrict(subject, true)` is honoured,
//! the subject's references must be SUPPRESSED — not deleted (restriction is "suppress, don't delete"),
//! re-enabled on `restrict(subject, false)`. This is the small set the Refs holder records into and the
//! indexer / backlink read / agent-use consult.
//!
//! Keyed on the **PSEUDONYMOUS opaque `origin_actor` id** (never a name) — Refs holds nothing but the
//! opaque subject id. A cloneable handle over shared state so the holder writes the SAME set the
//! reader consults. Tenant-scoping rides the subject id (the opaque principal id is tenant-unique).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

/// The set of restricted subject ids (the opaque `origin_actor` ids whose references are suppressed
/// from indexing/agent-use/analytics — GA-D7). A cloneable handle over shared state. PII-free: opaque
/// pseudonymous ids only.
#[derive(Clone, Default)]
pub struct RestrictSet {
    inner: Arc<Mutex<BTreeSet<String>>>,
}

impl RestrictSet {
    /// A fresh, empty restrict set.
    pub fn new() -> RestrictSet {
        RestrictSet::default()
    }

    /// Set (or clear) the restriction for `subject_id` (Art. 18/21). `on=true` SUPPRESSES the subject's
    /// references (suppress, don't delete); `on=false` re-enables them. Idempotent (setting an already-
    /// set restriction is a no-op; clearing an absent one is a no-op).
    pub fn set(&self, subject_id: &str, on: bool) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            g.insert(subject_id.to_string());
        } else {
            g.remove(subject_id);
        }
    }

    /// Is `subject_id` restricted? The indexer / backlink read / agent-use consult this to suppress a
    /// restricted subject's references (GA-D7). A non-restricted subject is `false` (the common path).
    pub fn is_restricted(&self, subject_id: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(subject_id)
    }

    /// The count of restricted subjects (observability — how many subjects are under restriction).
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Is the restrict set empty? (the common steady state — no restrictions in force).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **`restrict(subject, true)` suppresses; `restrict(subject, false)` re-enables (suppress, don't
    /// delete).** The set is keyed on the opaque pseudonymous id.
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

    /// **Idempotent: setting twice is one entry; clearing an absent one is a no-op.**
    #[test]
    fn restrict_is_idempotent() {
        let s = RestrictSet::new();
        s.set("a", true);
        s.set("a", true);
        assert_eq!(s.len(), 1, "double-set is one entry");
        s.set("b", false); // clear an absent one
        assert_eq!(s.len(), 1, "clearing an absent subject is a no-op");
        assert!(!s.is_empty());
    }

    /// **`len`/`is_empty` reflect the real cardinality (observability).** Empty → 0 + is_empty; one
    /// entry → 1 + not empty; two → 2. Catches a mutant that fixes `len` to a constant or `is_empty`
    /// to `false`.
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

    /// **A shared handle sees the same set (the holder writes, the reader consults).**
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
