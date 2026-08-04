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

#[test]
fn srch_p31_cross_cell_federated_search_is_leak_free_zero_pii_crossing() {
    let b = MemberCell::new("cell-fr-par-1");
    let c = MemberCell::new("cell-de-fra-1");
    let secret = "TOP-SECRET cross-org acquisition memo";

    let b_ok = aref("b-ok");
    let b_secret = aref("b-secret");
    b.index(&b_ok.0, "B visible row");
    b.index(&b_secret.0, secret);
    b.grant(&b_ok.0, "viewer1");
    let c_ok = aref("c-ok");
    c.index(&c_ok.0, "C visible row");
    c.grant(&c_ok.0, "viewer1");

    let mut fed = FederatedSearch::new(coordinator());
    register(&mut fed, cell_b(), b.clone());
    register(&mut fed, cell_c(), c.clone());

    let rows = fed.query(&tenant(), &region(), "acquisition", &viewer("viewer1"));

    let titles: HashSet<String> = rows
        .iter()
        .filter_map(|r| r.projection.as_ref().map(|p| p.title.clone()))
        .collect();
    assert_eq!(rows.len(), 2, "exactly the two visible rows surface");
    assert!(titles.contains("B visible row"));
    assert!(titles.contains("C visible row"));

    let rendered = format!("{rows:?}");
    assert!(
        !rendered.contains("SECRET") && !rendered.contains("acquisition memo"),
        "0 cross-cell leak: the secret must not cross, got `{rendered}`"
    );

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
    assert_eq!(b.ran_queries(), vec!["acquisition".to_string()]);
    assert_eq!(c.ran_queries(), vec!["acquisition".to_string()]);

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

#[test]
fn srch_p31_red_counter_case_a_resolver_without_the_check_would_leak() {
    struct LeakyCell {
        cell_token: String,
        titles: HashMap<String, String>,
        matches: Vec<String>,
    }
    impl CellLocalQuery for LeakyCell {
        fn run(&self, _t: &TenantId, _r: &Region, _q: &str, _v: &Principal) -> CellRanking {
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
    let rendered = format!("{rows:?}");
    assert!(
        rendered.contains("SECRET"),
        "the leaky path (no per-viewer check) leaks - so the green test's 0-leak assertion is earned"
    );
}

#[test]
fn srch_p31_chained_grant_surfaces_the_row_after_a_home_cell_grant() {
    let b = MemberCell::new("cell-fr-par-1");
    let row = aref("b-row");
    b.index(&row.0, "now-visible row");

    let mut fed = FederatedSearch::new(coordinator());
    register(&mut fed, cell_b(), b.clone());

    let before = fed.query(&tenant(), &region(), "q", &viewer("viewer1"));
    assert!(before.is_empty(), "before the grant the row is invisible");

    b.grant(&row.0, "viewer1");
    let after = fed.query(&tenant(), &region(), "q", &viewer("viewer1"));
    assert_eq!(after.len(), 1, "after the home-cell grant the row surfaces");
    assert_eq!(
        after[0].projection.as_ref().unwrap().title,
        "now-visible row"
    );
    assert_eq!(after[0].home_cell, cell_b(), "resolved in the home cell");
}

#[test]
fn srch_p31_cell_migration_re_homes_rankings_zero_loss() {
    let b_ranking = CellRanking::new(cell_b(), vec![aref("row1"), aref("row2")]);
    let migrated = migrate_ranking_home(&b_ranking, &cell_b(), &cell_c());
    assert_eq!(migrated.cell, cell_c(), "the ranking re-homed B → C");
    assert_eq!(
        migrated.refs, b_ranking.refs,
        "0 loss: the refs are preserved"
    );

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
        "the merged set is exactly the union of the rankings - no fabricated row"
    );
}
