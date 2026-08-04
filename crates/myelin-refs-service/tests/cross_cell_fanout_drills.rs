use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{
    cross_cell_backlink_pointer, cross_cell_erase_receipt, fanout_carried_fields,
    migrate_home_cell, CellLocalBacklinkResolver, CrossCellFanOut, Projection, Resolution,
    Tombstone, TombstoneReason,
};
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, Region, TenantId,
};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn cell_a() -> CellId {
    CellId::from_token("cell-fr-par-1")
}
fn cell_b() -> CellId {
    CellId::from_token("cell-fr-par-2")
}
fn cell_c() -> CellId {
    CellId::from_token("cell-de-fra-1")
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn corr() -> CorrelationId {
    CorrelationId("01J0CORR".into())
}

#[derive(Default)]
struct CellResolver {
    allowed: Mutex<Vec<(String, String)>>,
    erased: Mutex<Vec<String>>,
    titles: Mutex<HashMap<String, String>>,
    resolved: Mutex<Vec<String>>,
}

impl CellResolver {
    fn allow(&self, subject_urn: &str, viewer_id: &str) {
        self.allowed
            .lock()
            .unwrap()
            .push((subject_urn.into(), viewer_id.into()));
    }
    fn set_title(&self, subject_urn: &str, title: &str) {
        self.titles
            .lock()
            .unwrap()
            .insert(subject_urn.into(), title.into());
    }
    fn erase(&self, subject_urn: &str) {
        self.erased.lock().unwrap().push(subject_urn.into());
    }
    fn resolve_count(&self) -> usize {
        self.resolved.lock().unwrap().len()
    }
}

impl CellLocalBacklinkResolver for CellResolver {
    fn resolve_backlink_in_cell(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        pointer: &CrossCellPointer,
        viewer: &Principal,
    ) -> Resolution {
        let subject_urn = pointer.subject().artifact_ref().0.clone();
        self.resolved.lock().unwrap().push(subject_urn.clone());
        if self
            .erased
            .lock()
            .unwrap()
            .iter()
            .any(|e| e == &subject_urn)
        {
            return Resolution::Tombstone(Tombstone {
                root: pointer.subject().artifact_ref().clone(),
                reason: TombstoneReason::Erased,
            });
        }
        let allowed = self
            .allowed
            .lock()
            .unwrap()
            .iter()
            .any(|(s, v)| s == &subject_urn && v == &viewer.principal_id.0);
        if !allowed {
            return Resolution::Tombstone(Tombstone {
                root: pointer.subject().artifact_ref().clone(),
                reason: TombstoneReason::Denied,
            });
        }
        let title = self
            .titles
            .lock()
            .unwrap()
            .get(&subject_urn)
            .cloned()
            .unwrap_or_else(|| "untitled".into());
        Resolution::Projection(Projection {
            ref_: pointer.subject().artifact_ref().clone(),
            title,
            state: "open".into(),
            icon: "issue".into(),
            render_hint: "issue-card".into(),
            sub_anchor: None,
            flag: None,
        })
    }
}

fn issue_in(key: &str, cell: CellId) -> CrossCellPointer {
    cross_cell_backlink_pointer(
        &ArtifactRef(format!("myelin://acme/issues/issue/{key}")),
        ArtifactType::Issue,
        corr(),
        cell,
    )
}

#[test]
fn cp_d8_cross_cell_ref_pii_free_bridge_only_projection_or_tombstone_crosses() {
    let b = Arc::new(CellResolver::default());
    let secret = "TOP SECRET cross-org acquisition plan";
    let p = issue_in("ENG-42", cell_b());
    b.set_title(&p.subject().artifact_ref().0, secret);
    b.allow(&p.subject().artifact_ref().0, "insider");

    let mut fanout = CrossCellFanOut::new(cell_a());
    fanout.register(cell_b(), b.clone());

    let allowed = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("insider"));
    assert!(
        allowed.is_projection(),
        "the permitted cross-cell viewer is served the projection"
    );

    let denied = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("intruder"));
    assert!(
        denied.is_tombstone(),
        "the denied cross-cell viewer gets a tombstone (the leak invariant holds cross-cell)"
    );
    assert_eq!(denied.tombstone_reason(), Some(TombstoneReason::Denied));
    let rendered = format!("{denied:?}");
    assert!(
        !rendered.contains("SECRET") && !rendered.contains("acquisition"),
        "0 leak across the cell boundary - the secret must not appear, got `{rendered}`"
    );

    assert_eq!(
        fanout.raw_rows_crossed(),
        0,
        "0 raw rows / PII crossed the cell boundary (the CP-D8 zero)"
    );
    assert_eq!(
        fanout.fanned_out(),
        2,
        "two cross-cell resolves were served"
    );
    assert_eq!(
        b.resolve_count(),
        2,
        "both resolves dispatched to cell B (the home cell)"
    );

    let (subject, ty, corr_id, home) = fanout_carried_fields(&p);
    assert_eq!(
        subject.artifact_ref().0,
        "myelin://acme/issues/issue/ENG-42"
    );
    assert_eq!(ty, &ArtifactType::Issue);
    assert_eq!(corr_id, &corr());
    assert_eq!(home, &cell_b());
}

