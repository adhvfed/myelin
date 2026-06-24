//! **CDC pair for 12.6-LIVE (provider + consumer) — P-CP-19 / P-429.** The `CrossCellPointer` bridge
//! resolution is LIVE (architecture §6.2, always cell-local; contract 12.6). This CDC pins the
//! PROVIDER (the control-plane [`CrossCellBridge`] + the [`CellLocalResolver`] seam) against the
//! CONSUMERS that ride it — an **ISS cross-cell portfolio rollup**, a **KN cross-cell collab** resolve,
//! and a **CHAT cross-org channel** resolve — all over the SAME frozen four-field
//! [`myelin_tenancy::CrossCellPointer`] frame and the SAME cell-local resolution rule (EI-01 §7: ONE
//! frozen frame, ONE resolution rule).
//!
//! **The provider side** is the bridge dispatching the resolve to the pointer's home cell through the
//! [`CellLocalResolver`] seam — whose `resolve_in_cell(pointer, viewer, mode)` is the contract-5.2
//! `resolve(ref, viewer, mode) -> Projection | Tombstone` shape the production resolver
//! (`myelin_refs_service::ResolveService`) implements. The §2.9 DAG forbids a
//! `myelin-control-plane` -> `myelin-refs-service` edge, so the resolver is modelled here over the
//! frozen seam (the SAME pattern the events-side 12.6 CDC uses; the real wire is the named transport
//! floor). If the bridge shape or the seam signature drifts, this consumer stops compiling — the whole
//! point of a glue CDC.
//!
//! **The load-bearing properties pinned:** (1) the bridge carries ONLY the four frozen fields; (2)
//! resolution is permission-checked IN the home cell; (3) ONLY a filtered projection / tombstone
//! crosses back (never a raw row); (4) an unauthorised viewer gets a tombstone (no cross-cell leak).

use std::collections::HashMap;
use std::sync::Arc;

use myelin_control_plane::{
    bridge_carried_fields, BridgeMode, BridgeProjection, BridgeResolution, BridgeTombstone,
    BridgeTombstoneReason, CellLocalResolver, CellResolverRegistry, CrossCellBridge, ViewerId,
};
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId,
};

/// A refs-`resolve(ref, viewer, mode)`-shaped cell-local resolver (the home cell's `ResolveService`
/// stand-in): permission-checks IN this cell, returns ONLY a filtered projection / tombstone.
struct RefsShapedResolver {
    permitted: HashMap<(String, String), bool>,
    rendered: HashMap<String, (String, String, String)>,
}

impl RefsShapedResolver {
    fn new() -> Self {
        RefsShapedResolver {
            permitted: HashMap::new(),
            rendered: HashMap::new(),
        }
    }
    fn permit(mut self, subject: &str, viewer: &str) -> Self {
        self.permitted.insert((subject.into(), viewer.into()), true);
        self
    }
    fn render(mut self, subject: &str, title: &str) -> Self {
        self.rendered
            .insert(subject.into(), (title.into(), "open".into(), "doc".into()));
        self
    }
}

impl CellLocalResolver for RefsShapedResolver {
    // This signature IS the contract-5.2 `resolve(ref, viewer, mode) -> Projection | Tombstone` shape
    // (the production `ResolveService::resolve` returns a `Resolution::{Projection, Tombstone}`).
    fn resolve_in_cell(
        &self,
        pointer: &CrossCellPointer,
        viewer: &ViewerId,
        _mode: BridgeMode,
    ) -> BridgeResolution {
        let subject = pointer.subject().artifact_ref().0.clone();
        // Step 2: check IN this (home) cell against ITS tuples. Denied → tombstone (no leak).
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
        // Step 3: ONLY the already-filtered projection crosses back (never a raw row).
        let (title, state, icon) = self.rendered.get(&subject).cloned().unwrap();
        BridgeResolution::Projection(BridgeProjection {
            subject: pointer.subject().clone(),
            title,
            state,
            icon,
        })
    }
}

fn pointer(subject: &str, kind: ArtifactType, home: &str) -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef(subject.into())),
        kind,
        CorrelationId("01J0CHAIN".into()),
        CellId::from_token(home),
    )
}

