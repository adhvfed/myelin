use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Decision, ObjectId, Permission, Principal, PrincipalId, PrincipalKind, PseudonymHandle,
    RelName, RelationTuple, TupleDelta,
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

fn scope_of(p: &Principal) -> TenantScope {
    TenantScope::from_verified_token(p, p.region.clone())
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn at(t: &str) -> Timestamp {
    Timestamp(t.into())
}

fn seeded_engine(tenant: &str, members: &[&str]) -> StoreBackedCheck {
    let admin = principal(tenant, "p-admin");
    let scope = scope_of(&admin);
    let store = TupleStore::new(OutboxStore::new());
    let mut deltas = vec![add("project:web", "parent_team", "team:eng#view")];
    for m in members {
        deltas.push(add("team:eng", "member", m));
    }
    store
        .write_tuples(
            &scope,
            &admin,
            &deltas,
            None,
            None,
            at("2026-06-24T00:00:00Z"),
        )
        .expect("seed the tenant grant in this cell");
    StoreBackedCheck::new(store)
}

fn pointer_to(home_cell: &str) -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef(format!(
            "myelin://01JTENANT/projects/project/web@{home_cell}"
        ))),
        ArtifactType::Page,
        CorrelationId("01JCORR".into()),
        CellId::from_token(home_cell),
    )
}

fn two_cell_registry() -> MultiCellAuthority {
    let mut authority = MultiCellAuthority::new();
    authority.register_cell(CellPartition::new(
        CellId::from_token("cell-a"),
        Region(REGION.into()),
        seeded_engine("acme", &["p:ada"]),
    ));
    authority.register_cell(CellPartition::new(
        CellId::from_token("cell-b"),
        Region(REGION.into()),
        seeded_engine("acme", &["p:lin"]),
    ));
    authority
}

#[test]
fn principal_spanning_cells_is_resolved_in_the_objects_cell() {
    let authority = two_cell_registry();
    let ada = principal("acme", "p:ada");
    let object = ArtifactRef("project:web".into());

    let pointer_b = pointer_to("cell-b");
    let (res_b, audit_b) = authority.resolve_cross_cell(
        &CellId::from_token("cell-a"),
        &ada,
        &pointer_b,
        &Permission("view".into()),
        &object,
    );
    assert!(
        !res_b.is_authorized(),
        "Ada is granted in cell A but the object is homed in cell B - resolution is cell-local in B, \
         where Ada is not a member → tombstone, NOT an A-grant leak"
    );
    assert!(matches!(res_b, CrossCellResolution::Tombstone { .. }));
    assert_eq!(
        audit_b.cross_region_tuple_pulls, 0,
        "a cross-cell resolution NEVER pulls tuples cross-region (§OQ-I / ADR-11)"
    );
    assert!(audit_b.cell_local, "resolution is always cell-local");
    assert!(audit_b.is_pii_free());

    let pointer_a = pointer_to("cell-a");
    let (res_a, audit_a) = authority.resolve_cross_cell(
        &CellId::from_token("cell-b"),
        &ada,
        &pointer_a,
        &Permission("view".into()),
        &object,
    );
    assert!(
        res_a.is_authorized(),
        "the object is homed in cell A where Ada IS a member → resolution in A authorizes"
    );
    assert_eq!(audit_a.cross_region_tuple_pulls, 0);
    assert!(audit_a.is_pii_free());
}

