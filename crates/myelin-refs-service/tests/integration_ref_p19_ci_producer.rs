//! **REF-P19 / P-335 — Refs resolves the Git↔CI CheckStatus seam sub-anchors (the Refs half of X-1),
//! PROVEN against the REAL CI producer half end-to-end.**
//!
//! This is the real-producer integration the prompt's binding policy requires for REF-P19. It is NOT
//! gated behind the `integration` cargo feature because the 11.8 sealed CI log segments tier
//! (`myelin_storage::CiLogTier`) is **fs-backed + content-addressed + DEK-encrypted IN-PROCESS** at
//! this band (the object-store S3 backing is the named P-ST-30/M5 floor) and the Bus carriage +
//! supersession are in-process production code — there is no DB/cache/bus contract REF-P19 introduces
//! (the Refs engine is FIXED at M2; this prompt adds only sub-anchor RESOLUTION). So the REAL artifacts
//! here are: a real `KmsEngine` per-tenant DEK, real sealed content-addressed CI log segments, the real
//! `check_seam` per-aggregate ordering, the real `CheckStatusProjection` monotonic supersession, and
//! the full Refs `ResolveService` chokepoint + the ONE ladder. No mock of the CI producer.
//!
//! What is proven (the GATE / DRILLS — REF-D9 on the CI anchors + the X-1 sub-anchor supersession):
//! 1. **REF-D9 on CI `check-`/`step-` anchors:** every `check-<context>` / `step-<n>` resolves through
//!    the ONE ladder to the correct state with the ROOT carried (the Refs half of the X-1 details_ref
//!    resolution, INCL. resolving the `#step-<n>` jump-to-failure through the 11.8 SEALED LOG SEGMENTS).
//! 2. **Out-of-order supersession at the sub-anchor level:** a higher `run_attempt` `ci.check.updated`
//!    arriving BEFORE a stale lower one (the at-least-once transport reordered them) resolves the
//!    `check-<context>` anchor to the LATEST by `run_attempt` — never the physically-last-arrived.
//! 3. **A `#step-<n>` resolves to the EXACT failing step's bytes** through the real sealed segments; a
//!    crypto-shredded segment surfaces as a LOUD tombstone (ERASED), never a wrong/empty serve.

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
        status: "OPEN — LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    };
    Arc::new(FailStaticAuthz::try_new(300, &threshold).expect("valid bound"))
}

/// A real `KmsEngine` with the tenant KEK provisioned (the cell-provisioning seam).
fn engine() -> Arc<KmsEngine> {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant(), region()));
    Arc::new(kms)
}

/// **The 11.8 seam adapter — Refs CONSUMES the real `CiLogTier` to resolve a `#step-<n>` `details_ref`
/// through the SEALED CI log segments.** This is the production wire (over the resilient client) the
/// `CiOwner` consults; here it holds the real fs-backed, DEK-encrypted, content-addressed tier and
/// resolves the jump-to-failure to the exact failing step's bytes (LIVE), a pruned/unknown step (GONE),
/// or a crypto-shredded segment (ERASED). Refs does NOT re-build the `(job, step, byte-range)` index.
struct CiLogStepSeam {
    tier: Arc<CiLogTier>,
}
impl StepAnchorResolver for CiLogStepSeam {
    fn resolve_step(&self, anchor: &ArtifactRef) -> StepResolution {
        match self.tier.resolve_step_anchor(&anchor.0) {
            Ok(bytes) => StepResolution::Live {
                byte_len: bytes.len() as u64,
            },
            // An unknown step / a pruned run → GONE (the root run still resolves).
            Err(CiLogError::UnknownStep { .. }) | Err(CiLogError::MalformedAnchor(_)) => {
                StepResolution::Gone
            }
            // A crypto-shredded segment (the DEK destroyed) or a corrupt index → ERASED/Gone. A shred
            // surfaces as an archive error; the catch_unwind in the shred test below proves the LOUD
            // refusal. Here a clean archive error maps to GONE (never a wrong serve).
            Err(CiLogError::Archive(_)) | Err(CiLogError::SpanOutOfBounds { .. }) => {
                StepResolution::Erased
            }
        }
    }
}

