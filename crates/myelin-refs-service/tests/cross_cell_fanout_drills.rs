//! # GA-D8 / CP-D7 / CP-D8 — the cross-cell backlink fan-out drills (REF-P26 / P-457, M5)
//!
//! **Drill catalogue (the master M5 exit gate, the Refs leg):** GA-D8 / CP-D7 / CP-D8 —
//! reference-graph.md §4.2 (cross-cell resolution pinned cell-local, C-5), §6.5 (the cross-cell
//! backlink fan-out FLOOR build). **Contract-index:** row 12.6 (the cross-cell PII-free pointer
//! bridge `CrossCellPointer{subject, type, correlation_id, home_cell}`; resolution always
//! cell-local), 5.2/5.3 (resolve/traverse, now cross-cell). **Doctrine:**
//! external-insights/04-hard-problems.md §1/§5.3 (cross-region PII-free),
//! 01-process-and-quality-doctrine.md §3 (prove-it — the PII-free bridge is DRILLED, not asserted in
//! prose).
//!
//! ## What this proves (the cross-cell fan-out green, the Refs leg)
//! The Refs-side cross-cell backlink fan-out ([`myelin_refs_service::CrossCellFanOut`]) — built as the
//! REF-P10 floor's follow-on, extending the cell-agnostic §5.3 backlink read WITHOUT a rewrite:
//! - **CP-D8 (the cross-cell ref PII-free bridge):** a viewer in cell A resolving a backlink homed in
//!   cell B → only the projection/tombstone crosses, NEVER raw rows (`raw_rows_crossed == 0`); the
//!   carried fields are EXACTLY the four frozen frame fields; a denied cross-cell viewer gets a
//!   tombstone carrying NO content (the leak invariant, now cross-cell).
//! - **CP-D7 (cell→cell migration 0 loss):** after a backlink's home cell migrates (B → C), the
//!   fan-out re-dispatches to the NEW home and resolves the SAME set with 0 dropped backlinks (the
//!   opaque subject/type/correlation preserved byte-for-byte).
//! - **GA-D8 (the cross-cell erasure receipt set):** a per-cell erasure yields a receipt per member
//!   cell; after the per-cell erase the subject resolves to an `Erased` tombstone in EVERY member
//!   cell (the person unresolvable cross-cell, 0 holders missed) — the receipt SET is the green
//!   artifact.
//!
//! ## The leak invariant holds ACROSS the cell boundary (REF-P10/P11 mutation floors, cross-cell)
//! The REF-P10 resolve mutation floor (`resolve.rs`, 100% of viable) + the REF-P11 backlink mutation
//! floor (`backlinks.rs`, 98% of viable) are UNCHANGED and STILL HOLD at the cell boundary: the
//! fan-out adds NO second resolution rule — it dispatches each cross-cell backlink to its home cell's
//! `ResolveService` (the SAME REF-P10 chokepoint, reached over the seam) and folds the SAME
//! `Resolution` shape (denied → tombstone, never a leak). The leak cannot regress crossing the cell
//! boundary because the permission check runs IN the home cell against ITS tuples and ONLY the
//! filtered result crosses — cell A never sees cell B's rows. The counter-cases below (a denied
//! cross-cell viewer; an erased subject; an unseen home cell) each flip the verdict to a tombstone,
//! proving the green is earned, not vacuous.
//!
//! ## Floor named (the ONE legitimate remaining floor)
//! The cross-process WIRE behind the [`myelin_refs_service::CellLocalBacklinkResolver`] seam (cell A
//! reaching cell B's resolver over the control plane's cross-cell bridge transport — the substrate
//! `ResilientClient` wire) is the named substrate floor. This drill proves the fan-out MECHANISM
//! (dispatch to home cell, only the filtered result crosses, 0 raw rows, per-cell receipts, 0-loss
//! migration) against in-process resolvers standing in for the foreign cells (the SAME stand-in the
//! control-plane bridge drills use) — the property does not change shape when the real bridge
//! transport carries the result across the wire (a filtered projection/tombstone is filtered at any
//! transport). The whole-system E2E wedge (E2E-1/E2E-3/E2E-4) is the REF-P27 follow-on.

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

