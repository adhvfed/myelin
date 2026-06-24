//! # P-ID-35 — Multi-cell principal authority: the cross-cell read-through over the PII-free bridge
//!
//! The build-layer realisation of the **multi-cell** model (architecture `identity-and-access.md`
//! §13/§15; recon §OQ-I; contract-index rows 4.3/4.10 + 12.6). This file proves the deliverable's
//! LOGIC on the harness:
//!
//! - **Unit:** a principal spanning cells is resolved IN THE OBJECT'S CELL (no cross-region tuple
//!   pull); a cross-cell coarse grant is read-through zookie-bounded; a cell→cell migration
//!   preserves authority (the prompt's three required unit cases).
//! - **GA-D8 (FLOOR drill):** multi-cell erasure produces a per-cell receipt set — the DSR fan-out
//!   iterates `{home_cell} ∪ member_cells`, each cell's pseudonym-map shred a dated receipt; 0 cells
//!   missed.
//! - **CP-D7 (FLOOR drill):** a cell→cell migration → **0 loss of authority** (lands in-region).
//! - **CP-D8 (FLOOR drill):** a cross-cell ref → the PII-free bridge carries only the verdict (the
//!   already-rendered, already-permission-filtered projection or a tombstone); **0 cross-region tuple
//!   pulls**; unauthorized → tombstone.
//! - **Mutation floor:** the cell-local-resolution + the no-cross-region-pull invariant are core — a
//!   resolution that is NOT cell-local (or that pulls a tuple cross-region) MUST be caught (the
//!   `cross_region_tuple_pulls == 0` / `cell_local` assertions are the mutation-sensitive core).
//!
//! Floor named CLOSED: this CLOSES the single-home-cell floor (P-ID-10/11). The remaining floor above
//! is the real multi-region fleet wall-clock (the world-scale load drill on real hardware) — this
//! file proves the LOGIC on the harness; the fleet number is the run doctrine's named load floor.

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

// ── fixtures ──────────────────────────────────────────────────────────────────────────────────

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

/// Build a `StoreBackedCheck` seeded so `member` of `tenant`'s `team:eng` inherits `view` on
/// `project:web` (the core org→team→project hierarchy the engine resolves cell-locally). `members`
/// are the principal ids granted membership in this cell's partition.
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

/// A cross-cell pointer to an artifact homed in `home_cell` (the frozen 12.6 frame — PII-free).
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

/// A two-cell registry: cell A (home of `acme`, member `p:ada`) and cell B (home of `acme`, member
/// `p:lin`). Both in the SAME region (the HARD multi-cell single-region invariant). The "principal
/// spanning cells" is a viewer who is a member in one cell but not the other — resolution is always
/// in the OBJECT's cell.
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

// ── UNIT (the prompt's three required cases) ────────────────────────────────────────────────────

/// **A principal spanning cells is resolved IN THE OBJECT'S CELL (no cross-region tuple pull).** Ada
/// is a member in cell A but NOT cell B. A pointer to an artifact homed in cell B resolves IN CELL B
/// against B's tuples — so Ada (not a member in B) is DENIED → a tombstone — even though she is
/// granted in A. Resolution is always cell-local; A's grant is irrelevant to a B-homed object.
#[test]
fn principal_spanning_cells_is_resolved_in_the_objects_cell() {
    let authority = two_cell_registry();
    let ada = principal("acme", "p:ada");
    let object = ArtifactRef("project:web".into());

    // Pointer homed in cell B → resolve in B. Ada is NOT a member in B → tombstone.
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
        "Ada is granted in cell A but the object is homed in cell B — resolution is cell-local in B, \
         where Ada is not a member → tombstone, NOT an A-grant leak"
    );
    assert!(matches!(res_b, CrossCellResolution::Tombstone { .. }));
    // The structural no-cross-region-pull invariant: 0 cross-region tuple pulls, cell-local.
    assert_eq!(
        audit_b.cross_region_tuple_pulls, 0,
        "a cross-cell resolution NEVER pulls tuples cross-region (§OQ-I / ADR-11)"
    );
    assert!(audit_b.cell_local, "resolution is always cell-local");
    assert!(audit_b.is_pii_free());

    // Pointer homed in cell A → resolve in A. Ada IS a member in A → authorized (projection-ready).
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

/// **A cross-cell coarse grant is read-through zookie-bounded (rows 4.3/4.10).** Lin is a member in
/// cell B; a coarse-grant read-through for Lin on a B-homed object resolves `Allow`, stamped at the
/// home cell's zookie — so the read-through is consistency-bounded exactly like a cell-local read.
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
        "the read-through is stamped at the home cell's OWN current snapshot — zookie-bounded \
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

    // A non-member (Ada) read-through against cell B → Deny (cell-local in B), still zookie-bounded.
    let ada = principal("acme", "p:ada");
    let deny = authority.read_through_coarse_grant(
        &ada,
        &Permission("view".into()),
        &object,
        &CellId::from_token("cell-b"),
    );
    assert_eq!(deny.decision, Decision::Deny);
}

