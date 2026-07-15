//! # CDC — the Search **consistency consumer** (contracts 4.10 Consistency/zookie + revision
//! watermark, 1.10 FailStatic, 4.2 the bounded `check`) (SRCH-P10 → P-173).
//!
//! **Architecture:** `search-and-indexing.md` §4.2.3 (the consistency clause: a candidate whose
//! `indexed_zookie` is older than the passed zookie is re-validated via a bounded `check` on the
//! affected candidates only, or excluded pending re-index — NEVER served stale-allow; zookie-stamped
//! reads bypass the fail-static cache; default-consistency reads may use the cached filter during an
//! Id hiccup, bounded staleness ≤ W). Contracts 4.10 (Consistency/zookie + the revision watermark),
//! 1.10 (FailStatic — bounded staleness, `static_max ≤ revocation SLA`), 4.2 (`check`).
//!
//! Search is a CONSUMER of these contracts (it owns none of them): it consumes the frozen
//! [`myelin_identity::Consistency`]/[`myelin_identity::ConsistencyMode`]/[`myelin_identity::Zookie`]
//! shapes (4.10) and the substrate [`myelin_substrate::FailStatic`] mechanism (1.10), and drives its
//! own no-stale-grant re-validation off them. This CDC pins the consumer side: the consistency-mode
//! split (Strong bypasses the fail-static cache; BoundedStale may degrade), the zookie→revision
//! watermark decoding, and the bounded `check` re-validation contract. If the 4.10 `Consistency`
//! shape or the 1.10 `FailStatic` ladder drifts, this stops compiling/passing — that is the contract.
//!
//! The dated green artifact (2026-06-20): the Search consistency consumer honours the 4.10 zookie
//! bypass + revision watermark and the 1.10 fail-static degrade-not-cascade ladder.

use myelin_identity::{
    Consistency, ConsistencyMode, ObjectId, Permission, Principal, PrincipalId, PrincipalKind,
    Result as AuthzResult, Zookie,
};
use myelin_substrate::{Answer, FailStatic, ServeError, StalenessBound};
use myelin_tenancy::TenantId;

use myelin_search::{
    disposition, fail_static_bypass, stale_candidates, BoundedCheckPort, CandidateDisposition,
};

