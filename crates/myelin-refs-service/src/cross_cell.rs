use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_tenancy::{
    ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId, Region, TenantId,
};

use crate::resolve::{Resolution, Tombstone, TombstoneReason};
use myelin_events::ArtifactRef;
use myelin_identity::Principal;

pub const CROSS_CELL_RESOLVES_SIGNAL: &str = "refs.cross_cell_resolves";

pub const CROSS_CELL_RAW_ROWS_SIGNAL: &str = "refs.cross_cell_raw_rows";

pub trait CellLocalBacklinkResolver: Send + Sync {
    fn resolve_backlink_in_cell(
        &self,
        tenant: &TenantId,
        region: &Region,
        pointer: &CrossCellPointer,
        viewer: &Principal,
    ) -> Resolution;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossCellEraseReceipt {
    pub cell: CellId,
    pub subject: OpaqueSubjectId,
    pub erased: bool,
}

#[derive(Clone)]
pub struct CrossCellFanOut {
    home_cell: CellId,
    resolvers: HashMap<CellId, Arc<dyn CellLocalBacklinkResolver>>,
    fanned_out: Arc<AtomicU64>,
    raw_rows_crossed: Arc<AtomicU64>,
}

impl CrossCellFanOut {
    pub fn new(home_cell: CellId) -> CrossCellFanOut {
        CrossCellFanOut {
            home_cell,
            resolvers: HashMap::new(),
            fanned_out: Arc::new(AtomicU64::new(0)),
            raw_rows_crossed: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn register(&mut self, cell: CellId, resolver: Arc<dyn CellLocalBacklinkResolver>) {
        self.resolvers.insert(cell, resolver);
    }

    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    pub fn resolve_backlink(
        &self,
        tenant: &TenantId,
        region: &Region,
        pointer: &CrossCellPointer,
        viewer: &Principal,
    ) -> Resolution {
        self.fanned_out.fetch_add(1, Ordering::SeqCst);
        let home = pointer.home_cell();
        match self.resolvers.get(home) {
            Some(resolver) => resolver.resolve_backlink_in_cell(tenant, region, pointer, viewer),
            None => Resolution::Tombstone(Tombstone {
                root: pointer.subject().artifact_ref().clone(),
                reason: TombstoneReason::RootGone,
            }),
        }
    }

    pub fn rollup(
        &self,
        tenant: &TenantId,
        region: &Region,
        pointers: &[CrossCellPointer],
        viewer: &Principal,
    ) -> Vec<Resolution> {
        pointers
            .iter()
            .map(|p| self.resolve_backlink(tenant, region, p, viewer))
            .filter(Resolution::is_projection)
            .collect()
    }

    pub fn resolve_all(
        &self,
        tenant: &TenantId,
        region: &Region,
        pointers: &[CrossCellPointer],
        viewer: &Principal,
    ) -> Vec<Resolution> {
        pointers
            .iter()
            .map(|p| self.resolve_backlink(tenant, region, p, viewer))
            .collect()
    }

    pub fn fanned_out(&self) -> u64 {
        self.fanned_out.load(Ordering::SeqCst)
    }

    pub fn raw_rows_crossed(&self) -> u64 {
        self.raw_rows_crossed.load(Ordering::SeqCst)
    }
}

impl core::fmt::Debug for CrossCellFanOut {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CrossCellFanOut")
            .field("home_cell", &self.home_cell.as_str())
            .field("fanned_out", &self.fanned_out())
            .field("raw_rows_crossed", &self.raw_rows_crossed())
            .finish()
    }
}

pub fn fanout_carried_fields(
    pointer: &CrossCellPointer,
) -> (&OpaqueSubjectId, &ArtifactType, &CorrelationId, &CellId) {
    (
        pointer.subject(),
        pointer.artifact_type(),
        pointer.correlation_id(),
        pointer.home_cell(),
    )
}

#[must_use]
pub fn migrate_home_cell(
    pointer: &CrossCellPointer,
    from: &CellId,
    to: &CellId,
) -> CrossCellPointer {
    if pointer.home_cell() == from {
        CrossCellPointer::new(
            pointer.subject().clone(),
            pointer.artifact_type().clone(),
            pointer.correlation_id().clone(),
            to.clone(),
        )
    } else {
        pointer.clone()
    }
}

#[must_use]
pub fn cross_cell_erase_receipt(cell: &CellId, subject: &OpaqueSubjectId) -> CrossCellEraseReceipt {
    CrossCellEraseReceipt {
        cell: cell.clone(),
        subject: subject.clone(),
        erased: true,
    }
}

#[must_use]
pub fn cross_cell_backlink_pointer(
    ref_: &ArtifactRef,
    kind: ArtifactType,
    correlation_id: CorrelationId,
    cell: CellId,
) -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ref_.clone()),
        kind,
        correlation_id,
        cell,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::Projection;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use std::sync::Mutex;

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
    struct ForeignCellResolver {
        allowed: Mutex<Vec<(String, String)>>,
        erased: Mutex<Vec<String>>,
        titles: Mutex<HashMap<String, String>>,
        resolved: Mutex<Vec<String>>,
    }

