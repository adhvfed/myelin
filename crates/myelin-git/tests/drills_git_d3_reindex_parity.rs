use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::{
    reindex as bus_reindex, Actor, EmitContextBase, MonotonicMinter, OutboxStore, Region,
    ReindexSource, SnapshotScope, TenantId, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_search::{
    git_blob_search_projection, git_code_projection_spec, AclFilter, EmbeddingAdapter,
    GitBlobProjectionInput, IncrementalIndexer, MockEmbeddingAdapter, ProjectFetchError,
    ProjectFetcher, ReindexJob, SearchProjection, SearchReindexer,
};
use myelin_tenancy::ArtifactRef as TenancyArtifactRef;

use myelin_git::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusConsumer, CheckStatusProjection, GitOid,
    HumanisedRef, Timestamp as CsTimestamp, TrustTier,
};
use myelin_git::replay::{GitReindexSource, GitReplayKind};
use myelin_git::typed_edges::{emit_lifecycle_edges, REFS_EDGE_CREATED};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn principal() -> Principal {
    Principal::stub(
        PrincipalId("platform".into()),
        PrincipalKind::Service,
        tenant(),
    )
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(principal()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-22T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-22T00:00:00Z".into()),
        caused_by: None,
    }
}

struct CorpusBlob {
    blob_ref: String,
    aggregate: String,
    version: u64,
    input: GitBlobProjectionInput,
}

fn corpus() -> Vec<CorpusBlob> {
    let blobs = [
        (
            "src/scheduler/deadlock.rs",
            "rust",
            "fn detectDeadlock(graph: &WaitForGraph) -> bool { graph.has_cycle() }",
            vec!["cycle detected".to_string()],
            "fix: resolve the scheduler deadlock detection",
            "blob-oid-aaa",
        ),
        (
            "src/raft/log.rs",
            "rust",
            "fn appendEntries(term: u64) { replicate(term) }",
            vec!["term mismatch".to_string()],
            "feat: raft log replication",
            "blob-oid-bbb",
        ),
        (
            "README.md",
            "markdown",
            "# project\nA distributed log built on raft consensus.",
            vec![],
            "docs: readme",
            "blob-oid-ccc",
        ),
    ];
    blobs
        .into_iter()
        .enumerate()
        .map(|(i, (path, lang, text, literals, msg, oid))| {
            let blob_ref = format!("myelin://acme/git/blob/core:refs/heads/main:{path}");
            CorpusBlob {
                aggregate: blob_ref.clone(),
                blob_ref,
                version: (i as u64) + 1,
                input: GitBlobProjectionInput {
                    path: path.into(),
                    language: lang.into(),
                    text: text.into(),
                    literals,
                    commit_message: msg.into(),
                    blob_oid: oid.into(),
                },
            }
        })
        .collect()
}

#[derive(Default)]
struct GitProjectFetcher {
    bodies: Mutex<BTreeMap<String, SearchProjection>>,
    fetched: Mutex<Vec<String>>,
}
impl GitProjectFetcher {
    fn with_corpus(corpus: &[CorpusBlob]) -> GitProjectFetcher {
        let f = GitProjectFetcher::default();
        for b in corpus {
            f.bodies
                .lock()
                .unwrap()
                .insert(b.blob_ref.clone(), git_blob_search_projection(&b.input));
        }
        f
    }
    fn fetched_refs(&self) -> Vec<String> {
        self.fetched.lock().unwrap().clone()
    }
}
impl ProjectFetcher for GitProjectFetcher {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &TenancyArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        self.fetched.lock().unwrap().push(ref_.0.clone());
        match self.bodies.lock().unwrap().get(&ref_.0) {
            Some(p) => Ok(p.clone()),
            None => Err(ProjectFetchError::Gone),
        }
    }
}

fn git_blob_spec() -> myelin_search::IndexSpec {
    git_code_projection_spec()
}

fn git_source(corpus: &[CorpusBlob]) -> GitReindexSource {
    let mut s = GitReindexSource::new();
    for b in corpus {
        s.upsert(
            GitReplayKind::Blob,
            &b.aggregate,
            b.version,
            &b.blob_ref,
            serde_json::json!({
                "artifact_ref": b.blob_ref,
                "path": b.input.path,
                "language": b.input.language,
                "blob_oid": b.input.blob_oid,
            }),
        );
    }
    s
}

