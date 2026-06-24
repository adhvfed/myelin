//! P-CP-19 (global P-429) GATE / DRILL — **the `CrossCellPointer` bridge resolution is LIVE (CP-D8,
//! FLOOR): a cross-cell ref → the bridge carries ONLY `subject`/`type`/`correlation_id`/`home_cell`,
//! the target resolves per-viewer IN the home cell, an unauthorised viewer → tombstone, 0 PII across
//! the bridge** — dated green artifact.
//!
//! **The GATE (testing-strategy CP-D8 / tenancy-and-control-plane.md §6.2):** a cross-cell ref
//! (multi-cell) → the bridge carries ONLY the four frozen fields; the target resolves per-viewer in the
//! home cell; an unauthorised viewer → tombstone. Telemetry: the PII-free bridge proof, **0 PII across
//! the bridge**. SCHED. **Never weaken a threshold to pass.**
//!
//! **The load-bearing zero (EI-01 §2):** a cross-cell PII leak is stop-the-bleeding. The defence here
//! is STRUCTURAL: the bridge carries only the four-field PII-free [`myelin_tenancy::CrossCellPointer`]
//! frame across + the opaque viewer id, and ONLY an already-rendered, already-permission-filtered
//! projection / tombstone crosses back — never a raw row. So `cross_cell_raw_rows == 0` is by
//! construction, not by a query that "happened" to return nothing.
//!
//! **This drill proves the gate can go RED** (an unauthorised viewer IS tombstoned — no leak; a gate
//! that cannot go red is not a gate, EI-01 §3) **AND green** (an authorised viewer sees the projection
//! resolved IN the home cell), and emits the CP-D8 result on the SAME [`SignalSource`] every drill uses
//! (observability is part of the pass).
//!
//! **FLOOR (named, VISION §3):** the bridge RESOLUTION is live; the multi-element `member_cells`
//! FAN-OUT + cross-cell DSR + cross-cell zookie consistency + multi-cell rebalancing ride **P-CP-20**.
//! The `[OPEN — LEGAL]` cross-cell bridge residency proof (counsel sign-off that the four fields are
//! not personal data) ships regardless of ratification — PII-free by construction.

use std::collections::HashMap;
use std::sync::Arc;

use myelin_control_plane::{
    bridge_carried_fields, BridgeMode, BridgeProjection, BridgeResolution, BridgeTombstone,
    BridgeTombstoneReason, CellLocalResolver, CellResolverRegistry, CrossCellBridge, ViewerId,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId,
};

/// The home cell B's resolver: permission-checks IN B, returns ONLY a filtered projection / tombstone.
struct HomeCell {
    permitted: HashMap<(String, String), bool>,
    rendered: HashMap<String, String>,
}

impl HomeCell {
    fn new() -> Self {
        HomeCell {
            permitted: HashMap::new(),
            rendered: HashMap::new(),
        }
    }
    fn permit(&mut self, subject: &str, viewer: &str) {
        self.permitted.insert((subject.into(), viewer.into()), true);
    }
    fn render(&mut self, subject: &str, title: &str) {
        self.rendered.insert(subject.into(), title.into());
    }
}

impl CellLocalResolver for HomeCell {
    fn resolve_in_cell(
        &self,
        pointer: &CrossCellPointer,
        viewer: &ViewerId,
        _mode: BridgeMode,
    ) -> BridgeResolution {
        let subject = pointer.subject().artifact_ref().0.clone();
        if !*self
            .permitted
            .get(&(subject.clone(), viewer.as_str().into()))
            .unwrap_or(&false)
        {
            return BridgeResolution::Tombstone(BridgeTombstone {
                subject: pointer.subject().clone(),
                reason: BridgeTombstoneReason::Denied,
            });
        }
        BridgeResolution::Projection(BridgeProjection {
            subject: pointer.subject().clone(),
            title: self.rendered.get(&subject).cloned().unwrap(),
            state: "open".into(),
            icon: "issue".into(),
        })
    }
}

fn pointer(subject: &str, home: &str) -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef(subject.into())),
        ArtifactType::Issue,
        CorrelationId("01J0CHAIN".into()),
        CellId::from_token(home),
    )
}

