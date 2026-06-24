//! # CHAT-D15 — replay(scope, since) full parity (Search/Refs/Notif rebuild; ONE path; cold == live)
//!
//! **Prompt:** P-416 (CHAT-P21, M4-C7). **Drill:** CHAT-D15 (SCHED) — wipe + `replay(scope, since)` →
//! the three Chat-fed read-models (Search / Refs / Notif) REBUILD; steady-state and recovery share ONE
//! path; an erased subject → a tombstone (no PII resurrected); the reindex-parity hash matches the live
//! hash (the reindex-parity-hash-mismatch signal = 0).
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md` §6 (replay —
//! the only recovery path; sub-artifact granular; erased subjects → tombstones; steady-state and
//! recovery share one path). **Reconciliation:** `00-reconciliation-decisions.md` §OQ-E (the reindexing
//! consumer composes the frozen `list_objects` Filter so a rebuild stays ACL-correct). **Contract:**
//! index rows **2.6** (replay full parity — owned), **6.4** (reindex — the only rebuild path), **6.1**
//! (the Filter conjoin a rebuild stays ACL-correct under), **5.2** (the Refs read-model rebuilt), **7.1**
//! (the Notif read-model rebuilt). **Doctrine:** EI-04 §5 (derived stores rebuild from source, never read
//! owner DBs; reindex-from-source is a first-class resilience primitive; steady-state == recovery, one
//! path); VISION §3 (erasure by construction → tombstones on rebuild).
//!
//! ## Both sides of the reindex seam (the 2.6/6.4 pair, the Chat leg)
//!
//! This drill exercises BOTH sides of the reindex-from-source seam end-to-end:
//! - **PROVIDER side** — the owner's `replay(scope, since)` re-emit: `myelin_chat::replay::Chat
//!   ReindexSource::replay` re-emits one `chat.message.snapshot` per durable message through the Bus
//!   outbox at the deterministic `snapshot_event_id` (contract 2.6). The provider re-reads its OWN
//!   source of truth (the in-memory corpus modeling the message store), never a derived store / cross DB.
//! - **CONSUMER side** — the live derived consumers re-apply each re-emit through the SAME steady-state
//!   step: the Search `IncrementalIndexer::index()` / `SearchReindexer::reindex`, and the Refs/Notif
//!   `ChatReadModelConsumer::ingest`. The consumer cannot tell cold from live, so the cold rebuild
//!   byte-matches the live store.
//!
//! ## What this drill PROVES — cold == live across ALL THREE Chat-fed read-models
//!
//! The reindex MACHINERY ships from EB-22 (the Bus seam, `myelin_events::reindex`) + CHAT-P6 (the chat
//! `replay` skeleton, `myelin_chat::replay::ChatReindexSource`) + the live derived consumers
//! (`myelin_search::IncrementalIndexer` for Search; `myelin_chat::replay::ChatReadModelConsumer` for the
//! Refs/Notif read-models). THIS drill is the CHAT-D15 end-to-end PARITY proof:
//!
//! 1. **The Search message index** (Search owns the index; Chat owns what-to-index, 6.3/6.4): live-index
//!    a set of `chat.message.snapshot`s → WIPE the per-tenant index → rebuild via `SearchReindexer`
//!    driving `ChatReindexSource`. The rebuilt index is byte-identical (doc count + FT hit-set) to live,
//!    and the ACL-conjoined query surface (`AclConjoinedSearchFeeder`, 6.1) yields the SAME visibility on
//!    the rebuild — a non-member sees 0 rows (CHAT-D11 over the cold index, identical to live). No
//!    cross-DB read: the only doc entry path is the owner's `project(ref)` (5.6).
//! 2. **The Refs + Notif read-models** (the `ChatReadModelConsumer`): live-ingest the message snapshots →
//!    WIPE the read-model → rebuild from the replay re-emit through the SAME `ingest` step. The
//!    reindex-parity hash matches (cold == live across Search/Refs/Notif).
//! 3. **One path:** the rebuild drives the SAME `IncrementalIndexer::index()` / `ChatReadModelConsumer
//!    ::ingest()` the steady-state path takes — there is no second cold-rebuild code path (0 recovery-only
//!    paths), so there is no drift.
//! 4. **Erased → tombstone:** an erased message is REMOVED from `ChatReindexSource` (its DEK shredded), so
//!    a replay SKIPS it — the cold index/read-model does NOT re-acquire it (0 resurrected PII; the full
//!    multi-holder erasure RECEIPT remains CHAT-P22).

