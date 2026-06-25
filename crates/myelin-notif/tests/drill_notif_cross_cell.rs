//! # Drill — the cross-cell inbox-aggregation legs of GA-D8 / CP-D7 / CP-D8 (NOTIF-P24 / P-466, M5)
//!
//! The master M5 exit gate's cross-cell inbox legs, the Notif deliverable:
//! - **CP-D8** — the cross-cell inbox PII-free bridge: only the humanised projection/tombstone crosses,
//!   never raw `inbox_item` rows (`raw_rows_crossed == 0`); the carried fields are exactly the four
//!   frozen frame fields. An unauthorised cross-cell viewer gets a tombstone (never a leak).
//! - **CP-D7** — cell→cell migration 0 inbox-item loss: after an item's home cell MIGRATES (the
//!   pointer's `home_cell` is re-homed), the aggregation re-dispatches to the NEW home and resolves the
//!   SAME set with 0 dropped inbox items.
//! - **GA-D8** — the cross-cell erasure receipt SET: a per-cell erasure of a cross-cell inbox item's
//!   subject yields a receipt per member cell; after the per-cell erase, the aggregation resolves that
//!   subject to an `Erased` tombstone in EVERY member cell (the person unresolvable cross-cell, 0
//!   holders missed).
//!
//! Telemetry green artifact (the prompt's threshold, NEVER softened): **0 PII crossing cells; 0 inbox
//! items lost on migration.** Resolution is ALWAYS cell-local — the home cell humanises + permission-
//! checks IN the home cell; only the rendered projection/tombstone crosses back (residency-preserving,
//! ADR-11).

use std::collections::HashSet;
use std::sync::Arc;

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::{
    aggregation_carried_fields, cross_cell_inbox_pointer, erase_inbox_pointers_in_cell,
    migrate_item_home_cell, CellLocalInboxResolver, CrossCellInbox, HumanisedString,
    InboxProjectionSlice, InboxResolution, InboxTombstone, InboxTombstoneReason,
};
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, TenantId,
};

fn cell(token: &str) -> CellId {
    CellId::from_token(token)
}

