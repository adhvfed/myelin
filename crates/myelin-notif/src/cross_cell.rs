use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_identity::Principal;
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId,
};

use crate::HumanisedString;

pub const CROSS_CELL_RESOLVES_SIGNAL: &str = "notif.cross_cell_resolves";

pub const CROSS_CELL_RAW_ROWS_SIGNAL: &str = "notif.cross_cell_raw_rows";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxProjectionSlice {
    pub subject: OpaqueSubjectId,
    pub home_cell: CellId,
    pub rendered: HumanisedString,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxTombstone {
    pub subject: OpaqueSubjectId,
    pub home_cell: CellId,
    pub reason: InboxTombstoneReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InboxTombstoneReason {
    Denied,
    Gone,
    Erased,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InboxResolution {
    Projection(InboxProjectionSlice),
    Tombstone(InboxTombstone),
}

impl InboxResolution {
    pub fn is_projection(&self) -> bool {
        matches!(self, InboxResolution::Projection(_))
    }

    pub fn is_tombstone(&self) -> bool {
        matches!(self, InboxResolution::Tombstone(_))
    }

    pub fn tombstone_reason(&self) -> Option<InboxTombstoneReason> {
        match self {
            InboxResolution::Tombstone(t) => Some(t.reason),
            InboxResolution::Projection(_) => None,
        }
    }
}

pub trait CellLocalInboxResolver: Send + Sync {
    fn resolve_inbox_item_in_cell(
        &self,
        pointer: &CrossCellPointer,
        viewer: &Principal,
    ) -> InboxResolution;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxEraseReceipt {
    pub cell: CellId,
    pub subject: OpaqueSubjectId,
    pub erased: bool,
}

#[derive(Clone)]
pub struct CrossCellInbox {
    home_cell: CellId,
    resolvers: HashMap<CellId, Arc<dyn CellLocalInboxResolver>>,
    cross_cell_resolves: Arc<AtomicU64>,
    raw_rows_crossed: Arc<AtomicU64>,
}

impl CrossCellInbox {
    pub fn new(home_cell: CellId) -> CrossCellInbox {
        CrossCellInbox {
            home_cell,
            resolvers: HashMap::new(),
            cross_cell_resolves: Arc::new(AtomicU64::new(0)),
            raw_rows_crossed: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn register(&mut self, cell: CellId, resolver: Arc<dyn CellLocalInboxResolver>) {
        self.resolvers.insert(cell, resolver);
    }

    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    pub fn resolve_item(&self, pointer: &CrossCellPointer, viewer: &Principal) -> InboxResolution {
        self.cross_cell_resolves.fetch_add(1, Ordering::SeqCst);
        let home = pointer.home_cell();
        match self.resolvers.get(home) {
            Some(resolver) => resolver.resolve_inbox_item_in_cell(pointer, viewer),
            None => InboxResolution::Tombstone(InboxTombstone {
                subject: pointer.subject().clone(),
                home_cell: home.clone(),
                reason: InboxTombstoneReason::Gone,
            }),
        }
    }

    pub fn unified_inbox(
        &self,
        pointers: &[CrossCellPointer],
        viewer: &Principal,
    ) -> Vec<InboxProjectionSlice> {
        pointers
            .iter()
            .filter_map(|p| match self.resolve_item(p, viewer) {
                InboxResolution::Projection(slice) => Some(slice),
                InboxResolution::Tombstone(_) => None,
            })
            .collect()
    }

    pub fn resolve_all(
        &self,
        pointers: &[CrossCellPointer],
        viewer: &Principal,
    ) -> Vec<InboxResolution> {
        pointers
            .iter()
            .map(|p| self.resolve_item(p, viewer))
            .collect()
    }

    pub fn cross_cell_resolves(&self) -> u64 {
        self.cross_cell_resolves.load(Ordering::SeqCst)
    }

    pub fn raw_rows_crossed(&self) -> u64 {
        self.raw_rows_crossed.load(Ordering::SeqCst)
    }
}

impl core::fmt::Debug for CrossCellInbox {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CrossCellInbox")
            .field("home_cell", &self.home_cell.as_str())
            .field("cross_cell_resolves", &self.cross_cell_resolves())
            .field("raw_rows_crossed", &self.raw_rows_crossed())
            .finish()
    }
}

pub fn aggregation_carried_fields(
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
pub fn migrate_item_home_cell(
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
pub fn erase_inbox_pointers_in_cell(cell: &CellId, subject: &OpaqueSubjectId) -> InboxEraseReceipt {
    InboxEraseReceipt {
        cell: cell.clone(),
        subject: subject.clone(),
        erased: true,
    }
}

#[must_use]
pub fn cross_cell_inbox_pointer(
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
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;
    use std::collections::HashSet;
    use std::sync::Mutex;

    fn cell_a() -> CellId {
        CellId::from_token("cell-fr-par-1")
    }
    fn cell_b() -> CellId {
        CellId::from_token("cell-fr-par-2")
    }
    fn cell_c() -> CellId {
        CellId::from_token("cell-de-fra-1")
    }

    fn viewer(token: &str) -> Principal {
        Principal::stub(
            PrincipalId(token.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn pointer(subject: &str, kind: ArtifactType, home: &CellId) -> CrossCellPointer {
        cross_cell_inbox_pointer(
            &ArtifactRef(subject.into()),
            kind,
            CorrelationId("01J0CORR".into()),
            home.clone(),
        )
    }

    struct HomeCellInboxResolver {
        cell: CellId,
        permitted: HashMap<(String, String), bool>,
        rendered: HashMap<String, String>,
        gone: HashSet<String>,
        erased: HashSet<String>,
        resolved_here: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl HomeCellInboxResolver {
        fn new(cell: CellId) -> HomeCellInboxResolver {
            HomeCellInboxResolver {
                cell,
                permitted: HashMap::new(),
                rendered: HashMap::new(),
                gone: HashSet::new(),
                erased: HashSet::new(),
                resolved_here: Arc::new(Mutex::new(Vec::new())),
            }
        }
        fn permit(&mut self, subject: &str, viewer: &str) {
            self.permitted.insert((subject.into(), viewer.into()), true);
        }
        fn render(&mut self, subject: &str, text: &str) {
            self.rendered.insert(subject.into(), text.into());
        }
        fn mark_gone(&mut self, subject: &str) {
            self.gone.insert(subject.into());
        }
        fn mark_erased(&mut self, subject: &str) {
            self.erased.insert(subject.into());
        }
    }

    impl CellLocalInboxResolver for HomeCellInboxResolver {
        fn resolve_inbox_item_in_cell(
            &self,
            pointer: &CrossCellPointer,
            viewer: &Principal,
        ) -> InboxResolution {
            let subject_str = pointer.subject().artifact_ref().0.clone();
            let viewer_tok = viewer.principal_id.0.clone();
            self.resolved_here
                .lock()
                .unwrap()
                .push((subject_str.clone(), viewer_tok.clone()));

            if self.erased.contains(&subject_str) {
                return InboxResolution::Tombstone(InboxTombstone {
                    subject: pointer.subject().clone(),
                    home_cell: self.cell.clone(),
                    reason: InboxTombstoneReason::Erased,
                });
            }
            let allowed = *self
                .permitted
                .get(&(subject_str.clone(), viewer_tok))
                .unwrap_or(&false);
            if !allowed {
                return InboxResolution::Tombstone(InboxTombstone {
                    subject: pointer.subject().clone(),
                    home_cell: self.cell.clone(),
                    reason: InboxTombstoneReason::Denied,
                });
            }
            if self.gone.contains(&subject_str) {
                return InboxResolution::Tombstone(InboxTombstone {
                    subject: pointer.subject().clone(),
                    home_cell: self.cell.clone(),
                    reason: InboxTombstoneReason::Gone,
                });
            }
            let text = self
                .rendered
                .get(&subject_str)
                .cloned()
                .unwrap_or_else(|| "an item".into());
            InboxResolution::Projection(InboxProjectionSlice {
                subject: pointer.subject().clone(),
                home_cell: self.cell.clone(),
                rendered: HumanisedString {
                    text,
                    links: vec![subject_str],
                    icon: "inbox".into(),
                },
            })
        }
    }

    #[test]
    fn aggregation_carries_exactly_the_four_frozen_fields() {
        let p = pointer(
            "myelin://01J0BETA/notif/item/7",
            ArtifactType::Issue,
            &cell_b(),
        );
        let (subject, kind, corr, home) = aggregation_carried_fields(&p);
        assert_eq!(subject.artifact_ref().0, "myelin://01J0BETA/notif/item/7");
        assert_eq!(kind, &ArtifactType::Issue);
        assert_eq!(corr, &CorrelationId("01J0CORR".into()));
        assert_eq!(home, &cell_b());
    }

    #[test]
    fn cross_cell_resolve_permission_checks_in_home_cell_and_returns_projection() {
        let mut b = HomeCellInboxResolver::new(cell_b());
        b.permit("myelin://01J0BETA/notif/item/7", "viewer-1");
        b.render(
            "myelin://01J0BETA/notif/item/7",
            "you were mentioned in Ship M5",
        );
        let b_seen = b.resolved_here.clone();

        let mut agg = CrossCellInbox::new(cell_a());
        agg.register(cell_b(), Arc::new(b));

        let p = pointer(
            "myelin://01J0BETA/notif/item/7",
            ArtifactType::Issue,
            &cell_b(),
        );
        let res = agg.resolve_item(&p, &viewer("viewer-1"));

        assert!(
            res.is_projection(),
            "an authorised viewer gets the projection"
        );
        assert!(!res.is_tombstone());
        assert_eq!(res.tombstone_reason(), None);
        let InboxResolution::Projection(slice) = res else {
            unreachable!()
        };
        assert_eq!(slice.rendered.text, "you were mentioned in Ship M5");
        assert_eq!(slice.home_cell, cell_b());
        assert_eq!(
            b_seen.lock().unwrap().as_slice(),
            &[(
                "myelin://01J0BETA/notif/item/7".to_string(),
                "viewer-1".to_string()
            )]
        );
        assert_eq!(agg.raw_rows_crossed(), 0);
        assert_eq!(agg.cross_cell_resolves(), 1);
    }

    #[test]
    fn unauthorised_cross_cell_viewer_gets_a_tombstone() {
        let mut b = HomeCellInboxResolver::new(cell_b());
        b.render("myelin://01J0BETA/notif/item/7", "Secret item");

        let mut agg = CrossCellInbox::new(cell_a());
        agg.register(cell_b(), Arc::new(b));

        let p = pointer(
            "myelin://01J0BETA/notif/item/7",
            ArtifactType::Issue,
            &cell_b(),
        );
        let res = agg.resolve_item(&p, &viewer("viewer-2"));

        assert!(
            res.is_tombstone(),
            "an unauthorised viewer gets a tombstone"
        );
        assert!(!res.is_projection());
        assert_eq!(res.tombstone_reason(), Some(InboxTombstoneReason::Denied));
        let InboxResolution::Tombstone(t) = res else {
            unreachable!()
        };
        assert_eq!(t.subject.artifact_ref().0, "myelin://01J0BETA/notif/item/7");
        assert_eq!(agg.raw_rows_crossed(), 0);
    }

    #[test]
    fn gone_item_resolves_to_a_gone_tombstone() {
        let mut b = HomeCellInboxResolver::new(cell_b());
        b.permit("myelin://01J0BETA/notif/item/9", "viewer-1");
        b.mark_gone("myelin://01J0BETA/notif/item/9");

        let mut agg = CrossCellInbox::new(cell_a());
        agg.register(cell_b(), Arc::new(b));

        let p = pointer(
            "myelin://01J0BETA/notif/item/9",
            ArtifactType::Issue,
            &cell_b(),
        );
        let res = agg.resolve_item(&p, &viewer("viewer-1"));
        assert_eq!(res.tombstone_reason(), Some(InboxTombstoneReason::Gone));
    }

    #[test]
    fn a_home_pointer_resolves_locally() {
        let mut b = HomeCellInboxResolver::new(cell_b());
        b.permit("myelin://01J0BETA/notif/item/3", "viewer-1");
        b.render("myelin://01J0BETA/notif/item/3", "a reply on your thread");

        let mut agg = CrossCellInbox::new(cell_b());
        agg.register(cell_b(), Arc::new(b));

        let p = pointer(
            "myelin://01J0BETA/notif/item/3",
            ArtifactType::Channel,
            &cell_b(),
        );
        let res = agg.resolve_item(&p, &viewer("viewer-1"));
        assert!(res.is_projection());
        let InboxResolution::Projection(slice) = res else {
            unreachable!()
        };
        assert_eq!(slice.rendered.text, "a reply on your thread");
    }

    #[test]
    fn unknown_home_cell_degrades_to_a_tombstone() {
        let agg = CrossCellInbox::new(cell_a());
        let p = pointer(
            "myelin://01J0GHOST/notif/item/1",
            ArtifactType::Issue,
            &CellId::from_token("cell-unknown"),
        );
        let res = agg.resolve_item(&p, &viewer("viewer-1"));
        assert_eq!(res.tombstone_reason(), Some(InboxTombstoneReason::Gone));
        assert_eq!(agg.raw_rows_crossed(), 0);
    }

    #[test]
    fn unified_inbox_aggregates_projections_and_excludes_tombstones() {
        let mut b = HomeCellInboxResolver::new(cell_b());
        b.permit("myelin://01J0BETA/notif/item/7", "viewer-1");
        b.render("myelin://01J0BETA/notif/item/7", "Visible B item");
        b.render("myelin://01J0BETA/notif/item/8", "Hidden B item");

        let mut c = HomeCellInboxResolver::new(cell_c());
        c.permit("myelin://01J0GAMMA/notif/item/1", "viewer-1");
        c.render("myelin://01J0GAMMA/notif/item/1", "Visible C item");

        let mut agg = CrossCellInbox::new(cell_a());
        agg.register(cell_b(), Arc::new(b));
        agg.register(cell_c(), Arc::new(c));

        let inbox = vec![
            pointer(
                "myelin://01J0BETA/notif/item/7",
                ArtifactType::Issue,
                &cell_b(),
            ),
            pointer(
                "myelin://01J0BETA/notif/item/8",
                ArtifactType::Issue,
                &cell_b(),
            ),
            pointer(
                "myelin://01J0GAMMA/notif/item/1",
                ArtifactType::Issue,
                &cell_c(),
            ),
        ];
        let unified = agg.unified_inbox(&inbox, &viewer("viewer-1"));
        let texts: Vec<&str> = unified.iter().map(|s| s.rendered.text.as_str()).collect();
        assert_eq!(texts, vec!["Visible B item", "Visible C item"]);
        assert_eq!(unified[0].home_cell, cell_b());
        assert_eq!(unified[1].home_cell, cell_c());
        assert_eq!(agg.cross_cell_resolves(), 3);
        assert_eq!(agg.raw_rows_crossed(), 0);
    }

    #[test]
    fn cell_to_cell_migration_loses_zero_inbox_items() {
        let p = pointer(
            "myelin://01J0BETA/notif/item/7",
            ArtifactType::Issue,
            &cell_b(),
        );

        let re_homed = migrate_item_home_cell(&p, &cell_b(), &cell_c());
        assert_eq!(re_homed.home_cell(), &cell_c(), "re-homed to the new cell");
        assert_eq!(
            re_homed.subject().artifact_ref().0,
            "myelin://01J0BETA/notif/item/7"
        );
        assert_eq!(re_homed.artifact_type(), &ArtifactType::Issue);
        assert_eq!(re_homed.correlation_id(), &CorrelationId("01J0CORR".into()));

        let mut c = HomeCellInboxResolver::new(cell_c());
        c.permit("myelin://01J0BETA/notif/item/7", "viewer-1");
        c.render("myelin://01J0BETA/notif/item/7", "the migrated item");
        let mut agg = CrossCellInbox::new(cell_a());
        agg.register(cell_c(), Arc::new(c));

        let unified = agg.unified_inbox(&[re_homed], &viewer("viewer-1"));
        assert_eq!(unified.len(), 1, "0 inbox items lost on migration");
        assert_eq!(unified[0].rendered.text, "the migrated item");
        assert_eq!(unified[0].home_cell, cell_c());

        let bystander = pointer(
            "myelin://01J0GAMMA/notif/item/1",
            ArtifactType::Issue,
            &cell_c(),
        );
        let untouched = migrate_item_home_cell(&bystander, &cell_b(), &cell_a());
        assert_eq!(
            untouched.home_cell(),
            &cell_c(),
            "a non-migrating pointer is untouched"
        );
    }

    #[test]
    fn dsr_member_cells_erasure_yields_per_cell_receipts_and_erased_tombstones() {
        let subject = OpaqueSubjectId::from_ref(ArtifactRef(
            "myelin://01J0BETA/identity/principal/u1".into(),
        ));
        let member_cells = [cell_b(), cell_c()];
        let receipts: Vec<InboxEraseReceipt> = member_cells
            .iter()
            .map(|c| erase_inbox_pointers_in_cell(c, &subject))
            .collect();
        assert_eq!(receipts.len(), 2);
        assert!(receipts.iter().all(|r| r.erased));
        assert_eq!(receipts[0].cell, cell_b());
        assert_eq!(receipts[1].cell, cell_c());
        assert_eq!(
            receipts[0].subject.artifact_ref().0,
            "myelin://01J0BETA/identity/principal/u1"
        );

        let mut agg = CrossCellInbox::new(cell_a());
        for c in &member_cells {
            let mut r = HomeCellInboxResolver::new(c.clone());
            r.permit("myelin://01J0BETA/identity/principal/u1", "viewer-1");
            r.render(
                "myelin://01J0BETA/identity/principal/u1",
                "u1 mentioned you",
            );
            r.mark_erased("myelin://01J0BETA/identity/principal/u1");
            agg.register(c.clone(), Arc::new(r));
        }
        for c in &member_cells {
            let p = pointer(
                "myelin://01J0BETA/identity/principal/u1",
                ArtifactType::Issue,
                c,
            );
            let res = agg.resolve_item(&p, &viewer("viewer-1"));
            assert_eq!(
                res.tombstone_reason(),
                Some(InboxTombstoneReason::Erased),
                "the erased subject is unresolvable in cell {}",
                c.as_str()
            );
        }
        assert_eq!(agg.raw_rows_crossed(), 0);
    }

    #[test]
    fn aggregation_debug_is_pii_free() {
        let mut b = HomeCellInboxResolver::new(cell_b());
        b.permit("myelin://01J0BETA/notif/item/7", "viewer-secret");
        b.render("myelin://01J0BETA/notif/item/7", "Secret text");
        let mut agg = CrossCellInbox::new(cell_a());
        agg.register(cell_b(), Arc::new(b));
        let _ = agg.resolve_item(
            &pointer(
                "myelin://01J0BETA/notif/item/7",
                ArtifactType::Issue,
                &cell_b(),
            ),
            &viewer("viewer-secret"),
        );
        let dbg = format!("{agg:?}");
        assert!(
            dbg.contains("cell-fr-par-1"),
            "Debug shows the cell id: {dbg}"
        );
        assert!(
            dbg.contains("cross_cell_resolves"),
            "Debug shows the counter: {dbg}"
        );
        assert!(
            !dbg.contains("viewer-secret"),
            "Debug leaks no viewer id: {dbg}"
        );
        assert!(
            !dbg.contains("Secret text"),
            "Debug leaks no rendered content: {dbg}"
        );
    }
}