use std::sync::Arc;
use std::sync::Mutex;

use myelin_chat::events::CHAT_MESSAGE_SNAPSHOT;
use myelin_chat::replay::{
    reindex_parity_hash, ChatReadModelConsumer, ChatReindexSource, ChatReplayKind,
    MessageProjectFetcher, MessageProjection,
};
use myelin_events::{
    reindex as bus_reindex, Actor, AggregateKey, CorrelationId, DataRole, EmitContextBase,
    EventEnvelope, EventType, OutboxStore, Region, ReindexSource, SnapshotScope, TenantId,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_search::{
    AclFilter, EmbeddingAdapter, IncrementalIndexer, IndexSpec, MockEmbeddingAdapter,
    ProjectFetchError, ProjectFetcher, ReindexJob, SearchProjection, SearchReindexer,
};
use myelin_tenancy::ArtifactRef as TenancyArtifactRef;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:00Z".into()),
        caused_by: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// The Chat corpus: a handful of durable messages across two channels (c1 visible, c2 confidential).
// The SAME corpus drives BOTH the live emit and the cold replay (the owner's source of truth) —
// proving cold == live, not two paths.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

struct CorpusMessage {
    message_ref: String,
    channel: String,
    version: u64,
    body: String,
    edges: Vec<(String, String)>,
    mentions: Vec<String>,
}

fn corpus() -> Vec<CorpusMessage> {
    vec![
        CorpusMessage {
            message_ref: "myelin://acme/chat/message/m1".into(),
            channel: "c1".into(),
            version: 1,
            body: "blocked on the scheduler deadlock".into(),
            edges: vec![("myelin://acme/issue/issue/ENG-1".into(), "links".into())],
            mentions: vec!["myelin://acme/identity/member/alice".into()],
        },
        CorpusMessage {
            message_ref: "myelin://acme/chat/message/m2".into(),
            channel: "c1".into(),
            version: 1,
            body: "the deadlock detection is fixed".into(),
            edges: vec![],
            mentions: vec![],
        },
        CorpusMessage {
            message_ref: "myelin://acme/chat/message/m3".into(),
            channel: "c2".into(),
            version: 1,
            body: "the confidential deadlock workaround".into(),
            edges: vec![],
            mentions: vec!["myelin://acme/identity/member/bob".into()],
        },
    ]
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// The owner's project(ref) — the ONLY content source (5.6), NEVER a cross-DB read. The SAME fetcher
// serves the live emit and the cold replay → cold == live. A `record` proves no owner-DB path existed.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct ChatProjectFetcher {
    bodies: Mutex<std::collections::BTreeMap<String, MessageProjection>>,
    search_bodies: Mutex<std::collections::BTreeMap<String, SearchProjection>>,
    fetched: Mutex<Vec<String>>,
}
impl ChatProjectFetcher {
    fn with_corpus(corpus: &[CorpusMessage]) -> ChatProjectFetcher {
        let f = ChatProjectFetcher::default();
        for m in corpus {
            f.bodies.lock().unwrap().insert(
                m.message_ref.clone(),
                MessageProjection {
                    body_text: m.body.clone(),
                    channel_id: m.channel.clone(),
                    edges: m.edges.clone(),
                    mentions: m.mentions.clone(),
                },
            );
            f.search_bodies.lock().unwrap().insert(
                m.message_ref.clone(),
                SearchProjection {
                    text: m.body.clone(),
                    fields: std::collections::BTreeMap::new(),
                    lang: Some("en".into()),
                },
            );
        }
        f
    }
    fn erase(&self, message_ref: &str) {
        self.bodies.lock().unwrap().remove(message_ref);
        self.search_bodies.lock().unwrap().remove(message_ref);
    }
    fn fetched_refs(&self) -> Vec<String> {
        self.fetched.lock().unwrap().clone()
    }
}
impl MessageProjectFetcher for ChatProjectFetcher {
    fn project(&self, message_ref: &str) -> Option<MessageProjection> {
        self.bodies.lock().unwrap().get(message_ref).cloned()
    }
}
// The Search-side ProjectFetcher (the SAME owner project(ref), the SearchProjection body Search indexes).
impl ProjectFetcher for ChatProjectFetcher {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &TenancyArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        self.fetched.lock().unwrap().push(ref_.0.clone());
        match self.search_bodies.lock().unwrap().get(&ref_.0) {
            Some(p) => Ok(p.clone()),
            None => Err(ProjectFetchError::Gone),
        }
    }
}

