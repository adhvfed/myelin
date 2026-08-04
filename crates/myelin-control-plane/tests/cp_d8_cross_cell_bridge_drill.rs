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

#[test]
fn cp_d8_cross_cell_bridge_resolution() {
    let subject = "myelin://01J0BETA/issues/issue/7";
    let mut b = HomeCell::new();
    b.permit(subject, "viewer-1");
    b.render(subject, "Ship the bridge");

    let mut reg = CellResolverRegistry::new();
    reg.register(CellId::from_token("cell-b"), Arc::new(b));
    let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

    let p = pointer(subject, "cell-b");

    let (carried_subject, carried_type, carried_corr, carried_home) = bridge_carried_fields(&p);
    assert_eq!(carried_subject.artifact_ref().0, subject);
    assert_eq!(carried_type, &ArtifactType::Issue);
    assert_eq!(carried_corr, &CorrelationId("01J0CHAIN".into()));
    assert_eq!(carried_home.as_str(), "cell-b");

    let allowed = bridge.resolve(&p, &ViewerId::from_token("viewer-1"), BridgeMode::Live);
    assert!(
        allowed.is_projection(),
        "an authorised cross-cell viewer gets the home-cell-rendered projection (the gate is GREEN)"
    );
    let BridgeResolution::Projection(proj) = allowed else {
        unreachable!()
    };
    assert_eq!(proj.title, "Ship the bridge");

    let denied = bridge.resolve(&p, &ViewerId::from_token("viewer-2"), BridgeMode::Live);
    assert!(
        denied.is_tombstone(),
        "an unauthorised cross-cell viewer gets a tombstone (the gate is RED for the spoof)"
    );
    assert_eq!(
        denied.tombstone_reason(),
        Some(BridgeTombstoneReason::Denied)
    );
    let BridgeResolution::Tombstone(t) = denied else {
        unreachable!()
    };
    assert_eq!(t.subject.artifact_ref().0, subject);

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

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, raw_rows as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-429 CP-D8 GREEN 2026-06-24] CrossCellPointer bridge resolution LIVE (always cell-local): a \
         cross-cell ref homed in cell-b was resolved per-viewer IN cell-b - the bridge carried ONLY \
         subject/type/correlation_id/home_cell (the four frozen fields); an AUTHORISED viewer saw the \
         home-cell-rendered projection, an UNAUTHORISED viewer got a tombstone (no leak across the cell \
         boundary). cross_cell_resolves={}, PII/raw-rows across the bridge={} (the CP-D8 zero). FLOOR: \
         the multi-element member_cells fan-out + DSR + zookie + rebalancing ride P-CP-20; the \
         [OPEN - LEGAL] bridge-residency proof ships regardless of ratification (PII-free by \
         construction).",
        bridge.cross_cell_resolves(),
        raw_rows,
    );
}

#[test]
fn cp_d8_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a raw row / PII crossing the bridge MUST read RED - the CP-D8 zero is a real tripwire"
    );
}
