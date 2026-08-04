use std::collections::HashMap;
use std::sync::Arc;

use myelin_control_plane::{
    bridge_carried_fields, BridgeMode, BridgeProjection, BridgeResolution, BridgeTombstone,
    BridgeTombstoneReason, CellLocalResolver, CellResolverRegistry, CrossCellBridge, ViewerId,
};
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId,
};

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

#[test]
fn cdc_12_6_iss_rollup_consumer() {
    let cell_b = RefsShapedResolver::new()
        .permit("myelin://01J0BETA/issues/issue/7", "viewer-1")
        .render("myelin://01J0BETA/issues/issue/7", "Visible issue")
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
    let (_s, kind, _c, home) = bridge_carried_fields(&portfolio[0]);
    assert_eq!(kind, &ArtifactType::Issue);
    assert_eq!(home.as_str(), "cell-b");
    assert_eq!(bridge.cross_cell_raw_rows(), 0);
}

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

#[test]
fn cdc_12_6_chat_cross_org_unauthorised_tombstone() {
    let cell_b = RefsShapedResolver::new()
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
    let BridgeResolution::Tombstone(t) = res else {
        unreachable!()
    };
    assert_eq!(
        t.subject.artifact_ref().0,
        "myelin://01J0BETA/chat/channel/5"
    );
    assert_eq!(bridge.cross_cell_raw_rows(), 0);
}
