use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Decision, ObjectId, Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple,
    TupleDelta,
};
use myelin_identity_service::{
    CellPartition, CrossCellResolution, MultiCellAuthority, StoreBackedCheck, TupleStore,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId, Region,
    TenantId,
};

const REGION: &str = "fr-par";

fn principal(tenant: &str, id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region(REGION.into());
    p
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn home_cell_b() -> CellPartition {
    let admin = principal("acme", "p-admin");
    let scope = TenantScope::from_verified_token(&admin, admin.region.clone());
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &scope,
            &admin,
            &[
                add("project:web", "parent_team", "team:eng#view"),
                add("team:eng", "member", "p:lin"),
            ],
            None,
            None,
            Timestamp("2026-06-24T00:00:00Z".into()),
        )
        .expect("seed cell B");
    CellPartition::new(
        CellId::from_token("cell-b"),
        Region(REGION.into()),
        StoreBackedCheck::new(store),
    )
}

fn pointer_to_b() -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef(
            "myelin://01JACME/projects/project/web@cell-b".into(),
        )),
        ArtifactType::Page,
        CorrelationId("01JCORR".into()),
        CellId::from_token("cell-b"),
    )
}

fn render_cross_cell(pointer: &CrossCellPointer, resolution: &CrossCellResolution) -> String {
    match resolution {
        CrossCellResolution::Projection { home_cell, zookie } => {
            format!(
                "projection[{:?}]@{} zookie={} subject={}",
                pointer.artifact_type(),
                home_cell.as_str(),
                zookie.0,
                pointer.subject().artifact_ref().0,
            )
        }
        CrossCellResolution::Tombstone { home_cell } => {
            format!("tombstone@{}", home_cell.as_str())
        }
    }
}

#[test]
fn provider_consumer_authorized_cross_cell_read_through() {
    let mut authority = MultiCellAuthority::new();
    authority.register_cell(home_cell_b());

    let pointer = pointer_to_b();
    let lin = principal("acme", "p:lin");

    let (resolution, audit) = authority.resolve_cross_cell(
        &CellId::from_token("cell-a"),
        &lin,
        &pointer,
        &Permission("view".into()),
        &ArtifactRef("project:web".into()),
    );
    assert!(
        resolution.is_authorized(),
        "Lin is a member in the home cell B"
    );
    assert_eq!(
        audit.home_cell,
        CellId::from_token("cell-b"),
        "resolved in the home cell"
    );
    assert_eq!(
        audit.cross_region_tuple_pulls, 0,
        "0 cross-region tuple pulls (rows 4.3/4.10 + 12.6)"
    );
    assert!(audit.is_pii_free());
    match &resolution {
        CrossCellResolution::Projection { zookie, .. } => {
            assert!(
                !zookie.0.is_empty(),
                "the verdict is zookie-bounded at the home cell"
            );
        }
        _ => panic!("expected a projection"),
    }

    let rendered = render_cross_cell(&pointer, &resolution);
    assert!(rendered.starts_with("projection"), "{rendered}");
    assert!(
        rendered.contains("cell-b"),
        "the render names the home cell that authoritatively resolved"
    );
    assert!(
        rendered.contains("myelin://"),
        "the rendered subject is the opaque pointer, never a person"
    );
}

#[test]
fn provider_consumer_unauthorized_cross_cell_tombstone() {
    let mut authority = MultiCellAuthority::new();
    authority.register_cell(home_cell_b());

    let pointer = pointer_to_b();
    let ada = principal("acme", "p:ada");

    let (resolution, audit) = authority.resolve_cross_cell(
        &CellId::from_token("cell-a"),
        &ada,
        &pointer,
        &Permission("view".into()),
        &ArtifactRef("project:web".into()),
    );
    assert!(
        !resolution.is_authorized(),
        "Ada is not a member in the home cell B"
    );
    assert!(
        matches!(resolution, CrossCellResolution::Tombstone { .. }),
        "unauthorized → tombstone"
    );
    assert_eq!(audit.cross_region_tuple_pulls, 0);
    assert!(audit.is_pii_free());

    let rendered = render_cross_cell(&pointer, &resolution);
    assert_eq!(
        rendered, "tombstone@cell-b",
        "the consumer renders a tombstone, never the artifact"
    );
}

#[test]
fn provider_consumer_coarse_grant_zookie_bounded() {
    let mut authority = MultiCellAuthority::new();
    authority.register_cell(home_cell_b());
    let lin = principal("acme", "p:lin");

    let grant = authority.read_through_coarse_grant(
        &lin,
        &Permission("view".into()),
        &ArtifactRef("project:web".into()),
        &CellId::from_token("cell-b"),
    );
    assert_eq!(grant.decision, Decision::Allow);
    assert!(
        grant.is_bounded_allow(),
        "the coarse grant is zookie-bounded"
    );
    let admitted = grant.is_bounded_allow();
    assert!(
        admitted,
        "the cross-cell access gate admits a bounded Allow"
    );
}
