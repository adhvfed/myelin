//! # Drill — SRCH-P31 cross-cell federated search: leak-free scatter-gather + residency-free merge
//! (SRCH-P31 → P-464; S-M5; the §6.4 cross-cell leak-free gate — the dated green artifact)
//!
//! **Drill source:** `planning/05-refined-shared-systems-architecture/search-and-indexing.md` §6.4
//! (cross-cell federated search — designed-and-extends: *scatter-gather*, each cell runs the SAME
//! permission-filtered query LOCALLY over its own index/`list_objects`/residency; a *residency-free
//! merge* fuses ONLY ranking metadata + `ArtifactRef`s — never payload/PII — at the control-plane
//! boundary; result rows resolved **per-viewer in their HOME cell** over the cross-cell PII-free
//! pointer bridge, contract 12.6, resolution always cell-local). **Reconciliation:** OQ-I (single-cell
//! → multi-cell). **External insight:** `VISION.md` §3 (EU-sovereign — only ranking metadata crosses,
//! never PII); `01-process-and-quality-doctrine.md` §3 (prove-it — the leak-free property is DRILLED).
//!
//! ## What this drill proves (the dated green artifact, 2026-06-25)
//! A federated query across TWO cells (B and C, two member cells of one multi-cell tenant) for a
//! viewer coordinated at a third cell:
//! - **0 cross-cell leak:** a confidential row in cell B (the viewer cannot see it) NEVER surfaces —
//!   neither in cell B's ranking (the cell's `list_objects` pre-filter ran IN the cell) nor in the
//!   per-viewer home-cell resolution. The viewer gets ONLY the rows they may see, resolved
//!   per-viewer in B and C.
//! - **0 PII crossing the merge boundary:** the residency-free merge carries ONLY `ArtifactRef`s +
//!   ranking scores; `FederatedSearch::payload_crossed_merge() == 0` (a live tripwire, not a constant).
//! - **per-viewer home-cell resolution (5.6):** each row resolves IN its HOME cell (B's rows in B,
//!   C's rows in C — the coordinator never reaches into a member cell's rows).
//! - **the chained grant:** grant the row in its home cell → the federated query now surfaces it
//!   (the rejection was the home-cell ACL firing, not a blanket deny).
//! - **CP-D7 0-loss:** after cell B migrates B → C, the ranking re-homes and the rows resolve in C
//!   with 0 dropped rows.
//! - **the RED counter-case (the green is earned):** a federated search that (hypothetically) skipped
//!   the home-cell resolution check WOULD leak the secret — the drill shows the actual path does not.
//!
//! ## Driven end-to-end vs. scaled-down (recorded honestly per the prompt)
//! This is a **scaled-down two-cell in-process variant**: the member cells are in-process
//! [`CellLocalQuery`]/[`CellLocalRowResolver`] stand-ins (the SAME stand-in shape the refs cross-cell
//! fan-out + the control-plane bridge tests use). The cross-cell BUILD is **gated on multi-cell going
//! live** (contract 12.6, OQ-I); the cross-process WIRE behind the seams is the named substrate floor
//! (the control plane's `ResilientClient` bridge transport). The scatter-gather + residency-free merge
//! + per-viewer home-cell resolution MECHANISM is REAL and proven here; the wire is the substrate floor.
//!
//! ## Floors named
//! - The cross-process WIRE behind [`CellLocalQuery`]/[`CellLocalRowResolver`] is the substrate floor
//!   (the control plane's bridge transport).
//! - The member-cell ENUMERATION is the control plane's `placement_of`/`member_cells` fan-out
//!   (P-CP-20 / P-430); this drill supplies the member set + drives the scatter/merge/resolve.
//! - The whole-system E2E wedge (E2E-1 PR pane / E2E-3 reindex-parity / E2E-4 DSAR fan-out) is
//!   SRCH-P32.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use myelin_events::ArtifactRef;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_search::cross_cell::{
    migrate_ranking_home, CellLocalQuery, CellLocalRowResolver, CellRanking, FederatedSearch,
    MergedRef, RowProjection,
};
use myelin_tenancy::{CellId, Region, TenantId};

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}
fn region() -> Region {
    Region("fr-par".into())
}
fn coordinator() -> CellId {
    CellId::from_token("cell-fr-par-0")
}
fn cell_b() -> CellId {
    CellId::from_token("cell-fr-par-1")
}
fn cell_c() -> CellId {
    CellId::from_token("cell-de-fra-1")
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn aref(s: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://acme/issues/issue/{s}"))
}

/// A member-cell stand-in: an index that ranks rows for the query + a per-cell ACL that runs BOTH the
/// `list_objects` pre-filter (the scatter) AND the home-cell project check (the resolution). It records
/// the queries it ran + the rows it projected so the drill can assert the work happened IN the cell.
#[derive(Default)]
struct MemberCell {
    cell_token: String,
    matches: Mutex<Vec<String>>,
    allowed: Mutex<HashSet<(String, String)>>,
    titles: Mutex<HashMap<String, String>>,
    ran_queries: Mutex<Vec<String>>,
    projected: Mutex<Vec<String>>,
}

impl MemberCell {
    fn new(cell_token: &str) -> Arc<MemberCell> {
        Arc::new(MemberCell {
            cell_token: cell_token.into(),
            ..Default::default()
        })
    }
    fn index(&self, ref_urn: &str, title: &str) {
        self.matches.lock().unwrap().push(ref_urn.into());
        self.titles
            .lock()
            .unwrap()
            .insert(ref_urn.into(), title.into());
    }
    fn grant(&self, ref_urn: &str, viewer_id: &str) {
        self.allowed
            .lock()
            .unwrap()
            .insert((ref_urn.into(), viewer_id.into()));
    }
    fn allowed(&self, ref_urn: &str, viewer_id: &str) -> bool {
        self.allowed
            .lock()
            .unwrap()
            .contains(&(ref_urn.into(), viewer_id.into()))
    }
    fn projected(&self) -> Vec<String> {
        self.projected.lock().unwrap().clone()
    }
    fn ran_queries(&self) -> Vec<String> {
        self.ran_queries.lock().unwrap().clone()
    }
}

impl CellLocalQuery for MemberCell {
    fn run(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        query: &str,
        viewer: &Principal,
    ) -> CellRanking {
        self.ran_queries.lock().unwrap().push(query.into());
        // The cell's list_objects PRE-FILTER (§4.2.1): only the viewer's visible matches enter the
        // ranking. A confidential row the viewer cannot see is structurally absent (leak-free in-cell).
        let refs: Vec<ArtifactRef> = self
            .matches
            .lock()
            .unwrap()
            .iter()
            .filter(|r| self.allowed(r, &viewer.principal_id.0))
            .map(|r| ArtifactRef(r.clone()))
            .collect();
        CellRanking::new(CellId::from_token(self.cell_token.clone()), refs)
    }
}

impl CellLocalRowResolver for MemberCell {
    fn project_row(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
    ) -> Option<RowProjection> {
        self.projected.lock().unwrap().push(ref_.0.clone());
        // The home-cell project check (5.6): denied → None (a tombstone, the secret never crosses).
        if !self.allowed(&ref_.0, &viewer.principal_id.0) {
            return None;
        }
        let title = self
            .titles
            .lock()
            .unwrap()
            .get(&ref_.0)
            .cloned()
            .unwrap_or_else(|| "untitled".into());
        Some(RowProjection {
            title,
            state: "open".into(),
            render_hint: "issue-card".into(),
        })
    }
}

fn register(fed: &mut FederatedSearch, cell: CellId, member: Arc<MemberCell>) {
    fed.register(cell, member.clone(), member);
}

/// **SRCH-P31 — the cross-cell leak-free gate (the dated green artifact, 2026-06-25).** A federated
/// query across two cells returns ONLY the viewer's visible rows, resolved per-viewer in their home
/// cell; the residency-free merge carries only ranking metadata; 0 cross-cell leak, 0 PII crossing.
#[test]
fn srch_p31_cross_cell_federated_search_is_leak_free_zero_pii_crossing() {
    let b = MemberCell::new("cell-fr-par-1");
    let c = MemberCell::new("cell-de-fra-1");
    let secret = "TOP-SECRET cross-org acquisition memo";

    // cell B: a row the viewer may see + a CONFIDENTIAL row they may not.
    let b_ok = aref("b-ok");
    let b_secret = aref("b-secret");
    b.index(&b_ok.0, "B visible row");
    b.index(&b_secret.0, secret);
    b.grant(&b_ok.0, "viewer1"); // b_secret NOT granted to viewer1.
                                 // cell C: a row the viewer may see.
    let c_ok = aref("c-ok");
    c.index(&c_ok.0, "C visible row");
    c.grant(&c_ok.0, "viewer1");

    let mut fed = FederatedSearch::new(coordinator());
    register(&mut fed, cell_b(), b.clone());
    register(&mut fed, cell_c(), c.clone());

    let rows = fed.query(&tenant(), &region(), "acquisition", &viewer("viewer1"));

    // exactly the two visible rows surface — never the secret.
    let titles: HashSet<String> = rows
        .iter()
        .filter_map(|r| r.projection.as_ref().map(|p| p.title.clone()))
        .collect();
    assert_eq!(rows.len(), 2, "exactly the two visible rows surface");
    assert!(titles.contains("B visible row"));
    assert!(titles.contains("C visible row"));

    // 0 cross-cell leak: the secret never crosses, anywhere.
    let rendered = format!("{rows:?}");
    assert!(
        !rendered.contains("SECRET") && !rendered.contains("acquisition memo"),
        "0 cross-cell leak: the secret must not cross, got `{rendered}`"
    );

    // per-viewer home-cell resolution: each row resolved IN its home cell.
    assert!(
        b.projected().contains(&b_ok.0),
        "the B row was projected IN cell B"
    );
    assert!(
        c.projected().contains(&c_ok.0),
        "the C row was projected IN cell C"
    );
    assert!(
        rows.iter().any(|r| r.home_cell == cell_b())
            && rows.iter().any(|r| r.home_cell == cell_c()),
        "the rows resolved per-viewer in their home cells (B and C)"
    );
    // the SAME query ran in each cell (the scatter).
    assert_eq!(b.ran_queries(), vec!["acquisition".to_string()]);
    assert_eq!(c.ran_queries(), vec!["acquisition".to_string()]);

    // 0 PII crossing the residency-free merge boundary (the §6.4 zero — the dated green artifact).
    assert_eq!(
        fed.payload_crossed_merge(),
        0,
        "0 PII crossing the residency-free merge boundary"
    );
    assert_eq!(
        fed.scattered(),
        2,
        "the query scattered to both member cells"
    );
}

/// **The RED counter-case — the green is earned.** A "leaky" federated search that skipped the
/// home-cell resolution check (resolving a denied row WITHOUT the per-viewer check) WOULD surface the
/// secret. The drill PROVES the leaky path leaks (so the green is not vacuous) and the real path does
/// not. (We model the leaky resolver inline; the production [`FederatedSearch`] always runs the
/// home-cell check.)
#[test]
fn srch_p31_red_counter_case_a_resolver_without_the_check_would_leak() {
    // A LEAKY resolver: returns the title WITHOUT checking the per-viewer ACL — the bug the real path
    // structurally cannot have (the production resolver checks). Proves the assertion is load-bearing.
    struct LeakyCell {
        cell_token: String,
        titles: HashMap<String, String>,
        matches: Vec<String>,
    }
    impl CellLocalQuery for LeakyCell {
        fn run(&self, _t: &TenantId, _r: &Region, _q: &str, _v: &Principal) -> CellRanking {
            // the leaky scatter leaks EVERY match into the ranking (no pre-filter) — the bug.
            CellRanking::new(
                CellId::from_token(self.cell_token.clone()),
                self.matches.iter().map(|m| ArtifactRef(m.clone())),
            )
        }
    }
    impl CellLocalRowResolver for LeakyCell {
        fn project_row(
            &self,
            _t: &TenantId,
            _r: &Region,
            ref_: &ArtifactRef,
            _v: &Principal,
        ) -> Option<RowProjection> {
            // the leaky resolver returns the title WITHOUT the per-viewer check — the bug.
            self.titles.get(&ref_.0).map(|title| RowProjection {
                title: title.clone(),
                state: "open".into(),
                render_hint: "issue-card".into(),
            })
        }
    }

    let secret_ref = aref("b-secret");
    let leaky = Arc::new(LeakyCell {
        cell_token: "cell-fr-par-1".into(),
        titles: HashMap::from([(secret_ref.0.clone(), "SECRET".to_string())]),
        matches: vec![secret_ref.0.clone()],
    });
    let mut fed = FederatedSearch::new(coordinator());
    fed.register(cell_b(), leaky.clone(), leaky.clone());

    let rows = fed.query(&tenant(), &region(), "q", &viewer("intruder"));
    // the LEAKY path DOES leak — proving the assertion in the green test is load-bearing.
    let rendered = format!("{rows:?}");
    assert!(
        rendered.contains("SECRET"),
        "the leaky path (no per-viewer check) leaks — so the green test's 0-leak assertion is earned"
    );
}

/// **The chained grant — the rejection was the home-cell ACL firing.** Grant the confidential row in
/// its home cell → the federated query now surfaces it. (Proves the in-cell pre-filter + home-cell
/// resolution are the ACL, not a blanket deny.)
#[test]
fn srch_p31_chained_grant_surfaces_the_row_after_a_home_cell_grant() {
    let b = MemberCell::new("cell-fr-par-1");
    let row = aref("b-row");
    b.index(&row.0, "now-visible row");

    let mut fed = FederatedSearch::new(coordinator());
    register(&mut fed, cell_b(), b.clone());

    // before the grant: 0 rows (the in-cell pre-filter excludes it).
    let before = fed.query(&tenant(), &region(), "q", &viewer("viewer1"));
    assert!(before.is_empty(), "before the grant the row is invisible");

    // grant in the HOME cell → the federated query surfaces it.
    b.grant(&row.0, "viewer1");
    let after = fed.query(&tenant(), &region(), "q", &viewer("viewer1"));
    assert_eq!(after.len(), 1, "after the home-cell grant the row surfaces");
    assert_eq!(
        after[0].projection.as_ref().unwrap().title,
        "now-visible row"
    );
    assert_eq!(after[0].home_cell, cell_b(), "resolved in the home cell");
}

/// **CP-D7 0-loss — after cell B migrates B → C, the rows resolve in the new home with 0 loss.** The
/// ranking re-homes; the refs are preserved byte-for-byte; the per-viewer resolution dispatches to C.
#[test]
fn srch_p31_cell_migration_re_homes_rankings_zero_loss() {
    let b_ranking = CellRanking::new(cell_b(), vec![aref("row1"), aref("row2")]);
    let migrated = migrate_ranking_home(&b_ranking, &cell_b(), &cell_c());
    assert_eq!(migrated.cell, cell_c(), "the ranking re-homed B → C");
    assert_eq!(
        migrated.refs, b_ranking.refs,
        "0 loss: the refs are preserved"
    );

    // the merge homes the re-homed rows in C; the resolution dispatches there.
    let c = MemberCell::new("cell-de-fra-1");
    c.index(&aref("row1").0, "row1");
    c.index(&aref("row2").0, "row2");
    c.grant(&aref("row1").0, "owner");
    c.grant(&aref("row2").0, "owner");
    let mut fed = FederatedSearch::new(coordinator());
    register(&mut fed, cell_c(), c.clone());

    let merged = fed.residency_free_merge(&[migrated]);
    assert!(
        merged.iter().all(|m| m.home_cell == cell_c()),
        "every re-homed row homes in C"
    );
    let resolved = fed.resolve_rows(&tenant(), &region(), &merged, &viewer("owner"));
    assert_eq!(resolved.len(), 2, "0 dropped rows after the migration");
    assert!(
        resolved.iter().all(|r| r.is_visible()),
        "every re-homed row resolves in the new home (0 loss)"
    );
}

/// **The residency-free merge carries ONLY ranking metadata across the merge boundary.** The merged
/// entries are `MergedRef { ref_, home_cell, score }` — there is structurally no payload field. A
/// shared/replicated ref ranked in both cells fuses to the top (the cross-cell agreement boost).
#[test]
fn srch_p31_residency_free_merge_carries_only_ranking_metadata() {
    let shared = aref("shared");
    let rankings = vec![
        CellRanking::new(cell_b(), vec![shared.clone(), aref("b-only")]),
        CellRanking::new(cell_c(), vec![aref("c-only"), shared.clone()]),
    ];
    let fed = FederatedSearch::new(coordinator());
    let merged: Vec<MergedRef> = fed.residency_free_merge(&rankings);

    assert_eq!(
        merged[0].ref_, shared,
        "the cross-cell agreement ref surfaces first (RRF on rank, residency-free)"
    );
    // only ArtifactRefs + scores + home cells crossed — 0 payload.
    assert_eq!(
        fed.payload_crossed_merge(),
        0,
        "the merge carried only ranking metadata"
    );
    let got: HashSet<String> = merged.iter().map(|m| m.ref_.0.clone()).collect();
    let union: HashSet<String> = ["shared", "b-only", "c-only"]
        .into_iter()
        .map(|s| aref(s).0)
        .collect();
    assert_eq!(
        got, union,
        "the merged set is exactly the union of the rankings — no fabricated row"
    );
}
