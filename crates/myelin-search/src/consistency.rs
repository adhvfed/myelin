//! The **zookie/consistency path** (SRCH-P10 / P-173; architecture `search-and-indexing.md`
//! §4.2.3 + §4.10/§1.10): the **no-stale-grant zookie re-validation** + the **fail-static
//! degrade-not-cascade** mechanism that makes a just-revoked grant a *zero-escape* even when the
//! ACL filter the pipeline computed predates the revocation. This is the consistency mechanism the
//! pipeline (`crate::pipeline`) drives AFTER the ACL filter is computed and BEFORE results are
//! returned — the half of GDPR-safe-by-construction (VISION §3) that defends against the
//! **new-enemy problem**: a candidate doc whose indexed ACL state (`indexed_zookie`) is OLDER than
//! the consistency snapshot the read demanded must NOT be served stale-allow.
//!
//! ## The two frozen surfaces this module owns
//!
//! ### 1. The no-stale-grant zookie re-validation (§4.2.3, contract 4.2/4.10)
//! A query may carry a **zookie** (read-your-writes after a sharing change). Search forwards it to
//! `list_objects`; Id evaluates the reachable set at ≥ that snapshot. But the SEARCH INDEX is a
//! projection that lags the source — a candidate doc whose `indexed_zookie` is OLDER than the
//! passed query zookie for an ACL-relevant facet may carry a STALE permission projection (the
//! revocation has not been re-indexed yet). Such a candidate is:
//! - **re-validated** via a **bounded `check`** on the affected candidate only (contract 4.2 — a
//!   per-object gate at the demanded zookie), surfacing iff the check still ALLOWS; OR
//! - **excluded pending re-index** when no bounded-check port is wired (fail-CLOSED, ADR-03 — a
//!   stale candidate is NEVER served stale-allow).
//!
//! Only candidates whose `indexed_zookie` is stale relative to the passed zookie are re-validated
//! (the bounded set, §4.2: "a bounded check on the AFFECTED candidates only" — NOT a per-result
//! N+1 over every hit). A candidate indexed at-or-after the passed zookie is fresh — it is served
//! as-is (no re-check), because its indexed ACL state already reflects the demanded snapshot.
//!
//! ### 2. Fail-static degrade-not-cascade (contract 1.10/4.11, §1)
//! A **default-consistency** ([`ConsistencyMode::BoundedStale`]) query MAY use the cached ACL
//! filter during an Id hiccup — bounded staleness ≤ the revocation SLA W (contract 1.10): the
//! service **degrades** (serves a coarse stale grant) rather than **cascades** (failing every
//! request closed and turning one shared dependency into a platform-wide kill, EI-01 §2). A
//! **zookie-stamped** ([`ConsistencyMode::Strong`]) query does **NOT** use the stale cache — it
//! **bypasses the fail-static cache** (contract 4.10): a read-your-writes-after-revocation read
//! must see the revocation, so it waits/falls-back rather than serving a stale coarse grant.
//!
//! The fail-static cache itself is the substrate's `myelin_substrate::FailStatic<T>` (contract
//! 1.10, P-S18) — REUSED, not re-implemented (EI-01 §7): this module owns only the **bypass
//! decision** ([`fail_static_bypass`]) — WHICH consistency mode bypasses — and threads the
//! fail-static ratio telemetry (contract 1.8) the GATE asserts.
//!
//! ## What is NOT here (named floors — EI-01 §3)
//! - The revision-watermark JOIN wait/fallback is wired in `crate::pipeline` (P-172): a JOIN
//!   needing a fresher reverse-index revision than the index carries is a loud
//!   `QueryError::StaleReverseIndex`. P-173 adds the **bounded-check fallback** (via
//!   [`stale_candidates`] and the pipeline's re-validation) so a stale-revision JOIN can fall back
//!   to a bounded check on the affected candidates rather than only erroring.
//! - The hybrid/vector RRF fusion (SRCH-P11 / P-174) REUSES this same zookie path for the semantic
//!   surface (no-stale-grant for RAG too) — named, not duplicated.
//! - The full at-scale assertion of the consistency mechanism (the revocation-SLA load drill) is
//!   unchanged at M5 (the prompt's "FLOOR named: none new").

use std::sync::atomic::{AtomicU64, Ordering};

