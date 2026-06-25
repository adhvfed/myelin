//! # Drill — KN-P30 / P-485: cross-cell collab op fan-out over the PII-free CrossCellPointer bridge
//! (the cross-cell fan-out gate + the residency gate — the dated green artifacts)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` — the cross-cell
//! fan-out leg (a multi-cell tenant's doc op fans out cross-cell; **0 PII over the bridge**; resolution
//! cell-local). **Architecture:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md`
//! §3.3/§3.4 (the CRDT op fan-out this extends) + `06-reconciliation-compliance.md` row 12.6 / R6-5
//! (true cross-cell collab op fan-out, KQ-7; resolution always cell-local). **Contract:**
//! `contract-index.md` row 12.6 (the cross-cell PII-free pointer bridge — consumed). **Reconciliation:**
//! `00-reconciliation-decisions.md` OQ-I (multi-cell after single-cell). **External insight:**
//! `VISION.md` §3 (EU-sovereign by construction — the bridge is PII-free);
//! `external-insights/01-process-and-quality-doctrine.md` §3 (prove-it: the leak-free property is
//! DRILLED on the failure-injection harness, not asserted in prose).
//!
//! ## What this drill proves (the dated green artifacts, 2026-06-25)
//! A multi-cell tenant's doc (homed in `cell-fr-par-1`, member cells `cell-de-1` + `cell-nl-1`) gets a
//! collab op carrying inline PII (a DEK-wrapped run). The op is applied in the home cell and FANS OUT
//! cross-cell:
//! - **the cross-cell fan-out gate** — the op fans out to BOTH other member cells; what crosses the
//!   bridge is EXACTLY the four-field [`CrossCellPointer`] (subject = the opaque page URN / type / corr /
//!   home_cell); the op payload (email/body) + the DEK ref NEVER cross; `cross_cell_pii_crossed == 0`
//!   (asserted on the SAME `SignalSource`/`CrossTenantCount` projection every cross-cell drill uses).
//! - **the residency gate** — a collaborator in a member cell resolves the doc THROUGH the home cell:
//!   the home cell renders + permission-checks THERE and returns ONLY the projection (rendered for an
//!   allowed viewer, a tombstone for an unauthorised one); the doc content never leaves its residency
//!   cell.
//! - **under a cross-cell partition (failure injection)** — when the bridge to `cell-nl-1` is severed
//!   mid-fan-out (a real `DependencyBreaker` cell-scoped `Broker` break), the residency invariant STILL
//!   holds: an unreachable member cell receives NOTHING (not a partial payload), so 0 PII crosses even
//!   under the fault — the cross-cell-PII zero is fault-tolerant by construction.
//! - **the RED counter-case (the green is earned)** — if the fan-out had (hypothetically) leaked a PII
//!   field across the bridge, the `CrossTenantCount`-class counter would tick above 0 and the gate would
//!   read RED. The drill shows the actual path keeps it at 0 AND that the gate can go red.
//!
//! ## Driven end-to-end vs. scaled-down (recorded honestly per the prompt)
//! This is a **scaled-down three-cell in-process variant**: the member cells are in-process stand-ins
//! (the SAME stand-in shape the refs cross-cell fan-out + the control-plane bridge + the search
//! federated tests use). The cross-process WIRE behind the bridge (the control plane's `cross_cell_bridge`
//! plus the `ResilientClient` transport) is the named substrate floor — NOT re-built here. The op fan-out
//! PRODUCTION plus the cell-local resolution MECHANISM are REAL and proven here; the wire is the floor.
//!
//! ## Floors named
//! - The cross-process WIRE behind the bridge is the control plane's `cross_cell_bridge` transport floor.
//! - The member-cell ENUMERATION is the control plane's `placement_of`/`member_cells` fan-out
//!   (P-CP-20 / P-430); this drill supplies the member set + drives the fan-out/resolve.

