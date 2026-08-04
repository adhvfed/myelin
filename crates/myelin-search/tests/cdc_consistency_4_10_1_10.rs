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

#[test]
fn cdc_consumer_4_10_zookie_revision_watermark() {
    assert_eq!(
        disposition(Some("z@5"), "z@9"),
        CandidateDisposition::StaleNeedsRevalidation,
        "indexed below the passed zookie revision is stale (4.10 watermark)"
    );
    assert_eq!(
        disposition(Some("z@9"), "z@9"),
        CandidateDisposition::Fresh,
        "indexed at the passed zookie revision is fresh (the watermark is inclusive)"
    );
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

#[test]
fn cdc_consumer_1_10_fail_static_degrade_not_cascade() {
    let bound = StalenessBound {
        revocation_sla_secs: 300,
        agent_token_ttl_secs: 60,
    };
    let fs = FailStatic::<&str, u8>::try_new(30, 300, bound).expect("a valid fail-static window");
    assert_eq!(fs.get("acl:alice", || Ok(1u8)), Answer::Fresh(1));
    let degraded = fs.get("acl:alice", || Err(ServeError("identity hiccup".into())));
    assert!(
        degraded.is_fresh() || degraded.is_degraded(),
        "an Id hiccup inside the window degrades-not-cascades (never fails open), got {degraded:?}"
    );
    let cold = fs.get("acl:bob", || Err(ServeError("identity hiccup".into())));
    assert!(
        cold.is_closed(),
        "no cached coarse grant → fail CLOSED, never open (ADR-03)"
    );
    assert!(
        fs.static_max() <= 300,
        "the fail-static window never outlives the revocation SLA"
    );
}