use myelin_identity::{
    Consistency, ConsistencyMode, ObjectId, Permission, Principal, Result as AuthzResult,
};

use crate::pipeline::watermark_from_zookie;

/// **The bounded re-validation port (contract 4.2 — `check`).** The per-object authz gate the
/// no-stale-grant path calls on EXACTLY the candidates whose `indexed_zookie` is stale relative to
/// the passed query zookie (§4.2.3 — "a bounded `check` on the affected candidates ONLY"). It is
/// the consistency half of the `list_objects` port ([`crate::pipeline::ListObjectsPort`]); a query
/// path wired only for the bounded-set/relational lowering has no `check` resolver, in which case a
/// stale candidate is **excluded pending re-index** (fail-CLOSED, ADR-03 — never served
/// stale-allow).
///
/// **`check` is evaluated at the passed zookie** (the demanded consistency snapshot): it answers
/// "does `subject` STILL hold `permission` on `object` AS OF `at`?". A just-revoked grant returns
/// `Deny` here even though the stale index projection still carries the doc in the reachable set —
/// this is the new-enemy defence.
pub trait BoundedCheckPort {
    /// Re-validate ONE stale candidate at the demanded consistency `at` (contract 4.2). `Ok(true)`
    /// ⇒ the subject STILL holds `permission` on `object` at the snapshot (surface it); `Ok(false)`
    /// ⇒ the grant is gone (exclude it — the new-enemy is kept out); `Err` surfaces loudly (a
    /// bounded-check failure is NEVER a silent admit — deny-when-unsure, ADR-03). The pipeline
    /// calls this ONCE per stale candidate (the bounded set), never once per result.
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ObjectId,
        at: &Consistency,
    ) -> AuthzResult<bool>;
}

/// **The disposition of a single candidate under the no-stale-grant zookie path (§4.2.3).** Drives
/// the pipeline's post-ACL re-validation: a fresh candidate is served as-is; a stale candidate is
/// re-validated via a bounded `check` (admit iff it still ALLOWS) or excluded pending re-index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateDisposition {
    /// The candidate's `indexed_zookie` is at-or-after the passed query zookie — its indexed ACL
    /// state already reflects the demanded snapshot. Served as-is, NO re-check (it is not in the
    /// bounded affected set).
    Fresh,
    /// The candidate's `indexed_zookie` is OLDER than the passed query zookie for an ACL-relevant
    /// facet — it MUST be re-validated by a bounded `check` (or excluded pending re-index). Never
    /// served stale-allow.
    StaleNeedsRevalidation,
}

/// **Is candidate `indexed_zookie` STALE relative to the passed query zookie?** (§4.2.3). Both
/// zookies carry the monotone revision suffix `…@<rev>` (the embedded model — the real
/// zookie→revision mapping is Identity's, contract 4.10). A candidate is stale iff its indexed
/// revision is STRICTLY LESS than the passed query revision: indexed-at-or-after is fresh (its
/// projection reflects the demanded snapshot). An absent (`None`) candidate anchor is treated as
/// the **oldest possible** revision (0-stale) — a doc with no recorded staleness anchor is
/// conservatively re-validated, never assumed fresh (fail-safe; a missing anchor must not admit a
/// stale grant).
pub fn disposition(
    candidate_indexed_zookie: Option<&str>,
    passed_zookie: &str,
) -> CandidateDisposition {
    let required = watermark_from_zookie(passed_zookie).0;
    // An absent anchor is the oldest possible revision (0) — conservatively re-validated.
    let indexed = candidate_indexed_zookie.map(watermark_from_zookie).map_or(0, |w| w.0);
    if indexed < required {
        CandidateDisposition::StaleNeedsRevalidation
    } else {
        CandidateDisposition::Fresh
    }
}

/// **Partition candidate doc-ids into (fresh, stale) by the no-stale-grant rule (§4.2.3).** Returns
/// the doc-ids that are fresh (serve as-is) and those whose `indexed_zookie` is stale relative to
/// the passed zookie (the bounded affected set that must be re-validated or excluded). `anchor` is
/// the per-doc `indexed_zookie` point lookup ([`crate::engine::IndexBackend::indexed_zookie_of`]) —
/// a doc-id point read, NOT a scored search. The relative order of the inputs is preserved within
/// each partition (the ranked order is not disturbed for the fresh docs).
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

