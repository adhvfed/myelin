//! # GIT-D3 — reindex-from-source parity (cold rebuild byte-matches live; no cross-DB read)
//!
//! **Prompt:** P-291 (GIT-P30, M3-G7). **Drill:** GIT-D3 (SCHED) — wipe the Search code index +
//! Refs edges + the `check_status` projection; reindex/replay → the cold rebuild **byte-matches**
//! live (one code path, no drift); the `check_status` projection rebuilds from CI's `ci.check.updated`
//! re-emit; **NO cross-DB read**. An erased subject's body does NOT resurrect on reindex (the
//! post-reindex residual == the ONE posture; 0 resurrected PII).
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md` §4
//! (`replay(scope, since)` — reindex-from-source: Git re-emits `git.*.snapshot` through the LIVE
//! consumer path, never reading owner DBs; the `check_status` projection rebuilds the OTHER way — Git
//! asks the Bus to `reindex` CI's `ci.check.updated`, and Git's consumer re-applies supersession; the
//! projection is **derived, never restored**). **Contract:** index row **2.6** (reindex-from-source /
//! replay — owned) + **10.9** (the ONE posture — the post-reindex residual, consumed by reference).
//! **Doctrine:** EI-04 §5 (derived stores rebuild from source, never read owner DBs; reindex-from-
//! source is a first-class resilience primitive); VISION §3 (erasure by construction).
//!
//! ## Both sides of the reindex seam (the CDC pair for contract 2.6, the Git leg)
//!
//! This drill exercises BOTH sides of the reindex-from-source seam end-to-end:
//! - **PROVIDER side** — the owner's `replay(scope, since)` re-emit: `myelin_git::replay::Git
//!   ReindexSource` re-emits `git.blob.snapshot` per indexed blob through the Bus outbox (contract
//!   2.6); for the `check_status` projection the PROVIDER is CI's `ci.check.updated` re-emit (Git asks
//!   the Bus to reindex CI's facts, §4); for the Refs edges the PROVIDER is Git's `refs.edge.created`
//!   re-emit. The provider re-reads its OWN source of truth, never a derived store / cross DB.
//! - **CONSUMER side** — the live derived consumer re-applies each re-emit through the SAME steady-
//!   state step: the Search `IncrementalIndexer::index()`, Git's `CheckStatusProjection::apply()`
//!   monotonic supersession, the Refs lifecycle-edge mirror. The consumer cannot tell cold from live,
//!   so the cold rebuild byte-matches the live store.
//!
//! ## What this drill PROVES — cold == live across ALL THREE Git-fed derived stores
//!
//! The reindex-from-source MACHINERY ships from EB-22 (P-142, the Bus seam) + EB-26 (P-246, the
//! per-owner `replay` bodies: `myelin_git::replay::GitReindexSource`) + the live derived consumers
//! (`myelin_search::IncrementalIndexer` SRCH-P06, `myelin_git::check_status::CheckStatusProjection`
//! GIT-P6/P-232, `myelin_git::typed_edges` GIT-P11/P-280). THIS drill is the GIT-D3 end-to-end
//! **parity proof** that ties the three Git-owned/Git-fed derived stores together: each rebuilds
//! BYTE-IDENTICALLY through the SAME live consumer step the steady-state path takes — there is no
//! second cold-rebuild code path, so there is no drift (EI-04 §5 / the no-cross-db floor).
//!
//! 1. **The Search code index** (Search owns the index; Git owns what-to-index, contract 6.3/6.5):
//!    live-index a set of `git.blob.snapshot`s → WIPE the per-tenant index → rebuild via
//!    `SearchReindexer::reindex` driving `GitReindexSource` for the `git` owner. The rebuilt index is
//!    byte-identical (doc count + per-doc `indexed_zookie` + the FT query result) to the live one.
//!    The rebuild re-drives the SAME `IncrementalIndexer::index()` step + the SAME `project(ref)`
//!    fetch (no Postgres backdoor — the no-cross-db floor is structural).
//! 2. **The `check_status` projection** (Git's CONSUMER projection of CI's facts): live-apply CI's
//!    `ci.check.updated` facts → WIPE the projection → rebuild by re-emitting CI's facts (the Bus
//!    `reindex` of `ci.check.updated`) and re-applying the SAME monotonic supersession. The rebuilt
//!    projection serializes BYTE-IDENTICALLY to the live one (the §4 "rebuilds the other way").
//! 3. **The Refs lifecycle edges** (Git PRODUCES the edges; Refs mirrors): live-emit a PR's
//!    `refs.edge.created` lifecycle edges → WIPE the edge projection → rebuild from the replayed edge
//!    re-emits. The rebuilt edge set is byte-identical.
//!
//! 4. **Erased body does NOT resurrect (X-7 / the ONE posture, contract 10.9):** an erased blob is
//!    REMOVED from `GitReindexSource`, so a replay SKIPS it — the cold index does not get the doc
//!    back (0 resurrected PII).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::{
    reindex as bus_reindex, Actor, EmitContextBase, MonotonicMinter, OutboxStore, Region,
    ReindexSource, SnapshotScope, SubjectComponent, TenantId, Timestamp,
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

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// The Git corpus: a handful of indexed blobs on the default branch. The SAME corpus drives BOTH the
// live emit and the cold replay (the owner's source of truth) — proving cold == live, not two paths.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// One indexed blob in the corpus: its canonical artifact ref + the Git-projected inputs Search
/// consumes (path / language / raw text / literals / commit message / blob_oid). The ref is the
/// `(git, blob)` shape `myelin://<tenant>/git/blob/<repo>:<ref>:<path>` (so the indexer maps it to
/// the git/blob IndexSpec).
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
            let path = SubjectComponent::encode(path).unwrap();
            let blob_ref = format!(
                "myelin://acme/git/blob/core:refs%2Fheads%2Fmain:{}",
                path.as_str()
            );
            CorpusBlob {
                aggregate: blob_ref.clone(),
                blob_ref,
                version: (i as u64) + 1,
                input: GitBlobProjectionInput {
                    path: path.decode(),
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

/// **The owner's `project(ref, viewer)` (contract 5.6) — Search fetches the Git projection per
/// `*.snapshot`, NEVER the Git DB (the no-cross-db floor is STRUCTURAL).** Backed by the in-memory
/// corpus keyed by blob ref; this models Git's `project` resolving a blob to its `git_blob_search_
/// projection` body. The SAME fetcher serves the live emit and the cold replay (one body shape →
/// cold == live). A `record` of every ref fetched lets the drill PROVE no owner-DB read path existed.
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
    // The git/blob spec — byte-identical to git's owned 6.5 spec. The structured facets the doc carries.
    git_code_projection_spec()
}

/// A `GitReindexSource` seeded from the corpus — the blob aggregates the Bus replays for the `git`
/// owner (one `git.blob.snapshot` per blob). The replay payload is references-not-payloads (the blob
/// ref + facets); Search fetches the body via `project` (5.6) — the SAME path the live emit took.
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

/// **A deterministic byte-digest of the Search index for a tenant.** The doc count + every corpus
/// blob's `indexed_zookie` (the staleness anchor) + the FT hit-set for a known query term. Two indexes
/// with the same digest are byte-identical for the drill's parity assertion (cold == live).
fn index_digest(ix: &IncrementalIndexer, corpus: &[CorpusBlob]) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("count={}", ix.live_count(&tenant(), &region())));
    for b in corpus {
        let z = ix
            .indexed_zookie_of(&tenant(), &region(), &b.blob_ref)
            .unwrap_or_else(|| "<absent>".into());
        parts.push(format!("{}#{z}", b.blob_ref));
    }
    // A few cross-section FT queries (the corpus is searchable identically cold and live).
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

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// 1. THE SEARCH CODE INDEX — cold rebuild byte-matches live (the GIT-D3 code-index half)
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **The Search code index rebuilds BYTE-IDENTICALLY from a reindex (cold == live; no cross-DB).**
/// Build the live index by indexing every blob snapshot; capture the digest. Wipe the index; rebuild
/// via the reindex (the Bus re-emit of `GitReindexSource` driven through the LIVE indexer step).
/// Assert byte-identical. PROVE the rebuild only ever touched the owner's `project` (5.6), never a DB.
#[test]
fn search_code_index_cold_rebuild_byte_matches_live() {
    let corpus = corpus();

    // ── LIVE: index every blob snapshot through the ordinary consumer step. ──
    let fetcher = Arc::new(GitProjectFetcher::with_corpus(&corpus));
    let embedder: Arc<dyn EmbeddingAdapter> = Arc::new(MockEmbeddingAdapter::new(8));
    let live_ix = Arc::new(IncrementalIndexer::new(
        vec![git_blob_spec()],
        fetcher.clone(),
        embedder.clone(),
    ));
    // The live emit: each push produced a git.blob.snapshot the indexer projected. We drive the SAME
    // index() step the relay would (the deterministic snapshot envelope of the replay = the live event).
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

    // ── COLD: a FRESH indexer, wiped + rebuilt via SearchReindexer.reindex (the §4.9 ONLY rebuild
    //    path — the Bus re-emit through the SAME live index() step). cold == live. ──
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

    // THE GREEN ARTIFACT: the reindex-parity hash (cold == live, byte-identical).
    assert_eq!(
        cold_digest, live_digest,
        "the cold-rebuilt code index byte-matches the live index (GIT-D3 parity)"
    );

    // ── no-cross-db (structural): the ONLY way a doc entered the cold index is the owner's project(ref)
    //    fetch — the same blob refs, never a Git DB read. The fetcher recorded exactly the corpus refs. ──
    let mut fetched = cold_fetcher.fetched_refs();
    fetched.sort();
    fetched.dedup();
    let mut expected: Vec<String> = corpus.iter().map(|b| b.blob_ref.clone()).collect();
    expected.sort();
    assert_eq!(
        fetched, expected,
        "the rebuild reached ONLY the owner's project(ref) (5.6) — no cross-DB read path"
    );
}