/// **CONSUMER 1 — ISS cross-cell portfolio rollup (§6.2): aggregate the projections the viewer may
/// see across member cells.** It rides the bridge's `rollup` and reads back ONLY the four-field-derived
/// projections; a denied artifact does not contribute (no leak of a count the viewer isn't entitled to).
#[test]
fn cdc_12_6_iss_rollup_consumer() {
    let cell_b = RefsShapedResolver::new()
        .permit("myelin://01J0BETA/issues/issue/7", "viewer-1")
        .render("myelin://01J0BETA/issues/issue/7", "Visible issue")
        // issue/8 rendered but NOT permitted → denied → excluded.
        .render("myelin://01J0BETA/issues/issue/8", "Hidden issue");

    let mut reg = CellResolverRegistry::new();
    reg.register(CellId::from_token("cell-b"), Arc::new(cell_b));
    let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

    let portfolio = vec![
        pointer(
            "myelin://01J0BETA/issues/issue/7",
            ArtifactType::Issue,
            "cell-b",
        ),
        pointer(
            "myelin://01J0BETA/issues/issue/8",
            ArtifactType::Issue,
            "cell-b",
        ),
    ];
    let rolled = bridge.rollup(
        &portfolio,
        &ViewerId::from_token("viewer-1"),
        BridgeMode::Live,
    );
    assert_eq!(rolled.len(), 1, "only the permitted projection aggregates");
    assert_eq!(rolled[0].title, "Visible issue");
    // The bridge carried ONLY the four frozen fields; 0 raw rows.
    let (_s, kind, _c, home) = bridge_carried_fields(&portfolio[0]);
    assert_eq!(kind, &ArtifactType::Issue);
    assert_eq!(home.as_str(), "cell-b");
    assert_eq!(bridge.cross_cell_raw_rows(), 0);
}

/// **CONSUMER 2 — KN cross-cell collab (§6.2): resolve a page homed in another cell, in the home
/// cell.** An authorised collaborator sees the projection; the resolve was permission-checked IN the
/// home cell.
#[test]
fn cdc_12_6_kn_collab_consumer() {
    let cell_b = RefsShapedResolver::new()
        .permit("myelin://01J0BETA/kn/page/3", "collab-1")
        .render("myelin://01J0BETA/kn/page/3", "Shared design");
    let mut reg = CellResolverRegistry::new();
    reg.register(CellId::from_token("cell-b"), Arc::new(cell_b));
    let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

    let p = pointer("myelin://01J0BETA/kn/page/3", ArtifactType::Page, "cell-b");
    let res = bridge.resolve(&p, &ViewerId::from_token("collab-1"), BridgeMode::Live);
    assert!(
        res.is_projection(),
        "an authorised collaborator sees the page"
    );
    assert_eq!(bridge.cross_cell_raw_rows(), 0);
}

/// **CONSUMER 3 — CHAT cross-org channel (§6.2): an UNAUTHORISED cross-org viewer gets a TOMBSTONE
/// (no leak across the org boundary).** The home cell denies a non-member; only a `Denied` tombstone
/// (no content) crosses back.
#[test]
fn cdc_12_6_chat_cross_org_unauthorised_tombstone() {
    let cell_b = RefsShapedResolver::new()
        // member-1 is permitted; outsider-9 is NOT.
        .permit("myelin://01J0BETA/chat/channel/5", "member-1")
        .render("myelin://01J0BETA/chat/channel/5", "#secret-org-channel");
    let mut reg = CellResolverRegistry::new();
    reg.register(CellId::from_token("cell-b"), Arc::new(cell_b));
    let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

    let p = pointer(
        "myelin://01J0BETA/chat/channel/5",
        ArtifactType::Channel,
        "cell-b",
    );
    let res = bridge.resolve(&p, &ViewerId::from_token("outsider-9"), BridgeMode::Live);
    assert!(
        res.is_tombstone(),
        "an unauthorised cross-org viewer gets a tombstone"
    );
    assert_eq!(res.tombstone_reason(), Some(BridgeTombstoneReason::Denied));
    // The tombstone carries no channel name — structurally there is no content field to leak.
    let BridgeResolution::Tombstone(t) = res else {
        unreachable!()
    };
    assert_eq!(
        t.subject.artifact_ref().0,
        "myelin://01J0BETA/chat/channel/5"
    );
    assert_eq!(bridge.cross_cell_raw_rows(), 0);
}