/// **Does this read's consistency mode BYPASS the fail-static cache? (contract 4.10/1.10).** A
/// **zookie-stamped strong** read ([`ConsistencyMode::Strong`]) BYPASSES the fail-static cache —
/// read-your-writes-after-revocation must SEE the revocation, so it waits/falls-back rather than
/// serving a stale coarse grant (`true`). A **default-consistency** read
/// ([`ConsistencyMode::BoundedStale`]) MAY use the cached filter during an Id hiccup (bounded
/// staleness ≤ W) — it does NOT bypass (`false`), so the service degrades-not-cascades.
pub fn fail_static_bypass(at: &Consistency) -> bool {
    matches!(at.mode, ConsistencyMode::Strong)
}

/// **Telemetry for the consistency path (contract 1.8, §4.11 slice).** The observable counters the
/// SRCH-D2 GATE + the fail-static ratio assert: how many candidates were re-validated (the bounded
/// affected set), how many of those were EXCLUDED (the new-enemy kept out), how many fail-static
/// stale-grants were served vs bypassed. One `ConsistencyStats` is threaded through a query; the
/// drill reads it back.
#[derive(Debug, Default)]
pub struct ConsistencyStats {
    /// The number of candidates re-validated by a bounded `check` (the affected stale set, §4.2.3).
    /// The no-N+1 property: this is the STALE subset, never every hit.
    revalidated: AtomicU64,
    /// The number of stale candidates EXCLUDED (re-validation denied OR no check port wired → fail
    /// closed pending re-index). The zero-escape-under-staleness counter: a just-revoked grant is
    /// excluded here.
    excluded_stale: AtomicU64,
    /// The number of reads that BYPASSED the fail-static cache (zookie-stamped strong reads).
    fail_static_bypassed: AtomicU64,
    /// The number of reads that USED the fail-static cache during an Id hiccup (default-consistency
    /// degrade-not-cascade). The fail-static ratio (1.8) numerator.
    fail_static_served: AtomicU64,
}

impl ConsistencyStats {
    /// A fresh stats counter (all zero).
    pub fn new() -> ConsistencyStats {
        ConsistencyStats::default()
    }