#[test]
fn cross_cell_coarse_grant_is_read_through_zookie_bounded() {
    let authority = two_cell_registry();
    let lin = principal("acme", "p:lin");
    let object = ArtifactRef("project:web".into());

    let grant = authority.read_through_coarse_grant(
        &lin,
        &Permission("view".into()),
        &object,
        &CellId::from_token("cell-b"),
    );
    assert_eq!(grant.decision, Decision::Allow, "Lin is a member in cell B");
    assert!(
        !grant.zookie.0.is_empty(),
        "the read-through is stamped at the home cell's OWN current snapshot - zookie-bounded \
         (rows 4.3/4.10)"
    );
    assert_eq!(
        grant.home_cell,
        CellId::from_token("cell-b"),
        "the grant names the home cell that resolved it"
    );
    assert!(
        grant.is_bounded_allow(),
        "a cross-cell allow grant carries the bounding home-cell zookie"
    );

    let ada = principal("acme", "p:ada");
    let deny = authority.read_through_coarse_grant(
        &ada,
        &Permission("view".into()),
        &object,
        &CellId::from_token("cell-b"),
    );
    assert_eq!(deny.decision, Decision::Deny);
}

#[test]
fn cell_to_cell_migration_preserves_authority() {
    let mut authority = MultiCellAuthority::new();
    authority.register_cell(CellPartition::new(
        CellId::from_token("cell-a"),
        Region(REGION.into()),
        seeded_engine("acme", &["p:ada"]),
    ));
    authority.register_cell(CellPartition::new(
        CellId::from_token("cell-c"),
        Region(REGION.into()),
        seeded_engine("acme", &["p:ada"]),
    ));

    let ada = principal("acme", "p:ada");
    let grants = vec![(
        ada.clone(),
        Permission("view".into()),
        ArtifactRef("project:web".into()),
    )];
    let receipt = authority.migrate_cell(
        &TenantId("acme".into()),
        &CellId::from_token("cell-a"),
        &CellId::from_token("cell-c"),
        &grants,
    );
    assert_eq!(receipt.authority_before, 1);
    assert_eq!(receipt.authority_after, 1);
    assert_eq!(
        receipt.authority_lost, 0,
        "CP-D7: a cell→cell migration loses 0 authority"
    );
    assert!(receipt.is_green());
    assert_eq!(receipt.region, Region(REGION.into()), "lands in-region");
}

#[test]
fn migration_that_drops_grants_records_authority_lost_red() {
    let mut authority = MultiCellAuthority::new();
    authority.register_cell(CellPartition::new(
        CellId::from_token("cell-a"),
        Region(REGION.into()),
        seeded_engine("acme", &["p:ada"]),
    ));
    authority.register_cell(CellPartition::new(
        CellId::from_token("cell-c"),
        Region(REGION.into()),
        seeded_engine("acme", &[]),
    ));
    let ada = principal("acme", "p:ada");
    let grants = vec![(
        ada,
        Permission("view".into()),
        ArtifactRef("project:web".into()),
    )];
    let receipt = authority.migrate_cell(
        &TenantId("acme".into()),
        &CellId::from_token("cell-a"),
        &CellId::from_token("cell-c"),
        &grants,
    );
    assert_eq!(
        receipt.authority_lost, 1,
        "a dropped grant IS lost authority"
    );
    assert!(
        !receipt.is_green(),
        "a broken migration is RED, never softened"
    );
}

#[test]
fn cross_region_migration_is_rejected_red() {
    let mut authority = MultiCellAuthority::new();
    authority.register_cell(CellPartition::new(
        CellId::from_token("cell-a"),
        Region("fr-par".into()),
        seeded_engine("acme", &["p:ada"]),
    ));
    authority.register_cell(CellPartition::new(
        CellId::from_token("cell-x"),
        Region("nl-ams".into()),
        seeded_engine("acme", &["p:ada"]),
    ));
    let ada = principal("acme", "p:ada");
    let grants = vec![(
        ada,
        Permission("view".into()),
        ArtifactRef("project:web".into()),
    )];
    let receipt = authority.migrate_cell(
        &TenantId("acme".into()),
        &CellId::from_token("cell-a"),
        &CellId::from_token("cell-x"),
        &grants,
    );
    assert!(
        !receipt.is_green(),
        "a cross-region migration violates the single-region invariant → RED"
    );
}

