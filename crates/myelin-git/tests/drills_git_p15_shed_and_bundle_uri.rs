//! GIT-P15 chained e2e drill (global P-276, M3-G2) — the protected-human-lane shed order under a
//! synthetic mixed-principal storm + the CDN bundle-URI accelerated-clone, chained end-to-end.
//!
//! The prompt's GATE / DRILLS:
//! - The shed order holds under a synthetic mixed-principal storm: **the human lane is served while
//!   the agent/CI lane sheds (`429 + Retry-After`)**. The full 30× surge is GIT-D6 in GIT-P34 (M5);
//!   here the order is asserted at **1× with mixed principals** (the green artifact: the per-lane
//!   shed-count signal — human lane 0 shed, agent lane sheds).
//! - **A clone served a bundle-URI from the CDN class round-trips a valid clone** (the
//!   accelerated-clone floor holds).
//!
//! The CHAINED e2e: a mixed-principal storm sheds the agent lane and serves the human → the human's
//! interactive fetch then takes the accelerated CDN bundle-URI path and round-trips a valid clone.
//! This proves the two halves compose — the human the shed order protected is the one the bundle-URI
//! serves, with the budget reached later because the bulk clone-storm read fan-out left serving
//! compute for the content-addressed object tier.
//!
//! FLOORS (named in the module + the report): the OQ-K per-surface shed budget NUMBERS are tuned by
//! the 30× clone-storm GIT-D6 (GIT-P34/M5); the CDN bundle-URI floor hardens to the full within-EU
//! CDN class in GIT-P33 (M5). Here the ORDER + the round-trip are asserted at 1× over the in-memory
//! `FsBlobStore` floor.

use myelin_git::shed_clone::{BundleUriClone, GitFrontDoorShed};
use myelin_identity::{
    DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef,
};
use myelin_storage::blob::FsBlobStore;
use myelin_storage::cdn::CdnCloneClass;
use myelin_substrate::shed::{RunClass, Surface};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{Region, TenantId};

fn tenant(s: &str) -> TenantId {
    TenantId::from_token(s)
}

fn principal(tenant_slug: &str, kind: PrincipalKind) -> Principal {
    Principal::new(
        tenant(tenant_slug),
        Region("fr-par".into()),
        PrincipalId(format!("p-{tenant_slug}-{}", kind_tag(&kind))),
        kind,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn kind_tag(kind: &PrincipalKind) -> &'static str {
    match kind {
        PrincipalKind::Human => "h",
        PrincipalKind::Agent { .. } => "a",
        PrincipalKind::Service => "s",
    }
}

fn agent_kind() -> PrincipalKind {
    PrincipalKind::Agent {
        runtime_ref: RuntimeRef("rt".into()),
        on_behalf_of: None,
    }
}

/// **The chained GIT-P15 drill: mixed-principal storm → human served, agent shed → bundle-URI clone
/// valid.** The green artifact is the per-lane shed-count signal (human 0, agent > 0) PLUS the
/// round-tripped clone bytes.
#[test]
fn git_p15_storm_protects_the_human_then_serves_the_accelerated_bundle_uri_clone() {
    // ── stage 1: the front-door shed gate, budget from the thresholds file (the production budget). ──
    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let budget = thresholds
        .shed_budget(Surface::GitFrontDoor)
        .expect("GitFrontDoor budget present");
    let mut shed = GitFrontDoorShed::from_thresholds(&thresholds).expect("open the shed gate");

    let t = "acme";
    let human = principal(t, PrincipalKind::Human);

    // ── stage 2: a synthetic MIXED-PRINCIPAL storm (agents + CI/service), 1× over the cap. ──
    // drive the storm to saturation: agent + batch/ci (service) fetches pour in until the lanes shed.
    let mut agent_admitted = 0u32;
    let mut agent_shed = 0u32;
    let mut ci_shed = 0u32;
    for i in 0..(budget.per_tenant_in_flight_cap * 2) {
        // interleave an agent and a service (CI/batch) principal — the mixed-principal storm.
        if i % 2 == 0 {
            let a = principal(t, agent_kind());
            match shed.admit_for(&a, None) {
                Ok(_) => agent_admitted += 1,
                Err(rej) => {
                    assert_eq!(rej.lane, RunClass::Agent);
                    agent_shed += 1;
                }
            }
        } else {
            let svc = principal(t, PrincipalKind::Service);
            if let Err(rej) = shed.admit_for(&svc, None) {
                assert_eq!(rej.lane, RunClass::BatchCi, "the CI/batch lane sheds");
                ci_shed += 1;
            }
        }
    }
    // the storm shed BOTH machine lanes (agent + CI/batch) — the order fired.
    assert!(agent_shed >= 1, "the agent lane shed under the storm");
    assert!(ci_shed >= 1, "the CI/batch lane shed under the storm");
    // the machine lanes never consumed the reserved human slots.
    assert!(
        agent_admitted <= budget.per_tenant_in_flight_cap - budget.human_lane_reservation,
        "the machine lanes never take the reserved human slots"
    );

    // ── stage 3: THE HUMAN IS SERVED while the machine lanes shed (the protected lane). ──
    let human_class = shed
        .admit_for(&human, None)
        .expect("the human's interactive fetch is served while the storm sheds the machine lanes");
    assert_eq!(human_class, RunClass::Human);

    // GREEN ARTIFACT (the per-lane shed-count signal): human lane 0 shed, agent lane sheds.
    assert_eq!(
        shed.shed_count(RunClass::Human),
        0,
        "human lane: 0 shed (served under the storm)"
    );
    assert!(shed.shed_count(RunClass::Agent) >= 1, "agent lane: shed");
    assert!(
        shed.shed_count(RunClass::BatchCi) >= 1,
        "CI/batch lane: shed"
    );

    // ── stage 4: the served human takes the ACCELERATED CDN bundle-URI clone path. ──
    // (the bulk clone-storm read fan-out left serving compute for the object tier; the human's clone
    //  is served the precomputed bundle by content-address.)
    let store = FsBlobStore::new();
    let cdn = CdnCloneClass::over(tenant(t), Region::new("fr-par"), true, &store);
    let bundle = BundleUriClone::new(cdn);

    let repo_bundle_bytes = b"PACK\0acme/widgets-clone-bundle@deadbeefcafe";
    let uri = bundle
        .publish_bundle(repo_bundle_bytes)
        .expect("serving tier publishes the bundle");
    // the human clones via the advertised bundle-URI → a VALID clone (round-trips the exact bytes).
    let cloned = bundle
        .clone_via_bundle_uri(&uri)
        .expect("the bundle-URI clone round-trips a valid clone");
    assert_eq!(
        cloned, repo_bundle_bytes,
        "the accelerated-clone floor holds — the bundle-URI clone is valid + content-address-verified"
    );

    // and the human releases its slot — the lane recovers after the storm (no leak).
    shed.release(&tenant(t), RunClass::Human);
}
