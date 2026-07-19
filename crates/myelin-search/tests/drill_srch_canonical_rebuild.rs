//! # Adversarial drill — the legacy→canonical Git blob identity rebuild
//!
//! The identity cutover left the index unable to address its own documents: legacy ids survive under
//! a grammar the canonical writer never produces (see `myelin_search::canonical`). Every failure it
//! causes is SILENT, which is what makes a drill necessary — none of these show up as an error, only
//! as wrong answers.
//!
//! Each test below stages one adversarial state, runs the real coordinator over it, and asserts the
//! observable outcome. The states are chosen to be the ones a count-based or document-only check
//! would pass:
//!
//! | drill | the state it stages | what a naive check would miss |
//! |---|---|---|
//! | `legacy_live_document_*` | a live blob indexed only under its legacy id | it looks like a healthy index |
//! | `legacy_and_canonical_duplicate_*` | both ids for ONE blob | double-counted, double-ranked |
//! | `deleted_legacy_only_*` | blob deleted at the owner, legacy doc survives | deleted content still queryable |
//! | `restricted_legacy_only_*` | blob `restrict`ed, legacy doc survives | the exact content restriction suppresses |
//! | `legacy_vector_and_meta_*` | legacy VECTOR / META with no document | invisible to a doc-count check |
//! | `unrelated_corpora_*` | issues + knowledge + chat | a Git-only replay silently loses three corpora |
//! | `crash_after_wipe_*` | process dies post-wipe | restart re-wipes, or resumes into an empty index |
//! | `crash_during_replay_*` | process dies mid-replay | restart re-wipes away completed work |
//! | `concurrent_live_event_*` | a write racing the high-water boundary | applied twice, or never |
//! | `cross_tenant_*` / `cross_region_*` | a rebuild reaching outside its key | one tenant's migration damages another's index |
//! | `reads_fail_empty_*` | queries during every phase | partial answers served as complete |
//!
//! ## Why the Git blob spec is SEMANTIC here
//!
//! Production's `git`/`blob` spec is non-semantic (code search is trigram/symbol, not embedded). The
//! drill declares it semantic so that blob ids occupy the VECTOR id space too — otherwise the
//! "legacy vector survived" case could not be staged on a blob id at all, and the vector-parity leg
//! of verification would go unexercised on the identities this migration is about.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use myelin_events::reindex::ReferenceReindexSource;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EmitContextBase, EventEnvelope, OutboxTx,
    EventId, EventType, OutboxStore, ReindexSource, SnapshotScope, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_search::canonical::{is_canonical_blob_id, is_legacy_blob_id};
use myelin_search::engine::AclFilter;
use myelin_search::indexer::{
    IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher,
    SearchProjection,
};
use myelin_search::rebuild::{
    ExpectedCorpus, MemoryRebuildJournal, RebuildCoordinator, RebuildError, RebuildKey,
    RebuildPhase, RebuildReadGate, ReadMode, VerifyFailure,
};
use myelin_search::reindex::SearchReindexer;
use myelin_tenancy::{Region, TenantId};

const REGION: &str = "fr-par";
const OTHER_REGION: &str = "nl-ams";
const HOLDER: &str = "rebuild-worker-1";

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn other_tenant() -> TenantId {
    TenantId("globex".into())
}
fn region() -> Region {
    Region(REGION.into())
}
fn other_region() -> Region {
    Region(OTHER_REGION.into())
}
fn principal() -> Principal {
    Principal::stub(
        PrincipalId("platform".into()),
        PrincipalKind::Service,
        tenant(),
    )
}

/// The fence-time instant that bounds catch-up. Every drill event is recorded at
/// `2026-07-19T00:00:01Z`, so a boundary strictly after it admits them all; a drill that needs to
/// exclude an event stamps that event later.
fn fence_instant() -> Timestamp {
    Timestamp("2026-07-19T00:00:02Z".into())
}

/// The emit context the REPLAY runs under.
///
/// Its `recorded_at` is strictly AFTER [`fence_instant`], which is what production does: the replay
/// emits its `*.snapshot` rows after the fence was taken, so they sit ABOVE the catch-up ceiling and
/// are catch-up's business only through the replay path that already applied them. A fixed context
/// shared with pre-fence events would put replay snapshots below the ceiling and have catch-up
/// re-apply them — harmless (upserts are idempotent) but it would stop the boundary drill from
/// isolating the one event it is about.
fn replay_ctx() -> EmitContextBase {
    EmitContextBase {
        occurred_at: Timestamp("2026-07-19T00:00:05Z".into()),
        recorded_at: Timestamp("2026-07-19T00:00:05Z".into()),
        ..ctx_base()
    }
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(principal()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-07-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-19T00:00:00Z".into()),
        caused_by: None,
    }
}

// ───────────────────────────── identities ─────────────────────────────────────────────────────────

/// The canonical blob aggregate — the three percent-encoded components the emitter composes.
/// `ReferenceReindexSource` renders a subject as `myelin://t/<owner>/<artifact>/<aggregate>`, so an
/// aggregate that IS the component triple yields exactly the canonical doc id.
fn canonical_agg(repo: &str, ref_name: &str, path: &str) -> String {
    use myelin_events::SubjectComponent;
    format!(
        "{}:{}:{}",
        SubjectComponent::encode(repo).unwrap().as_str(),
        SubjectComponent::encode(ref_name).unwrap().as_str(),
        SubjectComponent::encode(path).unwrap().as_str(),
    )
}

/// The canonical doc id for a blob.
fn canonical_id(repo: &str, ref_name: &str, path: &str) -> String {
    format!(
        "myelin://t/git/blob/{}",
        canonical_agg(repo, ref_name, path)
    )
}

/// The LEGACY doc id for the same blob — raw, slash-delimited, ambiguous.
fn legacy_id(repo: &str, ref_name: &str, path: &str) -> String {
    format!("myelin://t/git/blob/{repo}/{ref_name}/{path}")
}

fn kn_id(page: &str) -> String {
    format!("myelin://t/knowledge/page/{page}")
}
fn issue_id(n: &str) -> String {
    format!("myelin://t/issues/issue/{n}")
}
fn chat_id(m: &str) -> String {
    format!("myelin://t/chat/message/{m}")
}

// ───────────────────────────── owner projection ───────────────────────────────────────────────────

/// The owner's `project(ref)` seam — Search's ONLY door to owner content (never an owner database).
///
/// A ref absent from the map resolves `Gone`, which is exactly how a DELETED or RESTRICTED blob
/// presents itself: the owner no longer projects it, so the live indexer removes its document rather
/// than fabricating one. Staging deletion and restriction as absence is not a shortcut — it is the
/// production semantics.
#[derive(Default)]
struct OwnerProjection {
    bodies: Mutex<BTreeMap<String, String>>,
}