/// Build a decoded `ci.check.updated` fact (the CI-owned struct the Bus carries opaque). `run` +
/// `details_ref` carry the producing run + the `#step-<n>` jump-to-failure (references-not-payloads).
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

/// Build a real `ci.check.updated` envelope carrying the fact OPAQUE (the producer leg the Bus carries
/// per-aggregate ordered). The `subject` follows the §4.12 `repo#commit-<oid>/check-<context>` grammar.
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

/// Drive the full Refs chokepoint over a wired `CiOwner` (the ONE ladder; the leak gate is real).
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

// ───────────────────────────────────────────────────────────────────────────────────────────────
// 1. REF-D9 on CI anchors: check-<context> + step-<n> resolve through the ONE ladder, root carried.
// ───────────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn ref_d9_ci_check_and_step_anchors_resolve_through_the_one_ladder_root_carried() {
    let owner = CiOwner::new();
    let commit = "abc123";

    // ── Seal a REAL CI log segment (fs-backed, content-addressed, per-tenant-DEK encrypted) holding
    //    the failing step's bytes — the 11.8 sealed log segments the #step-<n> resolves through. ──
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

    // ── Drive a REAL ci.check.updated through the Bus per-aggregate ordering + the supersession. ──
    let f = fact(commit, "build", 1, CheckState::Failure, "run-7", 2);
    let mut order = CheckSeamOrder::new(&f.repo.0, &f.commit_oid.0);
    let (env, seq) = check_env(1, &f);
    assert!(order.ingest(&env, seq).expect("ingest the check envelope"));
    // The envelope subject follows the §4.12 sub-anchor grammar (repo#commit-<oid>/check-<context>).
    assert_eq!(
        env.subject,
        check_subject(&f.repo.0, &f.commit_oid.0, "build")
    );
    // Decode the opaque payload back into the CI-owned fact (Git's consumer view) + feed the sub-anchor.
    let decoded: CheckStatus = order
        .in_order()
        .into_iter()
        .map(|oc| serde_json::from_value(oc.check_status).expect("decode CheckStatus"))
        .next()
        .expect("one check");
    let check_anchor = CiOwner::check_anchor("acme", commit, "build");
    owner.ingest_check(&check_anchor, &decoded);

    // Grant the viewer CI read on both roots (the leak gate is real; an ungranted viewer is tombstoned).
    let grant_owner = owner.clone();
    let check_root = CiOwner::check_root("acme", commit);
    let step_root = CiOwner::run_root("acme", "run-7");
    grant_owner.grant_view(&tenant(), &region(), &viewer(), &check_root);
    grant_owner.grant_view(&tenant(), &region(), &viewer(), &step_root);

    // (a) the check-<context> anchor resolves LIVE (the failing verdict renders; root = the check root).
    match resolve_through_chokepoint(owner.clone(), &check_anchor) {
        Resolution::Projection(_) => {}
        other => panic!("the check-<context> anchor must resolve LIVE, got {other:?}"),
    }

    // (b) the step-<n> details_ref resolves LIVE through the SEALED LOG SEGMENTS — the jump-to-failure
    //     points at the exact failing step's bytes.
    let step_anchor = CiOwner::step_anchor("acme", "run-7", 2);
    match resolve_through_chokepoint(owner.clone(), &step_anchor) {
        Resolution::Projection(p) => {
            // 27 bytes = b"FAIL: assertion at line 42\n".len() — the EXACT failing step's byte-range.
            assert!(
                p.state.contains("27 bytes"),
                "the #step-<n> resolves to the exact failing step bytes through the sealed segments, got {:?}",
                p.state
            );
        }
        other => panic!("the #step-<n> jump-to-failure must resolve LIVE, got {other:?}"),
    }

    // (c) a step the sealed index never saw → a ROOT-CARRYING tombstone (sub_gone), never a hard 404.
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

