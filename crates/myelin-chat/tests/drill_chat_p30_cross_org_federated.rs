use myelin_chat::cross_org::{
    fanned_out_carried_fields, CellLocalChannelResolution, ChannelProjection, CrossOrgChannel,
    CrossOrgPointer, FederatedMember,
};
use myelin_chat::store::ConversationId;
use myelin_events::{CellId, CorrelationId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId;

fn conv() -> ConversationId {
    ConversationId::new("acme", "fr-par", "01J0XORG")
}

fn viewer(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn roster() -> Vec<FederatedMember> {
    vec![
        FederatedMember::new("psn:creator", CellId::from_token("cell-fr-par-1")),
        FederatedMember::new("psn:de-a", CellId::from_token("cell-de-1")),
        FederatedMember::new("psn:de-b", CellId::from_token("cell-de-1")),
        FederatedMember::new("psn:nl", CellId::from_token("cell-nl-1")),
    ]
}

struct HomeCellResolver {
    allowed: Vec<String>,
}

impl CellLocalChannelResolution for HomeCellResolver {
    fn resolve_in_home_cell(&self, pointer: &CrossOrgPointer, v: &Principal) -> ChannelProjection {
        let subject = pointer.subject().clone();
        if self.allowed.iter().any(|id| id == &v.principal_id.0) {
            ChannelProjection::Rendered {
                subject,
                rendered: "#cross-org-incident · active".into(),
            }
        } else {
            ChannelProjection::Tombstone { subject }
        }
    }
}

#[test]
fn drill_cross_cell_resolution_is_cell_local_zero_raw_rows() {
    let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
    let fanned = xorg.fan_out_channel_event(&conv(), &CorrelationId("root".into()), &roster());

    let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
    assert_eq!(dests, vec!["cell-de-1", "cell-nl-1"]);

    for pp in &fanned {
        let (_to, subject, kind, _corr, home) = fanned_out_carried_fields(pp);
        assert_eq!(
            subject.artifact_ref().0,
            "myelin://acme/chat/channel/01J0XORG"
        );
        assert_eq!(kind, &myelin_events::ArtifactType::Channel);
        assert_eq!(home.as_str(), "cell-fr-par-1");
        let wire = serde_json::to_string(&pp.pointer).unwrap();
        assert!(!wire.contains("payload"), "no body field crossed: {wire}");
    }

    let home = HomeCellResolver {
        allowed: vec!["psn:de-a".into()],
    };
    let ok = xorg.resolve_cell_local(&fanned[0], &viewer("psn:de-a"), &home);
    assert!(
        ok.is_rendered(),
        "an authorised cross-org viewer gets the projection"
    );

    let denied = xorg.resolve_cell_local(&fanned[0], &viewer("psn:intruder"), &home);
    assert!(
        !denied.is_rendered(),
        "an unauthorised viewer gets a tombstone"
    );
    let wire = serde_json::to_string(&denied).unwrap();
    assert!(
        !wire.contains("cross-org-incident"),
        "the channel name never crossed: {wire}"
    );

    assert_eq!(
        xorg.raw_rows_crossed(),
        0,
        "0 raw rows crossed the cell boundary"
    );
}

#[test]
fn drill_multi_cell_dsr_iterates_member_cells_zero_missed() {
    let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
    let cells = xorg.dsr_member_cells(&roster());
    let names: Vec<&str> = cells.iter().map(CellId::as_str).collect();
    assert_eq!(names, vec!["cell-de-1", "cell-fr-par-1", "cell-nl-1"]);
    assert_eq!(
        cells.len(),
        3,
        "0 member cells missed by the DSR enumeration"
    );
}
