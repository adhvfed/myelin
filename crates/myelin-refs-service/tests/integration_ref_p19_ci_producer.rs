use std::sync::Arc;

use myelin_events::check_seam::{check_subject, check_updated_draft, CheckSeamOrder};
use myelin_events::{
    Actor, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType, Timestamp,
    Visibility,
};
use myelin_git::check_status::{
    CheckContext, CheckState, CheckStatus, GitOid, HumanisedRef, Timestamp as CheckTs, TrustTier,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{
    bounded_stale, ladder_root, CiOwner, NoOpCacheRead, Resolution, ResolveMode, ResolveService,
    StepAnchorResolver, StepResolution, TombstoneReason,
};
use myelin_storage::{CiLogError, CiLogFrame, CiLogTier, KekId, KmsEngine};
use myelin_substrate::{FailStaticAuthz, FailStaticThreshold};
use myelin_tenancy::{CellId, Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn cell() -> CellId {
    CellId::from_token("cell-fr-par-1")
}
fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("u-alice".into()),
        PrincipalKind::Human,
        tenant(),
    )
}
fn ci_actor() -> Actor {
    Actor(Principal::stub(
        PrincipalId("ci".into()),
        PrincipalKind::Service,
        tenant(),
    ))
}

fn authz() -> Arc<FailStaticAuthz> {
    let threshold = FailStaticThreshold {
        status: "OPEN - LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    };
    Arc::new(FailStaticAuthz::try_new(300, &threshold).expect("valid bound"))
}

fn engine() -> Arc<KmsEngine> {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant(), region()));
    Arc::new(kms)
}

struct CiLogStepSeam {
    tier: Arc<CiLogTier>,
}
impl StepAnchorResolver for CiLogStepSeam {
    fn resolve_step(&self, anchor: &ArtifactRef) -> StepResolution {
        match self.tier.resolve_step_anchor(&anchor.0) {
            Ok(bytes) => StepResolution::Live {
                byte_len: bytes.len() as u64,
            },
            Err(CiLogError::UnknownStep { .. })
            | Err(CiLogError::MalformedAnchor(_))
            | Err(CiLogError::LimitExceeded(_)) => StepResolution::Gone,
            Err(CiLogError::Archive(_)) | Err(CiLogError::SpanOutOfBounds { .. }) => {
                StepResolution::Erased
            }
        }
    }
}

fn fact(
    commit: &str,
    ctx: &str,
    attempt: u32,
    state: CheckState,
    run: &str,
    step: u32,
) -> CheckStatus {
    CheckStatus {
        tenant: tenant(),
        repo: ArtifactRef("myelin://acme/git/repo/core".into()),
        commit_oid: GitOid(commit.into()),
        context: CheckContext::ci(ctx),
        state,
        required: true,
        run: ArtifactRef(format!("myelin://acme/ci/run/{run}")),
        run_attempt: attempt,
        trust_tier: TrustTier::Trusted,
        details_ref: ArtifactRef(format!("myelin://acme/ci/run/{run}#step-{step}")),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args: std::collections::BTreeMap::new(),
        },
        started_at: CheckTs("2026-06-21T00:00:00Z".into()),
        completed_at: Some(CheckTs("2026-06-21T00:01:00Z".into())),
        cost_settled: true,
    }
}

fn check_env(seq: u64, f: &CheckStatus) -> (EventEnvelope, u64) {
    let draft = check_updated_draft(
        &f.repo.0,
        &f.commit_oid.0,
        &f.context.name,
        serde_json::to_value(f).expect("CheckStatus serialises"),
    );
    let env = EventEnvelope {
        event_id: EventId(format!("evt-{}-{}", f.context.name, f.run_attempt)),
        type_: EventType(draft.type_.0),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: ci_actor(),
        subject: draft.subject,
        aggregate: draft.aggregate,
        causation_id: None,
        correlation_id: CorrelationId(format!("corr-{}", f.commit_oid.0)),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload: draft.payload,
    };
    (env, seq)
}

fn resolve_through_chokepoint(owner: CiOwner, ref_: &ArtifactRef) -> Resolution {
    let svc = ResolveService::new(authz(), Arc::new(NoOpCacheRead), Arc::new(owner), cell());
    let root = ladder_root(ref_);
    svc.resolve(
        &tenant(),
        &region(),
        ref_,
        &root,
        &viewer(),
        ResolveMode::Live,
        &bounded_stale(),
        false,
    )
}