#[test]
fn ga_d8_multi_cell_erasure_per_cell_receipt_set() {
    let mut authority = MultiCellAuthority::new();
    let cells = ["cell-a", "cell-b", "cell-c"];
    for c in cells {
        authority.register_cell(CellPartition::new(
            CellId::from_token(c),
            Region(REGION.into()),
            seeded_engine("acme", &[]),
        ));
    }
    let subject = PrincipalId("p:ada".into());
    for c in cells {
        let part = authority.cell(&CellId::from_token(c)).unwrap();
        let scope = scope_of(&principal("acme", "p:ada"));
        part.engine()
            .pseudonyms()
            .put_mapping(
                &scope,
                &subject,
                PseudonymHandle::new("anon-ada", "acme").unwrap(),
            )
            .unwrap();
        assert!(
            part.engine()
                .pseudonyms()
                .resolve_subject(&scope, &subject)
                .is_some(),
            "the subject is mapped in cell {c} before the DSR"
        );
    }

    let home = CellId::from_token("cell-a");
    let members = vec![CellId::from_token("cell-b"), CellId::from_token("cell-c")];
    let set = authority
        .dsr_erase_across_cells(
            &subject,
            &TenantId("acme".into()),
            &home,
            &members,
            at("2026-06-24T12:00:00Z"),
        )
        .expect("all registered cells can reach their key registries");

    assert_eq!(
        set.member_cells.len(),
        3,
        "fan-out covers {{home}} ∪ members"
    );
    assert_eq!(set.per_cell.len(), 3, "one receipt per cell");
    assert_eq!(set.cells_missed(), 0, "GA-D8: 0 cells missed");
    assert!(set.is_complete(), "the per-cell receipt set is complete");
    for (cell_id, receipt) in &set.per_cell {
        assert_eq!(receipt.erased_at.0, "2026-06-24T12:00:00Z", "dated receipt");
        assert!(
            receipt.row_shredded,
            "the pseudonym map was shredded in {}",
            cell_id.as_str()
        );
        let part = authority.cell(cell_id).unwrap();
        let scope = scope_of(&principal("acme", "p:ada"));
        assert!(
            part.engine()
                .pseudonyms()
                .resolve_subject(&scope, &subject)
                .is_none(),
            "the subject is erased in {} after the DSR",
            cell_id.as_str()
        );
    }
    assert!(set.summary().contains("GREEN"), "{}", set.summary());
    eprintln!("{}", set.summary());
}

#[test]
fn ga_d8_unregistered_member_cell_is_recorded_missed() {
    let mut authority = MultiCellAuthority::new();
    authority.register_cell(CellPartition::new(
        CellId::from_token("cell-a"),
        Region(REGION.into()),
        seeded_engine("acme", &[]),
    ));
    let subject = PrincipalId("p:ada".into());
    let set = authority
        .dsr_erase_across_cells(
            &subject,
            &TenantId("acme".into()),
            &CellId::from_token("cell-a"),
            &[CellId::from_token("cell-ghost")],
            at("2026-06-24T12:00:00Z"),
        )
        .expect("the registered cell can reach its key registry");
    assert_eq!(
        set.cells_missed(),
        1,
        "the unregistered cell is recorded missed"
    );
    assert!(
        !set.is_complete(),
        "a missed cell makes the set incomplete (RED)"
    );
}