fn index_digest(ix: &IncrementalIndexer, corpus: &[CorpusBlob]) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("count={}", ix.live_count(&tenant(), &region())));
    for b in corpus {
        let z = ix
            .indexed_zookie_of(&tenant(), &region(), &b.blob_ref)
            .unwrap_or_else(|| "<absent>".into());
        parts.push(format!("{}#{z}", b.blob_ref));
    }
    for q in ["raft", "deadlock", "replication"] {
        let hits = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, q, 10)
            .expect("ft query");
        let mut docs: Vec<String> = hits.iter().map(|h| h.doc_id.clone()).collect();
        docs.sort();
        parts.push(format!("ft[{q}]={}", docs.join(",")));
    }
    parts.join("|")
}

#[test]
fn search_code_index_cold_rebuild_byte_matches_live() {
    let corpus = corpus();

    let fetcher = Arc::new(GitProjectFetcher::with_corpus(&corpus));
    let embedder: Arc<dyn EmbeddingAdapter> = Arc::new(MockEmbeddingAdapter::new(8));
    let live_ix = Arc::new(IncrementalIndexer::new(
        vec![git_blob_spec()],
        fetcher.clone(),
        embedder.clone(),
    ));
    let src = git_source(&corpus);
    let scope = SnapshotScope::new("git", "blob:all");
    {
        let mut outbox = OutboxStore::new();
        let sources: &[&dyn ReindexSource] = &[&src];
        bus_reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("emit");
        for draft in src.replay(&scope, None) {
            let row = outbox.row(&draft.event_id()).expect("snapshot row");
            live_ix.index(&row.envelope).expect("live index");
        }
    }
    assert_eq!(
        live_ix.live_count(&tenant(), &region()),
        corpus.len() as u64
    );
    let live_digest = index_digest(&live_ix, &corpus);

    let cold_fetcher = Arc::new(GitProjectFetcher::with_corpus(&corpus));
    let cold_ix = Arc::new(IncrementalIndexer::new(
        vec![git_blob_spec()],
        cold_fetcher.clone(),
        embedder,
    ));
    let reindexer = SearchReindexer::new(cold_ix.clone(), region());
    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&src];
    let job = reindexer
        .reindex(&tenant(), &scope, None, sources, &mut outbox, ctx_base())
        .expect("reindex returns a job");
    assert!(
        matches!(job, ReindexJob::Done(_)),
        "the rebuild completes under the batch cap"
    );
    assert_eq!(
        job.progress().snapshots_emitted,
        corpus.len(),
        "one git.blob.snapshot re-emitted per corpus blob (contract 2.6)"
    );
    assert_eq!(
        job.progress().docs_indexed,
        corpus.len(),
        "every snapshot driven through the LIVE indexer step (no second path)"
    );

    let cold_digest = index_digest(&cold_ix, &corpus);

    assert_eq!(
        cold_digest, live_digest,
        "the cold-rebuilt code index byte-matches the live index (GIT-D3 parity)"
    );

    let mut fetched = cold_fetcher.fetched_refs();
    fetched.sort();
    fetched.dedup();
    let mut expected: Vec<String> = corpus.iter().map(|b| b.blob_ref.clone()).collect();
    expected.sort();
    assert_eq!(
        fetched, expected,
        "the rebuild reached ONLY the owner's project(ref) (5.6) - no cross-DB read path"
    );
}

#[test]
fn search_code_index_reindex_is_idempotent() {
    let corpus = corpus();
    let fetcher = Arc::new(GitProjectFetcher::with_corpus(&corpus));
    let embedder: Arc<dyn EmbeddingAdapter> = Arc::new(MockEmbeddingAdapter::new(8));
    let ix = Arc::new(IncrementalIndexer::new(
        vec![git_blob_spec()],
        fetcher,
        embedder,
    ));
    let reindexer = SearchReindexer::new(ix.clone(), region());
    let src = git_source(&corpus);
    let scope = SnapshotScope::new("git", "blob:all");
    let sources: &[&dyn ReindexSource] = &[&src];

    let mut outbox = OutboxStore::new();
    reindexer
        .reindex(&tenant(), &scope, None, sources, &mut outbox, ctx_base())
        .expect("first reindex");
    let after_first = index_digest(&ix, &corpus);

    reindexer
        .reindex(&tenant(), &scope, None, sources, &mut outbox, ctx_base())
        .expect("second reindex");
    let after_second = index_digest(&ix, &corpus);
    assert_eq!(
        after_first, after_second,
        "a re-run is idempotent (cold == live, no double-index)"
    );
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        corpus.len() as u64,
        "no duplicate docs"
    );
}

