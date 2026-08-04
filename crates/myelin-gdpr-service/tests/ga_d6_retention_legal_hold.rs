use core::time::Duration;

use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::{
    holder_ids, legal_floor, platform_default, tenant_delete_immediately, tenant_window,
    EraseChecklist, ExpiryOutcome, HoldScope, InMemoryShredKms, LegalHoldRegistry, RetentionEngine,
    RetentionSource, SeamHolder, ShredKeyClass, ShredKeyHandle, UpstreamHolderOrchestrator,
    RETENTION_EXPIRY_RUNS, RETENTION_HELD_SCOPE_DELETIONS,
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

#[test]
fn ga_d6_retention_engine_legal_hold_suspend_dont_delete() {
    let kms = kms_with_all_holder_keys(1_000);
    let holders = seam_holders(&kms);
    let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
        holders
            .iter()
            .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
            .collect(),
    );
    let holds = LegalHoldRegistry::new();
    let engine = RetentionEngine::new(&holds);

    let effective = engine.effective_retention(&[
        platform_default(Duration::from_secs(90 * DAY)),
        tenant_window(Duration::from_secs(30 * DAY)),
    ]);
    assert_eq!(
        effective.window_secs(),
        30 * DAY,
        "tightest-wins: the tenant 30-day window"
    );
    assert_eq!(
        effective.winning_source,
        RetentionSource::TenantPolicy,
        "the tightest-wins decision is RECORDED (which input won - §5.1)"
    );

    let floor_won = engine.effective_retention(&[
        tenant_delete_immediately(),
        legal_floor(Duration::from_secs(180 * DAY)),
    ]);
    assert_eq!(
        floor_won.winning_source,
        RetentionSource::LegalFloor,
        "the legal floor won, recorded"
    );
    assert!(
        floor_won.floor_clamped,
        "the floor clamped the tenant delete-immediately UP"
    );

    let stored_at = 1_700_000_000;
    let now = stored_at + 30 * DAY;
    assert!(
        effective.has_elapsed(stored_at, now),
        "the 30-day window has elapsed"
    );

    let scope = subject_scope("u-d6");
    let checklist = EraseChecklist::new();
    holds.set(
        HoldScope::Subject {
            tenant: "acme".into(),
            subject: "u-d6".into(),
        },
        true,
    );

    let deferred = engine.expire(&scope, &upstream, &checklist).unwrap();
    assert_eq!(
        deferred,
        ExpiryOutcome::DeferredUnderHold,
        "the hold-defer receipt - suspend-don't-delete (Art. 17(3)(e))"
    );
    assert!(
        !deferred.ran_deletion(),
        "the expiry did NOT run under the hold"
    );

    let held_scope_deletions: u32 = holders.iter().map(|(_, h)| h.erase_call_count()).sum();
    assert_eq!(
        held_scope_deletions, 0,
        "0 held-scope deletions (the GA-D6 green artifact value)"
    );
    assert_eq!(
        checklist.done_count(),
        0,
        "no holder receipted under the hold"
    );

    holds.set(
        HoldScope::Subject {
            tenant: "acme".into(),
            subject: "u-d6".into(),
        },
        false,
    );
    let mut expiry_runs = 0u64;
    let resumed = engine.expire(&scope, &upstream, &checklist).unwrap();
    if resumed.ran_deletion() {
        expiry_runs += 1;
    }
    let receipts = match resumed {
        ExpiryOutcome::Expired(r) => r,
        other => panic!("expected Expired on resume, got {other:?}"),
    };
    assert_eq!(
        receipts.len(),
        6,
        "the §3 erasure mechanisms ran over every holder on resume"
    );
    assert_eq!(
        receipts[0].holder_id,
        holder_ids::IDENTITY,
        "Identity FIRST (canonical order)"
    );
    for hr in &receipts {
        assert!(
            hr.receipt.receipt.key_epoch_destroyed.is_some(),
            "each holder records its destroyed key epoch (the §3 crypto-shred mechanism, auditable)"
        );
    }

    for (id, h) in &holders {
        assert_eq!(
            h.erase_call_count(),
            1,
            "holder {id} erased exactly once (on resume only)"
        );
    }

    assert_eq!(
        expiry_runs, 1,
        "the retention-expiry ran exactly once (on resume)"
    );
    assert_eq!(
        RETENTION_HELD_SCOPE_DELETIONS,
        ("gdpr.retention_held_scope_deletions", "count"),
        "the GA-D6 invariant signal NAME + UNIT (its value across the held window was 0)"
    );
    assert_eq!(
        RETENTION_EXPIRY_RUNS,
        ("gdpr.retention_expiry_runs", "count"),
        "the retention-expiry-runs health signal NAME + UNIT"
    );
}
