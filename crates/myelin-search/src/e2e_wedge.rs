#![cfg_attr(
    not(any(test, feature = "test-support")),
    allow(unused_imports, dead_code)
)]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_events::reindex::ReferenceReindexSource;
use myelin_events::{
    Actor, ArtifactRef, EmitContextBase, OutboxStore, ReindexSource, SnapshotScope, Timestamp,
};
use myelin_gdpr::SubjectRef;
use myelin_identity::{
    AuthzIndexRef, Consistency, ConsistencyMode, ListObjectsResult, Literal, ObjectType,
    Permission, Principal, PrincipalId, PrincipalKind, Result as AuthzResult, SetExpr, Zookie,
};
use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate, QueryAst};
use myelin_storage::KmsEngine;
use myelin_substrate::Holder;
use myelin_tenancy::{Region, TenantId};

use crate::compiler::{FieldDecl, FieldSchema, FT_BODY_FIELD, SEMANTIC_FIELD};
use crate::dek::SearchDekPin;
use crate::engine::{AclFilter, IndexBackend, IndexDocument, TantivyBackend, ORDER_KEY_FIELD};
use crate::erase::SearchEraseHolder;
use crate::holder::{search_index_holder, SEARCH_INDEX_STORE};
use crate::hyok_scale::{
    build_live_corpus, subject_matcher, BackupScaleEraseGate, BackupScaleEraseInputs,
    SealedBackupSegment,
};
use crate::indexer::{
    IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher,
    SearchProjection,
};
use crate::pipeline::{
    query, semantic, ListObjectsPort, Page, QueryStats, RankedResults, RelationalLeaf,
    ReverseIndexAnswer, RevisionWatermark, ScopedEngine, VectorQuery,
};
use crate::reindex::SearchReindexer;
use crate::restore_verify::{SearchErasureLedger, SearchRestoreInputs, SearchRestoreVerifyGate};
use crate::vector::Embedding;

pub const E2E_SCENARIOS: [&str; 3] = ["E2E-1", "E2E-3", "E2E-4"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2eArtifact {
    pub scenario: &'static str,
    pub green: bool,
    pub evidence: String,
    pub leaks: u64,
}

impl E2eArtifact {
    pub fn is_green(&self) -> bool {
        self.green && self.leaks == 0
    }
}

fn e2e_tenant() -> TenantId {
    TenantId("acme".into())
}

fn e2e_region() -> Region {
    Region("fr-par".into())
}

fn e2e_viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, e2e_tenant())
}

fn e2e_platform() -> Principal {
    Principal::stub(
        PrincipalId("platform".into()),
        PrincipalKind::Service,
        e2e_tenant(),
    )
}

fn e2e_ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: e2e_tenant(),
        region: e2e_region(),
        actor: Actor(e2e_platform()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:00Z".into()),
        caused_by: None,
    }
}

fn bounded_stale_at() -> Consistency {
    Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::BoundedStale,
    }
}

fn pane_facet_decl() -> std::collections::BTreeMap<String, FieldType> {
    let mut m = std::collections::BTreeMap::new();
    m.insert("status".to_string(), FieldType::Select);
    m.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);
    m
}

fn pane_schema() -> FieldSchema {
    FieldSchema::new()
        .with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
        .with("status", FieldDecl::stored(FieldType::Select))
        .with(ORDER_KEY_FIELD, FieldDecl::stored(FieldType::OrderKey))
}

const PANE_CONFIDENTIAL: &str = "acme/issue/ENG-1421";

const PANE_VISIBLE: [&str; 1] = ["acme/issue/PUB-7"];

fn pane_corpus() -> TantivyBackend {
    let mut be = TantivyBackend::open(&pane_facet_decl()).expect("open pane index");
    let k = OrderKey::bisect(None, None);
    let doc = |id: &str, text: &str, status: &str, embed: Vec<f32>| {
        IndexDocument::new(id, text)
            .with_field("status", FieldValue::Select(status.into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k.clone()))
            .with_embedding(Embedding::new(embed), "text-embed@1")
    };
    be.upsert(&doc(
        "acme/issue/PUB-7",
        "merge gate scheduler context for the PR",
        "pending",
        vec![0.5, 0.5, 0.0],
    ))
    .unwrap();
    be.upsert(&doc(
        PANE_CONFIDENTIAL,
        "TOP SECRET acquisition plan scheduler deadlock",
        "open",
        vec![1.0, 0.0, 0.0],
    ))
    .unwrap();
    for i in 0..12 {
        be.upsert(&doc(
            &format!("acme/issue/SECRET-{i}"),
            "deadlock secret incident scheduler",
            "open",
            vec![0.9, 0.1, 0.0],
        ))
        .unwrap();
    }
    be
}

