//! # CDC 10.5 (the retention leg) — the retention engine + legal-hold-aware suspend (P-GA-22 → P-149)
//!
//! **Contract:** index row 10.5 (the retention leg — *"`effective_retention` (tightest-wins,
//! legal-hold-aware); legal-hold-aware suspend-don't-delete"*, gdpr §5.1). This is the consumer-
//! driven contract test the coverage scanner (P-S21) reads both halves of, for the retention leg of
//! 10.5 (the consent / sub-processor / `transfer_allowed` legs are P-GA-23 → P-150):
//!
//! - **provider** = the retention engine ([`RetentionEngine`]) — `effective_retention(inputs)`
//!   (tightest-policy-wins merge, deterministic + recorded which input won, legal-floor-respecting),
//!   plus the legal-hold-aware `expire(scope)` (suspend-don't-delete: an active hold defers the
//!   deletion; on lift it resumes — 0 held-scope deletions; expiry uses the §3 erasure mechanisms).
//! - **consumer** = (a) a **retention caller** (a per-tenant retention configurator / the periodic
//!   expiry sweep) that asks `effective_retention(category, tenant, store)` and reads the WINNING
//!   source (auditable which input won); (b) a **hold caller** (the legal-ops surface that backs the
//!   G4 hold) that sets a hold and observes the expiry DEFER (0 held-scope deletions) + RESUME.
//!
//! The dated green artifact: a tenant 30-day policy beats a 90-day default (the tenant wins,
//! recorded); a lawful 6-month floor overrides a tenant "delete immediately" (the floor wins,
//! recorded); a hold suspends an expiry (0 deletions) and the deletion resumes on lift via the §3
//! mechanisms. If 10.5's retention leg drifts (the merge stops being tightest-wins / floor-
//! respecting, a hold stops suspending an expiry, an expiry deletes under a hold), this stops
//! compiling/passing — that is the contract.

use core::time::Duration;

use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::{
    holder_ids, legal_floor, platform_default, tenant_delete_immediately, tenant_window,
    EraseChecklist, ExpiryOutcome, HoldScope, InMemoryShredKms, LegalHoldRegistry, RetentionEngine,
    RetentionSource, SeamHolder, ShredKeyClass, ShredKeyHandle, UpstreamHolderOrchestrator,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

const DAY: u64 = 24 * 60 * 60;

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

fn subject_scope(s: &str) -> EraseScope {
    EraseScope::Subject {
        subject: subject(s),
        tenant: tenant(),
    }
}

fn kms_with_all_holder_keys(base_epoch: u64) -> InMemoryShredKms {
    let kms = InMemoryShredKms::new();
    for (i, id) in [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ]
    .iter()
    .enumerate()
    {
        kms.provision(
            ShredKeyHandle {
                tenant: tenant(),
                class: ShredKeyClass::Subject((*id).to_string()),
            },
            base_epoch + i as u64,
        );
    }
    kms
}

fn seam_holders(kms: &InMemoryShredKms) -> Vec<(&'static str, SeamHolder<'_>)> {
    [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ]
    .into_iter()
    .map(|id| {
        (
            id,
            SeamHolder::new(id, ShredKeyClass::Subject(id.to_string()), kms),
        )
    })
    .collect()
}

/// **The tightest-policy-wins merge leg of 10.5 (the provider) ⇄ a retention caller (the
/// consumer).** A tenant 30-day policy beats a 90-day default (recorded: the tenant won); a lawful
/// 6-month floor overrides a tenant "delete immediately" (recorded: the floor won). The recorded
/// winning source is the seam the auditor consumes.
#[test]
fn cdc_10_5_effective_retention_tightest_wins_recorded() {
    let holds = LegalHoldRegistry::new();
    let engine = RetentionEngine::new(&holds);

    // consumer: the retention caller asks for the effective retention of a category.
    let tenant_wins = engine.effective_retention(&[
        platform_default(Duration::from_secs(90 * DAY)),
        tenant_window(Duration::from_secs(30 * DAY)),
    ]);
    assert_eq!(
        tenant_wins.window_secs(),
        30 * DAY,
        "the most restrictive (tenant 30d) wins"
    );
    assert_eq!(
        tenant_wins.winning_source,
        RetentionSource::TenantPolicy,
        "recorded which input won (the tenant) — the auditor consumes this"
    );

    // the legal floor overrides a tenant "delete immediately" (the floor is a lower bound).
    let floor_wins = engine.effective_retention(&[
        tenant_delete_immediately(),
        legal_floor(Duration::from_secs(180 * DAY)),
    ]);
    assert_eq!(
        floor_wins.window_secs(),
        180 * DAY,
        "the lawful 6-month floor clamps UP"
    );
    assert_eq!(
        floor_wins.winning_source,
        RetentionSource::LegalFloor,
        "recorded: the floor overrode the tenant"
    );
    assert!(floor_wins.floor_clamped);
}

/// **The legal-hold-aware suspend leg of 10.5 (the provider) ⇄ a hold caller (the consumer).** A
/// hold caller sets a hold; the engine SUSPENDS the retention-expiry (0 held-scope deletions); on
/// hold-lift the deferred deletion RESUMES via the §3 erasure mechanisms. This is the GA-D6 seam.
#[test]
fn cdc_10_5_legal_hold_suspends_expiry_and_resumes_on_lift() {
    let kms = kms_with_all_holder_keys(500);
    let holders = seam_holders(&kms);
    let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
        holders
            .iter()
            .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
            .collect(),
    );
    let holds = LegalHoldRegistry::new();
    let engine = RetentionEngine::new(&holds);
    let scope = subject_scope("u-cdc");
    let checklist = EraseChecklist::new();

    // consumer (the hold caller): set a hold. provider: the expiry is SUSPENDED.
    holds.set(
        HoldScope::Subject {
            tenant: "acme".into(),
            subject: "u-cdc".into(),
        },
        true,
    );
    let deferred = engine.expire(&scope, &upstream, &checklist).unwrap();
    assert_eq!(
        deferred,
        ExpiryOutcome::DeferredUnderHold,
        "suspend-don't-delete under the hold"
    );
    assert!(!deferred.ran_deletion(), "0 held-scope deletions");
    for (_, h) in &holders {
        assert_eq!(h.erase_call_count(), 0, "no holder erased under the hold");
    }

    // consumer: the hold caller LIFTS the hold. provider: the deferred deletion RESUMES.
    holds.set(
        HoldScope::Subject {
            tenant: "acme".into(),
            subject: "u-cdc".into(),
        },
        false,
    );
    let resumed = engine.expire(&scope, &upstream, &checklist).unwrap();
    assert!(
        resumed.ran_deletion(),
        "the deferred deletion resumes on hold-lift"
    );
    let receipts = match resumed {
        ExpiryOutcome::Expired(r) => r,
        other => panic!("expected Expired on resume, got {other:?}"),
    };
    assert_eq!(
        receipts.len(),
        6,
        "every holder fanned on resume (the §3 mechanisms)"
    );
    assert_eq!(
        receipts[0].holder_id,
        holder_ids::IDENTITY,
        "Identity FIRST — canonical order"
    );
}