#[test]
fn cp_d7_cell_to_cell_migration_zero_authority_loss() {
    let mut authority = MultiCellAuthority::new();
    authority.register_cell(CellPartition::new(
        CellId::from_token("cell-a"),
        Region(REGION.into()),
        seeded_engine("acme", &["p:ada", "p:lin"]),
    ));
    authority.register_cell(CellPartition::new(
        CellId::from_token("cell-c"),
        Region(REGION.into()),
        seeded_engine("acme", &["p:ada", "p:lin"]),
    ));
    let grants = vec![
        (
            principal("acme", "p:ada"),
            Permission("view".into()),
            ArtifactRef("project:web".into()),
        ),
        (
            principal("acme", "p:lin"),
            Permission("view".into()),
            ArtifactRef("project:web".into()),
        ),
    ];
    let receipt = authority.migrate_cell(
        &TenantId("acme".into()),
        &CellId::from_token("cell-a"),
        &CellId::from_token("cell-c"),
        &grants,
    );
    assert_eq!(receipt.authority_before, 2);
    assert_eq!(receipt.authority_after, 2);
    assert_eq!(receipt.authority_lost, 0, "CP-D7: 0 authority lost");
    assert!(receipt.is_green());
    assert_eq!(receipt.region, Region(REGION.into()), "lands in-region");
}

#[test]
fn cp_d8_cross_cell_ref_pii_free_bridge() {
    let authority = two_cell_registry();
    let object = ArtifactRef("project:web".into());
    let pointer_b = pointer_to("cell-b");

    let lin = principal("acme", "p:lin");
    let (res_ok, audit_ok) = authority.resolve_cross_cell(
        &CellId::from_token("cell-a"),
        &lin,
        &pointer_b,
        &Permission("view".into()),
        &object,
    );
    match res_ok {
        CrossCellResolution::Projection { home_cell, zookie } => {
            assert_eq!(
                home_cell,
                CellId::from_token("cell-b"),
                "resolved in the home cell"
            );
            assert!(
                !zookie.0.is_empty(),
                "the verdict is zookie-bounded at the home cell's own snapshot"
            );
        }
        CrossCellResolution::Tombstone { .. } => panic!("Lin is authorized in cell B"),
    }
    assert_eq!(
        audit_ok.cross_region_tuple_pulls, 0,
        "CP-D8: 0 cross-region tuple pulls - only the verdict crosses the bridge"
    );
    assert!(audit_ok.is_pii_free(), "the bridge is PII-free");

    let ada = principal("acme", "p:ada");
    let (res_deny, audit_deny) = authority.resolve_cross_cell(
        &CellId::from_token("cell-a"),
        &ada,
        &pointer_b,
        &Permission("view".into()),
        &object,
    );
    assert!(
        matches!(res_deny, CrossCellResolution::Tombstone { .. }),
        "unauthorized → tombstone"
    );
    assert_eq!(audit_deny.cross_region_tuple_pulls, 0);
    assert!(audit_deny.is_pii_free());
}

#[test]
fn cp_d8_unknown_home_cell_fails_closed_tombstone() {
    let authority = two_cell_registry();
    let lin = principal("acme", "p:lin");
    let pointer_ghost = pointer_to("cell-ghost");
    let (res, audit) = authority.resolve_cross_cell(
        &CellId::from_token("cell-a"),
        &lin,
        &pointer_ghost,
        &Permission("view".into()),
        &ArtifactRef("project:web".into()),
    );
    assert!(
        matches!(res, CrossCellResolution::Tombstone { .. }),
        "an unknown home cell fails CLOSED, never an open"
    );
    assert_eq!(audit.cross_region_tuple_pulls, 0);
}

#[test]
fn mutation_floor_no_cross_region_pull_is_invariant() {
    let authority = two_cell_registry();
    let object = ArtifactRef("project:web".into());
    let viewers = [principal("acme", "p:ada"), principal("acme", "p:lin")];
    let homes = ["cell-a", "cell-b", "cell-ghost"];
    for v in &viewers {
        for h in homes {
            let (_res, audit) = authority.resolve_cross_cell(
                &CellId::from_token("cell-a"),
                v,
                &pointer_to(h),
                &Permission("view".into()),
                &object,
            );
            assert_eq!(
                audit.cross_region_tuple_pulls, 0,
                "no resolution EVER pulls a tuple cross-region ({})",
                h
            );
            assert!(audit.cell_local, "every resolution is cell-local ({})", h);
            assert!(audit.is_pii_free());
        }
    }
}