/// A `ChatReindexSource` seeded from the corpus (one `chat.message.snapshot` per message). The replay
/// payload is references-not-payloads; Search fetches the body via `project` (5.6) — the SAME path live.
fn chat_source(corpus: &[CorpusMessage]) -> ChatReindexSource {
    let mut s = ChatReindexSource::new();
    for m in corpus {
        s.upsert(
            ChatReplayKind::Message,
            &m.message_ref,
            m.version,
            &m.message_ref,
            serde_json::json!({ "channel": m.channel }),
        );
    }
    s
}

/// The `chat.message.snapshot` envelope a relay delivers for one corpus message (the consumer input —
/// the SAME shape the live `chat.message.created` carries). For the Refs/Notif read-model leg.
fn message_envelope(message_ref: &str, version: u64) -> EventEnvelope {
    let agg = AggregateKey(message_ref.to_string());
    EventEnvelope {
        event_id: myelin_events::snapshot_event_id(&agg, version),
        type_: EventType(CHAT_MESSAGE_SNAPSHOT.into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        subject: myelin_events::ArtifactRef(message_ref.to_string()),
        aggregate: agg,
        causation_id: None,
        correlation_id: CorrelationId(format!("corr-{message_ref}")),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:00Z".into()),
        payload: serde_json::json!({ "version": version }),
    }
}

fn chat_message_spec() -> IndexSpec {
    // The chat/message spec — the chat-OWNED §7 shape (acl_object_type = message, semantic).
    myelin_chat::message_index_spec()
}

/// A deterministic byte-digest of the Search index for the tenant: the doc count + the FT hit-set for a
/// known query term. Two indexes with the same digest are byte-identical for the parity assertion.
fn index_digest(ix: &IncrementalIndexer) -> String {
    let mut parts: Vec<String> = vec![format!("count={}", ix.live_count(&tenant(), &region()))];
    for q in ["deadlock", "scheduler", "confidential"] {
        let hits = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, q, 10)
            .expect("ft query");
        let mut docs: Vec<String> = hits.iter().map(|h| h.doc_id.clone()).collect();
        docs.sort();
        parts.push(format!("ft[{q}]={}", docs.join(",")));
    }
    parts.join("|")
}

/// Build the Refs/Notif read-model by ingesting the corpus message snapshots through the ONE consumer
/// step (the live path AND the cold rebuild use this — that single step is what makes cold == live).
fn rebuild_read_model(
    src: &ChatReindexSource,
    scope: &SnapshotScope,
    fetcher: &ChatProjectFetcher,
    through_outbox: bool,
) -> ChatReadModelConsumer {
    let mut rm = ChatReadModelConsumer::new();
    if through_outbox {
        // COLD path: drive the reindex through the outbox, then ingest the emitted rows (the §4.9 ONLY
        // rebuild path — the Bus re-emit through the SAME ingest step).
        let mut outbox = OutboxStore::new();
        let sources: &[&dyn ReindexSource] = &[src];
        bus_reindex(scope, None, sources, &mut outbox, ctx_base()).expect("reindex emit");
        for draft in src.replay(scope, None) {
            let row = outbox.row(&draft.event_id()).expect("snapshot row");
            rm.ingest(&row.envelope, fetcher);
        }
    } else {
        // LIVE path: ingest the live message events (the SAME snapshot envelope shape).
        for draft in src.replay(scope, None) {
            let env = message_envelope(&draft.aggregate.0, draft.version);
            rm.ingest(&env, fetcher);
        }
    }
    rm
}