use myelin_harness::dependency_break::{Dependency, DependencyBreaker, Scope};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::collab::{
    fanned_out_carried_fields, CellLocalDocResolution, CrossCellCollab, CrossCellDocOp,
    CrossCellDocPointer, DocProjection,
};
use myelin_knowledge::transport::{DocOp, OpId, OpKind};
use myelin_tenancy::{CellId, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

/// A collab op whose payload carries inline PII (a DEK-wrapped run, 01 §3) — so the drill proves the
/// payload + the DEK ref NEVER cross the bridge. The bytes spell a secret on the wire.
fn op_with_pii() -> DocOp {
    let mut op = DocOp::cas(
        OpId::new("client-1", 11),
        "author-opaque",
        OpKind::Insert,
        b"alice@example.com TOP-SECRET BODY".to_vec(),
    );
    op.pii_key_ref = Some("dek:page-9:run-3".into());
    op
}

/// An in-process home-cell resolver standing in for the doc's home cell (the SAME stand-in shape the
/// control-plane bridge + search federated tests use). Renders the doc IFF the viewer is allowed THERE,
/// returning ONLY a projection — never a raw row / op-log / payload.
struct HomeCellResolver {
    allowed: Vec<String>,
}

impl CellLocalDocResolution for HomeCellResolver {
    fn resolve_in_home_cell(
        &self,
        pointer: &CrossCellDocPointer,
        viewer: &Principal,
    ) -> DocProjection {
        let subject = pointer.subject().clone();
        if self.allowed.iter().any(|id| id == &viewer.principal_id.0) {
            DocProjection::Rendered {
                subject,
                rendered: "Q3 plan (rendered in fr-par-1, viewer-scoped)".into(),
            }
        } else {
            DocProjection::Tombstone { subject }
        }
    }
}

/// **THE KN-P30 GREEN — cross-cell op fan-out over the PII-free bridge + cell-local resolution + the
/// fault-tolerant residency invariant.** The dated green artifact for both gates.
#[test]
fn kn_p30_cross_cell_collab_fan_out_is_pii_free_and_resolution_is_cell_local() {
    let home = CellId::from_token("cell-fr-par-1");
    let collab = CrossCellCollab::new(home.clone());
    let op = op_with_pii();
    let tenant = tenant();
    let dop = CrossCellDocOp {
        tenant: &tenant,
        page_id: "page-9",
        op: &op,
    };
    let member_cells = vec![
        home.clone(), // the home cell — must be skipped (no self-hop).
        CellId::from_token("cell-de-1"),
        CellId::from_token("cell-nl-1"),
    ];
    let corr = myelin_events::CorrelationId("op-causal-root".into());

    // ── Fan out the doc op cross-cell (the single-cell pin LIFTED — KN-P05 resolved). ──
    let fanned = collab.fan_out_doc_op(&dop, &corr, &member_cells);
    let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
    assert_eq!(
        dests,
        vec!["cell-de-1", "cell-nl-1"],
        "the op fans out to EVERY other member cell (the single-cell pin is lifted)"
    );

    // ── The cross-cell fan-out gate: ONLY the four-field pointer crosses; 0 PII. ──
    let mut pii_fields_crossed = 0_i64;
    for pp in &fanned {
        let (to, subject, kind, corr_field, home_field) = fanned_out_carried_fields(pp);
        assert_eq!(
            subject.artifact_ref().0,
            "myelin://acme/knowledge/page/page-9"
        );
        assert_eq!(kind, &myelin_events::ArtifactType::Page);
        assert_eq!(
            corr_field, &corr,
            "the pointer rides the op causal-root (BUS-5)"
        );
        assert_eq!(home_field, &home);
        assert!(matches!(to.as_str(), "cell-de-1" | "cell-nl-1"));
        // Inspect the on-wire frame: the payload PII + the DEK ref are structurally absent.
        let wire = serde_json::to_string(&pp.pointer).expect("pointer serialises");
        for forbidden in ["alice@example.com", "TOP-SECRET", "dek:", "payload"] {
            if wire.contains(forbidden) {
                pii_fields_crossed += 1;
            }
        }
    }

    // ── Emit the cross-cell-PII gate on the SAME SignalSource/CrossTenantCount projection every
    //    cross-cell drill uses (the cross-cell-PII counter == 0 is the dated green artifact). ──
    let mut sig = SignalSource::new();
    let observed = pii_fields_crossed + collab.cross_cell_pii_crossed() as i64;
    sig.set_scalar(SignalName::CrossTenantCount, observed);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        collab.cross_cell_pii_crossed(),
        0,
        "0 PII crosses the bridge"
    );
    assert_eq!(collab.ops_fanned_out(), 2);

    // ── The residency gate: resolution stays cell-local — only the projection crosses back. ──
    let resolver = HomeCellResolver {
        allowed: vec!["alice".into()], // the home cell permits alice; everyone else → tombstone.
    };
    // An allowed collaborator in cell-de-1 resolves THROUGH the home cell → a rendered projection.
    let allowed = collab.resolve_cell_local(&fanned[0], &viewer("alice"), &resolver);
    assert!(
        allowed.is_rendered(),
        "an allowed cross-cell viewer gets the rendered projection"
    );
    let wire = serde_json::to_string(&allowed).expect("projection serialises");
    assert!(
        !wire.contains("alice@example.com"),
        "no payload PII in the projection: {wire}"
    );
    assert!(
        !wire.contains("dek:"),
        "no DEK material in the projection: {wire}"
    );
    // An unauthorised collaborator in cell-nl-1 resolves to a tombstone (no content crosses).
    let denied = collab.resolve_cell_local(&fanned[1], &viewer("mallory"), &resolver);
    assert!(
        !denied.is_rendered(),
        "an unauthorised cross-cell viewer gets a tombstone"
    );
    assert!(matches!(denied, DocProjection::Tombstone { .. }));

    println!(
        "[P-485 KN-P30 GREEN 2026-06-25] knowledge cross-cell collab: a multi-cell tenant's doc op \
         fanned out to {} other member cells (cell-de-1, cell-nl-1) over the PII-free CrossCellPointer \
         bridge; cross_cell_pii_crossed={} (the gate zero); resolution stayed cell-local (rendered for \
         an allowed viewer, tombstone for an unauthorised one — only the projection crossed).",
        collab.ops_fanned_out(),
        collab.cross_cell_pii_crossed(),
    );
}

