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

fn message_envelope(message_ref: &str, version: u64) -> EventEnvelope {
    let agg = AggregateKey(message_ref.to_string());
    EventEnvelope {
        event_id: myelin_events::snapshot_event_id(&tenant(), &agg, version),
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
    myelin_chat::message_index_spec()
}

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

fn rebuild_read_model(
    src: &ChatReindexSource,
    scope: &SnapshotScope,
    fetcher: &ChatProjectFetcher,
    through_outbox: bool,
) -> ChatReadModelConsumer {
    let mut rm = ChatReadModelConsumer::new();
    if through_outbox {
        let mut outbox = OutboxStore::new();
        let sources: &[&dyn ReindexSource] = &[src];
        bus_reindex(scope, None, sources, &mut outbox, ctx_base()).expect("reindex emit");
        for draft in src.replay(scope, None) {
            let row = outbox
                .row(&draft.event_id(&tenant()))
                .expect("snapshot row");
            rm.ingest(&row.envelope, fetcher);
        }
    } else {
        for draft in src.replay(scope, None) {
            let env = message_envelope(&draft.aggregate.0, draft.version);
            rm.ingest(&env, fetcher);
        }
    }
    rm
}

#[test]
fn search_message_index_cold_rebuild_byte_matches_live() {
    let corpus = corpus();
    let scope = SnapshotScope::new("chat", "message:all");
    let src = chat_source(&corpus);

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
            let row = outbox
                .row(&draft.event_id(&tenant()))
                .expect("snapshot row");
            live_ix.index(&row.envelope).expect("live index");
        }
    }
    assert_eq!(
        live_ix.live_count(&tenant(), &region()),
        corpus.len() as u64
    );
    let live_digest = index_digest(&live_ix);

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

    assert_eq!(
        cold_digest, live_digest,
        "the cold-rebuilt message index byte-matches the live index (CHAT-D15 Search leg)"
    );

    let mut fetched = cold_fetcher.fetched_refs();
    fetched.sort();
    fetched.dedup();
    let mut expected: Vec<String> = corpus.iter().map(|m| m.message_ref.clone()).collect();
    expected.sort();
    assert_eq!(
        fetched, expected,
        "the rebuild reached ONLY the owner's project(ref) (5.6) - no cross-DB read path"
    );
}

#[test]
fn search_rebuild_stays_acl_correct_non_member_sees_zero_rows() {
    let corpus = corpus();
    let scope = SnapshotScope::new("chat", "message:all");
    let src = chat_source(&corpus);

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
        "only c1's messages surface - m3 (c2) is excluded incl. count (0 unfiltered rebuilt rows)"
    );

    let non_member = AclFilter::None;
    let none = ix
        .search_ft(&tenant(), &region(), &non_member, "deadlock", 10)
        .expect("ft query");
    assert!(
        none.is_empty(),
        "a non-member sees 0 rebuilt rows (CHAT-D11 holds over the cold index, identical to live)"
    );
}

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

    assert_eq!(
        reindex_parity_hash(&cold),
        reindex_parity_hash(&live),
        "the Refs/Notif read-models rebuild byte-identically (CHAT-D15 reindex-parity hash matches)"
    );
}

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

    let cold = rebuild_read_model(&src, &scope, &fetcher, true);
    let again = rebuild_read_model(&src, &scope, &fetcher, true);
    assert_eq!(
        reindex_parity_hash(&cold),
        reindex_parity_hash(&again),
        "a re-run is idempotent (cold == live holds after a re-run)"
    );
}

#[test]
fn an_erased_message_does_not_resurrect_on_reindex() {
    let corpus = corpus();
    let scope = SnapshotScope::new("chat", "message:all");
    let erased_ref = corpus[2].message_ref.clone();

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
        "the cold index has one fewer doc - the erased message did not resurrect"
    );
    let confidential = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "confidential", 10)
        .expect("ft query");
    assert!(
        confidential.is_empty(),
        "the erased confidential message is unsearchable after reindex (0 resurrected PII)"
    );

    let cold = rebuild_read_model(&src, &scope, &fetcher, true);
    assert_eq!(
        cold.search_len(),
        2,
        "only m1/m2 rebuilt - m3 did not resurrect"
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