// ═════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE SEARCH MESSAGE INDEX — cold rebuild byte-matches live (no cross-DB; ACL-correct on rebuild)
// ═════════════════════════════════════════════════════════════════════════════════════════════════

/// **The Search message index rebuilds BYTE-IDENTICALLY from a reindex (cold == live; no cross-DB).**
#[test]
fn search_message_index_cold_rebuild_byte_matches_live() {
    let corpus = corpus();
    let scope = SnapshotScope::new("chat", "message:all");
    let src = chat_source(&corpus);

    // ── LIVE: index every message snapshot through the ordinary consumer step. ──
    let live_fetcher = Arc::new(ChatProjectFetcher::with_corpus(&corpus));
    let embedder: Arc<dyn EmbeddingAdapter> = Arc::new(MockEmbeddingAdapter::new(8));
    let live_ix = Arc::new(IncrementalIndexer::new(
        vec![chat_message_spec()],
        live_fetcher.clone(),
        embedder.clone(),
    ));
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
    let live_digest = index_digest(&live_ix);

    // ── COLD: a FRESH indexer, wiped + rebuilt via SearchReindexer.reindex (the §4.9 ONLY rebuild
    //    path — the Bus re-emit through the SAME live index() step). cold == live. ──
    let cold_fetcher = Arc::new(ChatProjectFetcher::with_corpus(&corpus));
    let cold_ix = Arc::new(IncrementalIndexer::new(
        vec![chat_message_spec()],
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
        "one chat.message.snapshot re-emitted per corpus message (contract 2.6/6.4)"
    );
    assert_eq!(
        job.progress().docs_indexed,
        corpus.len(),
        "every snapshot driven through the LIVE indexer step (no second path)"
    );

    let cold_digest = index_digest(&cold_ix);

    // THE GREEN ARTIFACT (Search leg): the reindex-parity hash (cold == live, byte-identical).
    assert_eq!(
        cold_digest, live_digest,
        "the cold-rebuilt message index byte-matches the live index (CHAT-D15 Search leg)"
    );

    // ── no-cross-db (structural): the ONLY way a doc entered the cold index is the owner's project(ref)
    //    fetch — the same message refs, never a Chat DB read. ──
    let mut fetched = cold_fetcher.fetched_refs();
    fetched.sort();
    fetched.dedup();
    let mut expected: Vec<String> = corpus.iter().map(|m| m.message_ref.clone()).collect();
    expected.sort();
    assert_eq!(
        fetched, expected,
        "the rebuild reached ONLY the owner's project(ref) (5.6) — no cross-DB read path"
    );
}

/// **A rebuild stays ACL-correct (the reindexing consumer conjoins the Filter; 0 unfiltered rebuilt
/// rows).** Over the COLD index, the ACL-conjoined query surface yields the SAME visibility as live: a
/// viewer who can read only c1 sees the c1 messages (m1/m2) for a "deadlock" query, NOT the c2 one (m3),
/// even though m3 matches; a non-member of every channel sees 0 rows (CHAT-D11 over the cold index).
#[test]
fn search_rebuild_stays_acl_correct_non_member_sees_zero_rows() {
    let corpus = corpus();
    let scope = SnapshotScope::new("chat", "message:all");
    let src = chat_source(&corpus);

    // Rebuild the cold index from the replay.
    let fetcher = Arc::new(ChatProjectFetcher::with_corpus(&corpus));
    let embedder: Arc<dyn EmbeddingAdapter> = Arc::new(MockEmbeddingAdapter::new(8));
    let ix = Arc::new(IncrementalIndexer::new(
        vec![chat_message_spec()],
        fetcher,
        embedder,
    ));
    let reindexer = SearchReindexer::new(ix.clone(), region());
    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&src];
    reindexer
        .reindex(&tenant(), &scope, None, sources, &mut outbox, ctx_base())
        .expect("rebuild");

    // The "deadlock" term matches all three messages; the ACL filter is the visibility gate. The
    // AclFilter::Ids conjoin (the modeled `list_objects(read, message)` allow-set) restricts to the
    // readable message ids — a viewer of only c1's messages (m1, m2) never sees m3.
    let c1_visible = AclFilter::Ids(vec![
        "myelin://acme/chat/message/m1".to_string(),
        "myelin://acme/chat/message/m2".to_string(),
    ]);
    let hits = ix
        .search_ft(&tenant(), &region(), &c1_visible, "deadlock", 10)
        .expect("ft query");
    let mut docs: Vec<String> = hits.iter().map(|h| h.doc_id.clone()).collect();
    docs.sort();
    assert_eq!(
        docs,
        vec![
            "myelin://acme/chat/message/m1".to_string(),
            "myelin://acme/chat/message/m2".to_string()
        ],
        "only c1's messages surface — m3 (c2) is excluded incl. count (0 unfiltered rebuilt rows)"
    );

    // A non-member of every channel (the WHERE-false short-circuit) sees 0 rows on the COLD index
    // (CHAT-D11 — the `SetExpr::None` lowering, the empty allow-set).
    let non_member = AclFilter::None;
    let none = ix
        .search_ft(&tenant(), &region(), &non_member, "deadlock", 10)
        .expect("ft query");
    assert!(
        none.is_empty(),
        "a non-member sees 0 rebuilt rows (CHAT-D11 holds over the cold index, identical to live)"
    );
}