impl OwnerProjection {
    fn put(&self, ref_: &str, body: &str) {
        self.bodies
            .lock()
            .unwrap()
            .insert(ref_.to_string(), body.to_string());
    }
    fn remove(&self, ref_: &str) {
        self.bodies.lock().unwrap().remove(ref_);
    }
}

impl ProjectFetcher for OwnerProjection {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        match self.bodies.lock().unwrap().get(&ref_.0) {
            Some(body) => Ok(SearchProjection {
                text: body.clone(),
                fields: BTreeMap::new(),
                lang: None,
            }),
            None => Err(ProjectFetchError::Gone),
        }
    }
}

// ───────────────────────────── the harness ────────────────────────────────────────────────────────

/// Every registered owner corpus. The rebuild must replay ALL of these: the wipe destroys the whole
/// `(tenant, region)` index, so a Git-only replay ships a rebuild that silently lost three corpora.
fn all_specs() -> Vec<IndexSpec> {
    vec![
        // Semantic in the drill so blob ids occupy the vector id space — see the module docs.
        IndexSpec::new("git", "blob", BTreeMap::new()).semantic(),
        IndexSpec::new("issues", "issue", BTreeMap::new()),
        IndexSpec::new("knowledge", "page", BTreeMap::new()).semantic(),
        IndexSpec::new("chat", "message", BTreeMap::new()),
    ]
}

struct Harness {
    indexer: Arc<IncrementalIndexer>,
    fetcher: Arc<OwnerProjection>,
    reindexer: SearchReindexer,
    journal: Arc<MemoryRebuildJournal>,
    coordinator: RebuildCoordinator,
    outbox: OutboxStore,
}

impl Harness {
    fn new() -> Harness {
        let fetcher = Arc::new(OwnerProjection::default());
        let indexer = Arc::new(IncrementalIndexer::new(
            all_specs(),
            fetcher.clone(),
            Arc::new(MockEmbeddingAdapter::new(8)),
        ));
        let reindexer = SearchReindexer::new(indexer.clone(), region());
        let journal = Arc::new(MemoryRebuildJournal::new());
        let coordinator = RebuildCoordinator::new(journal.clone(), reindexer.clone());
        Harness {
            indexer,
            fetcher,
            reindexer,
            journal,
            coordinator,
            outbox: OutboxStore::new(),
        }
    }

    fn key(&self) -> RebuildKey {
        RebuildKey::new(&tenant(), &region())
    }

    fn gate(&self) -> RebuildReadGate {
        RebuildReadGate::new(self.journal.clone())
    }

    /// Index a document through the ORDINARY live consumer path, exactly as the bus would.
    ///
    /// This is how legacy documents got into the index in the first place — there is no special
    /// "write a legacy doc" door, and using one would prove nothing about the real intake path.
    fn live_index(&self, doc_id: &str, body: &str) {
        self.fetcher.put(doc_id, body);
        self.indexer
            .index(&created_event(doc_id))
            .unwrap_or_else(|e| panic!("live index of {doc_id} failed: {e:?}"));
    }

    fn doc_ids(&self) -> Vec<String> {
        self.indexer
            .inventory(&tenant(), &region())
            .expect("inventory")
            .doc_ids
    }

    fn vector_ids(&self) -> Vec<String> {
        self.indexer
            .inventory(&tenant(), &region())
            .expect("inventory")
            .vector_doc_ids
    }

    fn meta_ids(&self) -> Vec<String> {
        self.indexer
            .inventory(&tenant(), &region())
            .expect("inventory")
            .meta_doc_ids
    }