    impl ForeignCellResolver {
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
        fn resolved_subjects(&self) -> Vec<String> {
            self.resolved.lock().unwrap().clone()
        }
    }

    impl CellLocalBacklinkResolver for ForeignCellResolver {
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

    fn issue_in(cell_token: &str, key: &str, cell: CellId) -> CrossCellPointer {
        cross_cell_backlink_pointer(
            &ArtifactRef(format!("myelin://acme/issues/issue/{cell_token}-{key}")),
            ArtifactType::Issue,
            corr(),
            cell,
        )
    }

    #[test]
    fn cross_cell_backlink_resolves_in_home_cell_denied_gets_tombstone_zero_leak() {
        let b = Arc::new(ForeignCellResolver::default());
        let secret = "TOP SECRET cross-org acquisition";
        let p = issue_in("b", "42", cell_b());
        b.set_title(&p.subject().artifact_ref().0, secret);
        b.allow(&p.subject().artifact_ref().0, "insider");

        let mut fanout = CrossCellFanOut::new(cell_a());
        fanout.register(cell_b(), b.clone());

        let allowed = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("insider"));
        assert!(
            allowed.is_projection(),
            "the permitted viewer sees the cross-cell projection"
        );
        if let Resolution::Projection(proj) = &allowed {
            assert_eq!(
                proj.title, secret,
                "the permitted viewer is entitled to the title"
            );
        }

        let denied = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("intruder"));
        assert!(
            denied.is_tombstone(),
            "the denied cross-cell viewer gets a tombstone"
        );
        assert_eq!(denied.tombstone_reason(), Some(TombstoneReason::Denied));
        let rendered = format!("{denied:?}");
        assert!(
            !rendered.contains("SECRET") && !rendered.contains("acquisition"),
            "0 leak across the cell boundary: the secret must not appear, got `{rendered}`"
        );