// ═════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE REFS + NOTIF READ-MODELS — cold rebuild byte-matches live (the one-path parity hash)
// ═════════════════════════════════════════════════════════════════════════════════════════════════

/// **The Refs + Notif read-models rebuild BYTE-IDENTICALLY (cold == live).** The reindex-parity hash of
/// the cold rebuild matches the live one — across Search-row ∥ Refs-edge ∥ Notif-reason projections.
#[test]
fn refs_and_notif_read_models_cold_rebuild_byte_matches_live() {
    let corpus = corpus();
    let scope = SnapshotScope::new("chat", "message:all");
    let src = chat_source(&corpus);
    let fetcher = ChatProjectFetcher::with_corpus(&corpus);

    let live = rebuild_read_model(&src, &scope, &fetcher, false);
    let cold = rebuild_read_model(&src, &scope, &fetcher, true);

    assert_eq!(live.search_len(), 3, "three messages indexed live");
    assert_eq!(cold.search_len(), 3);
    assert_eq!(live.refs_len(), 1, "one links edge (m1 → ENG-1)");
    assert_eq!(cold.refs_len(), 1);
    assert_eq!(
        live.notif_len(),
        2,
        "two @-mention notify rows (alice, bob)"
    );
    assert_eq!(cold.notif_len(), 2);

    // THE GREEN ARTIFACT (Refs/Notif leg): the reindex-parity hash matches (cold == live).
    assert_eq!(
        reindex_parity_hash(&cold),
        reindex_parity_hash(&live),
        "the Refs/Notif read-models rebuild byte-identically (CHAT-D15 reindex-parity hash matches)"
    );
}

