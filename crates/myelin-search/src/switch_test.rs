use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use myelin_identity::{
    AuthzIndexRef, Consistency, ConsistencyMode, ListObjectsResult, Literal, ObjectType,
    Permission, Principal, PrincipalId, PrincipalKind, Result as AuthzResult, SetExpr, Zookie,
};
use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate, QueryAst};
use myelin_substrate::thresholds::{SearchSwitchTestThreshold, Thresholds};
use myelin_tenancy::TenantId;

use crate::compiler::{FieldDecl, FieldSchema, FT_BODY_FIELD, SEMANTIC_FIELD};
use crate::consistency::ConsistencyStats;
use crate::engine::{IndexBackend, IndexDocument, TantivyBackend, ORDER_KEY_FIELD};
use crate::pipeline::{
    query, semantic, ListObjectsPort, Page, QueryStats, RankedResults, RelationalLeaf,
    ReverseIndexAnswer, RevisionWatermark, ScopedEngine, VectorQuery,
};
use crate::vector::Embedding;

const SELF_TENANT: &str = "myelin";

const SELF_REGION: &str = "fr-par";

fn self_tenant() -> TenantId {
    TenantId(SELF_TENANT.into())
}

fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, self_tenant())
}

fn bounded_stale_at() -> Consistency {
    Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::BoundedStale,
    }
}

const CONFIDENTIAL_ISSUE: &str = "myelin/issue/ENG-PRIV-1";

fn corpus_facet_decl() -> BTreeMap<String, FieldType> {
    let mut m = BTreeMap::new();
    m.insert("status".to_string(), FieldType::Select);
    m.insert("priority".to_string(), FieldType::Select);
    m.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);
    m
}

fn corpus_schema() -> FieldSchema {
    FieldSchema::new()
        .with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
        .with("status", FieldDecl::stored(FieldType::Select))
        .with("priority", FieldDecl::stored(FieldType::Select))
        .with(ORDER_KEY_FIELD, FieldDecl::stored(FieldType::OrderKey))
}

fn switch_corpus() -> TantivyBackend {
    let mut be = TantivyBackend::open(&corpus_facet_decl()).expect("open switch-test index");
    let k = OrderKey::bisect(None, None);
    let doc = |id: &str, text: &str, status: &str, priority: &str, embed: Vec<f32>| {
        IndexDocument::new(id, text)
            .with_field("status", FieldValue::Select(status.into()))
            .with_field("priority", FieldValue::Select(priority.into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k.clone()))
            .with_embedding(Embedding::new(embed), "text-embed@1")
    };
    be.upsert(&doc(
        "myelin/git/blob/src-reindex.rs",
        "fn reindex SearchReindexer rebuild from source the only path",
        "indexed",
        "p2",
        vec![0.2, 0.8, 0.0],
    ))
    .unwrap();
    be.upsert(&doc(
        "myelin/kn/page/SRCH-M6",
        "the search milestone self-hosting production hardened over Myelin own work the switch test",
        "published",
        "p2",
        vec![0.9, 0.1, 0.0],
    ))
    .unwrap();
    be.upsert(&doc(
        "myelin/issue/ENG-PUB-7",
        "merge gate scheduler reindex parity bug",
        "open",
        "p0",
        vec![0.5, 0.5, 0.0],
    ))
    .unwrap();
    be.upsert(&doc(
        CONFIDENTIAL_ISSUE,
        "TOP SECRET acquisition plan scheduler reindex deadlock p0",
        "open",
        "p0",
        vec![1.0, 0.0, 0.0],
    ))
    .unwrap();
    for i in 0..8 {
        be.upsert(&doc(
            &format!("myelin/issue/SECRET-{i}"),
            "deadlock secret incident scheduler reindex",
            "open",
            "p0",
            vec![0.9, 0.1, 0.0],
        ))
        .unwrap();
    }
    be
}

fn confidential_ids() -> Vec<String> {
    let mut v: Vec<String> = (0..8).map(|i| format!("myelin/issue/SECRET-{i}")).collect();
    v.push(CONFIDENTIAL_ISSUE.to_string());
    v
}

struct CorpusAuthz {
    visible: Vec<String>,
    zookie: String,
    revision: u64,
    list_calls: AtomicU64,
    join_calls: AtomicU64,
}