    fn all_ids(&self) -> Vec<String> {
        self.indexer
            .inventory(&tenant(), &region())
            .expect("inventory")
            .all_ids()
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    fn ft(&self, q: &str) -> Vec<String> {
        self.indexer
            .search_ft(&tenant(), &region(), &AclFilter::All, q, 50)
            .expect("ft")
            .into_iter()
            .map(|h| h.doc_id)
            .collect()
    }

    /// Drive the whole rebuild, phase by phase, from a claim through to reopened reads.
    fn run_full_rebuild(
        &mut self,
        now: u64,
        scopes: &[SnapshotScope],
        sources: &[&dyn ReindexSource],
    ) -> Result<myelin_search::rebuild::RebuildReport, RebuildError> {
        let key = self.key();
        let c = self.coordinator.clone();
        c.claim(&key, HOLDER, now)?;
        let hwm = self.outbox.committed_count() as u64;
        c.fence(&key, HOLDER, now, hwm, &fence_instant())?;
        c.wipe(&key, HOLDER, now)?;
        c.reset_cursors(&key, HOLDER, now)?;
        let replayed = c.replay_all(
            &key,
            HOLDER,
            now,
            scopes,
            sources,
            &mut self.outbox,
            replay_ctx(),
        )?;
        let rows = self.outbox.committed_rows();
        let caught = c.catch_up(&key, HOLDER, now, &rows)?;
        let expected = ExpectedCorpus::from_index(&self.reindexer, &key, replayed, caught)?;
        c.verify_and_open(&key, HOLDER, now, &expected)
    }
}

/// A `*.created` live event for a doc — the ordinary intake shape.
fn created_event(doc: &str) -> EventEnvelope {
    typed_event(doc, subsystem_created_type(doc))
}

/// The `<subsystem>.<type>.created` token for a doc id.
fn subsystem_created_type(doc: &str) -> String {
    let rest = doc.strip_prefix("myelin://").unwrap_or(doc);
    let mut segs = rest.split('/');
    let _tenant = segs.next();
    let subsystem = segs.next().unwrap_or("git");
    let type_ = segs.next().unwrap_or("blob");
    format!("{subsystem}.{type_}.created")
}

fn typed_event(doc: &str, type_: String) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("ev:{type_}:{doc}")),
        type_: EventType(type_),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(principal()),
        subject: ArtifactRef(doc.into()),
        aggregate: AggregateKey(format!("agg:{doc}")),
        causation_id: None,
        correlation_id: CorrelationId(doc.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-07-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-19T00:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

// ───────────────────────────── owner truth ────────────────────────────────────────────────────────

/// The canonical Git blob truth: two live blobs. A DELETED or RESTRICTED blob is simply ABSENT —
/// the owner's replay enumerates current truth, and content that must not be indexed is content the
/// owner does not project.
fn git_source() -> ReferenceReindexSource {
    let mut src = ReferenceReindexSource::new("git", "blob");
    src.upsert(&canonical_agg("core", "refs/heads/main", "src/charge.rs"), 1, serde_json::json!({}));
    src.upsert(&canonical_agg("core", "refs/heads/main", "src/refund.rs"), 1, serde_json::json!({}));
    src
}

fn kn_source() -> ReferenceReindexSource {
    let mut src = ReferenceReindexSource::new("knowledge", "page");
    src.upsert("runbook", 1, serde_json::json!({}));
    src
}
fn issues_source() -> ReferenceReindexSource {
    let mut src = ReferenceReindexSource::new("issues", "issue");
    src.upsert("1421", 1, serde_json::json!({}));
    src
}
fn chat_source() -> ReferenceReindexSource {
    let mut src = ReferenceReindexSource::new("chat", "message");
    src.upsert("m-7", 1, serde_json::json!({}));
    src
}

fn all_scopes() -> Vec<SnapshotScope> {
    vec![
        SnapshotScope::new("git", "blob:all"),
        SnapshotScope::new("issues", "issue:all"),
        SnapshotScope::new("knowledge", "page:all"),
        SnapshotScope::new("chat", "message:all"),
    ]
}

/// Seed the owner projections every replayed corpus needs, so the live `index()` step can fetch a
/// body for each replayed snapshot.
fn seed_owner_bodies(h: &Harness) {
    h.fetcher.put(
        &canonical_id("core", "refs/heads/main", "src/charge.rs"),
        "fn charge() { settle_payment() }",
    );
    h.fetcher.put(
        &canonical_id("core", "refs/heads/main", "src/refund.rs"),
        "fn refund() { reverse_payment() }",
    );
    h.fetcher.put(&kn_id("runbook"), "the oncall runbook for paxos");
    h.fetcher.put(&issue_id("1421"), "an issue about raft elections");
    h.fetcher.put(&chat_id("m-7"), "a chat message about deployments");
}

/// The full rebuild every drill runs, with all four owners registered.
fn run_rebuild(h: &mut Harness, now: u64) -> Result<myelin_search::rebuild::RebuildReport, RebuildError> {
    let git = git_source();
    let kn = kn_source();
    let iss = issues_source();
    let chat = chat_source();
    let sources: Vec<&dyn ReindexSource> = vec![&git, &iss, &kn, &chat];
    let scopes = all_scopes();
    h.run_full_rebuild(now, &scopes, &sources)
}

// ═══════════════════════════ 1. legacy live document ══════════════════════════════════════════════

/// **A live blob indexed ONLY under its legacy id is re-keyed to canonical by the rebuild.**
///
/// This is the base case and the one that looks healthiest: the blob is searchable, the count is
/// right, nothing errors. It is still wrong — the document is unaddressable by the canonical writer,
/// so the next delete or restriction of that blob will silently fail to reach it.
#[test]
fn legacy_live_document_is_rekeyed_to_canonical() {
    let mut h = Harness::new();
    let legacy = legacy_id("core", "refs/heads/main", "src/charge.rs");
    h.live_index(&legacy, "fn charge() { settle_payment() }");
    seed_owner_bodies(&h);

    assert!(
        h.doc_ids().contains(&legacy),
        "precondition: the legacy document is live"
    );
    assert_eq!(h.ft("settle_payment"), vec![legacy.clone()]);

    let report = run_rebuild(&mut h, 100).expect("the rebuild completes");

    let canonical = canonical_id("core", "refs/heads/main", "src/charge.rs");
    assert!(
        h.doc_ids().contains(&canonical),
        "the blob is now indexed under its canonical identity"
    );
    assert!(
        !h.doc_ids().contains(&legacy),
        "and the legacy identity is gone"
    );
    assert_eq!(
        h.ft("settle_payment"),
        vec![canonical],
        "the SAME query now resolves to the canonical doc, exactly once"
    );
    assert_eq!(report.legacy_identities, 0);
}

// ═══════════════════════════ 2. legacy + canonical duplicate ══════════════════════════════════════

/// **A blob indexed under BOTH identities collapses to exactly one document.**
///
/// This is what an ordinary post-cutover re-index produces: the canonical write ADDS a document
/// rather than replacing the legacy one, because they are different keys. The blob then matches
/// twice, is ranked twice, and is counted twice.
#[test]
fn legacy_and_canonical_duplicate_collapses_to_one_document() {
    let mut h = Harness::new();
    let legacy = legacy_id("core", "refs/heads/main", "src/charge.rs");
    let canonical = canonical_id("core", "refs/heads/main", "src/charge.rs");
    h.live_index(&legacy, "fn charge() { settle_payment() }");
    h.live_index(&canonical, "fn charge() { settle_payment() }");
    seed_owner_bodies(&h);

    assert_eq!(
        h.ft("settle_payment").len(),
        2,
        "precondition: ONE blob answers a query TWICE (the duplicate)"
    );

    run_rebuild(&mut h, 100).expect("the rebuild completes");

    assert_eq!(
        h.ft("settle_payment"),
        vec![canonical.clone()],
        "one canonical document per live blob — the duplicate is collapsed, not merely deduped at read time"
    );
    assert_eq!(
        h.doc_ids().iter().filter(|id| id.contains("charge")).count(),
        1
    );
}

// ═══════════════════════════ 3. deleted legacy-only document ══════════════════════════════════════

/// **A blob DELETED at the owner, surviving only under its legacy id, becomes unqueryable.**
///
/// The delete removed the canonical document and could not address the legacy one, so deleted source
/// code kept answering queries. The rebuild resolves it through the owner: the blob is absent from
/// current truth, so nothing replays it and the legacy document does not come back.
#[test]
fn deleted_legacy_only_document_becomes_unqueryable() {
    let mut h = Harness::new();
    let deleted_legacy = legacy_id("core", "refs/heads/main", "src/deleted_secret.rs");
    h.live_index(&deleted_legacy, "fn deleted() { OBSOLETE_TOKEN }");
    seed_owner_bodies(&h);
    // The owner no longer projects it — the blob is gone from the indexed ref.
    h.fetcher.remove(&deleted_legacy);

    assert_eq!(
        h.ft("OBSOLETE_TOKEN").len(),
        1,
        "precondition: DELETED content is still queryable under the legacy id"
    );

    run_rebuild(&mut h, 100).expect("the rebuild completes");

    assert!(
        h.ft("OBSOLETE_TOKEN").is_empty(),
        "deleted content is unqueryable after the rebuild"
    );
    assert!(
        !h.all_ids().contains(&deleted_legacy),
        "and its identity survives in NO id space"
    );
}

// ═══════════════════════════ 4. restricted legacy-only document ═══════════════════════════════════

/// **A RESTRICTED blob surviving only under its legacy id becomes unqueryable.**
///
/// The sharpest case: `restrict` is a GDPR obligation, and the legacy document was serving precisely
/// the content the restriction exists to suppress. The restriction could not reach it because the
/// suppression addresses the canonical id.
///
/// Also asserts the restricted identity never reaches an error or a report — a restricted subject's
/// repo name and path must not leak into an operator's log while being removed.
#[test]
fn restricted_legacy_only_document_becomes_unqueryable_and_undisclosed() {
    let mut h = Harness::new();
    let restricted_legacy = legacy_id("core", "refs/heads/main", "src/patient_records.rs");
    h.live_index(&restricted_legacy, "const DIAGNOSIS_CODE = \"restricted-subject-data\";");
    seed_owner_bodies(&h);
    // Under an active restriction the owner does not project the body (`03 §6`).
    h.fetcher.remove(&restricted_legacy);

    assert_eq!(
        h.ft("restricted-subject-data").len(),
        1,
        "precondition: RESTRICTED content is still queryable under the legacy id"
    );

    let report = run_rebuild(&mut h, 100).expect("the rebuild completes");

    assert!(
        h.ft("restricted-subject-data").is_empty(),
        "restricted content is unqueryable after the rebuild"
    );
    assert!(!h.all_ids().contains(&restricted_legacy));

    // Disclosure: neither the report's Debug nor its Display may name the restricted document.
    let rendered = format!("{report:?}");
    for secret in [
        "patient_records",
        "restricted-subject-data",
        "DIAGNOSIS_CODE",
        "acme",
        "core",
    ] {
        assert!(
            !rendered.contains(secret),
            "the rebuild report must not disclose `{secret}`: {rendered}"
        );
    }
}

// ═══════════════════════════ 5. legacy vector + metadata ══════════════════════════════════════════

/// **Legacy VECTOR and METADATA entries are swept too, not just documents.**
///
/// A vector is its own id space: an embedding under a legacy id keeps answering semantic queries
/// even where no document remains, and a document-count check cannot see it. The same is true of the
/// metadata side-record, which keeps a permission re-stamp addressing a document that is gone.
#[test]
fn legacy_vector_and_metadata_entries_are_swept() {
    let mut h = Harness::new();
    let legacy = legacy_id("core", "refs/heads/main", "src/charge.rs");
    h.live_index(&legacy, "fn charge() { settle_payment() }");
    seed_owner_bodies(&h);

    // Precondition: the legacy identity occupies ALL THREE id spaces.
    assert!(h.doc_ids().contains(&legacy), "document space");
    assert!(
        h.vector_ids().contains(&legacy),
        "vector space (the drill's blob spec is semantic — see module docs)"
    );
    assert!(h.meta_ids().contains(&legacy), "metadata space");

    run_rebuild(&mut h, 100).expect("the rebuild completes");

    for (space, ids) in [
        ("document", h.doc_ids()),
        ("vector", h.vector_ids()),
        ("metadata", h.meta_ids()),
    ] {
        assert!(
            !ids.contains(&legacy),
            "the legacy identity survived the {space} space"
        );
        assert_eq!(
            ids.iter().filter(|id| is_legacy_blob_id(id)).count(),
            0,
            "NO legacy identity survives the {space} space"
        );
    }

    // And the canonical blobs DO carry vectors — the sweep removed the legacy, not the shape.
    let canonical = canonical_id("core", "refs/heads/main", "src/charge.rs");
    assert!(h.vector_ids().contains(&canonical));
}

// ═══════════════════════════ 6. unrelated corpora ═════════════════════════════════════════════════

/// **Issue, knowledge and chat documents are restored — the wipe destroyed them too.**
///
/// The failure this guards is the tempting one: the migration is *about* Git blobs, so a replay
/// scoped to Git looks correct. But the wipe is whole-index, so a Git-only replay ships an index
/// that quietly lost three corpora — and every check that only counts blobs would pass.
#[test]
fn unrelated_corpora_are_restored_by_the_rebuild() {
    let mut h = Harness::new();
    let legacy = legacy_id("core", "refs/heads/main", "src/charge.rs");
    h.live_index(&legacy, "fn charge() { settle_payment() }");
    seed_owner_bodies(&h);
    h.live_index(&kn_id("runbook"), "the oncall runbook for paxos");
    h.live_index(&issue_id("1421"), "an issue about raft elections");
    h.live_index(&chat_id("m-7"), "a chat message about deployments");

    run_rebuild(&mut h, 100).expect("the rebuild completes");

    let ids = h.doc_ids();
    for (corpus, id) in [
        ("knowledge", kn_id("runbook")),
        ("issues", issue_id("1421")),
        ("chat", chat_id("m-7")),
    ] {
        assert!(
            ids.contains(&id),
            "the {corpus} corpus was lost by the rebuild: {ids:?}"
        );
    }
    // And they are genuinely searchable, not merely present.
    assert_eq!(h.ft("paxos").len(), 1, "knowledge is searchable");
    assert_eq!(h.ft("raft").len(), 1, "issues is searchable");
    assert_eq!(h.ft("deployments").len(), 1, "chat is searchable");
    // Plus the two canonical blobs.
    assert_eq!(ids.len(), 5, "two blobs + three unrelated docs");
}

/// **A Git-only replay FAILS verification rather than shipping a lossy rebuild.**
///
/// The direct proof of the point above: if an operator replays only the corpus the migration is
/// about, the missing corpora must be caught, not shrugged off.
#[test]
fn a_git_only_replay_is_caught_as_lossy() {
    let mut h = Harness::new();
    seed_owner_bodies(&h);
    h.live_index(&kn_id("runbook"), "the oncall runbook for paxos");
    h.live_index(&issue_id("1421"), "an issue about raft elections");

    let key = h.key();
    let c = h.coordinator.clone();
    c.claim(&key, HOLDER, 100).unwrap();
    c.fence(&key, HOLDER, 100, 0, &fence_instant()).unwrap();
    c.wipe(&key, HOLDER, 100).unwrap();
    c.reset_cursors(&key, HOLDER, 100).unwrap();

    // Replay ONLY the Git scope — the tempting mistake.
    let git = git_source();
    let sources: Vec<&dyn ReindexSource> = vec![&git];
    let replayed = c
        .replay_all(
            &key,
            HOLDER,
            100,
            &[SnapshotScope::new("git", "blob:all")],
            &sources,
            &mut h.outbox,
            replay_ctx(),
        )
        .unwrap();
    let rows = h.outbox.committed_rows();
    c.catch_up(&key, HOLDER, 100, &rows).unwrap();

    // An operator asserting against a corpus they know SHOULD be there catches the loss.
    let mut expected = ExpectedCorpus::from_index(&h.reindexer, &key, replayed, 0).unwrap();
    expected.doc_ids.insert(kn_id("runbook"));
    expected.doc_ids.insert(issue_id("1421"));

    let err = c
        .verify_and_open(&key, HOLDER, 100, &expected)
        .expect_err("a lossy rebuild must NOT verify");
    assert!(
        matches!(
            err,
            RebuildError::VerificationFailed(VerifyFailure::DocCountMismatch { .. })
        ),
        "the missing corpora are caught by parity: {err}"
    );

    // And reads stay fenced — the half-rebuilt index is never served.
    assert_eq!(
        h.gate().read_mode(&tenant(), &region()),
        ReadMode::FailEmptyRebuilding,
        "a failed verification leaves reads fenced"
    );
}

// ═══════════════════════════ 7. crash after wipe ══════════════════════════════════════════════════

/// **A crash immediately after the wipe converges: the restart does NOT re-wipe, and finishes.**
///
/// The dangerous restart behaviour would be re-entering at `Claimed` and wiping again — harmless
/// here (nothing has been replayed yet) but catastrophic one phase later. The phase gate is what
/// makes it safe, so assert the gate: a second `wipe` call on the resumed job reports that it did
/// not run.
#[test]
fn crash_after_wipe_converges_without_rewiping() {
    let mut h = Harness::new();
    h.live_index(&legacy_id("core", "refs/heads/main", "src/charge.rs"), "fn charge() {}");
    seed_owner_bodies(&h);

    let key = h.key();
    let c = h.coordinator.clone();
    c.claim(&key, HOLDER, 100).unwrap();
    c.fence(&key, HOLDER, 100, 0, &fence_instant()).unwrap();
    assert!(c.wipe(&key, HOLDER, 100).unwrap(), "the wipe ran");

    // ── CRASH. A new process claims the same job. The durable phase survives. ──
    let resumed = "rebuild-worker-2";
    c.claim(&key, resumed, 500).expect("the expired lease is taken over");
    let rec = c.record(&key).unwrap().expect("the job survived the crash");
    assert_eq!(
        rec.phase,
        RebuildPhase::Wiped,
        "the restart resumes at the durable phase, not from the beginning"
    );

    assert!(
        !c.wipe(&key, resumed, 500).unwrap(),
        "the resumed process must NOT wipe a second time"
    );

    // It then finishes normally.
    c.reset_cursors(&key, resumed, 500).unwrap();
    let git = git_source();
    let kn = kn_source();
    let iss = issues_source();
    let chat = chat_source();
    let sources: Vec<&dyn ReindexSource> = vec![&git, &iss, &kn, &chat];
    let replayed = c
        .replay_all(&key, resumed, 500, &all_scopes(), &sources, &mut h.outbox, replay_ctx())
        .unwrap();
    let rows = h.outbox.committed_rows();
    let caught = c.catch_up(&key, resumed, 500, &rows).unwrap();
    let expected = ExpectedCorpus::from_index(&h.reindexer, &key, replayed, caught).unwrap();
    let report = c.verify_and_open(&key, resumed, 500, &expected).unwrap();

    assert_eq!(report.legacy_identities, 0);
    assert_eq!(
        h.gate().read_mode(&tenant(), &region()),
        ReadMode::Open,
        "reads reopen after the resumed rebuild verifies"
    );
}

// ═══════════════════════════ 8. crash during replay ═══════════════════════════════════════════════

/// **A crash MID-replay resumes without re-wiping and without losing replayed corpora.**
///
/// This is where a naive restart destroys real work: re-entering at `Claimed` would wipe away every
/// corpus already replayed, and on a large index that is an unbounded restart loop. The journal
/// records completed scopes, so the resumed replay skips them.
#[test]
fn crash_during_replay_resumes_without_rewiping_or_losing_work() {
    let mut h = Harness::new();
    seed_owner_bodies(&h);

    let key = h.key();
    let c = h.coordinator.clone();
    c.claim(&key, HOLDER, 100).unwrap();
    c.fence(&key, HOLDER, 100, 0, &fence_instant()).unwrap();
    c.wipe(&key, HOLDER, 100).unwrap();
    c.reset_cursors(&key, HOLDER, 100).unwrap();

    // Replay only the FIRST two scopes, then "crash".
    let git = git_source();
    let iss = issues_source();
    let kn = kn_source();
    let chat = chat_source();
    let sources: Vec<&dyn ReindexSource> = vec![&git, &iss, &kn, &chat];
    let partial = vec![
        SnapshotScope::new("git", "blob:all"),
        SnapshotScope::new("issues", "issue:all"),
    ];
    // `replay_all` marks the phase Replayed at the end, so drive the partial set as its own call and
    // then verify the journal recorded the finished scopes.
    c.replay_all(&key, HOLDER, 100, &partial, &sources, &mut h.outbox, replay_ctx())
        .unwrap();

    let after_partial = h.doc_ids();
    assert!(
        after_partial.iter().any(|d| d.contains("charge")),
        "the git corpus replayed before the crash"
    );
    assert!(
        after_partial.contains(&issue_id("1421")),
        "the issues corpus replayed before the crash"
    );
    let rec = c.record(&key).unwrap().unwrap();
    assert_eq!(
        rec.owners_replayed.len(),
        2,
        "the journal recorded the two finished scopes"
    );

    // ── CRASH, takeover. ──
    let resumed = "rebuild-worker-2";
    c.claim(&key, resumed, 900).unwrap();
    assert!(
        !c.wipe(&key, resumed, 900).unwrap(),
        "the resumed process must NOT wipe away the corpora already replayed"
    );
    assert!(
        h.doc_ids().iter().any(|d| d.contains("charge")),
        "the pre-crash replay survived the restart"
    );

    // Resume with the FULL scope set: the two finished scopes are skipped, the rest replay.
    c.replay_all(&key, resumed, 900, &all_scopes(), &sources, &mut h.outbox, replay_ctx())
        .unwrap();
    let rows = h.outbox.committed_rows();
    let caught = c.catch_up(&key, resumed, 900, &rows).unwrap();
    let expected = ExpectedCorpus::from_index(&h.reindexer, &key, 0, caught).unwrap();
    c.verify_and_open(&key, resumed, 900, &expected).unwrap();

    let ids = h.doc_ids();
    assert_eq!(ids.len(), 5, "every corpus is present exactly once: {ids:?}");
    assert_eq!(ids.iter().filter(|id| is_legacy_blob_id(id)).count(), 0);
}

// ═══════════════════════════ 9. concurrent live event at the boundary ═════════════════════════════

/// **A live event racing the high-water boundary is applied exactly once — neither lost nor doubled.**
///
/// The mark is taken at fence time. Events at or below it are catch-up's responsibility; events
/// above it are ordinary intake's, once reads reopen. The failure modes on either side are silent: a
/// mark taken too late leaves a permanent hole, and a boundary applied twice double-counts.
#[test]
fn a_concurrent_live_event_at_the_high_water_boundary_is_applied_exactly_once() {
    let mut h = Harness::new();
    seed_owner_bodies(&h);

    // A live event committed to the outbox BEFORE the fence — it is below the mark.
    let late_doc = kn_id("late-arrival");
    h.fetcher.put(&late_doc, "a page that landed just before the fence");

    let key = h.key();
    let c = h.coordinator.clone();
    c.claim(&key, HOLDER, 100).unwrap();

    // Stage the pre-fence event in the outbox, then take the mark ABOVE it.
    let mut tx = h.outbox.begin(
        Arc::new(myelin_events::MonotonicMinter::new()),
        ctx_base(),
    );
    tx.emit(
        myelin_events::EventDraft {
            type_: EventType("knowledge.page.created".into()),
            subject: ArtifactRef(late_doc.clone()),
            aggregate: AggregateKey("agg:late".into()),
            payload: serde_json::json!({}),
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        None,
    )
    .unwrap();
    tx.commit().unwrap();

    let hwm = h.outbox.committed_count() as u64;
    c.fence(&key, HOLDER, 100, hwm, &fence_instant()).unwrap();
    assert_eq!(hwm, 1, "the mark sits above the one pre-fence event");

    c.wipe(&key, HOLDER, 100).unwrap();
    c.reset_cursors(&key, HOLDER, 100).unwrap();

    let git = git_source();
    let kn = kn_source();
    let iss = issues_source();
    let chat = chat_source();
    let sources: Vec<&dyn ReindexSource> = vec![&git, &iss, &kn, &chat];
    let replayed = c
        .replay_all(&key, HOLDER, 100, &all_scopes(), &sources, &mut h.outbox, replay_ctx())
        .unwrap();

    // Catch up over the WHOLE committed stream: only the prefix at/below the mark is applied.
    let rows = h.outbox.committed_rows();
    assert!(
        rows.len() > hwm as usize,
        "the replay pushed more rows above the mark — the bound must still hold"
    );
    let caught = c.catch_up(&key, HOLDER, 100, &rows).unwrap();
    assert_eq!(caught, 1, "exactly the one pre-fence event was caught up");

    assert!(
        h.doc_ids().contains(&late_doc),
        "the boundary event is NOT lost"
    );
    assert_eq!(
        h.doc_ids().iter().filter(|d| **d == late_doc).count(),
        1,
        "and NOT applied twice"
    );

    // Re-running catch-up is a no-op — the phase gate holds, so a retry cannot double-apply.
    assert_eq!(
        c.catch_up(&key, HOLDER, 100, &rows).unwrap(),
        0,
        "catch-up is idempotent across a retry"
    );

    let expected = ExpectedCorpus::from_index(&h.reindexer, &key, replayed, caught).unwrap();
    c.verify_and_open(&key, HOLDER, 100, &expected).unwrap();
}

// ═══════════════════════════ 10. cross-tenant / cross-region ══════════════════════════════════════

/// **A rebuild claimed for one tenant does not touch another's index, journal, or reads.**
#[test]
fn a_cross_tenant_rebuild_attempt_is_scoped_out() {
    let h = Harness::new();
    // Index a doc for the OTHER tenant in the same region.
    let other_doc = "myelin://t/knowledge/page/other-tenant-page";
    h.fetcher.put(other_doc, "another tenant's page about paxos");
    h.indexer
        .index(&EventEnvelope {
            tenant: other_tenant(),
            ..created_event(other_doc)
        })
        .unwrap();
    assert_eq!(
        h.indexer.live_count(&other_tenant(), &region()),
        1,
        "precondition: the other tenant has a document"
    );

    let key = h.key();
    let c = h.coordinator.clone();
    c.claim(&key, HOLDER, 100).unwrap();
    c.fence(&key, HOLDER, 100, 0, &fence_instant()).unwrap();
    c.wipe(&key, HOLDER, 100).unwrap();

    assert_eq!(
        h.indexer.live_count(&other_tenant(), &region()),
        1,
        "the wipe did NOT reach the other tenant's index"
    );

    // The other tenant's reads are unaffected — a fence is per (tenant, region), not cell-wide.
    let gate = h.gate();
    assert_eq!(
        gate.read_mode(&tenant(), &region()),
        ReadMode::FailEmptyRebuilding,
        "the rebuilding tenant is fenced"
    );
    assert_eq!(
        gate.read_mode(&other_tenant(), &region()),
        ReadMode::Open,
        "an unrelated tenant keeps serving — a migration is not a cell-wide outage"
    );

    // And the other tenant's job row is independent: claiming it does not contend.
    let other_key = RebuildKey::new(&other_tenant(), &region());
    c.claim(&other_key, "other-worker", 100)
        .expect("a different tenant's rebuild never contends for this lease");
}

/// **A rebuild in one region does not touch the same tenant's index in another.**
///
/// Region is part of the key, not a filter applied afterwards — a residency mistake here would
/// destroy data in a region the operator never named.
#[test]
fn a_cross_region_rebuild_attempt_is_scoped_out() {
    let h = Harness::new();
    let doc = kn_id("eu-page");
    h.fetcher.put(&doc, "a page resident in the other region");
    h.indexer
        .index(&EventEnvelope {
            region: other_region(),
            ..created_event(&doc)
        })
        .unwrap();
    assert_eq!(h.indexer.live_count(&tenant(), &other_region()), 1);

    let key = h.key();
    let c = h.coordinator.clone();
    c.claim(&key, HOLDER, 100).unwrap();
    c.fence(&key, HOLDER, 100, 0, &fence_instant()).unwrap();
    c.wipe(&key, HOLDER, 100).unwrap();

    assert_eq!(
        h.indexer.live_count(&tenant(), &other_region()),
        1,
        "the wipe did NOT cross the region boundary"
    );
    assert_eq!(
        h.gate().read_mode(&tenant(), &other_region()),
        ReadMode::Open,
        "the same tenant keeps serving in the unaffected region"
    );
}

/// **Two holders cannot run the same rebuild at once.**
///
/// The concurrent-wipe-during-replay scenario: the second claimant must be refused while the first
/// holds an unexpired lease, and a holder whose lease was stolen must not be able to advance phases.
#[test]
fn a_second_holder_cannot_run_a_concurrent_rebuild() {
    let h = Harness::new();
    let key = h.key();
    let c = h.coordinator.clone();

    c.claim(&key, HOLDER, 100).unwrap();
    assert!(
        matches!(c.claim(&key, "rival", 101), Err(RebuildError::LeaseLost)),
        "a live lease refuses a rival"
    );

    // After expiry the rival takes over...
    c.claim(&key, "rival", 100_000).expect("an expired lease is stealable");

    // ...and the ORIGINAL holder can no longer advance: its fence epoch is stale.
    assert!(
        matches!(c.fence(&key, HOLDER, 100_001, 7, &fence_instant()), Err(RebuildError::LeaseLost)),
        "the displaced holder cannot journal a phase transition"
    );
    // The rival can.
    c.fence(&key, "rival", 100_001, 7, &fence_instant()).expect("the current holder proceeds");
}

// ═══════════════════════════ 11. reads fail-empty until verification ══════════════════════════════

/// **Reads are fail-empty in EVERY phase before `Complete`, and only reopen on a green verification.**
///
/// Asserted phase by phase rather than at the ends, because the ordering is the safety property: a
/// fence that lapses for one phase is a window in which a wiped index is served as an answer.
#[test]
fn reads_remain_fail_empty_until_verification_succeeds() {
    let mut h = Harness::new();
    h.live_index(&legacy_id("core", "refs/heads/main", "src/charge.rs"), "fn charge() {}");
    seed_owner_bodies(&h);

    let key = h.key();
    let gate = h.gate();
    let c = h.coordinator.clone();

    assert_eq!(
        gate.read_mode(&tenant(), &region()),
        ReadMode::Open,
        "before any rebuild, reads serve"
    );

    c.claim(&key, HOLDER, 100).unwrap();
    assert_eq!(
        gate.read_mode(&tenant(), &region()),
        ReadMode::FailEmptyRebuilding,
        "a CLAIM already fences — the index is about to be wiped"
    );

    for (phase, step) in [
        ("fence", 1),
        ("wipe", 2),
        ("reset_cursors", 3),
        ("replay", 4),
        ("catch_up", 5),
    ] {
        match step {
            1 => {
                c.fence(&key, HOLDER, 100, 0, &fence_instant()).unwrap();
            }
            2 => {
                c.wipe(&key, HOLDER, 100).unwrap();
            }
            3 => {
                c.reset_cursors(&key, HOLDER, 100).unwrap();
            }
            4 => {
                let git = git_source();
                let kn = kn_source();
                let iss = issues_source();
                let chat = chat_source();
                let sources: Vec<&dyn ReindexSource> = vec![&git, &iss, &kn, &chat];
                c.replay_all(&key, HOLDER, 100, &all_scopes(), &sources, &mut h.outbox, replay_ctx())
                    .unwrap();
            }
            _ => {
                let rows = h.outbox.committed_rows();
                c.catch_up(&key, HOLDER, 100, &rows).unwrap();
            }
        }
        assert_eq!(
            gate.read_mode(&tenant(), &region()),
            ReadMode::FailEmptyRebuilding,
            "reads must stay fenced through `{phase}`"
        );
        assert!(
            !gate.admits_intake(&tenant(), &region()),
            "and ordinary live intake must stay fenced through `{phase}`"
        );
    }

    // A FAILING verification does not reopen reads.
    let bogus = ExpectedCorpus {
        doc_ids: BTreeSet::from(["myelin://t/knowledge/page/never-existed".to_string()]),
        ..Default::default()
    };
    assert!(
        c.verify_and_open(&key, HOLDER, 100, &bogus).is_err(),
        "a failing verification errors"
    );
    assert_eq!(
        gate.read_mode(&tenant(), &region()),
        ReadMode::FailEmptyRebuilding,
        "and leaves reads FENCED — a half-rebuilt index is never served"
    );

    // Only a green verification reopens them.
    let expected = ExpectedCorpus::from_index(&h.reindexer, &key, 0, 0).unwrap();
    c.verify_and_open(&key, HOLDER, 100, &expected).unwrap();
    assert_eq!(
        gate.read_mode(&tenant(), &region()),
        ReadMode::Open,
        "a green verification reopens reads"
    );
    assert!(gate.admits_intake(&tenant(), &region()));
}

/// **The gate fails CLOSED.** A journal that cannot be read fences reads rather than serving.
#[test]
fn an_unreadable_journal_fences_reads() {
    struct BrokenJournal;
    impl myelin_search::rebuild::RebuildJournal for BrokenJournal {
        fn load(
            &self,
            _key: &RebuildKey,
        ) -> Result<Option<myelin_search::rebuild::RebuildRecord>, RebuildError> {
            Err(RebuildError::Journal("connection refused".into()))
        }
        fn compare_and_store(
            &self,
            _key: &RebuildKey,
            _expected: Option<u64>,
            _next: &myelin_search::rebuild::RebuildRecord,
        ) -> Result<bool, RebuildError> {
            Err(RebuildError::Journal("connection refused".into()))
        }
    }
    let gate = RebuildReadGate::new(Arc::new(BrokenJournal));
    assert_eq!(
        gate.read_mode(&tenant(), &region()),
        ReadMode::FailEmptyRebuilding,
        "an unreachable journal must fence, not serve — the alternative is serving a possibly \
         half-rebuilt index during exactly the incident that broke the journal"
    );
}

// ═══════════════════════════ 12. the final-state assertion ════════════════════════════════════════

/// **The whole adversarial corpus at once, with the full end-state assertion.**
///
/// Every staged pathology in one index, one rebuild, and the complete set of final-state claims the
/// migration owes: one canonical document per live blob; no legacy document, vector or metadata id;
/// deleted and restricted content unqueryable; unrelated corpora restored; parity and lag green.
#[test]
fn the_full_adversarial_corpus_converges_to_the_required_final_state() {
    let mut h = Harness::new();

    let charge_legacy = legacy_id("core", "refs/heads/main", "src/charge.rs");
    let charge_canonical = canonical_id("core", "refs/heads/main", "src/charge.rs");
    let refund_canonical = canonical_id("core", "refs/heads/main", "src/refund.rs");
    let deleted_legacy = legacy_id("core", "refs/heads/main", "src/deleted_secret.rs");
    let restricted_legacy = legacy_id("core", "refs/heads/main", "src/patient_records.rs");

    // (1) a legacy live document, (2) its canonical duplicate,
    h.live_index(&charge_legacy, "fn charge() { settle_payment() }");
    h.live_index(&charge_canonical, "fn charge() { settle_payment() }");
    // (3) a deleted legacy-only document, (4) a restricted legacy-only document,
    h.live_index(&deleted_legacy, "fn deleted() { OBSOLETE_TOKEN }");
    h.live_index(&restricted_legacy, "const D = \"restricted-subject-data\";");
    // (6) unrelated corpora.
    h.live_index(&kn_id("runbook"), "the oncall runbook for paxos");
    h.live_index(&issue_id("1421"), "an issue about raft elections");
    h.live_index(&chat_id("m-7"), "a chat message about deployments");

    seed_owner_bodies(&h);
    // Deleted and restricted blobs are ABSENT from owner truth — they resolve `Gone`.
    h.fetcher.remove(&deleted_legacy);
    h.fetcher.remove(&restricted_legacy);

    let report = run_rebuild(&mut h, 100).expect("the rebuild completes");

    // ── one canonical document per live blob ──
    let ids = h.doc_ids();
    assert!(ids.contains(&charge_canonical));
    assert!(ids.contains(&refund_canonical));
    assert_eq!(
        ids.iter().filter(|id| id.contains("charge")).count(),
        1,
        "exactly one document per live blob"
    );
    assert_eq!(
        h.ft("settle_payment"),
        vec![charge_canonical.clone()],
        "and it answers exactly once"
    );

    // ── no legacy document, vector, or metadata id ──
    for (space, space_ids) in [
        ("document", h.doc_ids()),
        ("vector", h.vector_ids()),
        ("metadata", h.meta_ids()),
    ] {
        assert_eq!(
            space_ids.iter().filter(|id| is_legacy_blob_id(id)).count(),
            0,
            "a legacy identity survived the {space} space: {space_ids:?}"
        );
    }
    assert_eq!(report.legacy_identities, 0);
    for blob in h.doc_ids().iter().filter(|id| id.contains("/git/blob/")) {
        assert!(
            is_canonical_blob_id(blob),
            "every surviving blob identity is canonical: {blob}"
        );
    }

    // ── deleted and restricted content unqueryable ──
    assert!(h.ft("OBSOLETE_TOKEN").is_empty(), "deleted content is gone");
    assert!(
        h.ft("restricted-subject-data").is_empty(),
        "restricted content is gone"
    );
    assert!(!h.all_ids().contains(&deleted_legacy));
    assert!(!h.all_ids().contains(&restricted_legacy));

    // ── unrelated corpora restored ──
    assert!(ids.contains(&kn_id("runbook")));
    assert!(ids.contains(&issue_id("1421")));
    assert!(ids.contains(&chat_id("m-7")));
    assert_eq!(ids.len(), 5, "two live blobs + three unrelated docs: {ids:?}");

    // ── parity and lag green ──
    assert_eq!(report.docs_indexed, 5);
    assert_eq!(h.indexer.index_lag(), 0, "zero lag before reads reopened");
    assert_eq!(
        report.vectors_indexed,
        h.vector_ids().len(),
        "vector parity holds"
    );

    // ── reads reopened ──
    assert_eq!(h.gate().read_mode(&tenant(), &region()), ReadMode::Open);

    // ── and the receipt discloses nothing ──
    let rendered = format!("{report:?}");
    for secret in [
        "acme",
        "core",
        "charge",
        "patient_records",
        "restricted-subject-data",
        "settle_payment",
        REGION,
    ] {
        assert!(
            !rendered.contains(secret),
            "the report disclosed `{secret}`: {rendered}"
        );
    }
}

/// **Catch-up is correct under the DURABLE store's row ordering, not just the in-memory one.**
///
/// The regression this pins: catch-up originally bounded itself positionally, taking the first
/// `high_water_mark` rows of the committed stream. The in-memory outbox returns rows in insertion
/// order, so a positional take looked exactly like a commit-order prefix and every in-process test
/// passed. The durable backing does not promise that — `committed_live_rows` orders by
/// `(aggregate, seq)`, aggregate-LEXICOGRAPHIC, while the count is over the whole live set. A
/// positional take therefore selected the N lexicographically-smallest-aggregate rows: an arbitrary
/// mix, silently dropping every pre-fence event on a high-sorting aggregate while live intake was
/// fenced.
///
/// So this drill hands catch-up the SAME rows in the durable store's ordering — sorted by aggregate,
/// with post-fence rows deliberately interleaved ahead of pre-fence ones — and asserts the pre-fence
/// events are still applied and the post-fence ones still are not. A positional bound cannot pass
/// this; the recorded-instant bound does.
#[test]
fn catch_up_is_correct_under_the_durable_stores_row_ordering() {
    let mut h = Harness::new();
    seed_owner_bodies(&h);

    // Three pre-fence events whose aggregates sort LAST, so a positional prefix would miss them.
    let pre: Vec<String> = ["zzz-1", "zzz-2", "zzz-3"]
        .iter()
        .map(|n| kn_id(n))
        .collect();
    for d in &pre {
        h.fetcher.put(d, "a page that landed before the fence");
    }
    // One post-fence event whose aggregate sorts FIRST — a positional prefix would wrongly take it.
    let post = kn_id("aaa-late");
    h.fetcher.put(&post, "a page that landed after the fence");

    // ONE minter across the whole drill: a fresh `MonotonicMinter` restarts its sequence, so a
    // per-emit minter would mint the same first id twice and the outbox would reject the duplicate.
    let minter: Arc<dyn myelin_events::IdMinter> =
        Arc::new(myelin_events::MonotonicMinter::new());
    let emit = |h: &mut Harness, doc: &str, agg: &str, at: &str| {
        let ctx = EmitContextBase {
            occurred_at: Timestamp(at.into()),
            recorded_at: Timestamp(at.into()),
            ..ctx_base()
        };
        let mut tx = h.outbox.begin(Arc::clone(&minter), ctx);
        tx.emit(
            myelin_events::EventDraft {
                type_: EventType("knowledge.page.created".into()),
                subject: ArtifactRef(doc.to_string()),
                aggregate: AggregateKey(agg.to_string()),
                payload: serde_json::json!({}),
                data_role: DataRole::Processor,
                visibility: Visibility::Internal,
                contains_personal_data: false,
                pii_key_ref: None,
            },
            None,
        )
        .unwrap();
        tx.commit().unwrap();
    };

    let key = h.key();
    let c = h.coordinator.clone();
    c.claim(&key, HOLDER, 100).unwrap();

    for (i, d) in pre.iter().enumerate() {
        let d = d.clone();
        emit(&mut h, &d, &format!("zzz-agg-{i}"), "2026-07-19T00:00:01Z");
    }
    let hwm = h.outbox.committed_count() as u64;
    c.fence(&key, HOLDER, 100, hwm, &fence_instant()).unwrap();

    // The post-fence write lands AFTER the ceiling.
    emit(&mut h, &post, "aaa-agg", "2026-07-19T00:00:09Z");

    c.wipe(&key, HOLDER, 100).unwrap();
    c.reset_cursors(&key, HOLDER, 100).unwrap();
    let git = git_source();
    let kn = kn_source();
    let iss = issues_source();
    let chat = chat_source();
    let sources: Vec<&dyn ReindexSource> = vec![&git, &iss, &kn, &chat];
    c.replay_all(
        &key,
        HOLDER,
        100,
        &all_scopes(),
        &sources,
        &mut h.outbox,
        replay_ctx(),
    )
    .unwrap();

    // Re-order the committed stream the way the DURABLE store returns it: by aggregate, ascending.
    // `aaa-agg` (post-fence) now sorts to the FRONT, ahead of every pre-fence `zzz-agg-*`.
    let mut rows = h.outbox.committed_rows();
    rows.sort_by(|a, b| a.aggregate.0.cmp(&b.aggregate.0));
    assert!(
        rows.iter()
            .position(|r| r.aggregate.0 == "aaa-agg")
            .unwrap()
            < rows
                .iter()
                .position(|r| r.aggregate.0.starts_with("zzz-agg"))
                .unwrap(),
        "precondition: the post-fence row sorts AHEAD of the pre-fence rows, so a positional \
         bound would take the wrong ones"
    );

    let caught = c.catch_up(&key, HOLDER, 100, &rows).unwrap();
    assert_eq!(
        caught, 3,
        "exactly the three pre-fence events are applied, regardless of row order"
    );

    let ids = h.doc_ids();
    for d in &pre {
        assert!(
            ids.contains(d),
            "a pre-fence event on a high-sorting aggregate must NOT be dropped: {d}"
        );
    }
    assert!(
        !ids.contains(&post),
        "a post-fence event must NOT be pulled in early — it is ordinary intake's business"
    );
}