/// **A cell→cell migration preserves authority (CP-D7).** Migrate `acme` from cell A to a fresh cell
/// C (same region) that holds the relocated grants. Every grant that resolved in A still resolves in
/// C → 0 authority lost.
#[test]
fn cell_to_cell_migration_preserves_authority() {
    let mut authority = MultiCellAuthority::new();
    // Source cell A: Ada is a member.
    authority.register_cell(CellPartition::new(
        CellId::from_token("cell-a"),
        Region(REGION.into()),
        seeded_engine("acme", &["p:ada"]),
    ));
    // Destination cell C: the relocated grants (Ada is a member here too — the migration moved them).
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

/// A migration whose source did NOT relocate the grants → authority IS lost (the RED case, recorded
/// honestly — the receipt is NOT softened to green). This is the negative of CP-D7.
#[test]
fn migration_that_drops_grants_records_authority_lost_red() {
    let mut authority = MultiCellAuthority::new();
    authority.register_cell(CellPartition::new(
        CellId::from_token("cell-a"),
        Region(REGION.into()),
        seeded_engine("acme", &["p:ada"]),
    ));
    // Destination did NOT receive Ada's grant (a broken migration).
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

/// A cross-region "migration" is rejected (RED) — the HARD multi-cell single-region invariant
/// (tenancy §5.1). multi-cell is single-region by construction.
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

// ── GA-D8 / CP-D7 / CP-D8 (the FLOOR drills) ────────────────────────────────────────────────────

/// **GA-D8 (FLOOR) — multi-cell erasure produces a per-cell receipt set.** The DSR fan-out iterates
/// `{home_cell} ∪ member_cells`; each cell's pseudonym-map shred (P-ID-20) produces a dated receipt;
/// 0 cells missed. The subject is mapped in EVERY cell (a principal whose pseudonym map exists per
/// cell); the erase shreds it in each.
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
    // Seed the subject's pseudonym map IN each cell (the per-cell real-identity link the shred
    // destroys).
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
    let set = authority.dsr_erase_across_cells(
        &subject,
        &TenantId("acme".into()),
        &home,
        &members,
        at("2026-06-24T12:00:00Z"),
    );

    // The fan-out covered {home} ∪ members = 3 cells, 0 missed.
    assert_eq!(
        set.member_cells.len(),
        3,
        "fan-out covers {{home}} ∪ members"
    );
    assert_eq!(set.per_cell.len(), 3, "one receipt per cell");
    assert_eq!(set.cells_missed(), 0, "GA-D8: 0 cells missed");
    assert!(set.is_complete(), "the per-cell receipt set is complete");
    // Each receipt is dated + shredded the pseudonym map in its cell.
    for (cell_id, receipt) in &set.per_cell {
        assert_eq!(receipt.erased_at.0, "2026-06-24T12:00:00Z", "dated receipt");
        assert!(
            receipt.row_shredded,
            "the pseudonym map was shredded in {}",
            cell_id.as_str()
        );
        // Post-erase: the subject's real identity no longer resolves in this cell.
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
    // The dated green artifact (the GA-D8 signal).
    assert!(set.summary().contains("GREEN"), "{}", set.summary());
    eprintln!("{}", set.summary());
}

/// A DSR whose fan-out names a member cell NOT registered → that cell is MISSED (recorded honestly,
/// never silently dropped). The negative of GA-D8's "0 cells missed".
#[test]
fn ga_d8_unregistered_member_cell_is_recorded_missed() {
    let mut authority = MultiCellAuthority::new();
    authority.register_cell(CellPartition::new(
        CellId::from_token("cell-a"),
        Region(REGION.into()),
        seeded_engine("acme", &[]),
    ));
    let subject = PrincipalId("p:ada".into());
    let set = authority.dsr_erase_across_cells(
        &subject,
        &TenantId("acme".into()),
        &CellId::from_token("cell-a"),
        &[CellId::from_token("cell-ghost")], // not registered
        at("2026-06-24T12:00:00Z"),
    );
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

/// **CP-D7 (FLOOR) — cell→cell migration → 0 loss of authority, lands in-region.** Two members'
/// grants relocate from cell A to cell C (same region); both still resolve in C → 0 lost.
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

/// **CP-D8 (FLOOR) — cross-cell ref: the PII-free bridge carries only the verdict; 0 cross-region
/// PII; unauthorized → tombstone.** A viewer in cell A resolves a pointer to a cell-B-homed
/// artifact. The authorized viewer gets a projection-ready verdict stamped at B's zookie (only the
/// verdict crosses — no raw tuples, no PII); the unauthorized viewer gets a tombstone. In both cases
/// `cross_region_tuple_pulls == 0`.
#[test]
fn cp_d8_cross_cell_ref_pii_free_bridge() {
    let authority = two_cell_registry();
    let object = ArtifactRef("project:web".into());
    let pointer_b = pointer_to("cell-b");

    // Authorized: Lin IS a member in cell B → projection-ready, stamped at B's own zookie.
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
        "CP-D8: 0 cross-region tuple pulls — only the verdict crosses the bridge"
    );
    assert!(audit_ok.is_pii_free(), "the bridge is PII-free");

    // Unauthorized: Ada is NOT a member in cell B → tombstone, no leak.
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

/// A pointer to an UNREGISTERED home cell fails CLOSED (a tombstone) — never an open over the bridge.
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

// ── MUTATION FLOOR (the cell-local + no-cross-region-pull core) ──────────────────────────────────

/// **Mutation floor: the no-cross-region-pull invariant is core.** EVERY cross-cell resolution —
/// authorized OR denied, known OR unknown home cell — reports `cross_region_tuple_pulls == 0` and
/// `cell_local == true`. A mutation that made the resolution pull a tuple cross-region (or resolve
/// non-cell-locally) would flip one of these and MUST be caught. This is the mutation-sensitive
/// core (the prompt's mutation floor: a tuple pulled cross-region MUST be caught).
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