#[test]
fn cp_d8_cross_cell_portfolio_rollup_folds_only_permitted() {
    let b = Arc::new(CellResolver::default());
    let c = Arc::new(CellResolver::default());

    let p_b_ok = issue_in("B-ok", cell_b());
    let p_b_no = issue_in("B-secret", cell_b());
    let p_c_ok = issue_in("C-ok", cell_c());

    b.set_title(&p_b_ok.subject().artifact_ref().0, "B visible");
    b.allow(&p_b_ok.subject().artifact_ref().0, "owner");
    b.set_title(&p_b_no.subject().artifact_ref().0, "B SECRET");
    c.set_title(&p_c_ok.subject().artifact_ref().0, "C visible");
    c.allow(&p_c_ok.subject().artifact_ref().0, "owner");

    let mut fanout = CrossCellFanOut::new(cell_a());
    fanout.register(cell_b(), b);
    fanout.register(cell_c(), c);

    let set = vec![p_b_ok, p_b_no, p_c_ok];
    let rollup = fanout.rollup(&tenant(), &region(), &set, &viewer("owner"));
    let titles: Vec<String> = rollup
        .iter()
        .filter_map(|r| match r {
            Resolution::Projection(p) => Some(p.title.clone()),
            Resolution::Tombstone(_) => None,
        })
        .collect();
    assert_eq!(
        titles,
        vec!["B visible".to_string(), "C visible".to_string()],
        "only the permitted cross-cell backlinks fold in; the denied is excluded (no leaked count)"
    );
    assert_eq!(
        fanout.raw_rows_crossed(),
        0,
        "0 raw rows crossed in the rollup"
    );
    assert_eq!(
        fanout.fanned_out(),
        3,
        "one resolve per member-cell pointer"
    );
}

#[test]
fn cp_d7_cell_to_cell_migration_zero_loss() {
    let p = issue_in("MIG-1", cell_b());
    let title = "migrated cross-cell issue";

    let b = Arc::new(CellResolver::default());
    let c = Arc::new(CellResolver::default());
    b.set_title(&p.subject().artifact_ref().0, title);
    b.allow(&p.subject().artifact_ref().0, "owner");
    c.set_title(&p.subject().artifact_ref().0, title);
    c.allow(&p.subject().artifact_ref().0, "owner");

    let mut fanout = CrossCellFanOut::new(cell_a());
    fanout.register(cell_b(), b.clone());
    fanout.register(cell_c(), c.clone());

    let before = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("owner"));
    assert!(before.is_projection());
    assert_eq!(b.resolve_count(), 1, "pre-migration resolve landed in B");
    assert_eq!(c.resolve_count(), 0);

    let migrated = migrate_home_cell(&p, &cell_b(), &cell_c());
    assert_eq!(migrated.home_cell(), &cell_c(), "the pointer re-homed to C");
    assert_eq!(
        migrated.subject(),
        p.subject(),
        "the subject is preserved (0 loss)"
    );
    assert_eq!(migrated.artifact_type(), p.artifact_type());
    assert_eq!(migrated.correlation_id(), p.correlation_id());

    let after = fanout.resolve_backlink(&tenant(), &region(), &migrated, &viewer("owner"));
    assert!(
        after.is_projection(),
        "the re-homed backlink resolves with 0 loss"
    );
    assert_eq!(c.resolve_count(), 1, "post-migration resolve landed in C");
    if let (Resolution::Projection(pb), Resolution::Projection(pc)) = (&before, &after) {
        assert_eq!(
            pb.title, pc.title,
            "the SAME projection - 0 loss in the migration"
        );
    }
}