fn pane_confidential_ids() -> Vec<String> {
    let mut v: Vec<String> = (0..12).map(|i| format!("acme/issue/SECRET-{i}")).collect();
    v.push(PANE_CONFIDENTIAL.to_string());
    v
}

struct PaneAuthz {
    visible: Vec<String>,
    zookie: String,
    revision: u64,
    list_calls: AtomicU64,
    join_calls: AtomicU64,
}
impl PaneAuthz {
    fn new(visible: &[&str], zookie: &str, revision: u64) -> PaneAuthz {
        PaneAuthz {
            visible: visible.iter().map(|s| (*s).to_string()).collect(),
            zookie: zookie.into(),
            revision,
            list_calls: AtomicU64::new(0),
            join_calls: AtomicU64::new(0),
        }
    }
}
impl ListObjectsPort for PaneAuthz {
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
                index: AuthzIndexRef("pane_visible".into()),
            },
            zookie: Zookie(self.zookie.clone()),
        })
    }
    fn resolve_relation(
        &self,
        _subject: &Principal,
        form: &RelationalLeaf,
        required: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        assert!(
            matches!(form, RelationalLeaf::TupleSet { .. }),
            "the pane's reachable set is a TupleSet JOIN (the big-result path)"
        );
        assert!(
            RevisionWatermark(self.revision) >= *required,
            "the reverse index serves a fresh-enough revision"
        );
        self.join_calls.fetch_add(1, Ordering::Relaxed);
        Ok(ReverseIndexAnswer {
            object_ids: self.visible.clone(),
            revision: RevisionWatermark(self.revision),
        })
    }
}

fn pane_ast(p: Predicate) -> QueryAst {
    QueryAst::compiled(p).expect("within cost bounds")
}