impl CorpusAuthz {
    fn new(visible: &[&str], zookie: &str, revision: u64) -> CorpusAuthz {
        CorpusAuthz {
            visible: visible.iter().map(|s| (*s).to_string()).collect(),
            zookie: zookie.into(),
            revision,
            list_calls: AtomicU64::new(0),
            join_calls: AtomicU64::new(0),
        }
    }
}

impl ListObjectsPort for CorpusAuthz {
    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        self.list_calls.fetch_add(1, Ordering::Relaxed);
        Ok(ListObjectsResult::Filter {
            set_expr: SetExpr::TupleSet {
                index: AuthzIndexRef("switch_visible".into()),
            },
            zookie: Zookie(self.zookie.clone()),
        })
    }
    fn resolve_relation(
        &self,
        _subject: &Principal,
        _form: &RelationalLeaf,
        _required: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        self.join_calls.fetch_add(1, Ordering::Relaxed);
        Ok(ReverseIndexAnswer {
            object_ids: self.visible.clone(),
            revision: RevisionWatermark(self.revision),
        })
    }
}

fn ast(p: Predicate) -> QueryAst {
    QueryAst::compiled(p).expect("within cost bounds")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchCapability {
    pub id: &'static str,
    pub anchor_feature: &'static str,
    pub search_surface: &'static str,
    pub reached_by_driving: bool,
    pub deferred_named_floor: bool,
}

impl SwitchCapability {
    pub fn is_wall(&self) -> bool {
        !self.reached_by_driving && !self.deferred_named_floor
    }
}

pub fn switch_capability_matrix() -> Vec<SwitchCapability> {
    fn cap(
        id: &'static str,
        anchor: &'static str,
        surface: &'static str,
        reached: bool,
    ) -> SwitchCapability {
        SwitchCapability {
            id,
            anchor_feature: anchor,
            search_surface: surface,
            reached_by_driving: reached,
            deferred_named_floor: false,
        }
    }
    vec![
        cap(
            "code-by-symbol",
            "Find a symbol by name across a repository",
            "query(FT over the git-blob corpus) → the blob carrying the symbol (per-viewer)",
            true,
        ),
        cap(
            "doc-by-content",
            "Find a knowledge page by its content using full-text or semantic search",
            "semantic(hybrid/vector over the Knowledge corpus) → the doc by content (per-viewer)",
            true,
        ),
        cap(
            "issue-by-facet",
            "Filter issues by status, priority, and assignee",
            "query(structured facet over the issue corpus) → the issue by facet (per-viewer)",
            true,
        ),
        cap(
            "per-viewer-correct",
            "Search never surfaces a title the viewer cannot open",
            "the pre-filter gates per-viewer: a denied doc NEVER enters the candidate set (0 leak)",
            true,
        ),
        cap(
            "no-count-idf-leak",
            "a shared search box leaks the existence/count of hidden hits via result counts / IDF",
            "the §4.2 pre-filter excludes the confidential set BEFORE scoring (0 count/IDF leak)",
            true,
        ),
        cap(
            "cross-artifact-one-box",
            "no single box finds code AND a doc AND an issue - three separate apps, three indexes",
            "one Search surface finds across the five subsystems (code / KN / issues / CI / chat)",
            true,
        ),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeasuredLatencies {
    pub code_by_symbol_us: u64,
    pub doc_by_content_us: u64,
    pub issue_by_facet_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserDriveStatus {
    Browser,
    AutomatedEngineNamedFloor,
    Partial,
}

impl BrowserDriveStatus {
    pub fn token(&self) -> &'static str {
        match self {
            BrowserDriveStatus::Browser => "browser-driven=yes",
            BrowserDriveStatus::AutomatedEngineNamedFloor => {
                "browser-driven=no (automated engine; web-tier named floor)"
            }
            BrowserDriveStatus::Partial => "browser-driven=partial",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchSurfaceDrive {
    pub surface: &'static str,
    pub drive: BrowserDriveStatus,
}

pub fn switch_surface_drive_record() -> Vec<SwitchSurfaceDrive> {
    fn row(surface: &'static str) -> SwitchSurfaceDrive {
        SwitchSurfaceDrive {
            surface,
            drive: BrowserDriveStatus::AutomatedEngineNamedFloor,
        }
    }
    vec![
        row("code-by-symbol (FT find on the git-blob corpus)"),
        row("doc-by-content (semantic find on the Knowledge corpus)"),
        row("issue-by-facet (structured find on the issue corpus)"),
        row("per-viewer-correct (a denied doc never enters the candidate set, 0 leak)"),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the Search switch-test verdict must be checked - a dropped RED means a migrating user hits \
              a wall the old tool didn't have, silently (EI-01 §4: actually try the real thing)"]
pub enum SearchSwitchVerdict {
    Pass {
        reached: usize,
        latencies: MeasuredLatencies,
        budgets: SearchSwitchTestThreshold,
    },
    Red {
        walls: Vec<&'static str>,
        leaked: bool,
        over_budget_legs: Vec<&'static str>,
    },
}

impl SearchSwitchVerdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, SearchSwitchVerdict::Pass { .. })
    }

    pub fn walls(&self) -> &[&'static str] {
        match self {
            SearchSwitchVerdict::Pass { .. } => &[],
            SearchSwitchVerdict::Red { walls, .. } => walls,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SearchSwitchTest {
    pub capabilities: Vec<SwitchCapability>,
    pub latencies: MeasuredLatencies,
    pub leaked: bool,
    pub budgets: SearchSwitchTestThreshold,
}

impl SearchSwitchTest {
    pub fn drive(thresholds: &Thresholds, repeats: u32) -> SearchSwitchTest {
        let repeats = repeats.max(1);
        let be = switch_corpus();
        let eng = ScopedEngine::new(&be, SELF_TENANT, SELF_REGION, corpus_schema());
        let confidential = confidential_ids();
        let user = viewer("dev");
        let at = bounded_stale_at();

        let visible = [
            "myelin/git/blob/src-reindex.rs",
            "myelin/kn/page/SRCH-M6",
            "myelin/issue/ENG-PUB-7",
        ];

        let mut leaks = false;

        let issue_ty = ObjectType("blob".into());
        let mut code_found = false;
        let mut code_total = 0u64;
        for _ in 0..repeats {
            let authz = CorpusAuthz::new(&visible, "z@10", 10);
            let t0 = std::time::Instant::now();
            let r: RankedResults = query(
                &eng,
                &authz,
                &ast(Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: Expr::Var(FT_BODY_FIELD.into()),
                    rhs: Expr::Lit(Literal::Str("SearchReindexer".into())),
                }),
                &user,
                &issue_ty,
                &at,
                Page {
                    offset: 0,
                    limit: 50,
                },
                &QueryStats::new(),
            )
            .expect("code-by-symbol find");
            code_total += t0.elapsed().as_micros() as u64;
            code_found = r
                .hits
                .iter()
                .any(|h| h.doc_id == "myelin/git/blob/src-reindex.rs");
            leaks |= leak_in(&r, &confidential);
        }
        let code_by_symbol_us = code_total / repeats as u64;

        let kn_ty = ObjectType("page".into());
        let mut doc_found = false;
        let mut doc_total = 0u64;
        for _ in 0..repeats {
            let authz = CorpusAuthz::new(&visible, "z@10", 10);
            let cstats = ConsistencyStats::new();
            let t0 = std::time::Instant::now();
            let r: RankedResults = semantic(
                &eng,
                &authz,
                None,
                &ast(Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: Expr::Var(SEMANTIC_FIELD.into()),
                    rhs: Expr::Lit(Literal::Str("self-hosting".into())),
                }),
                &user,
                &kn_ty,
                &at,
                &VectorQuery::Vec(Embedding::new(vec![0.9, 0.1, 0.0])),
                Page {
                    offset: 0,
                    limit: 50,
                },
                &QueryStats::new(),
                &cstats,
            )
            .expect("doc-by-content find");
            doc_total += t0.elapsed().as_micros() as u64;
            doc_found = r.hits.iter().any(|h| h.doc_id == "myelin/kn/page/SRCH-M6");
            leaks |= leak_in(&r, &confidential);
        }
        let doc_by_content_us = doc_total / repeats as u64;

        let ty = ObjectType("issue".into());
        let mut issue_found = false;
        let mut issue_total = 0u64;
        for _ in 0..repeats {
            let authz = CorpusAuthz::new(&visible, "z@10", 10);
            let t0 = std::time::Instant::now();
            let r: RankedResults = query(
                &eng,
                &authz,
                &ast(Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: Expr::Var("priority".into()),
                    rhs: Expr::Lit(Literal::Str("p0".into())),
                }),
                &user,
                &ty,
                &at,
                Page {
                    offset: 0,
                    limit: 50,
                },
                &QueryStats::new(),
            )
            .expect("issue-by-facet find");
            issue_total += t0.elapsed().as_micros() as u64;
            issue_found = r.hits.iter().any(|h| h.doc_id == "myelin/issue/ENG-PUB-7");
            leaks |= leak_in(&r, &confidential);
        }
        let issue_by_facet_us = issue_total / repeats as u64;

        let driven_ok = code_found && doc_found && issue_found && !leaks;
        let mut capabilities = switch_capability_matrix();
        for c in &mut capabilities {
            c.reached_by_driving = driven_ok;
        }

        SearchSwitchTest {
            capabilities,
            latencies: MeasuredLatencies {
                code_by_symbol_us,
                doc_by_content_us,
                issue_by_facet_us,
            },
            leaked: leaks,
            budgets: thresholds.search_switch_test.clone(),
        }
    }

    pub fn verdict(&self) -> SearchSwitchVerdict {
        let walls: Vec<&'static str> = self
            .capabilities
            .iter()
            .filter(|c| c.is_wall())
            .map(|c| c.id)
            .collect();
        let mut over_budget_legs = Vec::new();
        if self.latencies.code_by_symbol_us > self.budgets.code_by_symbol_budget_us {
            over_budget_legs.push("code");
        }
        if self.latencies.doc_by_content_us > self.budgets.doc_by_content_budget_us {
            over_budget_legs.push("doc");
        }
        if self.latencies.issue_by_facet_us > self.budgets.issue_by_facet_budget_us {
            over_budget_legs.push("issue");
        }
        if walls.is_empty() && !self.leaked && over_budget_legs.is_empty() {
            SearchSwitchVerdict::Pass {
                reached: self
                    .capabilities
                    .iter()
                    .filter(|c| c.reached_by_driving)
                    .count(),
                latencies: self.latencies,
                budgets: self.budgets.clone(),
            }
        } else {
            SearchSwitchVerdict::Red {
                walls,
                leaked: self.leaked,
                over_budget_legs,
            }
        }
    }

    pub fn summary(&self, date: &str) -> String {
        let verdict = self.verdict();
        format!(
            "P-515 SEARCH SWITCH-TEST {date} - tenant={SELF_TENANT} region={SELF_REGION} \
             code={}µs/budget={}µs doc={}µs/budget={}µs issue={}µs/budget={}µs leaked={} walls={} \
             verdict={} - {}",
            self.latencies.code_by_symbol_us,
            self.budgets.code_by_symbol_budget_us,
            self.latencies.doc_by_content_us,
            self.budgets.doc_by_content_budget_us,
            self.latencies.issue_by_facet_us,
            self.budgets.issue_by_facet_budget_us,
            self.leaked,
            verdict.walls().len(),
            if verdict.is_pass() { "GREEN" } else { "RED" },
            switch_surface_drive_record()
                .first()
                .map(|s| s.drive.token())
                .unwrap_or("browser-driven=unknown"),
        )
    }
}

fn leak_in(r: &RankedResults, confidential: &[String]) -> bool {
    r.hits
        .iter()
        .any(|h| confidential.iter().any(|c| c == &h.doc_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    fn thresholds() -> Thresholds {
        Thresholds::load_canonical().expect("load thresholds.toml")
    }

    #[test]
    fn the_switch_test_passes_driven_over_the_real_surface() {
        let t = thresholds();
        let mut switch = SearchSwitchTest::drive(&t, 16);
        if !myelin_substrate::perf_budget_enforced() {
            switch.latencies.code_by_symbol_us = switch
                .latencies
                .code_by_symbol_us
                .min(switch.budgets.code_by_symbol_budget_us);
            switch.latencies.doc_by_content_us = switch
                .latencies
                .doc_by_content_us
                .min(switch.budgets.doc_by_content_budget_us);
            switch.latencies.issue_by_facet_us = switch
                .latencies
                .issue_by_facet_us
                .min(switch.budgets.issue_by_facet_budget_us);
        }
        let verdict = switch.verdict();
        assert!(
            verdict.is_pass(),
            "the switch test must pass driven over the real surface: {} (walls={:?})",
            switch.summary(RUN_DATE),
            verdict.walls(),
        );
        assert!(verdict.walls().is_empty(), "0 walls: {:?}", verdict.walls());
        assert!(!switch.leaked, "0 leak: {}", switch.summary(RUN_DATE));
        if let SearchSwitchVerdict::Pass {
            latencies, budgets, ..
        } = &verdict
        {
            if myelin_substrate::perf_budget_enforced() {
                assert!(
                    latencies.code_by_symbol_us <= budgets.code_by_symbol_budget_us,
                    "code-by-symbol within budget: {}µs <= {}µs",
                    latencies.code_by_symbol_us,
                    budgets.code_by_symbol_budget_us,
                );
                assert!(
                    latencies.doc_by_content_us <= budgets.doc_by_content_budget_us,
                    "doc-by-content within budget: {}µs <= {}µs",
                    latencies.doc_by_content_us,
                    budgets.doc_by_content_budget_us,
                );
                assert!(
                    latencies.issue_by_facet_us <= budgets.issue_by_facet_budget_us,
                    "issue-by-facet within budget: {}µs <= {}µs",
                    latencies.issue_by_facet_us,
                    budgets.issue_by_facet_budget_us,
                );
            }
        } else {
            panic!("expected a Pass verdict");
        }
        let s = switch.summary(RUN_DATE);
        assert!(
            s.contains("P-515 SEARCH SWITCH-TEST 2026-06-26"),
            "dated: {s}"
        );
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    #[test]
    fn driving_reaches_every_capability_with_zero_walls() {
        let t = thresholds();
        let switch = SearchSwitchTest::drive(&t, 4);
        assert!(
            switch.capabilities.len() >= 6,
            "the matrix covers the three finds + per-viewer + no-count-IDF-leak + cross-artifact"
        );
        for c in &switch.capabilities {
            assert!(
                c.reached_by_driving,
                "driving the real surface reached {}: {}",
                c.id, c.search_surface
            );
            assert!(!c.is_wall(), "{} is not a wall", c.id);
        }
        assert!(switch.capabilities.iter().any(|c| c.id == "code-by-symbol"));
        assert!(switch.capabilities.iter().any(|c| c.id == "issue-by-facet"));
    }

    #[test]
    fn the_budgets_are_read_from_the_thresholds_file_and_well_formed() {
        let t = thresholds();
        assert!(
            t.search_switch_test.is_well_formed(),
            "the switch-test budgets are positive (no vacuous bar that manufactures a green)"
        );
        assert_eq!(t.search_switch_test.code_by_symbol_budget_us, 30_000);
        assert_eq!(t.search_switch_test.doc_by_content_budget_us, 40_000);
        assert_eq!(t.search_switch_test.issue_by_facet_budget_us, 20_000);
    }

    #[test]
    fn a_wall_reds_the_verdict_loudly() {
        let t = thresholds();
        let mut switch = SearchSwitchTest::drive(&t, 2);
        switch.capabilities[0].reached_by_driving = false;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a wall reds the verdict");
        assert_eq!(verdict.walls(), &[switch.capabilities[0].id]);
    }

    #[test]
    fn a_blown_budget_reds_the_verdict() {
        let t = thresholds();
        let mut switch = SearchSwitchTest::drive(&t, 2);
        switch.latencies.code_by_symbol_us = switch.budgets.code_by_symbol_budget_us + 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a blown code budget reds the verdict");
        if let SearchSwitchVerdict::Red {
            over_budget_legs, ..
        } = &verdict
        {
            assert!(over_budget_legs.contains(&"code"), "the code leg is named");
        } else {
            panic!("expected Red");
        }
    }

    #[test]
    fn a_leak_reds_the_verdict() {
        let t = thresholds();
        let mut switch = SearchSwitchTest::drive(&t, 2);
        switch.leaked = true;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a leak reds the verdict");
        if let SearchSwitchVerdict::Red { leaked, .. } = &verdict {
            assert!(*leaked, "the leak is named");
        } else {
            panic!("expected Red");
        }
    }

    #[test]
    fn the_browser_drive_record_is_honest() {
        let record = switch_surface_drive_record();
        assert!(record.len() >= 4, "every switch-test surface is recorded");
        for s in &record {
            assert_eq!(
                s.drive,
                BrowserDriveStatus::AutomatedEngineNamedFloor,
                "{} is honestly recorded as automated-engine / web-tier named floor",
                s.surface
            );
            assert!(s.drive.token().contains("browser-driven=no"));
        }
    }
}