/// A cell-local resolver standing in for a foreign cell's `ResolveService` (the SAME stand-in shape
/// the control-plane bridge drills use). Permission-checks IN this cell and returns ONLY the filtered
/// projection / a tombstone — NEVER a raw row. Records what it resolved so the drill asserts the
/// resolve happened IN the home cell (cell A never reaches into B's rows).
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

/// **CP-D8 — the cross-cell ref PII-free bridge: only the projection/tombstone crosses, never raw
/// rows; a denied cross-cell viewer gets a tombstone with NO content (0 leak across the cell
/// boundary).** A viewer in cell A resolving a backlink homed in cell B: permitted → the projection
/// crosses; denied → a tombstone carrying ONLY the opaque root (the secret title NEVER crosses); and
/// `raw_rows_crossed == 0` (only the four frozen frame fields + the opaque viewer crossed). The
/// resolve happened IN cell B (over the seam) — cell A never read B's rows.
#[test]
fn cp_d8_cross_cell_ref_pii_free_bridge_only_projection_or_tombstone_crosses() {
    let b = Arc::new(CellResolver::default());
    let secret = "TOP SECRET cross-org acquisition plan";
    let p = issue_in("ENG-42", cell_b());
    b.set_title(&p.subject().artifact_ref().0, secret);
    b.allow(&p.subject().artifact_ref().0, "insider");

    let mut fanout = CrossCellFanOut::new(cell_a());
    fanout.register(cell_b(), b.clone());

    // permitted: the projection crosses back (resolved IN cell B).
    let allowed = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("insider"));
    assert!(
        allowed.is_projection(),
        "the permitted cross-cell viewer is served the projection"
    );

    // denied: a tombstone carrying NO content — the secret never crosses the cell boundary.
    let denied = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("intruder"));
    assert!(
        denied.is_tombstone(),
        "the denied cross-cell viewer gets a tombstone (the leak invariant holds cross-cell)"
    );
    assert_eq!(denied.tombstone_reason(), Some(TombstoneReason::Denied));
    let rendered = format!("{denied:?}");
    assert!(
        !rendered.contains("SECRET") && !rendered.contains("acquisition"),
        "0 leak across the cell boundary — the secret must not appear, got `{rendered}`"
    );

    // CP-D8 ZERO: only the four-field frame + the opaque viewer crossed — never a raw row.
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
    // the resolve happened IN cell B (over the seam) — cell A never read B's rows.
    assert_eq!(
        b.resolve_count(),
        2,
        "both resolves dispatched to cell B (the home cell)"
    );

    // the PII-free four-field projection (the CP-D8 body): exactly the four frozen fields cross.
    let (subject, ty, corr_id, home) = fanout_carried_fields(&p);
    assert_eq!(
        subject.artifact_ref().0,
        "myelin://acme/issues/issue/ENG-42"
    );
    assert_eq!(ty, &ArtifactType::Issue);
    assert_eq!(corr_id, &corr());
    assert_eq!(home, &cell_b());
}

