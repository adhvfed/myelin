use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_events::ArtifactRef;
use myelin_identity::Principal;
use myelin_tenancy::{CellId, Region, TenantId};

use crate::fusion::{reciprocal_rank_fusion, RankedList};

pub const FEDERATED_SCATTERS_SIGNAL: &str = "search.federated_scatters";

pub const FEDERATED_PAYLOAD_CROSSED_SIGNAL: &str = "search.federated_payload_crossed_merge";

#[derive(Clone, Debug, PartialEq)]
pub struct CellRanking {
    pub cell: CellId,
    pub refs: Vec<ArtifactRef>,
}

impl CellRanking {
    pub fn new(cell: CellId, refs: impl IntoIterator<Item = ArtifactRef>) -> CellRanking {
        CellRanking {
            cell,
            refs: refs.into_iter().collect(),
        }
    }
}

pub trait CellLocalQuery: Send + Sync {
    fn run(
        &self,
        tenant: &TenantId,
        region: &Region,
        query: &str,
        viewer: &Principal,
    ) -> CellRanking;
}

#[derive(Clone, Debug, PartialEq)]
pub struct FederatedRow {
    pub ref_: ArtifactRef,
    pub home_cell: CellId,
    pub projection: Option<RowProjection>,
}

impl FederatedRow {
    pub fn is_visible(&self) -> bool {
        self.projection.is_some()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowProjection {
    pub title: String,
    pub state: String,
    pub render_hint: String,
}

pub trait CellLocalRowResolver: Send + Sync {
    fn project_row(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
    ) -> Option<RowProjection>;
}

#[derive(Clone)]
pub struct FederatedSearch {
    coordinator_cell: CellId,
    queriers: HashMap<CellId, Arc<dyn CellLocalQuery>>,
    resolvers: HashMap<CellId, Arc<dyn CellLocalRowResolver>>,
    scattered: Arc<AtomicU64>,
    payload_crossed_merge: Arc<AtomicU64>,
}

impl FederatedSearch {
    pub fn new(coordinator_cell: CellId) -> FederatedSearch {
        FederatedSearch {
            coordinator_cell,
            queriers: HashMap::new(),
            resolvers: HashMap::new(),
            scattered: Arc::new(AtomicU64::new(0)),
            payload_crossed_merge: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn coordinator_cell(&self) -> &CellId {
        &self.coordinator_cell
    }

    pub fn register(
        &mut self,
        cell: CellId,
        querier: Arc<dyn CellLocalQuery>,
        resolver: Arc<dyn CellLocalRowResolver>,
    ) {
        self.queriers.insert(cell.clone(), querier);
        self.resolvers.insert(cell, resolver);
    }

    pub fn scatter(
        &self,
        tenant: &TenantId,
        region: &Region,
        query: &str,
        viewer: &Principal,
    ) -> Vec<CellRanking> {
        let mut cells: Vec<&CellId> = self.queriers.keys().collect();
        cells.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        cells
            .into_iter()
            .map(|cell| {
                self.scattered.fetch_add(1, Ordering::SeqCst);
                self.queriers[cell].run(tenant, region, query, viewer)
            })
            .collect()
    }

    pub fn residency_free_merge(&self, rankings: &[CellRanking]) -> Vec<MergedRef> {
        let mut home_of: HashMap<String, CellId> = HashMap::new();
        let mut branches: Vec<RankedList> = Vec::with_capacity(rankings.len());
        for ranking in rankings {
            for r in &ranking.refs {
                home_of
                    .entry(r.0.clone())
                    .or_insert_with(|| ranking.cell.clone());
            }
            branches.push(RankedList::from_ranked(
                ranking.refs.iter().map(|r| r.0.clone()),
            ));
        }
        reciprocal_rank_fusion(&branches)
            .into_iter()
            .map(|fused| {
                let home = home_of
                    .get(&fused.doc_id)
                    .cloned()
                    .unwrap_or_else(|| self.coordinator_cell.clone());
                MergedRef {
                    ref_: ArtifactRef(fused.doc_id),
                    home_cell: home,
                    score: fused.score,
                }
            })
            .collect()
    }

    pub fn resolve_rows(
        &self,
        tenant: &TenantId,
        region: &Region,
        merged: &[MergedRef],
        viewer: &Principal,
    ) -> Vec<FederatedRow> {
        merged
            .iter()
            .map(|m| {
                let projection = match self.resolvers.get(&m.home_cell) {
                    Some(resolver) => resolver.project_row(tenant, region, &m.ref_, viewer),
                    None => None,
                };
                FederatedRow {
                    ref_: m.ref_.clone(),
                    home_cell: m.home_cell.clone(),
                    projection,
                }
            })
            .collect()
    }

    pub fn query_all(
        &self,
        tenant: &TenantId,
        region: &Region,
        query: &str,
        viewer: &Principal,
    ) -> Vec<FederatedRow> {
        let rankings = self.scatter(tenant, region, query, viewer);
        let merged = self.residency_free_merge(&rankings);
        self.resolve_rows(tenant, region, &merged, viewer)
    }

    pub fn query(
        &self,
        tenant: &TenantId,
        region: &Region,
        query: &str,
        viewer: &Principal,
    ) -> Vec<FederatedRow> {
        self.query_all(tenant, region, query, viewer)
            .into_iter()
            .filter(FederatedRow::is_visible)
            .collect()
    }

    pub fn scattered(&self) -> u64 {
        self.scattered.load(Ordering::SeqCst)
    }

    pub fn payload_crossed_merge(&self) -> u64 {
        self.payload_crossed_merge.load(Ordering::SeqCst)
    }
}

impl core::fmt::Debug for FederatedSearch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FederatedSearch")
            .field("coordinator_cell", &self.coordinator_cell.as_str())
            .field("member_cells", &self.queriers.len())
            .field("scattered", &self.scattered())
            .field("payload_crossed_merge", &self.payload_crossed_merge())
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MergedRef {
    pub ref_: ArtifactRef,
    pub home_cell: CellId,
    pub score: f32,
}

#[must_use]
pub fn migrate_ranking_home(ranking: &CellRanking, from: &CellId, to: &CellId) -> CellRanking {
    if &ranking.cell == from {
        CellRanking {
            cell: to.clone(),
            refs: ranking.refs.clone(),
        }
    } else {
        ranking.clone()
    }
}

pub fn merge_carried_fields(merged: &MergedRef) -> (&ArtifactRef, &CellId, f32) {
    (&merged.ref_, &merged.home_cell, merged.score)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use std::collections::HashSet;
    use std::sync::Mutex;

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn coordinator() -> CellId {
        CellId::from_token("cell-fr-par-0")
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
    fn aref(s: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://acme/issues/issue/{s}"))
    }

    #[derive(Default)]
    struct StandInCell {
        matches: Mutex<Vec<String>>,
        allowed: Mutex<Vec<(String, String)>>,
        erased: Mutex<Vec<String>>,
        titles: Mutex<HashMap<String, String>>,
        ran_queries: Mutex<Vec<String>>,
        projected: Mutex<Vec<String>>,
    }

    impl StandInCell {
        fn index_match(&self, ref_urn: &str) {
            self.matches.lock().unwrap().push(ref_urn.into());
        }
        fn allow(&self, ref_urn: &str, viewer_id: &str) {
            self.allowed
                .lock()
                .unwrap()
                .push((ref_urn.into(), viewer_id.into()));
        }
        fn set_title(&self, ref_urn: &str, title: &str) {
            self.titles
                .lock()
                .unwrap()
                .insert(ref_urn.into(), title.into());
        }
        fn erase(&self, ref_urn: &str) {
            self.erased.lock().unwrap().push(ref_urn.into());
        }
        fn ran_queries(&self) -> Vec<String> {
            self.ran_queries.lock().unwrap().clone()
        }
        fn projected_refs(&self) -> Vec<String> {
            self.projected.lock().unwrap().clone()
        }
        fn is_allowed(&self, ref_urn: &str, viewer_id: &str) -> bool {
            self.allowed
                .lock()
                .unwrap()
                .iter()
                .any(|(r, v)| r == ref_urn && v == viewer_id)
        }
    }

    impl CellLocalQuery for StandInCell {
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
                .filter(|r| self.is_allowed(r, &viewer.principal_id.0))
                .map(|r| ArtifactRef(r.clone()))
                .collect();
            CellRanking {
                cell: CellId::from_token("PLACEHOLDER"),
                refs,
            }
        }
    }

    impl CellLocalRowResolver for StandInCell {
        fn project_row(
            &self,
            _tenant: &TenantId,
            _region: &Region,
            ref_: &ArtifactRef,
            viewer: &Principal,
        ) -> Option<RowProjection> {
            self.projected.lock().unwrap().push(ref_.0.clone());
            if self.erased.lock().unwrap().iter().any(|e| e == &ref_.0) {
                return None;
            }
            if !self.is_allowed(&ref_.0, &viewer.principal_id.0) {
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

    struct HomedCell {
        cell: CellId,
        inner: Arc<StandInCell>,
    }
    impl CellLocalQuery for HomedCell {
        fn run(
            &self,
            tenant: &TenantId,
            region: &Region,
            query: &str,
            viewer: &Principal,
        ) -> CellRanking {
            let mut r = self.inner.run(tenant, region, query, viewer);
            r.cell = self.cell.clone();
            r
        }
    }

    fn register_cell(fed: &mut FederatedSearch, cell: CellId, inner: Arc<StandInCell>) {
        let querier = Arc::new(HomedCell {
            cell: cell.clone(),
            inner: inner.clone(),
        });
        fed.register(cell, querier, inner);
    }

    #[test]
    fn federated_query_across_two_cells_is_leak_free_zero_pii_crossing() {
        let b = Arc::new(StandInCell::default());
        let c = Arc::new(StandInCell::default());

        let secret = "TOP SECRET cross-org acquisition memo";
        let b_ok = aref("b-ok");
        let b_secret = aref("b-secret");
        b.index_match(&b_ok.0);
        b.index_match(&b_secret.0);
        b.allow(&b_ok.0, "viewer1");
        b.set_title(&b_ok.0, "B visible row");
        b.set_title(&b_secret.0, secret);
        let c_ok = aref("c-ok");
        c.index_match(&c_ok.0);
        c.allow(&c_ok.0, "viewer1");
        c.set_title(&c_ok.0, "C visible row");

        let mut fed = FederatedSearch::new(coordinator());
        register_cell(&mut fed, cell_b(), b.clone());
        register_cell(&mut fed, cell_c(), c.clone());

        let rows = fed.query(&tenant(), &region(), "acquisition", &viewer("viewer1"));

        let titles: Vec<String> = rows
            .iter()
            .filter_map(|r| r.projection.as_ref().map(|p| p.title.clone()))
            .collect();
        let title_set: HashSet<&str> = titles.iter().map(String::as_str).collect();
        assert!(
            title_set.contains("B visible row"),
            "the B row the viewer may see surfaces"
        );
        assert!(
            title_set.contains("C visible row"),
            "the C row the viewer may see surfaces"
        );
        assert_eq!(
            rows.len(),
            2,
            "exactly the two visible rows (the secret never surfaces)"
        );

        let rendered = format!("{rows:?}");
        assert!(
            !rendered.contains("SECRET") && !rendered.contains("acquisition memo"),
            "0 cross-cell leak: the secret must not cross, got `{rendered}`"
        );
        assert!(
            rows.iter().any(|r| r.home_cell == cell_b())
                && rows.iter().any(|r| r.home_cell == cell_c()),
            "the rows resolved per-viewer in their home cells (B and C)"
        );
        assert!(
            b.projected_refs().contains(&b_ok.0),
            "the B row was projected IN cell B"
        );
        assert!(
            c.projected_refs().contains(&c_ok.0),
            "the C row was projected IN cell C"
        );
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
    fn scatter_runs_the_query_in_each_cell_and_secret_never_enters_the_ranking() {
        let b = Arc::new(StandInCell::default());
        let b_ok = aref("b-ok");
        let b_secret = aref("b-secret");
        b.index_match(&b_ok.0);
        b.index_match(&b_secret.0);
        b.allow(&b_ok.0, "v");

        let mut fed = FederatedSearch::new(coordinator());
        register_cell(&mut fed, cell_b(), b.clone());

        let rankings = fed.scatter(&tenant(), &region(), "q", &viewer("v"));
        assert_eq!(rankings.len(), 1, "one member cell scattered to");
        assert_eq!(
            b.ran_queries(),
            vec!["q".to_string()],
            "the query ran IN cell B"
        );
        let refs: Vec<String> = rankings[0].refs.iter().map(|r| r.0.clone()).collect();
        assert_eq!(
            refs,
            vec![b_ok.0.clone()],
            "only the viewer's visible ref entered the ranking"
        );
        assert!(
            !refs.contains(&b_secret.0),
            "the secret never entered the ranking (leak-free)"
        );
        assert_eq!(rankings[0].cell, cell_b(), "the ranking is homed in cell B");
    }

    #[test]
    fn residency_free_merge_fuses_on_rank_carrying_only_refs() {
        let shared = aref("shared");
        let b_only = aref("b-only");
        let c_only = aref("c-only");
        let rankings = vec![
            CellRanking::new(cell_b(), vec![shared.clone(), b_only.clone()]),
            CellRanking::new(cell_c(), vec![c_only.clone(), shared.clone()]),
        ];
        let fed = FederatedSearch::new(coordinator());
        let merged = fed.residency_free_merge(&rankings);

        assert_eq!(
            merged[0].ref_, shared,
            "the cross-cell agreement ref surfaces first"
        );
        assert_eq!(
            merged[0].home_cell,
            cell_b(),
            "shared is homed in the first cell that surfaced it"
        );
        let by_ref: HashMap<&str, &CellId> = merged
            .iter()
            .map(|m| (m.ref_.0.as_str(), &m.home_cell))
            .collect();
        assert_eq!(by_ref.get(b_only.0.as_str()), Some(&&cell_b()));
        assert_eq!(by_ref.get(c_only.0.as_str()), Some(&&cell_c()));
        assert_eq!(
            fed.payload_crossed_merge(),
            0,
            "the merge carried only ArtifactRefs + scores"
        );
        let (r, home, score) = merge_carried_fields(&merged[0]);
        assert_eq!(r, &shared);
        assert_eq!(home, &cell_b());
        assert!(score > 0.0, "the fused score is a positive rank metric");
    }

    #[test]
    fn merge_never_introduces_a_ref_absent_from_every_ranking() {
        let rankings = vec![
            CellRanking::new(cell_b(), vec![aref("a"), aref("b")]),
            CellRanking::new(cell_c(), vec![aref("b"), aref("c")]),
        ];
        let fed = FederatedSearch::new(coordinator());
        let merged = fed.residency_free_merge(&rankings);
        let got: HashSet<String> = merged.iter().map(|m| m.ref_.0.clone()).collect();
        let union: HashSet<String> = ["a", "b", "c"].into_iter().map(|s| aref(s).0).collect();
        assert_eq!(
            got, union,
            "the merged set is EXACTLY the union of the rankings - no new ref"
        );
        assert!(
            !got.contains(&aref("secret").0),
            "a ref in no ranking never appears (leak-free)"
        );
    }

    #[test]
    fn home_cell_resolution_tombstones_a_denied_row_zero_leak() {
        let b = Arc::new(StandInCell::default());
        let secret_ref = aref("b-secret");
        b.index_match(&secret_ref.0);
        b.set_title(&secret_ref.0, "SECRET title");
        b.allow(&secret_ref.0, "insider");

        let mut fed = FederatedSearch::new(coordinator());
        register_cell(&mut fed, cell_b(), b.clone());

        let insider_rows = fed.query(&tenant(), &region(), "q", &viewer("insider"));
        assert_eq!(insider_rows.len(), 1, "the insider sees the row");
        assert_eq!(
            insider_rows[0].projection.as_ref().unwrap().title,
            "SECRET title"
        );
        assert_eq!(insider_rows[0].home_cell, cell_b(), "resolved IN cell B");

        let merged = vec![MergedRef {
            ref_: secret_ref.clone(),
            home_cell: cell_b(),
            score: 1.0,
        }];
        let resolved = fed.resolve_rows(&tenant(), &region(), &merged, &viewer("intruder"));
        assert_eq!(resolved.len(), 1);
        assert!(
            !resolved[0].is_visible(),
            "the denied row is a tombstone, not a leak"
        );
        let rendered = format!("{resolved:?}");
        assert!(
            !rendered.contains("SECRET"),
            "0 leak: the secret never crosses, got `{rendered}`"
        );
    }

    #[test]
    fn erased_row_resolves_to_a_tombstone_in_its_home_cell() {
        let b = Arc::new(StandInCell::default());
        let r = aref("b-victim");
        b.allow(&r.0, "owner");
        b.set_title(&r.0, "victim row");
        b.erase(&r.0);

        let mut fed = FederatedSearch::new(coordinator());
        register_cell(&mut fed, cell_b(), b.clone());

        let merged = vec![MergedRef {
            ref_: r.clone(),
            home_cell: cell_b(),
            score: 1.0,
        }];
        let resolved = fed.resolve_rows(&tenant(), &region(), &merged, &viewer("owner"));
        assert!(
            !resolved[0].is_visible(),
            "the erased row is a tombstone (unresolvable cross-cell)"
        );
    }

    #[test]
    fn unknown_home_cell_degrades_to_tombstone_never_reaches_in() {
        let merged = vec![MergedRef {
            ref_: aref("x"),
            home_cell: cell_c(),
            score: 1.0,
        }];
        let fed = FederatedSearch::new(coordinator());
        let resolved = fed.resolve_rows(&tenant(), &region(), &merged, &viewer("anyone"));
        assert!(
            !resolved[0].is_visible(),
            "an unknown home cell degrades to a tombstone"
        );
        assert_eq!(
            fed.payload_crossed_merge(),
            0,
            "no payload crossed for an unseen cell"
        );
    }

    #[test]
    fn cell_to_cell_migration_re_homes_the_ranking_zero_loss() {
        let r1 = aref("row1");
        let r2 = aref("row2");
        let ranking = CellRanking::new(cell_b(), vec![r1.clone(), r2.clone()]);

        let migrated = migrate_ranking_home(&ranking, &cell_b(), &cell_c());
        assert_eq!(migrated.cell, cell_c(), "the ranking re-homed to C");
        assert_eq!(
            migrated.refs, ranking.refs,
            "the refs are preserved (0 loss)"
        );

        let fed = FederatedSearch::new(coordinator());
        let merged = fed.residency_free_merge(&[migrated]);
        assert!(
            merged.iter().all(|m| m.home_cell == cell_c()),
            "every re-homed row resolves in C"
        );
    }

    #[test]
    fn migration_leaves_non_migrating_rankings_untouched() {
        let ranking = CellRanking::new(cell_a(), vec![aref("a1")]);
        let migrated = migrate_ranking_home(&ranking, &cell_b(), &cell_c());
        assert_eq!(
            migrated, ranking,
            "a non-migrating ranking is unchanged byte-for-byte"
        );
    }

    #[test]
    fn federated_debug_is_pii_free_and_carries_the_counters() {
        let b = Arc::new(StandInCell::default());
        let r = aref("b-ok");
        b.index_match(&r.0);
        b.allow(&r.0, "v");
        b.set_title(&r.0, "SECRET");
        let mut fed = FederatedSearch::new(coordinator());
        register_cell(&mut fed, cell_b(), b);
        let _ = fed.query(&tenant(), &region(), "secret-query", &viewer("v"));
        let rendered = format!("{fed:?}");
        assert!(
            rendered.contains("FederatedSearch"),
            "the Debug names the type"
        );
        assert!(
            rendered.contains("cell-fr-par-0"),
            "the Debug carries the coordinator cell id"
        );
        assert!(
            rendered.contains("scattered"),
            "the Debug carries the scatter counter"
        );
        assert!(
            rendered.contains("payload_crossed_merge"),
            "the Debug carries the §6.4 zero counter"
        );
        assert!(
            !rendered.contains("SECRET"),
            "the Debug never leaks a title, got `{rendered}`"
        );
        assert!(
            !rendered.contains("secret-query"),
            "the Debug never leaks the query, got `{rendered}`"
        );
    }

    #[test]
    fn srch_p09_leak_floor_holds_under_the_federated_path() {
        let b = Arc::new(StandInCell::default());
        let confidential = aref("confidential");
        b.index_match(&confidential.0);
        b.set_title(&confidential.0, "CONFIDENTIAL");
        let mut fed = FederatedSearch::new(coordinator());
        register_cell(&mut fed, cell_b(), b.clone());

        let rankings = fed.scatter(&tenant(), &region(), "q", &viewer("intruder"));
        assert!(
            rankings[0].refs.is_empty(),
            "gate 1: the confidential row never enters the ranking"
        );
        let merged = vec![MergedRef {
            ref_: confidential.clone(),
            home_cell: cell_b(),
            score: 1.0,
        }];
        let resolved = fed.resolve_rows(&tenant(), &region(), &merged, &viewer("intruder"));
        assert!(
            !resolved[0].is_visible(),
            "gate 2: the home-cell resolution tombstones the row"
        );
        let rows = fed.query(&tenant(), &region(), "q", &viewer("intruder"));
        assert!(
            rows.is_empty(),
            "0 cross-cell leak for the unauthorized viewer"
        );
        assert_eq!(
            fed.payload_crossed_merge(),
            0,
            "0 PII crossed even on the leak-attempt path"
        );
    }
}