fn check_fact(
    commit: &str,
    name: &str,
    attempt: u32,
    state: CheckState,
    trust: TrustTier,
) -> CheckStatus {
    CheckStatus {
        tenant: tenant(),
        repo: TenancyArtifactRef("myelin://acme/git/repo/core".into()),
        commit_oid: GitOid(commit.into()),
        context: CheckContext::ci(name),
        state,
        required: true,
        run: TenancyArtifactRef(format!("myelin://acme/ci/run/{commit}-{name}-{attempt}")),
        run_attempt: attempt,
        trust_tier: trust,
        details_ref: TenancyArtifactRef(format!(
            "myelin://acme/ci/run/{commit}-{name}-{attempt}#step-1"
        )),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args: BTreeMap::new(),
        },
        started_at: CsTimestamp("2026-06-22T00:00:00Z".into()),
        completed_at: Some(CsTimestamp("2026-06-22T00:01:00Z".into())),
        cost_settled: true,
    }
}

#[test]
fn check_status_projection_rebuilds_from_ci_reemit_byte_identical() {
    let mut ci = myelin_events::reindex::ReferenceReindexSource::new("ci", "check");

    let live_consumer = CheckStatusConsumer::new();
    let live_proj = {
        let mut p = CheckStatusProjection::new();
        for fact in [
            check_fact("c1", "build", 1, CheckState::Failure, TrustTier::Trusted),
            check_fact("c1", "build", 2, CheckState::Success, TrustTier::Trusted),
            check_fact("c1", "test", 1, CheckState::Success, TrustTier::Trusted),
            check_fact(
                "c2",
                "build",
                1,
                CheckState::Success,
                TrustTier::UntrustedFork,
            ),
        ] {
            p.apply(&fact);
            let agg = format!("ci.check:{}:{}", fact.commit_oid.0, fact.context.name);
            ci.upsert(
                &agg,
                fact.run_attempt as u64,
                serde_json::to_value(&fact).unwrap(),
            );
        }
        p
    };
    let _ = &live_consumer;
    let live_bytes = serde_json::to_value(serialize_projection(&live_proj)).unwrap();

    let scope = SnapshotScope::new("ci", "check:all");
    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&ci];
    let receipt = bus_reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("ci reindex");
    assert_eq!(
        receipt.snapshots_emitted, 3,
        "one current fact per (commit,context) key re-emitted"
    );

    let mut cold_proj = CheckStatusProjection::new();
    for draft in ci.replay(&scope, None) {
        let row = outbox.row(&draft.event_id()).expect("snapshot row");
        let fact = CheckStatusConsumer::decode(&row.envelope.payload).expect("decode CI fact");
        cold_proj.apply(&fact);
    }
    let cold_bytes = serde_json::to_value(serialize_projection(&cold_proj)).unwrap();

    assert_eq!(
        cold_bytes, live_bytes,
        "the check_status projection rebuilds byte-identically from CI's ci.check re-emit (GIT-D3)"
    );

    let key = myelin_git::check_status::CheckKey {
        commit_oid: GitOid("c1".into()),
        context: CheckContext::ci("build"),
    };
    let row = cold_proj.current(&key).expect("c1/build current");
    assert_eq!(
        row.run_attempt, 2,
        "the re-run supersession survived the rebuild"
    );
    assert_eq!(row.state, CheckState::Success);
}

fn serialize_projection(p: &CheckStatusProjection) -> BTreeMap<String, serde_json::Value> {
    let mut out: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for commit in ["c1", "c2"] {
        let oid = GitOid(commit.into());
        for row in p.rows_for_commit(&oid) {
            let k = format!("{}:{}", row.commit_oid.0, row.context.name);
            out.insert(k, serde_json::to_value(row).unwrap());
        }
    }
    out
}

fn edge_projection_from_rows(
    outbox: &OutboxStore,
    ids: &[myelin_events::EventId],
) -> BTreeMap<String, serde_json::Value> {
    let mut out: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for id in ids {
        let row = outbox.row(id).expect("edge row");
        assert_eq!(
            row.envelope.type_.0, REFS_EDGE_CREATED,
            "a lifecycle edge is a refs.edge.created"
        );
        let pl = &row.envelope.payload;
        let key = format!(
            "{}->{}",
            pl["source"].as_str().unwrap_or(""),
            pl["target"].as_str().unwrap_or("")
        );
        out.insert(key, pl.clone());
    }
    out
}