        assert_eq!(
            b.resolved_subjects(),
            vec![
                p.subject().artifact_ref().0.clone(),
                p.subject().artifact_ref().0.clone()
            ],
            "both resolves dispatched to cell B (the home cell), not resolved in A"
        );
        assert_eq!(
            fanout.raw_rows_crossed(),
            0,
            "0 raw rows / PII crossed the cell boundary"
        );
        assert_eq!(
            fanout.fanned_out(),
            2,
            "two cross-cell resolves were served"
        );
    }

    #[test]
    fn home_cell_pointer_resolves_locally_over_the_same_seam() {
        let a = Arc::new(ForeignCellResolver::default());
        let p = issue_in("a", "1", cell_a());
        a.set_title(&p.subject().artifact_ref().0, "local issue");
        a.allow(&p.subject().artifact_ref().0, "insider");

        let mut fanout = CrossCellFanOut::new(cell_a());
        fanout.register(cell_a(), a.clone());

        let r = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("insider"));
        assert!(r.is_projection(), "the home-cell pointer resolves locally");
        assert_eq!(
            a.resolved_subjects().len(),
            1,
            "resolved over the home-cell's own seam"
        );
        assert_eq!(fanout.raw_rows_crossed(), 0);
    }

    #[test]
    fn unknown_home_cell_degrades_to_tombstone_never_reaches_in() {
        let p = issue_in("c", "9", cell_c());
        let fanout = CrossCellFanOut::new(cell_a());
        let r = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("anyone"));
        assert!(
            r.is_tombstone(),
            "an unknown home cell degrades to a tombstone"
        );
        assert_eq!(r.tombstone_reason(), Some(TombstoneReason::RootGone));
        assert_eq!(
            fanout.raw_rows_crossed(),
            0,
            "no raw row crossed for an unseen cell"
        );
    }

    #[test]
    fn rollup_folds_only_permitted_projections_across_member_cells() {
        let b = Arc::new(ForeignCellResolver::default());
        let c = Arc::new(ForeignCellResolver::default());

        let p_b_ok = issue_in("b", "ok", cell_b());
        let p_b_denied = issue_in("b", "secret", cell_b());
        let p_c_ok = issue_in("c", "ok", cell_c());

        b.set_title(&p_b_ok.subject().artifact_ref().0, "B visible");
        b.allow(&p_b_ok.subject().artifact_ref().0, "viewer1");
        b.set_title(&p_b_denied.subject().artifact_ref().0, "B SECRET");
        c.set_title(&p_c_ok.subject().artifact_ref().0, "C visible");
        c.allow(&p_c_ok.subject().artifact_ref().0, "viewer1");

        let mut fanout = CrossCellFanOut::new(cell_a());
        fanout.register(cell_b(), b);
        fanout.register(cell_c(), c);

        let set = vec![p_b_ok.clone(), p_b_denied, p_c_ok.clone()];
        let rollup = fanout.rollup(&tenant(), &region(), &set, &viewer("viewer1"));
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
            "only the permitted cross-cell backlinks fold in (in input order); the denied is excluded"
        );
        assert_eq!(fanout.fanned_out(), 3);
        assert_eq!(fanout.raw_rows_crossed(), 0);
    }

    #[test]
    fn resolve_all_tombstones_the_denied_before_rollup_excludes_it() {
        let b = Arc::new(ForeignCellResolver::default());
        let p_ok = issue_in("b", "ok", cell_b());
        let p_denied = issue_in("b", "secret", cell_b());
        b.set_title(&p_ok.subject().artifact_ref().0, "ok");
        b.allow(&p_ok.subject().artifact_ref().0, "v");
        b.set_title(&p_denied.subject().artifact_ref().0, "SECRET");

        let mut fanout = CrossCellFanOut::new(cell_a());
        fanout.register(cell_b(), b);

        let all = fanout.resolve_all(&tenant(), &region(), &[p_ok, p_denied], &viewer("v"));
        assert_eq!(all.len(), 2);
        assert!(all[0].is_projection());
        assert!(
            all[1].is_tombstone(),
            "the denied backlink is a tombstone, not absent"
        );
        assert_eq!(all[1].tombstone_reason(), Some(TombstoneReason::Denied));
    }

    #[test]
    fn cell_to_cell_migration_re_homes_the_pointer_zero_loss() {
        let p = issue_in("b", "42", cell_b());
        let secret = "migrated issue";

        let b = Arc::new(ForeignCellResolver::default());
        let c = Arc::new(ForeignCellResolver::default());
        b.set_title(&p.subject().artifact_ref().0, secret);
        b.allow(&p.subject().artifact_ref().0, "owner");
        c.set_title(&p.subject().artifact_ref().0, secret);
        c.allow(&p.subject().artifact_ref().0, "owner");

        let mut fanout = CrossCellFanOut::new(cell_a());
        fanout.register(cell_b(), b.clone());
        fanout.register(cell_c(), c.clone());

        let before = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("owner"));
        assert!(before.is_projection());
        assert_eq!(
            b.resolved_subjects().len(),
            1,
            "pre-migration resolve landed in B"
        );
        assert_eq!(c.resolved_subjects().len(), 0);

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
        assert_eq!(
            c.resolved_subjects().len(),
            1,
            "post-migration resolve landed in C"
        );
        if let (Resolution::Projection(pb), Resolution::Projection(pc)) = (&before, &after) {
            assert_eq!(
                pb.title, pc.title,
                "the SAME projection - 0 loss in the migration"
            );
        }
    }

    #[test]
    fn migration_leaves_non_migrating_pointers_untouched() {
        let p_a = issue_in("a", "1", cell_a());
        let migrated = migrate_home_cell(&p_a, &cell_b(), &cell_c());
        assert_eq!(
            migrated.home_cell(),
            &cell_a(),
            "a non-migrating pointer is untouched"
        );
        assert_eq!(migrated, p_a, "the pointer is unchanged byte-for-byte");
    }

    #[test]
    fn cross_cell_erase_yields_receipt_set_and_subject_unresolvable_in_every_cell() {
        let b = Arc::new(ForeignCellResolver::default());
        let c = Arc::new(ForeignCellResolver::default());
        let p_b = issue_in("b", "victim", cell_b());
        let p_c = issue_in("c", "victim", cell_c());
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
            "unresolvable in B"
        );
        assert_eq!(
            r_c.tombstone_reason(),
            Some(TombstoneReason::Erased),
            "unresolvable in C"
        );
        assert_eq!(
            fanout.raw_rows_crossed(),
            0,
            "no PII crossed even on the erased path"
        );
    }

    #[test]
    fn fanout_debug_is_pii_free_and_carries_the_counters() {
        let b = Arc::new(ForeignCellResolver::default());
        let p = issue_in("b", "42", cell_b());
        b.set_title(&p.subject().artifact_ref().0, "t");
        b.allow(&p.subject().artifact_ref().0, "v");
        let mut fanout = CrossCellFanOut::new(cell_a());
        fanout.register(cell_b(), b);
        let _ = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("v"));
        let rendered = format!("{fanout:?}");
        assert!(
            rendered.contains("CrossCellFanOut"),
            "the Debug names the type"
        );
        assert!(
            rendered.contains("cell-fr-par-1"),
            "the Debug carries the home cell id"
        );
        assert!(
            rendered.contains("fanned_out"),
            "the Debug carries the resolve counter"
        );
        assert!(
            rendered.contains("raw_rows_crossed"),
            "the Debug carries the CP-D8 zero counter"
        );
        assert!(
            !rendered.contains("issues/issue"),
            "the Debug never leaks a pointer subject, got `{rendered}`"
        );
    }

    #[test]
    fn fanout_carries_exactly_the_four_frozen_frame_fields() {
        let p = issue_in("b", "42", cell_b());
        let (subject, ty, corr_id, home) = fanout_carried_fields(&p);
        assert_eq!(subject.artifact_ref().0, "myelin://acme/issues/issue/b-42");
        assert_eq!(ty, &ArtifactType::Issue);
        assert_eq!(corr_id, &corr());
        assert_eq!(home, &cell_b());
    }
}