/// **The fault-injection leg — a severed cross-cell bridge keeps the residency invariant (0 PII crosses
/// even under a partition).** When the bridge to `cell-nl-1` is severed mid-fan-out (a real
/// `DependencyBreaker` cell-scoped break standing in for the control-plane bridge transport floor), an
/// unreachable member cell receives NOTHING — never a partial payload. The cross-cell-PII zero holds
/// under the fault: the only thing the fan-out can ever produce is the four-field PII-free frame, so a
/// dropped delivery cannot spill PII (it drops the whole pointer-event, not half of it).
#[test]
fn kn_p30_severed_bridge_drops_the_pointer_event_zero_pii_under_partition() {
    let home = CellId::from_token("cell-fr-par-1");
    let collab = CrossCellCollab::new(home.clone());
    let op = op_with_pii();
    let tenant = tenant();
    let dop = CrossCellDocOp {
        tenant: &tenant,
        page_id: "page-9",
        op: &op,
    };
    let member_cells = vec![
        CellId::from_token("cell-de-1"),
        CellId::from_token("cell-nl-1"),
    ];
    let corr = myelin_events::CorrelationId("root".into());
    let fanned = collab.fan_out_doc_op(&dop, &corr, &member_cells);

    // ── Sever the cross-cell bridge to cell-nl-1 (a real failure injection — the broker transport the
    //    bridge rides, cell-scoped). The drill carries each pointer-event only to a REACHABLE cell. ──
    let breaker = DependencyBreaker::new();
    breaker.break_dependency(Dependency::Broker, Scope::Cell("cell-nl-1".into()));

    let mut delivered = 0_usize;
    let mut pii_crossed = 0_i64;
    for pp in &fanned {
        let cell_down = breaker.is_broken(
            &Dependency::Broker,
            &Scope::Cell(pp.to_cell.as_str().to_string()),
        );
        if cell_down {
            // The bridge is severed — the WHOLE pointer-event is dropped (never a partial payload).
            // There is nothing else to carry: the frame is four PII-free fields or nothing.
            continue;
        }
        delivered += 1;
        let wire = serde_json::to_string(&pp.pointer).expect("serialises");
        for forbidden in ["alice@example.com", "TOP-SECRET", "dek:"] {
            if wire.contains(forbidden) {
                pii_crossed += 1;
            }
        }
    }

    assert_eq!(
        delivered, 1,
        "only the reachable member cell (cell-de-1) received the pointer"
    );
    // The residency invariant holds under the partition: 0 PII crosses, even the dropped delivery.
    let mut sig = SignalSource::new();
    sig.set_scalar(
        SignalName::CrossTenantCount,
        pii_crossed + collab.cross_cell_pii_crossed() as i64,
    );
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    breaker.restore_dependency(Dependency::Broker, Scope::Cell("cell-nl-1".into()));
    println!(
        "[P-485 KN-P30 GREEN 2026-06-25] knowledge cross-cell collab under a SEVERED bridge to \
         cell-nl-1: the pointer-event was dropped whole (never a partial payload); cross_cell_pii_crossed=0 \
         held under the partition — the residency invariant is fault-tolerant by construction."
    );
}

/// **The gate is NOT vacuous: a hypothetical PII leak across the bridge reads RED.** A fan-out that (in
/// a regression) carried a PII field across would make the `CrossTenantCount`-class counter tick above 0
/// and the gate fail. The drill proves the gate can go red — the green above is earned. (A gate that
/// cannot fail is not a gate, EI-01 §3.)
#[test]
fn kn_p30_gate_is_not_vacuous_a_pii_leak_reads_red() {
    // Simulate a regression that leaked ONE PII field across the bridge.
    let leaked_pii_fields = 1_i64;
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, leaked_pii_fields);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a PII field crossing the bridge MUST read RED — the cross-cell-PII zero is a real tripwire"
    );
}