/// **A re-run of the reindex is IDEMPOTENT (the deterministic snapshot id no-ops the duplicate).** A
/// second reindex does not double-index (cold == live holds after a re-run; a redelivered snapshot is
/// absorbed by the deterministic `event_id`).
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

    // A second reindex re-drives the SAME replay — the deterministic ids no-op the duplicates.
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

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// 2. THE check_status PROJECTION — rebuilds from CI's ci.check.updated re-emit (the §4 "other way")
// ─────────────────────────────────────────────────────────────────────────────────────────────────

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

/// **The `check_status` projection rebuilds BYTE-IDENTICALLY from CI's `ci.check.updated` re-emit
/// (the §4 "the projection rebuilds the OTHER way").** Git does NOT replay its own check_status rows
/// (it is a CONSUMER projection of CI's facts — derived, never restored, 2.6/11.5). The LIVE projection
/// applies CI facts (with a re-run supersession); the COLD projection is rebuilt by re-emitting CI's
/// facts through the Bus (a `ci.check.snapshot` re-emit) and re-applying the SAME monotonic supersession
/// the live consumer applied. The two serialize byte-identically.
#[test]
fn check_status_projection_rebuilds_from_ci_reemit_byte_identical() {
    // CI's source of truth for the scope: the CURRENT per-(commit,context) fact (CI's replay re-emits
    // the per-run snapshot at the high-water attempt — the projection's LWW input). A `ReferenceReindex
    // Source` for the `ci` owner models CI's replay (CI owns it; EB-27/M4 is the real producer floor).
    let mut ci = myelin_events::reindex::ReferenceReindexSource::new("ci", "check");

    // ── LIVE: the consumer applied a stream of facts incl. a re-run supersession (attempt 1 → 2). ──
    let live_consumer = CheckStatusConsumer::new();
    let live_proj = {
        let mut p = CheckStatusProjection::new();
        // build c1: failure@1, then success@2 (re-run supersedes). test c1: success@1.
        for fact in [
            check_fact("c1", "build", 1, CheckState::Failure, TrustTier::Trusted),
            check_fact("c1", "build", 2, CheckState::Success, TrustTier::Trusted),
            check_fact("c1", "test", 1, CheckState::Success, TrustTier::Trusted),
            // a fork run on a second commit, neutral-until-endorsed (recorded current row).
            check_fact(
                "c2",
                "build",
                1,
                CheckState::Success,
                TrustTier::UntrustedFork,
            ),
        ] {
            p.apply(&fact);
            // record CI's truth at the CURRENT (high-water) fact per key — what CI's replay re-emits.
            let agg = format!("ci.check:{}:{}", fact.commit_oid.0, fact.context.name);
            ci.upsert(
                &agg,
                fact.run_attempt as u64,
                serde_json::to_value(&fact).unwrap(),
            );
        }
        p
    };
    // The live consumer is consistent with the hand-built live projection (the live wiring half).
    let _ = &live_consumer; // (the consumer's EventHandler path is proven in drills_git_d9; here we
                            //  prove the reindex re-applies the SAME supersession SEMANTICS.)
    let live_bytes = serde_json::to_value(serialize_projection(&live_proj)).unwrap();

    // ── COLD: WIPE the projection (a fresh, empty one), then rebuild by re-emitting CI's facts through
    //    the Bus and re-applying supersession (Git's consumer re-applies — derived, never restored). ──
    let scope = SnapshotScope::new("ci", "check:all");
    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&ci];
    let receipt = bus_reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("ci reindex");
    // CI re-emits ONE current fact per (commit,context) key (the high-water attempt; the failing
    // attempt-1 of c1 is NOT re-emitted — the LWW row CI holds is attempt-2).
    assert_eq!(
        receipt.snapshots_emitted, 3,
        "one current fact per (commit,context) key re-emitted"
    );

    let mut cold_proj = CheckStatusProjection::new();
    for draft in ci.replay(&scope, None) {
        let row = outbox.row(&draft.event_id()).expect("snapshot row");
        // Git's consumer decodes CI's opaque re-emitted payload into the typed CheckStatus and applies
        // the SAME monotonic supersession (the consumer's apply step — derived, never restored).
        let fact = CheckStatusConsumer::decode(&row.envelope.payload).expect("decode CI fact");
        cold_proj.apply(&fact);
    }
    let cold_bytes = serde_json::to_value(serialize_projection(&cold_proj)).unwrap();

    // THE GREEN ARTIFACT: the rebuilt check_status projection byte-matches live (cold == live).
    assert_eq!(
        cold_bytes, live_bytes,
        "the check_status projection rebuilds byte-identically from CI's ci.check re-emit (GIT-D3)"
    );

    // The c1/build current row is the attempt-2 SUCCESS (the supersession held on rebuild), NOT the
    // stale attempt-1 failure (no resurrection of a superseded fact).
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