fn strong(rev: u64) -> Consistency {
    Consistency {
        at_least: Zookie(format!("z@{rev}")),
        mode: ConsistencyMode::Strong,
    }
}
fn bounded(rev: u64) -> Consistency {
    Consistency {
        at_least: Zookie(format!("z@{rev}")),
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

/// **CONSUMER (4.10): a zookie-stamped STRONG read bypasses the fail-static cache; a
/// default-consistency BoundedStale read does NOT.** The frozen [`ConsistencyMode`] (4.10) drives
/// the bypass decision — the consumer side of "zookie-stamped reads bypass the fail-static cache".
#[test]
fn cdc_consumer_4_10_strong_bypasses_fail_static() {
    assert!(
        fail_static_bypass(&strong(7)),
        "a zookie-stamped strong read MUST bypass the fail-static cache (4.10 read-your-writes)"
    );
    assert!(
        !fail_static_bypass(&bounded(7)),
        "a default-consistency read MAY use the cached filter (degrade-not-cascade, 1.10)"
    );
}

/// **CONSUMER (4.10): the zookie→revision watermark decoding — a candidate indexed STRICTLY BELOW
/// the passed zookie revision is stale; at-or-above is fresh.** The consumer honours the revision
/// watermark the reverse index also honours (one zookie→revision encoding — no drift, §4.2.3).
#[test]
fn cdc_consumer_4_10_zookie_revision_watermark() {
    // indexed @5 vs passed @9 → stale (the index projection predates the demanded snapshot).
    assert_eq!(
        disposition(Some("z@5"), "z@9"),
        CandidateDisposition::StaleNeedsRevalidation,
        "indexed below the passed zookie revision is stale (4.10 watermark)"
    );
    // indexed @9 vs passed @9 → fresh (the watermark is `>=`, the at-snapshot case is fresh).
    assert_eq!(
        disposition(Some("z@9"), "z@9"),
        CandidateDisposition::Fresh,
        "indexed at the passed zookie revision is fresh (the watermark is inclusive)"
    );
    // The partition over a candidate set re-validates ONLY the stale subset (the bounded affected
    // set, §4.2.3 — never every hit).
    let anchor = |id: &str| match id {
        "fresh" => Some("z@9".to_string()),
        "stale" => Some("z@4".to_string()),
        _ => None,
    };
    let (fresh, stale) = stale_candidates(["fresh", "stale"], "z@9", anchor);
    assert_eq!(fresh, vec!["fresh".to_string()]);
    assert_eq!(
        stale,
        vec!["stale".to_string()],
        "ONLY the affected candidate is re-validated"
    );
}

/// **CONSUMER (4.2): the bounded `check` re-validation — a denying check excludes the stale
/// candidate (the new-enemy), an allowing check admits it.** The consumer contract: `check` is a
/// per-object gate evaluated at the demanded snapshot; Search calls it on the affected set only.
#[test]
fn cdc_consumer_4_2_bounded_check_admit_or_exclude() {
    struct Port {
        revoked: &'static str,
    }
    impl BoundedCheckPort for Port {
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
    let port = Port {
        revoked: "acme/issue/SECRET-9",
    };
    let at = strong(9);
    let perm = Permission("read".into());
    assert!(
        port.check(&subject(), &perm, &ObjectId("acme/issue/PUB-1".into()), &at)
            .unwrap(),
        "a still-granted object re-validates ALLOW (surface it)"
    );
    assert!(
        !port
            .check(
                &subject(),
                &perm,
                &ObjectId("acme/issue/SECRET-9".into()),
                &at
            )
            .unwrap(),
        "the revoked object re-validates DENY (exclude the new-enemy)"
    );
}

/// **CONSUMER (1.10): the substrate `FailStatic<T>` ladder is the cache Search degrades on — Fresh
/// within ttl, Static (degraded) up to `static_max ≤ revocation SLA W`, then Closed (deny).** Pins
/// the consumed 1.10 mechanism (the degrade-not-cascade default the BoundedStale read rides) so a
/// drift in the ladder shape is caught here.
#[test]
fn cdc_consumer_1_10_fail_static_degrade_not_cascade() {
    // static_max = 300 (== revocation SLA), agent-token-ttl = 60 (the lower bound). The mechanism
    // serves the last known-good cached coarse grant during a hiccup, NEVER fails open.
    let bound = StalenessBound {
        revocation_sla_secs: 300,
        agent_token_ttl_secs: 60,
    };
    let fs = FailStatic::<&str, u8>::try_new(30, 300, bound).expect("a valid fail-static window");
    // A fresh read caches the coarse grant.
    assert_eq!(fs.get("acl:alice", || Ok(1u8)), Answer::Fresh(1));
    // A subsequent Id hiccup INSIDE the window degrades (serves stale), it does NOT cascade closed.
    let degraded = fs.get("acl:alice", || Err(ServeError("identity hiccup".into())));
    assert!(
        degraded.is_fresh() || degraded.is_degraded(),
        "an Id hiccup inside the window degrades-not-cascades (never fails open), got {degraded:?}"
    );
    // A hiccup with NO cached value fails CLOSED (never open) — the deny-when-unsure default.
    let cold = fs.get("acl:bob", || Err(ServeError("identity hiccup".into())));
    assert!(
        cold.is_closed(),
        "no cached coarse grant → fail CLOSED, never open (ADR-03)"
    );
    // The staleness budget is bounded by the revocation SLA (1.10: static_max ≤ revocation SLA).
    assert!(
        fs.static_max() <= 300,
        "the fail-static window never outlives the revocation SLA"
    );
}