/// **THE CP-D8 DRILL (dated green artifact): a cross-cell ref → bridge carries only the four frozen
/// fields, resolves per-viewer in the home cell, unauthorised → tombstone, 0 PII across the bridge.**
#[test]
fn cp_d8_cross_cell_bridge_resolution() {
    // Cell A serves the viewer; the artifact is homed in cell B. B authorises viewer-1, denies viewer-2.
    let subject = "myelin://01J0BETA/issues/issue/7";
    let mut b = HomeCell::new();
    b.permit(subject, "viewer-1");
    b.render(subject, "Ship the bridge");

    let mut reg = CellResolverRegistry::new();
    reg.register(CellId::from_token("cell-b"), Arc::new(b));
    let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

    let p = pointer(subject, "cell-b");

    // ── The bridge carries ONLY the four frozen §6.1 fields (the PII-free bridge proof). ──
    let (carried_subject, carried_type, carried_corr, carried_home) = bridge_carried_fields(&p);
    assert_eq!(carried_subject.artifact_ref().0, subject);
    assert_eq!(carried_type, &ArtifactType::Issue);
    assert_eq!(carried_corr, &CorrelationId("01J0CHAIN".into()));
    assert_eq!(carried_home.as_str(), "cell-b");

    // ── GREEN leg: the authorised viewer resolves per-viewer IN the home cell → projection. ──
    let allowed = bridge.resolve(&p, &ViewerId::from_token("viewer-1"), BridgeMode::Live);
    assert!(
        allowed.is_projection(),
        "an authorised cross-cell viewer gets the home-cell-rendered projection (the gate is GREEN)"
    );
    let BridgeResolution::Projection(proj) = allowed else {
        unreachable!()
    };
    assert_eq!(proj.title, "Ship the bridge");

    // ── RED leg: the UNAUTHORISED viewer gets a TOMBSTONE — no leak across the cell boundary. ──
    let denied = bridge.resolve(&p, &ViewerId::from_token("viewer-2"), BridgeMode::Live);
    assert!(
        denied.is_tombstone(),
        "an unauthorised cross-cell viewer gets a tombstone (the gate is RED for the spoof)"
    );
    assert_eq!(
        denied.tombstone_reason(),
        Some(BridgeTombstoneReason::Denied)
    );
    // The tombstone carries NO content — structurally there is no title field to leak into.
    let BridgeResolution::Tombstone(t) = denied else {
        unreachable!()
    };
    assert_eq!(t.subject.artifact_ref().0, subject);

    // ── The CP-D8 ZERO: 0 PII / 0 raw rows crossed the bridge across BOTH resolves. ──
    let raw_rows = bridge.cross_cell_raw_rows();
    assert_eq!(
        raw_rows, 0,
        "0 PII / 0 raw rows across the bridge (the CP-D8 zero)"
    );
    assert_eq!(
        bridge.cross_cell_resolves(),
        2,
        "two cross-cell resolves served"
    );

    // ── Emit the CP-D8 gate result on the SAME SignalSource every drill uses (observability is part
    //    of the pass, EI-01 §3): the cross-cell raw-row / PII count == 0 (the headline CP-D8 zero;
    //    the CrossTenantCount projection is the cross-cell/cross-tenant leak counter). ──
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, raw_rows as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-429 CP-D8 GREEN 2026-06-24] CrossCellPointer bridge resolution LIVE (always cell-local): a \
         cross-cell ref homed in cell-b was resolved per-viewer IN cell-b — the bridge carried ONLY \
         subject/type/correlation_id/home_cell (the four frozen fields); an AUTHORISED viewer saw the \
         home-cell-rendered projection, an UNAUTHORISED viewer got a tombstone (no leak across the cell \
         boundary). cross_cell_resolves={}, PII/raw-rows across the bridge={} (the CP-D8 zero). FLOOR: \
         the multi-element member_cells fan-out + DSR + zookie + rebalancing ride P-CP-20; the \
         [OPEN — LEGAL] bridge-residency proof ships regardless of ratification (PII-free by \
         construction).",
        bridge.cross_cell_resolves(),
        raw_rows,
    );
}

/// **The gate is NOT vacuous: a raw row / PII crossing the bridge would read RED.** Proves the CP-D8
/// zero is a real tripwire — if a (hypothetical) regression carried one raw row across the bridge,
/// `CrossTenantCount > 0` would fail the predicate. (The structural defence pins the real value to 0;
/// this asserts the assertion itself is load-bearing — EI-01 §3, a gate that cannot go red is not a
/// gate.)
#[test]
fn cp_d8_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    // A hypothetical regression that carried ONE raw row / PII across the bridge.
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a raw row / PII crossing the bridge MUST read RED — the CP-D8 zero is a real tripwire"
    );
}
