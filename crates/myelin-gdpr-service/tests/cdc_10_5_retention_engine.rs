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

#[test]
fn cdc_10_5_effective_retention_tightest_wins_recorded() {
    let holds = LegalHoldRegistry::new();
    let engine = RetentionEngine::new(&holds);

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
        "recorded which input won (the tenant) - the auditor consumes this"
    );

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
        "Identity FIRST - canonical order"
    );
}