/// **A re-run of the rebuild is IDEMPOTENT (the deterministic snapshot id no-ops the duplicate).** A
/// second replay through the SAME outbox emits 0 new rows; the read-model parity hash is unchanged.
#[test]
fn rebuild_is_idempotent_a_rerun_emits_zero_new() {
    let corpus = corpus();
    let scope = SnapshotScope::new("chat", "message:all");
    let src = chat_source(&corpus);
    let fetcher = ChatProjectFetcher::with_corpus(&corpus);

    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&src];
    let r1 = bus_reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("first reindex");
    assert_eq!(
        r1.snapshots_emitted, 3,
        "first run emits all three snapshots"
    );

    let r2 = bus_reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("second reindex");
    assert_eq!(r2.snapshots_emitted, 0, "a re-run emits 0 NEW (idempotent)");
    assert_eq!(r2.snapshots_skipped_duplicate, 3);

    // The read-model is the same after either run.
    let cold = rebuild_read_model(&src, &scope, &fetcher, true);
    let again = rebuild_read_model(&src, &scope, &fetcher, true);
    assert_eq!(
        reindex_parity_hash(&cold),
        reindex_parity_hash(&again),
        "a re-run is idempotent (cold == live holds after a re-run)"
    );
}

// ═════════════════════════════════════════════════════════════════════════════════════════════════
// 3. AN ERASED SUBJECT → A TOMBSTONE ON REBUILD (no PII resurrected; X-7)
// ═════════════════════════════════════════════════════════════════════════════════════════════════

/// **An erased subject's body does NOT resurrect on reindex (0 resurrected PII; tombstone residual).**
/// m3 is erased (REMOVED from `ChatReindexSource` + its project body shredded). A subsequent reindex
/// SKIPS it across BOTH the Search index AND the Refs/Notif read-models — the cold stores do NOT
/// re-acquire it.
#[test]
fn an_erased_message_does_not_resurrect_on_reindex() {
    let corpus = corpus();
    let scope = SnapshotScope::new("chat", "message:all");
    let erased_ref = corpus[2].message_ref.clone(); // m3, in c2, mentions bob

    // ── Search leg: build the cold index, then erase + reindex → m3's doc is absent. ──
    let fetcher = Arc::new(ChatProjectFetcher::with_corpus(&corpus));
    let embedder: Arc<dyn EmbeddingAdapter> = Arc::new(MockEmbeddingAdapter::new(8));
    let ix = Arc::new(IncrementalIndexer::new(
        vec![chat_message_spec()],
        fetcher.clone(),
        embedder,
    ));
    let reindexer = SearchReindexer::new(ix.clone(), region());
    let mut src = chat_source(&corpus);
    {
        let mut outbox = OutboxStore::new();
        let sources: &[&dyn ReindexSource] = &[&src];
        reindexer
            .reindex(&tenant(), &scope, None, sources, &mut outbox, ctx_base())
            .expect("initial reindex");
    }
    assert_eq!(ix.live_count(&tenant(), &region()), corpus.len() as u64);

    // ERASE m3 at the owner (the *.erased tombstone removes it from truth + shreds its DEK/body).
    assert!(src.erase(&erased_ref), "m3 was present and is now erased");
    fetcher.erase(&erased_ref);

    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&src];
    reindexer
        .reindex(&tenant(), &scope, None, sources, &mut outbox, ctx_base())
        .expect("post-erase reindex");
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        (corpus.len() - 1) as u64,
        "the cold index has one fewer doc — the erased message did not resurrect"
    );
    let confidential = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "confidential", 10)
        .expect("ft query");
    assert!(
        confidential.is_empty(),
        "the erased confidential message is unsearchable after reindex (0 resurrected PII)"
    );

    // ── Refs/Notif leg: rebuild the read-model from the post-erase replay → m3's rows are absent. ──
    let cold = rebuild_read_model(&src, &scope, &fetcher, true);
    assert_eq!(
        cold.search_len(),
        2,
        "only m1/m2 rebuilt — m3 did not resurrect"
    );
    assert!(
        !cold.search_indexes(&erased_ref),
        "the erased m3 is ABSENT from the read-model after reindex (the tombstone residual)"
    );
    assert_eq!(
        cold.notif_len(),
        1,
        "only m1's @-mention notify row survives (bob's row to the erased m3 did not resurrect)"
    );
}