    /// Record one bounded re-validation (an affected stale candidate was `check`ed).
    pub fn record_revalidation(&self) {
        self.revalidated.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one stale candidate EXCLUDED (revalidation denied / no check port → fail closed).
    pub fn record_excluded_stale(&self) {
        self.excluded_stale.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one fail-static BYPASS (a zookie-stamped strong read did not use the stale cache).
    pub fn record_fail_static_bypass(&self) {
        self.fail_static_bypassed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one fail-static SERVE (a default-consistency read used the cached filter on a hiccup).
    pub fn record_fail_static_served(&self) {
        self.fail_static_served.fetch_add(1, Ordering::Relaxed);
    }

    /// The number of bounded re-validations recorded (the affected stale set — never every hit).
    pub fn revalidated(&self) -> u64 {
        self.revalidated.load(Ordering::Relaxed)
    }

    /// The number of stale candidates excluded (the zero-escape-under-staleness counter).
    pub fn excluded_stale(&self) -> u64 {
        self.excluded_stale.load(Ordering::Relaxed)
    }

    /// The number of fail-static bypasses (zookie-stamped strong reads).
    pub fn fail_static_bypassed(&self) -> u64 {
        self.fail_static_bypassed.load(Ordering::Relaxed)
    }

    /// The number of fail-static serves (default-consistency degrade-not-cascade).
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
        Consistency { at_least: Zookie(zookie.into()), mode: ConsistencyMode::Strong }
    }
    fn bounded(zookie: &str) -> Consistency {
        Consistency { at_least: Zookie(zookie.into()), mode: ConsistencyMode::BoundedStale }
    }
    fn subject() -> Principal {
        Principal::stub(PrincipalId("p:alice".into()), PrincipalKind::Human, TenantId("acme".into()))
    }

    /// **A candidate indexed STRICTLY BEFORE the passed zookie is stale; at-or-after is fresh.**
    #[test]
    fn disposition_is_stale_iff_indexed_revision_is_below_passed() {
        // indexed @5 vs passed @9 → stale (the index projection predates the demanded snapshot).
        assert_eq!(disposition(Some("z@5"), "z@9"), CandidateDisposition::StaleNeedsRevalidation);
        // indexed @9 vs passed @9 → fresh (indexed AT the snapshot — its ACL state reflects it).
        assert_eq!(disposition(Some("z@9"), "z@9"), CandidateDisposition::Fresh);
        // indexed @11 vs passed @9 → fresh (indexed AFTER — strictly newer).
        assert_eq!(disposition(Some("z@11"), "z@9"), CandidateDisposition::Fresh);
    }

    /// **An ABSENT candidate anchor is conservatively STALE (the oldest possible revision) — a doc
    /// with no recorded staleness anchor is re-validated, never assumed fresh.**
    #[test]
    fn absent_anchor_is_conservatively_stale() {
        assert_eq!(disposition(None, "z@1"), CandidateDisposition::StaleNeedsRevalidation);
        // ...except when the passed zookie carries NO watermark (rev 0): nothing can be below 0, so
        // an absent anchor (treated as 0) is fresh — a default-consistency query with no zookie
        // does not re-validate every hit (that would be the N+1 the pre-filter avoids).
        assert_eq!(disposition(None, "z-no-suffix"), CandidateDisposition::Fresh);
    }

    /// **`stale_candidates` partitions into (fresh, stale) by the no-stale-grant rule, preserving
    /// order, and re-validates ONLY the stale subset (the bounded affected set, not every hit).**
    #[test]
    fn stale_candidates_partitions_only_the_affected_set() {
        // PUB-1 indexed @9 (fresh vs @9), SECRET-9 indexed @4 (stale vs @9), OTHER-2 indexed @9.
        let anchor = |id: &str| match id {
            "PUB-1" => Some("z@9".to_string()),
            "SECRET-9" => Some("z@4".to_string()),
            "OTHER-2" => Some("z@9".to_string()),
            _ => None,
        };
        let (fresh, stale) =
            stale_candidates(["PUB-1", "SECRET-9", "OTHER-2"], "z@9", anchor);
        assert_eq!(fresh, vec!["PUB-1".to_string(), "OTHER-2".to_string()], "fresh kept in order");
        assert_eq!(stale, vec!["SECRET-9".to_string()], "ONLY the stale candidate is re-validated");
    }

    /// **A zookie-stamped STRONG read BYPASSES the fail-static cache; a default-consistency
    /// BoundedStale read does NOT (it may degrade-not-cascade).** (contract 4.10/1.10).
    #[test]
    fn strong_bypasses_fail_static_bounded_does_not() {
        assert!(fail_static_bypass(&strong("z@7")), "a strong zookie read bypasses the stale cache");
        assert!(
            !fail_static_bypass(&bounded("z@7")),
            "a default-consistency read may use the stale cache (degrade-not-cascade)"
        );
    }

    /// **`BoundedCheckPort::check` is the bounded re-validation seam — a denying check excludes the
    /// stale candidate (the new-enemy is kept out), an allowing check admits it.** Proven against a
    /// scripted port that revokes exactly the just-revoked object.
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
                // The just-revoked object no longer ALLOWS at the demanded snapshot (the new-enemy).
                Ok(object.0 != self.revoked)
            }
        }
        let port = Revoker { revoked: "acme/issue/SECRET-9" };
        let at = strong("z@9");
        let perm = Permission("read".into());
        // The still-granted object is admitted.
        assert!(port
            .check(&subject(), &perm, &ObjectId("acme/issue/PUB-1".into()), &at)
            .unwrap());
        // The just-revoked object is EXCLUDED (the new-enemy is kept out under the zookie).
        assert!(!port
            .check(&subject(), &perm, &ObjectId("acme/issue/SECRET-9".into()), &at)
            .unwrap());
    }

    /// **The stats counters each record exactly their event (kills the accessor/record mutants —
    /// the SRCH-D2 zero-escape + fail-static-ratio buckets are computed from these).**
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
        assert_eq!(s.excluded_stale(), 1, "one stale candidate excluded (zero-escape counter)");
        assert_eq!(s.fail_static_bypassed(), 1, "one fail-static bypass (strong read)");
        assert_eq!(s.fail_static_served(), 2, "two fail-static serves (degrade-not-cascade)");
    }
}