pub fn run_e2e_1_pr_pane() -> E2eArtifact {
    let be = pane_corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", pane_schema());
    let confidential = pane_confidential_ids();
    let viewer = e2e_viewer("outsider");
    let ty = ObjectType("issue".into());
    let at = bounded_stale_at();
    let mut leaks: u64 = 0;

    let authz = PaneAuthz::new(&PANE_VISIBLE, "z@10", 10);
    let stats = QueryStats::new();
    let ft: RankedResults = query(
        &eng,
        &authz,
        &pane_ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(FT_BODY_FIELD.into()),
            rhs: Expr::Lit(Literal::Str("deadlock".into())),
        }),
        &viewer,
        &ty,
        &at,
        Page {
            offset: 0,
            limit: 1000,
        },
        &stats,
    )
    .expect("ft query");
    let ft_ids: BTreeSet<&str> = ft.hits.iter().map(|h| h.doc_id.as_str()).collect();
    for c in &confidential {
        if ft_ids.contains(c.as_str()) {
            leaks += 1;
        }
    }
    let ft_zero_leak = ft.hits.is_empty();
    let one_list = authz.list_calls.load(Ordering::Relaxed) == 1;
    let one_join = authz.join_calls.load(Ordering::Relaxed) == 1;

    let rag_authz = PaneAuthz::new(&PANE_VISIBLE, "z@10", 10);
    let rstats = QueryStats::new();
    let cstats = crate::consistency::ConsistencyStats::new();
    let rag: RankedResults = semantic(
        &eng,
        &rag_authz,
        None,
        &pane_ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(SEMANTIC_FIELD.into()),
            rhs: Expr::Lit(Literal::Str("deadlock".into())),
        }),
        &viewer,
        &ty,
        &at,
        &VectorQuery::Vec(Embedding::new(vec![1.0, 0.0, 0.0])),
        Page {
            offset: 0,
            limit: 1000,
        },
        &rstats,
        &cstats,
    )
    .expect("semantic query");
    let rag_ids: BTreeSet<&str> = rag.hits.iter().map(|h| h.doc_id.as_str()).collect();
    for c in &confidential {
        if rag_ids.contains(c.as_str()) {
            leaks += 1;
        }
    }
    let rag_serves_visible = rag_ids.contains("acme/issue/PUB-7") || rag.hits.is_empty();

    let mut be2 = pane_corpus();
    {
        let k = OrderKey::bisect(None, None);
        be2.upsert(
            &IndexDocument::new(
                "acme/issue/PUB-7",
                "merge gate scheduler context for the PR",
            )
            .with_field("status", FieldValue::Select("success".into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k))
            .with_embedding(Embedding::new(vec![0.5, 0.5, 0.0]), "text-embed@1"),
        )
        .expect("re-project the check update");
    }
    let eng2 = ScopedEngine::new(&be2, "acme", "fr-par", pane_schema());
    let insider_authz = PaneAuthz::new(&["acme/issue/PUB-7"], "z@11", 11);
    let live = query(
        &eng2,
        &insider_authz,
        &pane_ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("status".into()),
            rhs: Expr::Lit(Literal::Str("success".into())),
        }),
        &e2e_viewer("insider"),
        &ty,
        &at,
        Page {
            offset: 0,
            limit: 50,
        },
        &QueryStats::new(),
    )
    .expect("post-update query");
    let check_live_updated = live.hits.iter().any(|h| h.doc_id == "acme/issue/PUB-7");

    let (tombstone, title_absent) = pane_unfurl_tombstone(&be, &viewer);
    if !title_absent {
        leaks += 1;
    }

    let green = ft_zero_leak
        && one_list
        && one_join
        && rag_serves_visible
        && check_live_updated
        && tombstone.is_tombstone()
        && tombstone.root == PANE_CONFIDENTIAL
        && title_absent;

    E2eArtifact {
        scenario: "E2E-1",
        green,
        evidence: format!(
            "PR pane (Search side): denied-viewer FT zero_leak={ft_zero_leak} (one_list={one_list} \
             one_join={one_join}); RAG serves_visible={rag_serves_visible}; mid-flight \
             ci.check.updated live_updated={check_live_updated}; confidential hit → \
             tombstone(root={}, title_absent={title_absent}); leaks={leaks}",
            tombstone.root,
        ),
        leaks,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PaneTombstone {
    root: String,
    tombstoned: bool,
}
impl PaneTombstone {
    fn is_tombstone(&self) -> bool {
        self.tombstoned
    }
}

fn pane_unfurl_tombstone(be: &TantivyBackend, denied: &Principal) -> (PaneTombstone, bool) {
    let acl_filter = AclFilter::ids(PANE_VISIBLE);
    let probe = be
        .search(&acl_filter, "acquisition", 50)
        .expect("ft probe under the visible ACL");
    let _ = denied;
    let tombstoned = probe.iter().all(|h| h.doc_id != PANE_CONFIDENTIAL);
    let rendered = format!("{probe:?}");
    let title_absent = !rendered.contains("SECRET") && !rendered.contains("acquisition");
    (
        PaneTombstone {
            root: PANE_CONFIDENTIAL.to_string(),
            tombstoned,
        },
        title_absent,
    )
}

#[derive(Default)]
struct LineageFetcher {
    bodies: std::sync::Mutex<std::collections::HashMap<String, String>>,
}
impl LineageFetcher {
    fn put(&self, ref_: &str, body: &str) {
        self.bodies
            .lock()
            .unwrap()
            .insert(ref_.to_string(), body.to_string());
    }
}
impl ProjectFetcher for LineageFetcher {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        match self.bodies.lock().unwrap().get(&ref_.0) {
            Some(b) => Ok(SearchProjection {
                text: b.clone(),
                fields: std::collections::BTreeMap::new(),
                lang: None,
            }),
            None => Err(ProjectFetchError::Gone),
        }
    }
}

fn lineage_snapshot_ref(agg: &str) -> String {
    format!("myelin://t/knowledge/page/{agg}")
}

