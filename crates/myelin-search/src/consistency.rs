use std::sync::atomic::{AtomicU64, Ordering};

use myelin_identity::{
    Consistency, ConsistencyMode, ObjectId, Permission, Principal, Result as AuthzResult,
};

use crate::pipeline::watermark_from_zookie;

pub trait BoundedCheckPort {
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ObjectId,
        at: &Consistency,
    ) -> AuthzResult<bool>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateDisposition {
    Fresh,
    StaleNeedsRevalidation,
}

pub fn disposition(
    candidate_indexed_zookie: Option<&str>,
    passed_zookie: &str,
) -> CandidateDisposition {
    let required = watermark_from_zookie(passed_zookie).0;
    let indexed = candidate_indexed_zookie
        .map(watermark_from_zookie)
        .map_or(0, |w| w.0);
    if indexed < required {
        CandidateDisposition::StaleNeedsRevalidation
    } else {
        CandidateDisposition::Fresh
    }
}

pub fn stale_candidates<'a>(
    doc_ids: impl IntoIterator<Item = &'a str>,
    passed_zookie: &str,
    anchor: impl Fn(&str) -> Option<String>,
) -> (Vec<String>, Vec<String>) {
    let mut fresh = Vec::new();
    let mut stale = Vec::new();
    for doc_id in doc_ids {
        let indexed = anchor(doc_id);
        match disposition(indexed.as_deref(), passed_zookie) {
            CandidateDisposition::Fresh => fresh.push(doc_id.to_string()),
            CandidateDisposition::StaleNeedsRevalidation => stale.push(doc_id.to_string()),
        }
    }
    (fresh, stale)
}

pub fn fail_static_bypass(at: &Consistency) -> bool {
    matches!(at.mode, ConsistencyMode::Strong)
}

#[derive(Debug, Default)]
pub struct ConsistencyStats {
    revalidated: AtomicU64,
    excluded_stale: AtomicU64,
    fail_static_bypassed: AtomicU64,
    fail_static_served: AtomicU64,
}

impl ConsistencyStats {
    pub fn new() -> ConsistencyStats {
        ConsistencyStats::default()
    }

    pub fn record_revalidation(&self) {
        self.revalidated.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_excluded_stale(&self) {
        self.excluded_stale.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_fail_static_bypass(&self) {
        self.fail_static_bypassed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_fail_static_served(&self) {
        self.fail_static_served.fetch_add(1, Ordering::Relaxed);
    }

    pub fn revalidated(&self) -> u64 {
        self.revalidated.load(Ordering::Relaxed)
    }

    pub fn excluded_stale(&self) -> u64 {
        self.excluded_stale.load(Ordering::Relaxed)
    }

    pub fn fail_static_bypassed(&self) -> u64 {
        self.fail_static_bypassed.load(Ordering::Relaxed)
    }

    pub fn fail_static_served(&self) -> u64 {
        self.fail_static_served.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{ObjectId, PrincipalId, PrincipalKind, Zookie};
    use myelin_tenancy::TenantId;

    fn strong(zookie: &str) -> Consistency {
        Consistency {
            at_least: Zookie(zookie.into()),
            mode: ConsistencyMode::Strong,
        }
    }
    fn bounded(zookie: &str) -> Consistency {
        Consistency {
            at_least: Zookie(zookie.into()),
            mode: ConsistencyMode::BoundedStale,
        }
    }
    fn subject() -> Principal {
        Principal::stub(
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    #[test]
    fn disposition_is_stale_iff_indexed_revision_is_below_passed() {
        assert_eq!(
            disposition(Some("z@5"), "z@9"),
            CandidateDisposition::StaleNeedsRevalidation
        );
        assert_eq!(disposition(Some("z@9"), "z@9"), CandidateDisposition::Fresh);
        assert_eq!(
            disposition(Some("z@11"), "z@9"),
            CandidateDisposition::Fresh
        );
    }

    #[test]
    fn absent_anchor_is_conservatively_stale() {
        assert_eq!(
            disposition(None, "z@1"),
            CandidateDisposition::StaleNeedsRevalidation
        );
        assert_eq!(
            disposition(None, "z-no-suffix"),
            CandidateDisposition::Fresh
        );
    }

    #[test]
    fn stale_candidates_partitions_only_the_affected_set() {
        let anchor = |id: &str| match id {
            "PUB-1" => Some("z@9".to_string()),
            "SECRET-9" => Some("z@4".to_string()),
            "OTHER-2" => Some("z@9".to_string()),
            _ => None,
        };
        let (fresh, stale) = stale_candidates(["PUB-1", "SECRET-9", "OTHER-2"], "z@9", anchor);
        assert_eq!(
            fresh,
            vec!["PUB-1".to_string(), "OTHER-2".to_string()],
            "fresh kept in order"
        );
        assert_eq!(
            stale,
            vec!["SECRET-9".to_string()],
            "ONLY the stale candidate is re-validated"
        );
    }

    #[test]
    fn strong_bypasses_fail_static_bounded_does_not() {
        assert!(
            fail_static_bypass(&strong("z@7")),
            "a strong zookie read bypasses the stale cache"
        );
        assert!(
            !fail_static_bypass(&bounded("z@7")),
            "a default-consistency read may use the stale cache (degrade-not-cascade)"
        );
    }

    #[test]
    fn bounded_check_admits_still_granted_excludes_revoked() {
        struct Revoker {
            revoked: &'static str,
        }
        impl BoundedCheckPort for Revoker {
            fn check(
                &self,
                _s: &Principal,
                _p: &Permission,
                object: &ObjectId,
                _at: &Consistency,
            ) -> AuthzResult<bool> {
                Ok(object.0 != self.revoked)
            }
        }
        let port = Revoker {
            revoked: "acme/issue/SECRET-9",
        };
        let at = strong("z@9");
        let perm = Permission("read".into());
        assert!(port
            .check(&subject(), &perm, &ObjectId("acme/issue/PUB-1".into()), &at)
            .unwrap());
        assert!(!port
            .check(
                &subject(),
                &perm,
                &ObjectId("acme/issue/SECRET-9".into()),
                &at
            )
            .unwrap());
    }

    #[test]
    fn stats_counters_record_their_own_event() {
        let s = ConsistencyStats::new();
        s.record_revalidation();
        s.record_revalidation();
        s.record_excluded_stale();
        s.record_fail_static_bypass();
        s.record_fail_static_served();
        s.record_fail_static_served();
        assert_eq!(s.revalidated(), 2, "two bounded re-validations");
        assert_eq!(
            s.excluded_stale(),
            1,
            "one stale candidate excluded (zero-escape counter)"
        );
        assert_eq!(
            s.fail_static_bypassed(),
            1,
            "one fail-static bypass (strong read)"
        );
        assert_eq!(
            s.fail_static_served(),
            2,
            "two fail-static serves (degrade-not-cascade)"
        );
    }
}