#[test]
fn refs_lifecycle_edges_cold_rebuild_byte_matches_live() {
    let pr = TenancyArtifactRef("myelin://acme/git/pr/core:42".into());
    let issue = TenancyArtifactRef("myelin://acme/issue/issue/ENG-1".into());
    let linked = TenancyArtifactRef("myelin://acme/git/pr/core:7".into());

    let cause = pr_merged_envelope(&pr);

    let live_outbox = OutboxStore::new();
    let live_ids = {
        let mut tx = live_outbox.begin(Arc::new(MonotonicMinter::new()), ctx_base());
        let ids = emit_lifecycle_edges(
            &mut tx,
            &pr,
            std::slice::from_ref(&issue),
            std::slice::from_ref(&linked),
            &cause,
        )
        .expect("emit edges");
        tx.commit().expect("commit");
        ids
    };
    assert_eq!(live_ids.len(), 2, "one closes + one relates edge");
    let live_edges = edge_projection_from_rows(&live_outbox, &live_ids);

    let cold_outbox = OutboxStore::new();
    let cold_ids = {
        let mut tx = cold_outbox.begin(Arc::new(MonotonicMinter::new()), ctx_base());
        let ids = emit_lifecycle_edges(
            &mut tx,
            &pr,
            std::slice::from_ref(&issue),
            std::slice::from_ref(&linked),
            &cause,
        )
        .expect("re-emit edges");
        tx.commit().expect("commit");
        ids
    };
    let cold_edges = edge_projection_from_rows(&cold_outbox, &cold_ids);

    let live_bytes = serde_json::to_value(&live_edges).unwrap();
    let cold_bytes = serde_json::to_value(&cold_edges).unwrap();
    assert_eq!(
        cold_bytes, live_bytes,
        "the lifecycle edge set rebuilds byte-identically (GIT-D3)"
    );

    assert!(cold_edges.contains_key(&format!("{}->{}", pr.0, issue.0)));
    assert!(cold_edges.contains_key(&format!("{}->{}", pr.0, linked.0)));
}

fn pr_merged_envelope(pr: &TenancyArtifactRef) -> myelin_events::EventEnvelope {
    use myelin_events::{
        AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
        Visibility,
    };
    EventEnvelope {
        event_id: EventId("git.pr.merged:core:42".into()),
        type_: EventType("git.pr.merged".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(principal()),
        subject: ArtifactRef(pr.0.clone()),
        aggregate: AggregateKey(format!("pr:{}", pr.0)),
        causation_id: None,
        correlation_id: CorrelationId("git.pr.merged:core:42".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-22T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-22T00:00:00Z".into()),
        payload: serde_json::json!({ "state": "merged" }),
    }
}

#[test]
fn an_erased_blob_does_not_resurrect_on_reindex() {
    let corpus = corpus();
    let erased_ref = corpus[1].blob_ref.clone();

    let fetcher = Arc::new(GitProjectFetcher::with_corpus(&corpus));
    let embedder: Arc<dyn EmbeddingAdapter> = Arc::new(MockEmbeddingAdapter::new(8));
    let ix = Arc::new(IncrementalIndexer::new(
        vec![git_blob_spec()],
        fetcher,
        embedder,
    ));
    let reindexer = SearchReindexer::new(ix.clone(), region());

    let scope = SnapshotScope::new("git", "blob:all");
    let mut src = git_source(&corpus);
    {
        let mut outbox = OutboxStore::new();
        let sources: &[&dyn ReindexSource] = &[&src];
        reindexer
            .reindex(&tenant(), &scope, None, sources, &mut outbox, ctx_base())
            .expect("initial reindex");
    }
    assert_eq!(ix.live_count(&tenant(), &region()), corpus.len() as u64);
    assert!(
        ix.indexed_zookie_of(&tenant(), &region(), &erased_ref)
            .is_some(),
        "the blob is indexed before erasure"
    );

    assert!(
        src.erase(&erased_ref),
        "the blob was present and is now erased"
    );

    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&src];
    reindexer
        .reindex(&tenant(), &scope, None, sources, &mut outbox, ctx_base())
        .expect("post-erase reindex");

    assert_eq!(
        ix.live_count(&tenant(), &region()),
        (corpus.len() - 1) as u64,
        "the cold rebuild has one fewer doc - the erased blob did not resurrect"
    );
    assert!(
        ix.indexed_zookie_of(&tenant(), &region(), &erased_ref).is_none(),
        "the erased blob's doc is ABSENT after reindex (0 resurrected PII; the ONE posture residual)"
    );
    assert!(ix
        .indexed_zookie_of(&tenant(), &region(), &corpus[0].blob_ref)
        .is_some());
    assert!(ix
        .indexed_zookie_of(&tenant(), &region(), &corpus[2].blob_ref)
        .is_some());
}