#[test]
fn ga_d8_cross_cell_erasure_receipt_set_subject_unresolvable_everywhere() {
    let b = Arc::new(CellResolver::default());
    let c = Arc::new(CellResolver::default());
    let p_b = issue_in("VICTIM", cell_b());
    let p_c = issue_in("VICTIM", cell_c());
    let subject = p_b.subject().clone();

    b.set_title(&p_b.subject().artifact_ref().0, "B ref");
    b.allow(&p_b.subject().artifact_ref().0, "owner");
    c.set_title(&p_c.subject().artifact_ref().0, "C ref");
    c.allow(&p_c.subject().artifact_ref().0, "owner");

    let mut fanout = CrossCellFanOut::new(cell_a());
    fanout.register(cell_b(), b.clone());
    fanout.register(cell_c(), c.clone());

    b.erase(&p_b.subject().artifact_ref().0);
    c.erase(&p_c.subject().artifact_ref().0);
    let receipts = vec![
        cross_cell_erase_receipt(&cell_b(), &subject),
        cross_cell_erase_receipt(&cell_c(), &subject),
    ];
    assert_eq!(
        receipts.len(),
        2,
        "a receipt per member cell that held a reference"
    );
    for r in &receipts {
        assert!(
            r.erased,
            "every member cell ran the erase (0 holders missed)"
        );
        assert_eq!(
            r.subject, subject,
            "the receipt names the erased opaque subject"
        );
    }
    let rendered = format!("{receipts:?}");
    assert!(
        !rendered.contains("ref"),
        "the receipt is PII-free (no title), got `{rendered}`"
    );

    let r_b = fanout.resolve_backlink(&tenant(), &region(), &p_b, &viewer("owner"));
    let r_c = fanout.resolve_backlink(&tenant(), &region(), &p_c, &viewer("owner"));
    assert_eq!(
        r_b.tombstone_reason(),
        Some(TombstoneReason::Erased),
        "the subject is unresolvable in cell B (Erased)"
    );
    assert_eq!(
        r_c.tombstone_reason(),
        Some(TombstoneReason::Erased),
        "the subject is unresolvable in cell C (Erased)"
    );
    assert_eq!(
        fanout.raw_rows_crossed(),
        0,
        "no PII crossed even on the erased path"
    );
}

#[test]
fn counter_case_a_home_cell_leak_would_be_caught() {
    struct LeakyResolver {
        secret: String,
    }
    impl CellLocalBacklinkResolver for LeakyResolver {
        fn resolve_backlink_in_cell(
            &self,
            _t: &TenantId,
            _r: &Region,
            pointer: &CrossCellPointer,
            _viewer: &Principal,
        ) -> Resolution {
            Resolution::Projection(Projection {
                ref_: pointer.subject().artifact_ref().clone(),
                title: self.secret.clone(),
                state: "open".into(),
                icon: "issue".into(),
                render_hint: "issue-card".into(),
                sub_anchor: None,
                flag: None,
            })
        }
    }
    let p = issue_in("LEAK-1", cell_b());
    let mut fanout = CrossCellFanOut::new(cell_a());
    fanout.register(
        cell_b(),
        Arc::new(LeakyResolver {
            secret: "LEAKED SECRET".into(),
        }),
    );
    let leaked = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("intruder"));
    let rendered = format!("{leaked:?}");
    assert!(
        rendered.contains("LEAKED SECRET"),
        "a home-cell leak is OBSERVABLE at the boundary - the drill flips RED if the chokepoint regresses"
    );
}