#[test]
fn ref_d9_ci_check_and_step_anchors_resolve_through_the_one_ladder_root_carried() {
    let owner = CiOwner::new();
    let commit = "abc123";

    let tier = Arc::new(CiLogTier::with_tenant_dek(
        "run-7",
        tenant(),
        region(),
        engine(),
    ));
    tier.seal_ci_batch(&[
        (1, CiLogFrame::new("run-7", 1, b"checkout ok\n".to_vec())),
        (
            2,
            CiLogFrame::new("run-7", 2, b"FAIL: assertion at line 42\n".to_vec()),
        ),
    ])
    .expect("seal the real CI log batch");
    owner.wire_step_resolver(Arc::new(CiLogStepSeam { tier: tier.clone() }));

    let f = fact(commit, "build", 1, CheckState::Failure, "run-7", 2);
    let mut order = CheckSeamOrder::new(&f.repo.0, &f.commit_oid.0);
    let (env, seq) = check_env(1, &f);
    assert!(order.ingest(&env, seq).expect("ingest the check envelope"));
    assert_eq!(
        env.subject,
        check_subject(&f.repo.0, &f.commit_oid.0, "build")
    );
    let decoded: CheckStatus = order
        .in_order()
        .into_iter()
        .map(|oc| serde_json::from_value(oc.check_status).expect("decode CheckStatus"))
        .next()
        .expect("one check");
    let check_anchor = CiOwner::check_anchor("acme", commit, "build");
    owner.ingest_check(&check_anchor, &decoded);

    let grant_owner = owner.clone();
    let check_root = CiOwner::check_root("acme", commit);
    let step_root = CiOwner::run_root("acme", "run-7");
    grant_owner.grant_view(&tenant(), &region(), &viewer(), &check_root);
    grant_owner.grant_view(&tenant(), &region(), &viewer(), &step_root);

    match resolve_through_chokepoint(owner.clone(), &check_anchor) {
        Resolution::Projection(_) => {}
        other => panic!("the check-<context> anchor must resolve LIVE, got {other:?}"),
    }

    let step_anchor = CiOwner::step_anchor("acme", "run-7", 2);
    match resolve_through_chokepoint(owner.clone(), &step_anchor) {
        Resolution::Projection(p) => {
            assert!(
                p.state.contains("27 bytes"),
                "the #step-<n> resolves to the exact failing step bytes through the sealed segments, got {:?}",
                p.state
            );
        }
        other => panic!("the #step-<n> jump-to-failure must resolve LIVE, got {other:?}"),
    }

    let unknown_step = CiOwner::step_anchor("acme", "run-7", 99);
    match resolve_through_chokepoint(owner.clone(), &unknown_step) {
        Resolution::Tombstone(t) => {
            assert_eq!(t.reason, TombstoneReason::SubGone);
            assert_eq!(
                t.root,
                CiOwner::run_root("acme", "run-7"),
                "the tombstone carries the root run (the embed shows the parent)"
            );
        }
        other => {
            panic!("an unknown step must tombstone (sub_gone) with the root carried, got {other:?}")
        }
    }
}

#[test]
fn out_of_order_ci_check_resolves_latest_by_run_attempt_through_the_real_carriage() {
    let owner = CiOwner::new();
    let commit = "deadbeef";
    let check_anchor = CiOwner::check_anchor("acme", commit, "build");
    owner.grant_view(
        &tenant(),
        &region(),
        &viewer(),
        &CiOwner::check_root("acme", commit),
    );

    let lo = fact(commit, "build", 1, CheckState::Failure, "1", 3);
    let hi = fact(commit, "build", 2, CheckState::Success, "2", 3);
    let mut order = CheckSeamOrder::new(&lo.repo.0, &lo.commit_oid.0);
    let (env_hi, _) = check_env(2, &hi);
    let (env_lo, _) = check_env(1, &lo);
    assert!(order.ingest(&env_hi, 2).unwrap());
    assert!(order.ingest(&env_lo, 1).unwrap());
    assert_eq!(order.ordering_gap(), 0, "contiguous, fully ordered");

    for oc in order.in_order() {
        let decoded: CheckStatus =
            serde_json::from_value(oc.check_status).expect("decode CheckStatus");
        owner.ingest_check(&check_anchor, &decoded);
    }

    match resolve_through_chokepoint(owner.clone(), &check_anchor) {
        Resolution::Projection(_) => {}
        other => {
            panic!("the check anchor must resolve the latest-by-attempt success, got {other:?}")
        }
    }
    let row = owner
        .current_row(&hi)
        .expect("a current row for the (commit, context) key");
    assert_eq!(
        row.run_attempt, 2,
        "the high-water mark is the re-run attempt"
    );
    assert_eq!(
        row.state,
        CheckState::Success,
        "the current sub-anchor state is the re-run success, never the stale failure"
    );

    let outcome = owner.ingest_check(&check_anchor, &lo);
    assert_eq!(
        outcome,
        myelin_git::check_status::ApplyOutcome::DroppedStale {
            incoming_attempt: 1,
            current_attempt: 2
        },
        "a late lower-attempt re-delivery is dropped - the success stays current"
    );
    assert_eq!(
        owner.current_row(&hi).unwrap().state,
        CheckState::Success,
        "the sub-anchor never regresses to the stale failure"
    );
}

#[test]
fn a_crypto_shredded_ci_log_segment_makes_the_step_anchor_unrecoverable() {
    use myelin_storage::{DekId, KeyClass};

    let eng = engine();
    let tier = Arc::new(CiLogTier::with_tenant_dek(
        "run-1",
        tenant(),
        region(),
        eng.clone(),
    ));
    tier.seal_ci_batch(&[(
        1,
        CiLogFrame::new("run-1", 1, b"inline-PII-step-log".to_vec()),
    )])
    .expect("seal");

    assert_eq!(
        tier.resolve_step_anchor("myelin://acme/ci/run/run-1#step-1")
            .expect("resolves before the shred"),
        b"inline-PII-step-log"
    );

    assert!(eng.destroy_dek(&DekId::new(tenant(), KeyClass::Tenant)));
    let seam = CiLogStepSeam { tier: tier.clone() };
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        seam.resolve_step(&ArtifactRef("myelin://acme/ci/run/run-1#step-1".into()))
    }));
    match res {
        Ok(StepResolution::Erased) | Ok(StepResolution::Gone) => {}
        Err(_) => {}
        Ok(StepResolution::Live { .. }) => {
            panic!("a crypto-shredded segment must NEVER resolve LIVE - that is a leak of shredded data")
        }
    }
}