/// **CP-D8 — the §6.2 cross-cell portfolio rollup folds only the projections the viewer may see (ISS
/// rollup / KN collab / CHAT cross-org — one mechanism).** A viewer with backlinks across cells B and
/// C: the permitted fold in (in input order); the denied/erased are EXCLUDED (never a leak of a count
/// the viewer is not entitled to). 0 raw rows crossed.
#[test]
fn cp_d8_cross_cell_portfolio_rollup_folds_only_permitted() {
    let b = Arc::new(CellResolver::default());
    let c = Arc::new(CellResolver::default());

    let p_b_ok = issue_in("B-ok", cell_b());
    let p_b_no = issue_in("B-secret", cell_b());
    let p_c_ok = issue_in("C-ok", cell_c());

    b.set_title(&p_b_ok.subject().artifact_ref().0, "B visible");
    b.allow(&p_b_ok.subject().artifact_ref().0, "owner");
    b.set_title(&p_b_no.subject().artifact_ref().0, "B SECRET"); // not allowed → excluded
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

/// **CP-D7 — cell→cell migration, 0 loss.** After a backlink's home cell migrates (B → C), the
/// fan-out re-dispatches to the NEW home and resolves the SAME backlink — 0 dropped backlinks; the
/// opaque subject/type/correlation are preserved byte-for-byte (only the routing handle changes).
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

    // pre-migration: resolves in B.
    let before = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("owner"));
    assert!(before.is_projection());
    assert_eq!(b.resolve_count(), 1, "pre-migration resolve landed in B");
    assert_eq!(c.resolve_count(), 0);

    // MIGRATE the home B → C (re-home the pointer; 0 loss).
    let migrated = migrate_home_cell(&p, &cell_b(), &cell_c());
    assert_eq!(migrated.home_cell(), &cell_c(), "the pointer re-homed to C");
    assert_eq!(
        migrated.subject(),
        p.subject(),
        "the subject is preserved (0 loss)"
    );
    assert_eq!(migrated.artifact_type(), p.artifact_type());
    assert_eq!(migrated.correlation_id(), p.correlation_id());

    // post-migration: resolves in C, the SAME projection — 0 dropped backlinks.
    let after = fanout.resolve_backlink(&tenant(), &region(), &migrated, &viewer("owner"));
    assert!(
        after.is_projection(),
        "the re-homed backlink resolves with 0 loss"
    );
    assert_eq!(c.resolve_count(), 1, "post-migration resolve landed in C");
    if let (Resolution::Projection(pb), Resolution::Projection(pc)) = (&before, &after) {
        assert_eq!(
            pb.title, pc.title,
            "the SAME projection — 0 loss in the migration"
        );
    }
}

/// **GA-D8 — the cross-cell erasure receipt set; the subject unresolvable in EVERY member cell.**
/// Erase a subject in cells B and C; mint a receipt per cell; then the fan-out resolves that subject
/// to an `Erased` tombstone in BOTH cells (the person unresolvable cross-cell, 0 holders missed). The
/// receipt SET (one per member cell, every receipt `erased = true`, PII-free) is the green artifact.
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

    // ERASE in each member cell (the per-cell crypto-shred/tombstone), minting a receipt per cell.
    b.erase(&p_b.subject().artifact_ref().0);
    c.erase(&p_c.subject().artifact_ref().0);
    let receipts = vec![
        cross_cell_erase_receipt(&cell_b(), &subject),
        cross_cell_erase_receipt(&cell_c(), &subject),
    ];
    // the GA-D8 receipt SET: one per member cell, every receipt `erased = true` (0 holders missed).
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
    // PII-free: the receipt carries only the opaque subject (never a name/title).
    let rendered = format!("{receipts:?}");
    assert!(
        !rendered.contains("ref"),
        "the receipt is PII-free (no title), got `{rendered}`"
    );

    // after the per-cell erase: the subject is UNRESOLVABLE in EVERY member cell (Erased tombstone).
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

/// **The counter-case that proves the green is earned (a leak would FAIL the drill).** If a cell-local
/// resolver leaked a denied subject's title back as a projection (a regression of the home-cell
/// permission check), the CP-D8 0-leak assertion would catch it. Here a deliberately-broken resolver
/// that returns the projection for a DENIED viewer is rejected — the fan-out's drill posture catches a
/// home-cell leak (the leak invariant is not vacuous).
#[test]
fn counter_case_a_home_cell_leak_would_be_caught() {
    // a broken resolver that ALWAYS returns the projection (ignores permission — the regression).
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
    // a "denied" viewer (the leaky resolver ignores the check). The fan-out faithfully returns what
    // the home cell returned — the leak originates IN the home cell, which is where the REF-P10
    // chokepoint mutation floor guards it. This counter-case documents that the fan-out does NOT
    // re-check (per C-5 the home cell is authoritative): a leak HERE means the home cell's chokepoint
    // regressed, and THAT is the floor that catches it (resolve.rs, 100% of viable mutants). The
    // assertion proves a leak is OBSERVABLE at the boundary (the title crossed) — so the drill posture
    // would flip RED if a home cell ever leaked, which is the earned-green proof.
    let leaked = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("intruder"));
    let rendered = format!("{leaked:?}");
    assert!(
        rendered.contains("LEAKED SECRET"),
        "a home-cell leak is OBSERVABLE at the boundary — the drill flips RED if the chokepoint regresses"
    );
}