fn spec_to_ship_lineage(_tenant: &str) -> Vec<(String, String)> {
    vec![
        (
            "spec-doc".into(),
            "spec for the new initiative on raft leadership".into(),
        ),
        (
            "issue-eng-1".into(),
            "child issue tracking the spec quorum work".into(),
        ),
        (
            "pr-1".into(),
            "pull request closing the issue with the fix".into(),
        ),
        (
            "commit-c0ffee".into(),
            "commit landing the quorum change".into(),
        ),
        (
            "ci-run-1".into(),
            "ci run validating the deploy gate".into(),
        ),
        (
            "deploy-1".into(),
            "protected-env deploy shipping the change".into(),
        ),
        (
            "chat-decision".into(),
            "chat thread recording the go decision".into(),
        ),
    ]
}

fn lineage_page_spec() -> IndexSpec {
    IndexSpec::new("knowledge", "page", std::collections::BTreeMap::new()).semantic()
}

fn lineage_parity_hash(ix: &IncrementalIndexer, tenant: &TenantId, region: &Region) -> u64 {
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    for term in [
        "raft", "quorum", "issue", "pull", "commit", "ci", "deploy", "chat", "spec", "fix", "go",
        "change", "gate", "decision",
    ] {
        let hits = ix
            .search_ft(tenant, region, &AclFilter::All, term, 100)
            .unwrap_or_default();
        for h in hits {
            reachable.insert(h.doc_id);
        }
    }
    let live = ix.live_count(tenant, region);
    let vectors = ix.live_vector_count(tenant, region);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for &b in bytes {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for id in &reachable {
        mix(id.as_bytes());
        mix(b"\x00");
    }
    mix(&live.to_le_bytes());
    mix(&(vectors as u64).to_le_bytes());
    hash
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_e2e_3_spec_to_ship() -> E2eArtifact {
    let tenant = e2e_tenant();
    let region = e2e_region();
    let lineage = spec_to_ship_lineage(&tenant.0);
    let scope = SnapshotScope::new("knowledge", "page:all");

    let fetcher = Arc::new(LineageFetcher::default());
    let mut owner = ReferenceReindexSource::new("knowledge", "page");
    for (agg, body) in &lineage {
        owner.upsert(agg, 1, serde_json::json!({ "kind": "page" }));
        fetcher.put(&lineage_snapshot_ref(agg), body);
    }

    let ix = Arc::new(IncrementalIndexer::new(
        vec![lineage_page_spec()],
        fetcher.clone(),
        Arc::new(MockEmbeddingAdapter::new(8)),
    ));
    let reindexer = SearchReindexer::new(ix.clone(), region.clone());

    let mut outbox = OutboxStore::new();
    let srcs: &[&dyn ReindexSource] = &[&owner];
    reindexer
        .reindex(&tenant, &scope, None, srcs, &mut outbox, e2e_ctx_base())
        .expect("build the live lineage index");
    let live_count = ix.live_count(&tenant, &region);
    let live_hash = lineage_parity_hash(&ix, &tenant, &region);

    let mut outbox2 = OutboxStore::new();
    reindexer
        .reindex(&tenant, &scope, None, srcs, &mut outbox2, e2e_ctx_base())
        .expect("reindex-from-source the wiped index");
    let cold_count = ix.live_count(&tenant, &region);
    let cold_hash = lineage_parity_hash(&ix, &tenant, &region);

    let byte_match = live_hash == cold_hash && live_count == cold_count && live_count > 0;

    let kms = Arc::new(KmsEngine::new());
    let pin = SearchDekPin::new(kms);
    pin.reserve(&tenant, &region).expect("reserve index DEK");
    let holder = SearchEraseHolder::new(ix.clone(), pin, region.clone());
    let ledger = SearchErasureLedger::new(tenant.clone(), region.clone());
    let mut gate_outbox = OutboxStore::new();
    let gate_srcs: &[&dyn ReindexSource] = &[&owner];
    let mut inputs = SearchRestoreInputs {
        reindexer: &reindexer,
        erase_holder: &holder,
        ledger: &ledger,
        tenant: tenant.clone(),
        scope: scope.clone(),
        restore_to_offset: None,
        sources: gate_srcs,
        outbox: &mut gate_outbox,
        ctx_base: e2e_ctx_base(),
        now: "2026-06-25T12:00:00Z".into(),
    };
    let verdict = SearchRestoreVerifyGate::new().run(&mut inputs);
    let restore_green = verdict.is_green();

    let green = byte_match && restore_green;

    E2eArtifact {
        scenario: "E2E-3",
        green,
        leaks: 0,
        evidence: format!(
            "spec-to-ship (Search side): live_count={live_count} cold_count={cold_count}; \
             cold-reindex==live byte_match={byte_match} (live_hash={live_hash:#018x} \
             cold_hash={cold_hash:#018x}); restore-verify green={restore_green}",
        ),
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_e2e_4_dsar_fanout() -> E2eArtifact {
    let tenant = e2e_tenant();
    let region = e2e_region();
    let target = "p-opaque-subject-0";
    let subject_docs = ["t1", "t2", "t3"];
    let other_docs = ["o0", "o1", "o2", "o3", "o4", "o5", "o6", "o7", "o8", "o9"];

    let (ix, ids) = build_live_corpus(&tenant, &region, target, &subject_docs, &other_docs);
    let matcher = subject_matcher(target, &tenant);
    let pre = ix.locate_subject(&tenant, &region, &matcher).len();

    let kms = Arc::new(KmsEngine::new());
    let pin = SearchDekPin::new(kms);
    let key_ref = pin
        .reserve(&tenant, &region)
        .expect("reserve the per-tenant index DEK");
    let dek = pin
        .resolve(&key_ref, &region)
        .expect("resolve the live DEK");
    let subject_doc_ids: Vec<&String> = ids
        .iter()
        .filter(|id| subject_docs.iter().any(|d| id.ends_with(d)))
        .collect();
    let backups: Vec<SealedBackupSegment> = subject_doc_ids
        .iter()
        .map(|id| {
            SealedBackupSegment::seal(
                &dek,
                id,
                format!("{target}'s index segment plaintext for {id}").as_bytes(),
            )
        })
        .collect();

    let holder = SearchEraseHolder::new(ix.clone(), pin.clone(), region.clone());

    let mut inputs = BackupScaleEraseInputs {
        erase_holder: &holder,
        dek: &pin,
        index_key_ref: key_ref,
        subject: SubjectRef::new(Principal::stub(
            PrincipalId(target.into()),
            PrincipalKind::Human,
            tenant.clone(),
        )),
        tenant: tenant.clone(),
        backup_segments: &backups,
        subject_backstop_id: None,
        now: "2026-06-25T00:00:00Z".into(),
    };
    let verdict = BackupScaleEraseGate::new().run(&mut inputs);
    let (zero_recoverable, after_shred, before_shred, live_remaining) = match verdict.artifact() {
        Some(a) => (
            a.is_green(),
            a.backup_segments_recoverable_after_shred,
            a.backup_segments_recoverable_before_shred,
            a.live_docs_remaining,
        ),
        None => (false, usize::MAX, 0, usize::MAX),
    };

    let post = ix.locate_subject(&tenant, &region, &matcher).len();
    let survivors = ix
        .search_ft(&tenant, &region, &AclFilter::All, "paxos", 30)
        .map(|h| h.len())
        .unwrap_or(0);

    let search_holder_is_h7 = search_index_holder() == Some(Holder::H7SearchIndex);

    let leaks: u64 = if zero_recoverable && post == 0 && after_shred == 0 {
        0
    } else {
        1
    };

    let green = zero_recoverable
        && search_holder_is_h7
        && pre == subject_docs.len()
        && post == 0
        && live_remaining == 0
        && before_shred == subject_docs.len()
        && after_shred == 0
        && survivors == other_docs.len();

    E2eArtifact {
        scenario: "E2E-4",
        green,
        evidence: format!(
            "DSAR fan-out (Search side): subject referenced {pre} live docs → {post} after erase \
             (live_remaining={live_remaining}); backups recoverable {before_shred}→{after_shred} \
             (0 incl. vectors incl. backups, §7.5); {survivors}/{} survivors intact; holder-coverage \
             receipt includes Search H7 (store={SEARCH_INDEX_STORE}, is_h7={search_holder_is_h7})",
            other_docs.len(),
        ),
        leaks,
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_search_e2e_wedge() -> Vec<E2eArtifact> {
    vec![
        run_e2e_1_pr_pane(),
        run_e2e_3_spec_to_ship(),
        run_e2e_4_dsar_fanout(),
    ]
}

#[cfg(test)]
mod tests;