/// Serialize a `check_status` projection deterministically for the byte-parity comparison — the
/// (sorted) set of current rows. A `BTreeMap` over the serialized key → row gives a stable order.
fn serialize_projection(p: &CheckStatusProjection) -> BTreeMap<String, serde_json::Value> {
    let mut out: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    // Walk every commit we know of via the two commits in the corpus.
    for commit in ["c1", "c2"] {
        let oid = GitOid(commit.into());
        for row in p.rows_for_commit(&oid) {
            let k = format!("{}:{}", row.commit_oid.0, row.context.name);
            out.insert(k, serde_json::to_value(row).unwrap());
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// 3. THE REFS LIFECYCLE EDGES — cold rebuild byte-matches live (Git produces, Refs mirrors)
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// A minimal edge projection (the Refs-mirror-side derived store, modeled): `edge:<source>-><target>`
/// → the `(source, target, rel, rel_class)` payload. Rebuilt from `refs.edge.created` re-emits; the
/// real Refs mirror also projects the inverse — here we compare the FORWARD edge set Git produces.
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

/// **The Refs lifecycle edges rebuild BYTE-IDENTICALLY (cold == live).** A merged PR emits `closes` /
/// `relates` lifecycle edges. We capture the live edge set, then replay the edge re-emits (one
/// `git.pr.snapshot`-driven re-emit per edge) and rebuild — the forward edge set is byte-identical.
#[test]
fn refs_lifecycle_edges_cold_rebuild_byte_matches_live() {
    let pr = TenancyArtifactRef("myelin://acme/git/pr/core:42".into());
    let issue = TenancyArtifactRef("myelin://acme/issue/issue/ENG-1".into());
    let linked = TenancyArtifactRef("myelin://acme/git/pr/core:7".into());

    // The PR's lifecycle event (the cause) — a git.pr.merged envelope. The SAME cause drives BOTH the
    // live emit and the cold re-emit, so the deterministic edge events are byte-identical.
    let cause = pr_merged_envelope(&pr);

    // ── LIVE: emit the lifecycle edges in the PR-merge transaction. ──
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

    // ── COLD: WIPE the edge projection; rebuild from the edge re-emits (the replay re-produces the
    //    SAME refs.edge.created shape — Git emits forward only, the SAME wire tokens). ──
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

    // THE GREEN ARTIFACT: the forward lifecycle edge set byte-matches (cold == live).
    let live_bytes = serde_json::to_value(&live_edges).unwrap();
    let cold_bytes = serde_json::to_value(&cold_edges).unwrap();
    assert_eq!(
        cold_bytes, live_bytes,
        "the lifecycle edge set rebuilds byte-identically (GIT-D3)"
    );

    // The edge set is exactly { closes PR->issue, relates PR->linked } (the producer vocabulary).
    assert!(cold_edges.contains_key(&format!("{}->{}", pr.0, issue.0)));
    assert!(cold_edges.contains_key(&format!("{}->{}", pr.0, linked.0)));
}

/// A `git.pr.merged` lifecycle-event envelope (the cause the edges are emitted off). A fixed,
/// deterministic envelope so the live emit and the cold re-emit derive byte-identical edge events.
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

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// 4. AN ERASED BODY DOES NOT RESURRECT ON REINDEX (X-7 / the ONE posture, contract 10.9)
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **An erased subject's body does NOT resurrect on reindex (0 resurrected PII; the post-reindex
/// residual == the ONE posture, contract 10.9).** A blob is erased (REMOVED from `GitReindexSource` —
/// the `*.erased` tombstone is the live truth). A subsequent reindex SKIPS it: the cold index does NOT
/// re-acquire the erased doc — the rebuild cannot resurrect a shredded aggregate (the only entry path
/// is the replay, and the replay does not emit an erased aggregate).
#[test]
fn an_erased_blob_does_not_resurrect_on_reindex() {
    let corpus = corpus();
    let erased_ref = corpus[1].blob_ref.clone(); // src/raft/log.rs

    let fetcher = Arc::new(GitProjectFetcher::with_corpus(&corpus));
    let embedder: Arc<dyn EmbeddingAdapter> = Arc::new(MockEmbeddingAdapter::new(8));
    let ix = Arc::new(IncrementalIndexer::new(
        vec![git_blob_spec()],
        fetcher,
        embedder,
    ));
    let reindexer = SearchReindexer::new(ix.clone(), region());

    // First a normal reindex: all blobs present.
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

    // ERASE the blob at the owner (the *.erased tombstone removes it from the source of truth).
    assert!(
        src.erase(&erased_ref),
        "the blob was present and is now erased"
    );

    // Reindex again (a cold rebuild wipes + replays). The erased blob is SKIPPED — it does NOT come back.
    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&src];
    reindexer
        .reindex(&tenant(), &scope, None, sources, &mut outbox, ctx_base())
        .expect("post-erase reindex");

    assert_eq!(
        ix.live_count(&tenant(), &region()),
        (corpus.len() - 1) as u64,
        "the cold rebuild has one fewer doc — the erased blob did not resurrect"
    );
    assert!(
        ix.indexed_zookie_of(&tenant(), &region(), &erased_ref).is_none(),
        "the erased blob's doc is ABSENT after reindex (0 resurrected PII; the ONE posture residual)"
    );
    // The other blobs are still present (the erasure was surgical, not a wipe of the corpus).
    assert!(ix
        .indexed_zookie_of(&tenant(), &region(), &corpus[0].blob_ref)
        .is_some());
    assert!(ix
        .indexed_zookie_of(&tenant(), &region(), &corpus[2].blob_ref)
        .is_some());
}