fn viewer(token: &str) -> Principal {
    Principal::stub(
        PrincipalId(token.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn pointer(subject: &str, home: &CellId) -> CrossCellPointer {
    cross_cell_inbox_pointer(
        &ArtifactRef(subject.into()),
        ArtifactType::Issue,
        CorrelationId("01J0CORR".into()),
        home.clone(),
    )
}

/// A home-cell inbox resolver standing in for cell B's Notif `humanise` resolve path: it
/// permission-checks IN this cell and returns ONLY the humanised projection / a tombstone (never a raw
/// row). The `erased` set wins over a permit/render (the GA-D8 leg).
struct HomeCell {
    cell: CellId,
    permitted: HashSet<(String, String)>,
    rendered: std::collections::HashMap<String, String>,
    erased: HashSet<String>,
}

impl HomeCell {
    fn new(cell: CellId) -> HomeCell {
        HomeCell {
            cell,
            permitted: HashSet::new(),
            rendered: std::collections::HashMap::new(),
            erased: HashSet::new(),
        }
    }
    fn permit(mut self, subject: &str, viewer: &str, text: &str) -> HomeCell {
        self.permitted.insert((subject.into(), viewer.into()));
        self.rendered.insert(subject.into(), text.into());
        self
    }
    fn erase(mut self, subject: &str) -> HomeCell {
        self.erased.insert(subject.into());
        self
    }
}

impl CellLocalInboxResolver for HomeCell {
    fn resolve_inbox_item_in_cell(
        &self,
        pointer: &CrossCellPointer,
        viewer: &Principal,
    ) -> InboxResolution {
        let subject = pointer.subject().artifact_ref().0.clone();
        let v = viewer.principal_id.0.clone();
        if self.erased.contains(&subject) {
            return InboxResolution::Tombstone(InboxTombstone {
                subject: pointer.subject().clone(),
                home_cell: self.cell.clone(),
                reason: InboxTombstoneReason::Erased,
            });
        }
        if !self.permitted.contains(&(subject.clone(), v)) {
            return InboxResolution::Tombstone(InboxTombstone {
                subject: pointer.subject().clone(),
                home_cell: self.cell.clone(),
                reason: InboxTombstoneReason::Denied,
            });
        }
        let text = self.rendered.get(&subject).cloned().unwrap_or_default();
        InboxResolution::Projection(InboxProjectionSlice {
            subject: pointer.subject().clone(),
            home_cell: self.cell.clone(),
            rendered: HumanisedString {
                text,
                links: vec![subject],
                icon: "inbox".into(),
            },
        })
    }
}

/// **CP-D8 — the cross-cell inbox PII-free bridge.** A viewer in cell A with items homed in cells B and
/// C gets a unified inbox: cell B and cell C each humanise + permission-check IN-cell; only the rendered
/// projection (or a tombstone) crosses; 0 raw rows cross; the carried fields are exactly the four frozen
/// frame fields. An unauthorised cross-cell item is a tombstone (never a leak), excluded from the view.
#[test]
fn cp_d8_cross_cell_inbox_resolves_cell_local_with_zero_pii_crossing() {
    let cell_a = cell("cell-fr-par-1");
    let cell_b = cell("cell-fr-par-2");
    let cell_c = cell("cell-de-fra-1");

    let b = HomeCell::new(cell_b.clone())
        .permit(
            "myelin://01J0BETA/notif/item/7",
            "viewer-1",
            "mentioned in Ship M5",
        )
        // item/8 NOT permitted for viewer-1 → denied tombstone, excluded from the unified view.
        .permit("myelin://01J0BETA/notif/item/8", "someone-else", "secret");
    let c = HomeCell::new(cell_c.clone()).permit(
        "myelin://01J0GAMMA/notif/item/1",
        "viewer-1",
        "review requested",
    );

    let mut agg = CrossCellInbox::new(cell_a);
    agg.register(cell_b.clone(), Arc::new(b));
    agg.register(cell_c.clone(), Arc::new(c));

    let inbox = vec![
        pointer("myelin://01J0BETA/notif/item/7", &cell_b),
        pointer("myelin://01J0BETA/notif/item/8", &cell_b),
        pointer("myelin://01J0GAMMA/notif/item/1", &cell_c),
    ];

    // The carried fields are exactly the four frozen frame fields (no fifth — no PII).
    for p in &inbox {
        let (subject, _kind, _corr, home) = aggregation_carried_fields(p);
        assert!(subject.artifact_ref().0.starts_with("myelin://"));
        assert!(home.as_str().starts_with("cell-"));
    }

    // The full per-pointer resolve PROVES the denied item is a tombstone (never a leak), not just
    // absent.
    let all = agg.resolve_all(&inbox, &viewer("viewer-1"));
    assert!(all[0].is_projection());
    assert_eq!(
        all[1].tombstone_reason(),
        Some(InboxTombstoneReason::Denied)
    );
    assert!(all[2].is_projection());

    // The unified view aggregates only what the viewer may see (the denied item excluded — no leak).
    let unified = agg.unified_inbox(&inbox, &viewer("viewer-1"));
    let texts: Vec<&str> = unified.iter().map(|s| s.rendered.text.as_str()).collect();
    assert_eq!(texts, vec!["mentioned in Ship M5", "review requested"]);
    // The unified view spans two member cells (cell-local resolution in each).
    assert_eq!(unified[0].home_cell, cell_b);
    assert_eq!(unified[1].home_cell, cell_c);

    // THE CP-D8 GREEN ARTIFACT: 0 PII crossing cells (0 raw rows). Six cross-cell resolves served
    // (three for the `resolve_all` proof pass + three for the `unified_inbox` render pass over the
    // same three-item inbox).
    assert_eq!(
        agg.raw_rows_crossed(),
        0,
        "0 PII crosses cells (the CP-D8 zero — never softened)"
    );
    assert_eq!(agg.cross_cell_resolves(), 6);
}

/// **CP-D7 — a cell→cell migration loses 0 inbox items.** Before migration two items are homed in cell
/// B; cell B migrates to cell C; the aggregation re-homes both pointers and resolves the SAME set in the
/// new home with 0 dropped items.
#[test]
fn cp_d7_cell_to_cell_migration_loses_zero_inbox_items() {
    let cell_a = cell("cell-fr-par-1");
    let cell_b = cell("cell-fr-par-2");
    let cell_c = cell("cell-de-fra-1");

    let before = [
        pointer("myelin://01J0BETA/notif/item/7", &cell_b),
        pointer("myelin://01J0BETA/notif/item/8", &cell_b),
    ];

    // Migrate cell B → cell C: re-home every pointer homed in B (0 loss; opaque fields preserved).
    let after: Vec<CrossCellPointer> = before
        .iter()
        .map(|p| migrate_item_home_cell(p, &cell_b, &cell_c))
        .collect();
    assert!(
        after.iter().all(|p| p.home_cell() == &cell_c),
        "every item re-homed to the new cell"
    );
    // The opaque subjects are preserved byte-for-byte (the SAME items, now in the new home).
    let subjects_before: HashSet<String> = before
        .iter()
        .map(|p| p.subject().artifact_ref().0.clone())
        .collect();
    let subjects_after: HashSet<String> = after
        .iter()
        .map(|p| p.subject().artifact_ref().0.clone())
        .collect();
    assert_eq!(
        subjects_before, subjects_after,
        "0 inbox items lost on migration (subjects preserved)"
    );

    // The aggregation resolves the re-homed set in the NEW home cell — both items survive.
    let c = HomeCell::new(cell_c.clone())
        .permit("myelin://01J0BETA/notif/item/7", "viewer-1", "item 7")
        .permit("myelin://01J0BETA/notif/item/8", "viewer-1", "item 8");
    let mut agg = CrossCellInbox::new(cell_a);
    agg.register(cell_c.clone(), Arc::new(c));

    let unified = agg.unified_inbox(&after, &viewer("viewer-1"));
    assert_eq!(
        unified.len(),
        2,
        "0 inbox items lost on migration (both resolve in the new home)"
    );
    assert_eq!(agg.raw_rows_crossed(), 0);
}

/// **GA-D8 — the cross-cell erasure leg: per-cell receipts + Erased tombstone in EVERY member cell.**
/// The DSR orchestrator iterates the recipient's `member_cells` (10.4), erasing inbox refs in each;
/// every member cell mints a receipt (0 holders missed); after the per-cell erase the subject resolves
/// to an `Erased` tombstone in every member cell (the person unresolvable cross-cell).
#[test]
fn ga_d8_cross_cell_erasure_yields_receipts_and_erased_in_every_member_cell() {
    let cell_a = cell("cell-fr-par-1");
    let member_cells = [cell("cell-fr-par-2"), cell("cell-de-fra-1")];
    let subject_urn = "myelin://01J0BETA/identity/principal/u1";
    let subject = myelin_tenancy::OpaqueSubjectId::from_ref(ArtifactRef(subject_urn.into()));

    // The DSR orchestrator iterates member_cells over the bridge — one erase receipt per cell.
    let receipts: Vec<_> = member_cells
        .iter()
        .map(|c| erase_inbox_pointers_in_cell(c, &subject))
        .collect();
    assert_eq!(
        receipts.len(),
        member_cells.len(),
        "one receipt per member cell (0 holders missed)"
    );
    assert!(
        receipts.iter().all(|r| r.erased),
        "every member cell ran the erase"
    );

    // After the per-cell erase, the subject resolves to an Erased tombstone in EVERY member cell — even
    // though the viewer WAS permitted + the item WAS rendered (the erase wins).
    let mut agg = CrossCellInbox::new(cell_a);
    for c in &member_cells {
        let resolver = HomeCell::new(c.clone())
            .permit(subject_urn, "viewer-1", "u1 mentioned you")
            .erase(subject_urn);
        agg.register(c.clone(), Arc::new(resolver));
    }
    for c in &member_cells {
        let res = agg.resolve_item(&pointer(subject_urn, c), &viewer("viewer-1"));
        assert_eq!(
            res.tombstone_reason(),
            Some(InboxTombstoneReason::Erased),
            "the erased subject is unresolvable in cell {}",
            c.as_str()
        );
    }
    // 0 PII crossed during the erasure resolves.
    assert_eq!(agg.raw_rows_crossed(), 0);
}