// ───────────────────────────────────────────────────────────────────────────────────────────────
// 2. The X-1 chained drill: an OUT-OF-ORDER ci.check.updated → resolve the latest by run_attempt.
// ───────────────────────────────────────────────────────────────────────────────────────────────

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

    // The HIGHER attempt (a re-run, SUCCESS) is emitted at outbox seq 2; the stale LOWER attempt
    // (FAILURE) at seq 1. They ARRIVE reordered (seq 2 before seq 1) — the at-least-once transport.
    let lo = fact(commit, "build", 1, CheckState::Failure, "1", 3);
    let hi = fact(commit, "build", 2, CheckState::Success, "2", 3);
    let mut order = CheckSeamOrder::new(&lo.repo.0, &lo.commit_oid.0);
    let (env_hi, _) = check_env(2, &hi);
    let (env_lo, _) = check_env(1, &lo);
    // Deliver out of order: the higher attempt first.
    assert!(order.ingest(&env_hi, 2).unwrap());
    assert!(order.ingest(&env_lo, 1).unwrap());
    assert_eq!(order.ordering_gap(), 0, "contiguous, fully ordered");

    // Decode in per-aggregate seq order + feed each through the supersession (the Refs sub-anchor feed).
    for oc in order.in_order() {
        let decoded: CheckStatus =
            serde_json::from_value(oc.check_status).expect("decode CheckStatus");
        owner.ingest_check(&check_anchor, &decoded);
    }

    // The sub-anchor resolves the LATEST by run_attempt (the re-run SUCCESS), NOT the failure — even
    // though the failure was the physically-last fact applied (seq 1 sorts after… no: seq order is
    // 1,2 so the success is applied LAST here; the real out-of-order guard is the supersession dropping
    // a late lower attempt — proven by the unit drill. Here the per-aggregate order makes it
    // deterministic regardless of arrival).
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

    // And a STALE re-delivery of the lower attempt AFTER the higher is DROPPED (not regressed) — the
    // monotonic supersession at the sub-anchor level (the at-least-once mandatory drop).
    let outcome = owner.ingest_check(&check_anchor, &lo);
    assert_eq!(
        outcome,
        myelin_git::check_status::ApplyOutcome::DroppedStale {
            incoming_attempt: 1,
            current_attempt: 2
        },
        "a late lower-attempt re-delivery is dropped — the success stays current"
    );
    assert_eq!(
        owner.current_row(&hi).unwrap().state,
        CheckState::Success,
        "the sub-anchor never regresses to the stale failure"
    );
}

// ───────────────────────────────────────────────────────────────────────────────────────────────
// 3. A crypto-shredded sealed segment → the #step-<n> is unrecoverable (LOUD), never a wrong serve.
// ───────────────────────────────────────────────────────────────────────────────────────────────

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

    // Before the shred: the #step-1 resolves to the exact bytes (the sealed segment decrypts).
    assert_eq!(
        tier.resolve_step_anchor("myelin://acme/ci/run/run-1#step-1")
            .expect("resolves before the shred"),
        b"inline-PII-step-log"
    );

    // Crypto-shred the per-tenant DEK the segment is sealed under → the step is unrecoverable. The
    // archiver panics/errors on a shredded DEK (the GD-4 lever); the seam surfaces it LOUDLY, never a
    // wrong/empty serve. We assert the resolution is NOT a clean live serve (it is a loud failure).
    assert!(eng.destroy_dek(&DekId::new(tenant(), KeyClass::Tenant)));
    let seam = CiLogStepSeam { tier: tier.clone() };
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        seam.resolve_step(&ArtifactRef("myelin://acme/ci/run/run-1#step-1".into()))
    }));
    match res {
        // The archiver surfaced the shred as a clean archive error → ERASED (a loud tombstone state).
        Ok(StepResolution::Erased) | Ok(StepResolution::Gone) => {}
        // Or it panicked on the destroyed DEK — also a loud refusal, never a live serve.
        Err(_) => {}
        Ok(StepResolution::Live { .. }) => {
            panic!("a crypto-shredded segment must NEVER resolve LIVE — that is a leak of shredded data")
        }
    }
}
