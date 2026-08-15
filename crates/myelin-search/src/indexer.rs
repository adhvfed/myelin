use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use myelin_query::{FieldType, FieldValue};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

use crate::engine::{IndexBackend, IndexDocument, TantivyBackend};
use crate::vector::{Embedding, ModelRef};

pub const INDEXER_CONSUMER: &str = "search-incremental-indexer";

pub static INDEXER_SUBJECTS: &[SubjectPattern] = &[];

pub const INDEXER_SUBJECT_PREFIXES: &[&str] = &["issue.", "knowledge.", "chat.", "git.", "authz."];

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct IndexSpec {
    pub subsystem: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub struct_fields: BTreeMap<String, FieldType>,
    pub semantic: bool,
    pub acl_object_type: String,
}

impl IndexSpec {
    pub fn new(
        subsystem: impl Into<String>,
        type_: impl Into<String>,
        struct_fields: BTreeMap<String, FieldType>,
    ) -> IndexSpec {
        let type_ = type_.into();
        IndexSpec {
            subsystem: subsystem.into(),
            acl_object_type: type_.clone(),
            type_,
            struct_fields,
            semantic: false,
        }
    }

    pub fn semantic(mut self) -> IndexSpec {
        self.semantic = true;
        self
    }

    pub fn with_acl_object_type(mut self, acl_object_type: impl Into<String>) -> IndexSpec {
        self.acl_object_type = acl_object_type.into();
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchProjection {
    pub text: String,
    pub fields: BTreeMap<String, FieldValue>,
    pub lang: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectFetchError {
    Unavailable(String),
    Gone,
}

pub trait ProjectFetcher: Send + Sync {
    fn project(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError>;
}

pub trait EmbeddingAdapter: Send + Sync {
    fn embed(&self, text: &str) -> Option<Embedding>;

    fn model_ref(&self) -> ModelRef;
}

#[derive(Clone, Debug)]
pub struct MockEmbeddingAdapter {
    model_ref: ModelRef,
    dim: usize,
}

impl MockEmbeddingAdapter {
    pub const DEFAULT_MODEL: &'static str = "mock-embed-v1";

    pub fn new(dim: usize) -> MockEmbeddingAdapter {
        MockEmbeddingAdapter {
            model_ref: ModelRef(Self::DEFAULT_MODEL.to_string()),
            dim: dim.max(1),
        }
    }

    pub fn with_model(model_ref: impl Into<ModelRef>, dim: usize) -> MockEmbeddingAdapter {
        MockEmbeddingAdapter {
            model_ref: model_ref.into(),
            dim: dim.max(1),
        }
    }
}

impl EmbeddingAdapter for MockEmbeddingAdapter {
    fn embed(&self, text: &str) -> Option<Embedding> {
        if text.trim().is_empty() {
            return None;
        }
        let mut v = vec![0.0f32; self.dim];
        for (d, slot) in v.iter_mut().enumerate() {
            let mut h: u64 = 0xcbf29ce484222325 ^ (d as u64).wrapping_mul(0x100000001b3);
            for &b in text.as_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            let frac = (h >> 40) as f32 / (1u64 << 24) as f32;
            *slot = frac * 2.0 - 1.0;
        }
        Some(Embedding::new(v))
    }

    fn model_ref(&self) -> ModelRef {
        self.model_ref.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PartKey {
    tenant: TenantId,
    region: Region,
}

struct IndexRegistry {
    indices: Mutex<HashMapBackends>,
    facets: BTreeMap<String, FieldType>,
}

type HashMapBackends = std::collections::HashMap<PartKey, TantivyBackend>;

impl IndexRegistry {
    fn new(facets: BTreeMap<String, FieldType>) -> IndexRegistry {
        IndexRegistry {
            indices: Mutex::new(std::collections::HashMap::new()),
            facets,
        }
    }

    fn with_backend<T>(
        &self,
        tenant: &TenantId,
        region: &Region,
        f: impl FnOnce(&mut TantivyBackend) -> Result<T, crate::engine::IndexError>,
    ) -> Result<T, crate::engine::IndexError> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut guard = self.indices.lock().unwrap_or_else(|e| e.into_inner());
        if !guard.contains_key(&pk) {
            let be = TantivyBackend::open(&self.facets)?;
            guard.insert(pk.clone(), be);
        }
        let be = guard.get_mut(&pk).expect("backend just inserted");
        f(be)
    }

    fn try_live_count(
        &self,
        tenant: &TenantId,
        region: &Region,
    ) -> Result<u64, crate::engine::IndexError> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut guard = self.indices.lock().unwrap_or_else(|e| e.into_inner());
        match guard.get_mut(&pk) {
            Some(be) => be.snapshot(),
            None => Ok(0),
        }
    }

    fn live_count(&self, tenant: &TenantId, region: &Region) -> u64 {
        self.try_live_count(tenant, region)
            .unwrap_or_else(|e| panic!("live index snapshot failed: {e}"))
    }

    fn locate_subject(
        &self,
        tenant: &TenantId,
        region: &Region,
        matcher: &crate::engine::SubjectMatcher,
    ) -> Vec<String> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let guard = self.indices.lock().unwrap_or_else(|e| e.into_inner());
        match guard.get(&pk) {
            Some(be) => be.locate_subject(matcher),
            None => Vec::new(),
        }
    }

    fn compact(&self, tenant: &TenantId, region: &Region) -> Result<(), crate::engine::IndexError> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut guard = self.indices.lock().unwrap_or_else(|e| e.into_inner());
        match guard.get_mut(&pk) {
            Some(be) => be.merge(),
            None => Ok(()),
        }
    }

    fn wipe(&self, tenant: &TenantId, region: &Region) {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut guard = self.indices.lock().unwrap_or_else(|e| e.into_inner());
        guard.remove(&pk);
    }

    fn has_orphan_embedding(&self, tenant: &TenantId, region: &Region) -> bool {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let guard = self.indices.lock().unwrap_or_else(|e| e.into_inner());
        match guard.get(&pk) {
            Some(be) => be.vectors().has_orphan_embedding(),
            None => false,
        }
    }

    fn live_vector_count(&self, tenant: &TenantId, region: &Region) -> usize {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let guard = self.indices.lock().unwrap_or_else(|e| e.into_inner());
        match guard.get(&pk) {
            Some(be) => be.vectors().live_len(),
            None => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexEventError {
    Malformed(String),
    Engine(String),
    Transient(String),
}

#[derive(Clone)]
pub struct IncrementalIndexer {
    registry: Arc<IndexRegistry>,
    specs: Arc<BTreeMap<(String, String), IndexSpec>>,
    fetcher: Arc<dyn ProjectFetcher>,
    embedder: Arc<dyn EmbeddingAdapter>,
    index_lag: Arc<AtomicU64>,
}

impl IncrementalIndexer {
    pub const INDEX_LAG_SIGNAL: &'static str = "search.index_lag";

    pub const PERMISSION_CHANGED_SUFFIX: &'static str = "permission.changed";

    pub const REMOVED_SUFFIXES: &'static [&'static str] = &["deleted", "removed", "erased"];

    pub fn new(
        specs: Vec<IndexSpec>,
        fetcher: Arc<dyn ProjectFetcher>,
        embedder: Arc<dyn EmbeddingAdapter>,
    ) -> IncrementalIndexer {
        let mut facets: BTreeMap<String, FieldType> = BTreeMap::new();
        let mut by_key: BTreeMap<(String, String), IndexSpec> = BTreeMap::new();
        for spec in specs {
            for (name, ty) in &spec.struct_fields {
                facets.insert(name.clone(), *ty);
            }
            by_key.insert((spec.subsystem.clone(), spec.type_.clone()), spec);
        }
        IncrementalIndexer {
            registry: Arc::new(IndexRegistry::new(facets)),
            specs: Arc::new(by_key),
            fetcher,
            embedder,
            index_lag: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn index_lag(&self) -> u64 {
        self.index_lag.load(Ordering::SeqCst)
    }

    pub fn live_count(&self, tenant: &TenantId, region: &Region) -> u64 {
        self.registry.live_count(tenant, region)
    }

    pub fn try_live_count(
        &self,
        tenant: &TenantId,
        region: &Region,
    ) -> Result<u64, IndexEventError> {
        self.registry
            .try_live_count(tenant, region)
            .map_err(|e| IndexEventError::Engine(e.to_string()))
    }

    pub fn search_ft(
        &self,
        tenant: &TenantId,
        region: &Region,
        acl_filter: &crate::engine::AclFilter,
        text_query: &str,
        limit: usize,
    ) -> Result<Vec<crate::engine::Hit>, crate::engine::IndexError> {
        self.registry.with_backend(tenant, region, |be| {
            be.search(acl_filter, text_query, limit)
        })
    }

    pub fn search_structured(
        &self,
        tenant: &TenantId,
        region: &Region,
        acl_filter: &crate::engine::AclFilter,
        field: &str,
        value: &FieldValue,
        limit: usize,
    ) -> Result<Vec<crate::engine::Hit>, crate::engine::IndexError> {
        use crate::engine::IndexBackend;
        self.registry.with_backend(tenant, region, |be| {
            be.search_structured(acl_filter, field, value, limit)
        })
    }

    pub fn search_semantic(
        &self,
        tenant: &TenantId,
        region: &Region,
        acl_filter: &crate::engine::AclFilter,
        query: &Embedding,
        k: usize,
    ) -> Result<Vec<crate::vector::VectorHit>, crate::engine::IndexError> {
        use crate::engine::IndexBackend;
        self.registry
            .with_backend(tenant, region, |be| be.semantic(acl_filter, query, k))
    }

    pub fn indexed_zookie_of(
        &self,
        tenant: &TenantId,
        region: &Region,
        doc_id: &str,
    ) -> Option<String> {
        self.registry
            .with_backend(tenant, region, |be| Ok(be.indexed_zookie_of(doc_id)))
            .ok()
            .flatten()
    }

    pub fn locate_subject(
        &self,
        tenant: &TenantId,
        region: &Region,
        matcher: &crate::engine::SubjectMatcher,
    ) -> Vec<String> {
        self.registry.locate_subject(tenant, region, matcher)
    }

    pub fn compact(
        &self,
        tenant: &TenantId,
        region: &Region,
    ) -> Result<(), crate::engine::IndexError> {
        self.registry.compact(tenant, region)
    }

    pub fn has_orphan_embedding(&self, tenant: &TenantId, region: &Region) -> bool {
        self.registry.has_orphan_embedding(tenant, region)
    }

    pub fn live_vector_count(&self, tenant: &TenantId, region: &Region) -> usize {
        self.registry.live_vector_count(tenant, region)
    }

    pub fn wipe(&self, tenant: &TenantId, region: &Region) {
        self.registry.wipe(tenant, region);
    }

    pub fn index(&self, ev: &EventEnvelope) -> Result<(), IndexEventError> {
        self.index_lag.fetch_add(1, Ordering::SeqCst);
        let result = self.index_inner(ev);
        self.index_lag.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn index_inner(&self, ev: &EventEnvelope) -> Result<(), IndexEventError> {
        let subject = Self::canonical_reference(&ev.subject, &ev.tenant)?;
        let type_ = ev.type_.0.as_str();

        if type_.ends_with(Self::PERMISSION_CHANGED_SUFFIX) {
            return self.apply_permission_changed(ev);
        }

        let event_name = type_.rsplit('.').next().unwrap_or("");
        if Self::REMOVED_SUFFIXES.contains(&event_name) {
            return self.apply_removed(ev);
        }

        self.apply_upsert(ev, &subject)
    }

    fn canonical_reference(
        reference: &ArtifactRef,
        tenant: &TenantId,
    ) -> Result<myelin_refs::ParsedArtifactRef, IndexEventError> {
        let parsed = myelin_refs::parse_scoped(&reference.0).map_err(|error| {
            IndexEventError::Malformed(format!("event carries an invalid ArtifactRef: {error}"))
        })?;
        if parsed.artifact_ref != *reference || parsed.tenant != *tenant {
            return Err(IndexEventError::Malformed(
                "event ArtifactRef is non-canonical or belongs to another tenant".into(),
            ));
        }
        Ok(parsed)
    }

    fn apply_upsert(
        &self,
        ev: &EventEnvelope,
        subject: &myelin_refs::ParsedArtifactRef,
    ) -> Result<(), IndexEventError> {
        let ref_ = &ev.subject;
        let subsystem = &subject.subsystem;
        let type_ = &subject.type_;

        let spec = match self.specs.get(&(subsystem.clone(), type_.clone())) {
            Some(s) => s.clone(),
            None => return Ok(()),
        };

        let projection = match self.fetcher.project(&ev.tenant, &ev.region, ref_) {
            Ok(p) => p,
            Err(ProjectFetchError::Unavailable(why)) => {
                return Err(IndexEventError::Transient(why))
            }
            Err(ProjectFetchError::Gone) => {
                return self.remove_doc(&ev.tenant, &ev.region, &ref_.0);
            }
        };

        let lang = projection
            .lang
            .clone()
            .unwrap_or_else(|| Self::detect_lang(&projection.text));
        let _analyzed_terms = crate::analysis::Analyzer::for_tag(&lang).analyze(&projection.text);

        let acl_object = myelin_refs::strip_sub(&subject.artifact_ref).0;
        debug_assert_eq!(
            subject.sub.is_some(),
            acl_object != ref_.0,
            "a sub-artifact doc pins its ACL on the #sub-stripped parent (5.7/§3.1)"
        );
        let mut doc =
            IndexDocument::new(ref_.0.clone(), projection.text.clone()).with_acl_object(acl_object);
        for (name, value) in &projection.fields {
            if !spec.struct_fields.contains_key(name) {
                return Err(IndexEventError::Malformed(format!(
                    "projection of `{}` carries facet `{name}` not declared in the IndexSpec for ({subsystem}, {type_})",
                    ref_.0
                )));
            }
            doc = doc.with_field(name.clone(), value.clone());
        }
        doc = doc.with_lang(lang);

        if spec.semantic {
            if let Some(embedding) = self.embedder.embed(&projection.text) {
                doc = doc.with_embedding(embedding, self.embedder.model_ref());
            }
        }

        let zookie = Self::str_field(&ev.payload, "zookie").unwrap_or_default();
        let version = Self::u64_field(&ev.payload, "version").unwrap_or(0);

        self.registry
            .with_backend(&ev.tenant, &ev.region, |be| {
                be.upsert_stamped(&doc, &zookie, version)
            })
            .map_err(|e| IndexEventError::Engine(e.to_string()))
    }

    fn apply_removed(&self, ev: &EventEnvelope) -> Result<(), IndexEventError> {
        if let Some(payload_ref) = ev.payload.get("ref") {
            if payload_ref.as_str() != Some(ev.subject.0.as_str()) {
                return Err(IndexEventError::Malformed(
                    "removal payload `ref` must equal the envelope subject".into(),
                ));
            }
        }
        self.remove_doc(&ev.tenant, &ev.region, &ev.subject.0)
    }

    fn remove_doc(
        &self,
        tenant: &TenantId,
        region: &Region,
        doc_id: &str,
    ) -> Result<(), IndexEventError> {
        self.registry
            .with_backend(tenant, region, |be| be.delete(doc_id))
            .map_err(|e| IndexEventError::Engine(e.to_string()))
    }

    fn apply_permission_changed(&self, ev: &EventEnvelope) -> Result<(), IndexEventError> {
        let new_zookie = Self::str_field(&ev.payload, "zookie").ok_or_else(|| {
            IndexEventError::Malformed(format!(
                "{} permission-change carries no `zookie` (the new consistency token)",
                ev.type_.0
            ))
        })?;
        let refs = Self::str_array_field(&ev.payload, "refs").ok_or_else(|| {
            IndexEventError::Malformed(format!(
                "{} permission-change carries no `refs` (the affected objects)",
                ev.type_.0
            ))
        })?;
        for doc_id in &refs {
            Self::canonical_reference(&ArtifactRef(doc_id.clone()), &ev.tenant)?;
        }
        for doc_id in &refs {
            self.registry
                .with_backend(&ev.tenant, &ev.region, |be| {
                    be.restamp_zookie(doc_id, &new_zookie);
                    Ok(())
                })
                .map_err(|e| IndexEventError::Engine(e.to_string()))?;
        }
        Ok(())
    }

    fn detect_lang(text: &str) -> String {
        crate::analysis::detect_language(text).tag().to_string()
    }

    fn str_field(payload: &serde_json::Value, key: &str) -> Option<String> {
        payload
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn u64_field(payload: &serde_json::Value, key: &str) -> Option<u64> {
        payload.get(key).and_then(|v| v.as_u64())
    }

    fn str_array_field(payload: &serde_json::Value, key: &str) -> Option<Vec<String>> {
        payload
            .get(key)?
            .as_array()?
            .iter()
            .map(|value| value.as_str().map(str::to_string))
            .collect()
    }
}

impl EventHandler for IncrementalIndexer {
    fn subjects(&self) -> &'static [SubjectPattern] {
        INDEXER_SUBJECTS
    }

    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        match self.index(ev) {
            Ok(()) => HandleOutcome::Done,
            Err(IndexEventError::Malformed(why)) => HandleOutcome::NonRetryable(Reason(why)),
            Err(IndexEventError::Engine(why)) => HandleOutcome::NonRetryable(Reason(why)),
            Err(IndexEventError::Transient(_why)) => {
                HandleOutcome::Retry(myelin_events::Backoff { seconds: 2 })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::AclFilter;
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::collections::HashMap;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-1".into()),
            PrincipalKind::Human,
            tenant(),
        )
    }

    #[derive(Default)]
    struct FakeFetcher {
        projections: Mutex<HashMap<String, SearchProjection>>,
        flaky: Mutex<HashMap<String, u32>>,
        calls: Mutex<HashMap<String, u32>>,
    }
    impl FakeFetcher {
        fn with(ref_: &str, p: SearchProjection) -> FakeFetcher {
            let f = FakeFetcher::default();
            f.projections.lock().unwrap().insert(ref_.to_string(), p);
            f
        }
        fn put(&self, ref_: &str, p: SearchProjection) {
            self.projections.lock().unwrap().insert(ref_.to_string(), p);
        }
        fn set_flaky(&self, ref_: &str, fail_times: u32) {
            self.flaky
                .lock()
                .unwrap()
                .insert(ref_.to_string(), fail_times);
        }
        fn call_count(&self, ref_: &str) -> u32 {
            self.calls.lock().unwrap().get(ref_).copied().unwrap_or(0)
        }
    }
    impl ProjectFetcher for FakeFetcher {
        fn project(
            &self,
            _tenant: &TenantId,
            _region: &Region,
            ref_: &ArtifactRef,
        ) -> Result<SearchProjection, ProjectFetchError> {
            *self
                .calls
                .lock()
                .unwrap()
                .entry(ref_.0.clone())
                .or_insert(0) += 1;
            if let Some(n) = self.flaky.lock().unwrap().get_mut(&ref_.0) {
                if *n > 0 {
                    *n -= 1;
                    return Err(ProjectFetchError::Unavailable(
                        "owner transiently down".into(),
                    ));
                }
            }
            match self.projections.lock().unwrap().get(&ref_.0) {
                Some(p) => Ok(p.clone()),
                None => Err(ProjectFetchError::Gone),
            }
        }
    }

    fn proj(text: &str) -> SearchProjection {
        SearchProjection {
            text: text.into(),
            fields: BTreeMap::new(),
            lang: None,
        }
    }

    fn proj_with(text: &str, fields: BTreeMap<String, FieldValue>) -> SearchProjection {
        SearchProjection {
            text: text.into(),
            fields,
            lang: None,
        }
    }

    fn issue_spec() -> IndexSpec {
        let mut fields = BTreeMap::new();
        fields.insert("status".to_string(), FieldType::Select);
        IndexSpec::new("issue", "issue", fields)
    }

    fn page_spec() -> IndexSpec {
        IndexSpec::new("knowledge", "page", BTreeMap::new()).semantic()
    }

    fn indexer_with(specs: Vec<IndexSpec>, fetcher: Arc<FakeFetcher>) -> IncrementalIndexer {
        IncrementalIndexer::new(specs, fetcher, Arc::new(MockEmbeddingAdapter::new(8)))
    }

    fn event(id: &str, type_: &str, subject: &str, payload: serde_json::Value) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(principal()),
            subject: ArtifactRef(subject.into()),
            aggregate: AggregateKey(format!("agg:{subject}")),
            causation_id: None,
            correlation_id: CorrelationId(id.into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload,
        }
    }

    #[test]
    fn per_event_pipeline_indexes_and_is_searchable() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let mut fields = BTreeMap::new();
        fields.insert("status".to_string(), FieldValue::Select("open".into()));
        let fetcher = Arc::new(FakeFetcher::with(
            r,
            proj_with("deadlock in the scheduler", fields),
        ));
        let ix = indexer_with(vec![issue_spec()], fetcher.clone());

        let ev = event(
            "01J-1",
            "issue.issue.created",
            r,
            serde_json::json!({ "zookie": "zk-7", "version": 3 }),
        );
        assert_eq!(
            ix.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );

        assert_eq!(
            fetcher.call_count(r),
            1,
            "the owner projection was fetched once (5.6)"
        );

        let hits = ix
            .search_ft(&tenant(), &region(), &AclFilter::ids([r]), "deadlock", 10)
            .expect("search");
        assert_eq!(hits.len(), 1, "the indexed doc is searchable");
        assert_eq!(hits[0].doc_id, r);
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            1,
            "exactly one live doc"
        );

        assert_eq!(
            ix.indexed_zookie_of(&tenant(), &region(), r).as_deref(),
            Some("zk-7")
        );
    }

    #[test]
    fn replaying_the_same_event_upserts_one_doc() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("body text")));
        let ix = indexer_with(vec![issue_spec()], fetcher.clone());
        let ev = event(
            "01J-1",
            "issue.issue.created",
            r,
            serde_json::json!({ "zookie": "z1" }),
        );

        assert_eq!(ix.index(&ev), Ok(()));
        assert_eq!(ix.index(&ev), Ok(()));
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            1,
            "idempotent on doc_id: one doc, not two"
        );

        use myelin_events::{
            Consumer, ConsumerName, DedupLedger, Message, PrefetchBound, Subscription,
        };
        let sub = Subscription::bind(
            ConsumerName(INDEXER_CONSUMER.into()),
            &["myelin://acme/issue/"],
            PrefetchBound::DEFAULT,
        )
        .unwrap();
        let consumer = Consumer::new(ix.clone(), sub, DedupLedger::new());
        let msg = Message {
            subject: r.to_string(),
            envelope: ev.clone(),
        };
        assert_eq!(consumer.deliver(&msg), myelin_events::Delivered::Acked);
        let before = fetcher.call_count(r);
        assert_eq!(
            consumer.deliver(&msg),
            myelin_events::Delivered::Deduplicated,
            "redelivery deduped"
        );
        assert_eq!(
            fetcher.call_count(r),
            before,
            "the deduped redelivery never re-fetched/re-indexed"
        );
    }

    #[test]
    fn unregistered_type_is_a_noop() {
        let r = "myelin://acme/chat/message/m1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("hi")));
        let ix = indexer_with(vec![issue_spec()], fetcher.clone());
        let ev = event("01J-x", "chat.message.created", r, serde_json::json!({}));
        assert_eq!(ix.index(&ev), Ok(()), "unregistered type → no-op");
        assert_eq!(
            fetcher.call_count(r),
            0,
            "no projection fetched for an unindexed type"
        );
        assert_eq!(ix.live_count(&tenant(), &region()), 0);
    }

    #[test]
    fn malformed_subject_is_a_nonretryable_poison() {
        let fetcher = Arc::new(FakeFetcher::default());
        let ix = indexer_with(vec![issue_spec()], fetcher);
        let ev = event(
            "01J-bad",
            "issue.issue.created",
            "not-a-ref",
            serde_json::json!({}),
        );
        match ix.handle(&ev, &mut myelin_events::HandlerTx::none()) {
            HandleOutcome::NonRetryable(Reason(r)) => {
                assert!(r.contains("invalid ArtifactRef"), "names it: {r}")
            }
            other => panic!("expected a non-retryable poison, got {other:?}"),
        }
    }

    #[test]
    fn projection_with_undeclared_facet_is_a_poison() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let mut fields = BTreeMap::new();
        fields.insert("severity".to_string(), FieldValue::Int(9));
        let fetcher = Arc::new(FakeFetcher::with(r, proj_with("x", fields)));
        let ix = indexer_with(vec![issue_spec()], fetcher);
        let ev = event("01J-1", "issue.issue.created", r, serde_json::json!({}));
        match ix.handle(&ev, &mut myelin_events::HandlerTx::none()) {
            HandleOutcome::NonRetryable(Reason(m)) => {
                assert!(m.contains("severity"), "names the facet: {m}")
            }
            other => panic!("expected a poison, got {other:?}"),
        }
    }

    #[test]
    fn semantic_type_embeds_via_the_mock_adapter() {
        let r = "myelin://acme/knowledge/page/42";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("distributed consensus and raft")));
        let ix = indexer_with(vec![page_spec()], fetcher);
        let ev = event(
            "01J-p",
            "knowledge.page.created",
            r,
            serde_json::json!({ "zookie": "z" }),
        );
        assert_eq!(
            ix.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );

        let query = MockEmbeddingAdapter::new(8)
            .embed("distributed consensus and raft")
            .unwrap();
        let hits = ix
            .registry
            .with_backend(&tenant(), &region(), |be| {
                be.semantic(&AclFilter::ids([r]), &query, 1)
            })
            .expect("semantic");
        assert_eq!(
            hits.len(),
            1,
            "the semantically-indexed doc is reachable by k-NN"
        );
        assert_eq!(hits[0].doc_id, r);
        assert_eq!(
            hits[0].model_ref,
            ModelRef(MockEmbeddingAdapter::DEFAULT_MODEL.into())
        );
    }

    #[test]
    fn mock_embedding_is_deterministic_and_model_pinned() {
        let a = MockEmbeddingAdapter::new(8);
        let v1 = a.embed("alpha beta").unwrap();
        let v2 = a.embed("alpha beta").unwrap();
        assert_eq!(
            v1.0, v2.0,
            "the same text embeds to the same vector (deterministic, idempotent)"
        );
        assert_ne!(
            a.embed("gamma").unwrap().0,
            v1.0,
            "different text → different vector"
        );
        assert!(a.embed("   ").is_none(), "empty text gets no embedding");
        let b = MockEmbeddingAdapter::with_model("eu-model-v2", 8);
        assert_ne!(
            a.model_ref(),
            b.model_ref(),
            "a model swap is a distinct model_ref"
        );
    }

    #[test]
    fn permission_change_restamps_indexed_zookie() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("body")));
        let ix = indexer_with(vec![issue_spec()], fetcher.clone());

        let create = event(
            "01J-1",
            "issue.issue.created",
            r,
            serde_json::json!({ "zookie": "zk-1" }),
        );
        assert_eq!(
            ix.handle(&create, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        assert_eq!(
            ix.indexed_zookie_of(&tenant(), &region(), r).as_deref(),
            Some("zk-1")
        );
        let fetches_before = fetcher.call_count(r);

        let perm = event(
            "01J-perm",
            "authz.tuple.permission.changed",
            r,
            serde_json::json!({ "zookie": "zk-2", "refs": [r] }),
        );
        assert_eq!(
            ix.handle(&perm, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );

        assert_eq!(
            ix.indexed_zookie_of(&tenant(), &region(), r).as_deref(),
            Some("zk-2"),
            "zookie advanced"
        );
        assert_eq!(
            fetcher.call_count(r),
            fetches_before,
            "the body was NOT re-fetched on a permission change"
        );
        let hits = ix
            .search_ft(&tenant(), &region(), &AclFilter::ids([r]), "body", 10)
            .expect("search");
        assert_eq!(hits.len(), 1, "the re-stamped doc still has its body");
    }

    #[test]
    fn permission_change_on_unindexed_object_is_a_noop() {
        let fetcher = Arc::new(FakeFetcher::default());
        let ix = indexer_with(vec![issue_spec()], fetcher);
        let perm = event(
            "01J-perm",
            "authz.tuple.permission.changed",
            "myelin://acme/issue/issue/NONE",
            serde_json::json!({ "zookie": "zk-2", "refs": ["myelin://acme/issue/issue/NONE"] }),
        );
        assert_eq!(
            ix.handle(&perm, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done,
            "a perm change on an un-indexed object is a no-op"
        );
    }

    #[test]
    fn malformed_permission_change_is_a_poison() {
        let fetcher = Arc::new(FakeFetcher::default());
        let ix = indexer_with(vec![issue_spec()], fetcher);
        let perm = event(
            "01J-perm",
            "authz.tuple.permission.changed",
            "myelin://acme/issue/issue/X",
            serde_json::json!({}),
        );
        assert!(
            matches!(
                ix.handle(&perm, &mut myelin_events::HandlerTx::none()),
                HandleOutcome::NonRetryable(_)
            ),
            "missing zookie/refs → poison"
        );

        let mixed = event(
            "01J-mixed",
            "authz.tuple.permission.changed",
            "myelin://acme/issue/issue/X",
            serde_json::json!({
                "zookie": "zk-2",
                "refs": ["myelin://acme/issue/issue/X", 7]
            }),
        );
        assert!(matches!(
            ix.handle(&mixed, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::NonRetryable(_)
        ));
    }

    #[test]
    fn permission_batch_validates_every_reference_before_restamping_any() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("body")));
        let ix = indexer_with(vec![issue_spec()], fetcher);
        assert_eq!(
            ix.index(&event(
                "01J-create",
                "issue.issue.created",
                r,
                serde_json::json!({ "zookie": "zk-1" }),
            )),
            Ok(())
        );

        let forged = event(
            "01J-forged",
            "authz.tuple.permission.changed",
            r,
            serde_json::json!({
                "zookie": "zk-forged",
                "refs": [r, "myelin://other/issue/issue/ENG-2"]
            }),
        );
        assert!(matches!(
            ix.index(&forged),
            Err(IndexEventError::Malformed(_))
        ));
        assert_eq!(
            ix.indexed_zookie_of(&tenant(), &region(), r).as_deref(),
            Some("zk-1"),
            "a bad batch cannot partially restamp its valid prefix"
        );
    }

    #[test]
    fn delete_and_erase_remove_the_doc() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("body")));
        let ix = indexer_with(vec![issue_spec()], fetcher);
        ix.handle(
            &event("01J-1", "issue.issue.created", r, serde_json::json!({})),
            &mut myelin_events::HandlerTx::none(),
        );
        assert_eq!(ix.live_count(&tenant(), &region()), 1);

        let erased = event(
            "01J-e",
            "issue.issue.erased",
            r,
            serde_json::json!({ "ref": r }),
        );
        assert_eq!(
            ix.handle(&erased, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            0,
            "the erased doc is removed from the index"
        );
    }

    #[test]
    fn a_removal_cannot_redirect_its_effect_to_another_document() {
        let first = "myelin://acme/issue/issue/ENG-1";
        let second = "myelin://acme/issue/issue/ENG-2";
        let fetcher = Arc::new(FakeFetcher::with(first, proj("first")));
        fetcher.put(second, proj("second"));
        let ix = indexer_with(vec![issue_spec()], fetcher);
        for (event_id, reference) in [("01J-1", first), ("01J-2", second)] {
            assert_eq!(
                ix.index(&event(
                    event_id,
                    "issue.issue.created",
                    reference,
                    serde_json::json!({}),
                )),
                Ok(())
            );
        }

        let redirected = event(
            "01J-remove",
            "issue.issue.erased",
            first,
            serde_json::json!({ "ref": second }),
        );
        assert!(matches!(
            ix.index(&redirected),
            Err(IndexEventError::Malformed(_))
        ));
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            2,
            "a contradictory removal must delete neither document"
        );
    }

    #[test]
    fn every_index_event_subject_is_canonical_and_tenant_bound() {
        let fetcher = Arc::new(FakeFetcher::default());
        let ix = indexer_with(vec![issue_spec()], fetcher);
        for subject in [
            "myelin://acme/issue/issue/",
            "myelin://acme/issue/issue/ENG 1",
            "myelin://other/issue/issue/ENG-1",
        ] {
            assert!(
                matches!(
                    ix.index(&event(
                        "01J-malformed",
                        "issue.issue.created",
                        subject,
                        serde_json::json!({}),
                    )),
                    Err(IndexEventError::Malformed(_))
                ),
                "Search admitted a malformed subject: {subject:?}"
            );
        }
    }

    #[test]
    fn upsert_of_a_gone_artifact_removes_the_doc() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("body")));
        let ix = indexer_with(vec![issue_spec()], fetcher.clone());
        ix.handle(
            &event("01J-1", "issue.issue.created", r, serde_json::json!({})),
            &mut myelin_events::HandlerTx::none(),
        );
        assert_eq!(ix.live_count(&tenant(), &region()), 1);

        fetcher.projections.lock().unwrap().remove(r);
        assert_eq!(
            ix.handle(
                &event("01J-2", "issue.issue.updated", r, serde_json::json!({})),
                &mut myelin_events::HandlerTx::none()
            ),
            HandleOutcome::Done
        );
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            0,
            "a gone projection removes the doc"
        );
    }

    #[test]
    fn sub_artifact_doc_pins_acl_on_the_parent() {
        let sub_ref = "myelin://acme/knowledge/page/42#block-9";
        let parent = "myelin://acme/knowledge/page/42";
        let fetcher = Arc::new(FakeFetcher::with(sub_ref, proj("a block of prose")));
        let ix = indexer_with(vec![page_spec()], fetcher);
        let ev = event(
            "01J-b",
            "knowledge.page.created",
            sub_ref,
            serde_json::json!({}),
        );
        assert_eq!(
            ix.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );

        let by_parent = ix
            .search_ft(&tenant(), &region(), &AclFilter::ids([parent]), "prose", 10)
            .expect("search by parent acl");
        assert_eq!(
            by_parent.len(),
            1,
            "the sub-artifact doc is admitted by the PARENT's ACL (5.7/§3.1)"
        );
        assert_eq!(
            by_parent[0].doc_id, sub_ref,
            "but keyed by the full sub-precise doc_id"
        );
    }

    #[test]
    fn chained_index_permchange_reindex_across_restart_is_exactly_once_in_effect() {
        use myelin_events::{
            Consumer, ConsumerName, DedupLedger, Delivered, Message, PrefetchBound, Subscription,
        };
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("first body")));
        let ix = indexer_with(vec![issue_spec()], fetcher.clone());
        let ledger = DedupLedger::new();
        let bind = || {
            Subscription::bind(
                ConsumerName(INDEXER_CONSUMER.into()),
                &["myelin://acme/"],
                PrefetchBound::DEFAULT,
            )
            .unwrap()
        };
        let msg = |ev: &EventEnvelope| Message {
            subject: r.to_string(),
            envelope: ev.clone(),
        };

        let e_index = event(
            "01J-1",
            "issue.issue.created",
            r,
            serde_json::json!({ "zookie": "zk-1" }),
        );
        let e_perm = event(
            "01J-2",
            "authz.tuple.permission.changed",
            r,
            serde_json::json!({ "zookie": "zk-2", "refs": [r] }),
        );

        {
            let c = Consumer::new(ix.clone(), bind(), ledger.clone());
            assert_eq!(c.deliver(&msg(&e_index)), Delivered::Acked);
            assert_eq!(c.deliver(&msg(&e_perm)), Delivered::Acked);
        }
        assert_eq!(
            ix.indexed_zookie_of(&tenant(), &region(), r).as_deref(),
            Some("zk-2"),
            "perm change advanced zookie"
        );

        fetcher.put(r, proj("second body"));
        let e_reindex = event(
            "01J-3",
            "issue.issue.updated",
            r,
            serde_json::json!({ "zookie": "zk-3" }),
        );

        let c2 = Consumer::new(ix.clone(), bind(), ledger.clone());
        assert_eq!(
            c2.deliver(&msg(&e_index)),
            Delivered::Deduplicated,
            "e_index already handled → 0 dup"
        );
        assert_eq!(
            c2.deliver(&msg(&e_perm)),
            Delivered::Deduplicated,
            "e_perm already handled → 0 dup"
        );
        assert_eq!(
            c2.deliver(&msg(&e_reindex)),
            Delivered::Acked,
            "the new re-index is handled → 0 lost"
        );

        assert_eq!(
            ix.live_count(&tenant(), &region()),
            1,
            "exactly one doc (no dupe across restart)"
        );
        let fresh = ix
            .search_ft(&tenant(), &region(), &AclFilter::ids([r]), "second", 10)
            .expect("search");
        assert_eq!(fresh.len(), 1, "the re-index applied the fresh body");
        let stale = ix
            .search_ft(&tenant(), &region(), &AclFilter::ids([r]), "first", 10)
            .expect("search");
        assert!(
            stale.is_empty(),
            "the old body was replaced (delete-then-add upsert)"
        );
        assert_eq!(
            ix.indexed_zookie_of(&tenant(), &region(), r).as_deref(),
            Some("zk-3"),
            "latest zookie stamped"
        );
    }

    #[test]
    fn transient_owner_hiccup_retries_then_succeeds() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("body")));
        fetcher.set_flaky(r, 1);
        let ix = indexer_with(vec![issue_spec()], fetcher);
        let ev = event("01J-1", "issue.issue.created", r, serde_json::json!({}));

        assert!(
            matches!(
                ix.handle(&ev, &mut myelin_events::HandlerTx::none()),
                HandleOutcome::Retry(_)
            ),
            "a transient hiccup retries"
        );
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            0,
            "nothing indexed on the hiccup (no fabrication)"
        );
        assert_eq!(
            ix.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done,
            "the redelivery succeeds"
        );
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            1,
            "0 lost: the doc indexed on the redelivery"
        );
    }

    #[test]
    fn index_lag_telemetry_is_zero_in_steady_state_and_named() {
        let r = "myelin://acme/issue/issue/ENG-1";
        let fetcher = Arc::new(FakeFetcher::with(r, proj("body")));
        let ix = indexer_with(vec![issue_spec()], fetcher);
        assert_eq!(ix.index_lag(), 0, "a fresh indexer has no lag");
        ix.handle(
            &event("01J-1", "issue.issue.created", r, serde_json::json!({})),
            &mut myelin_events::HandlerTx::none(),
        );
        assert_eq!(
            ix.index_lag(),
            0,
            "index_lag returns to 0 after projection (synchronous apply)"
        );
        assert_eq!(
            IncrementalIndexer::INDEX_LAG_SIGNAL,
            "search.index_lag",
            "the contract-1.8 signal name"
        );
    }

    #[test]
    fn index_spec_shape_is_the_synthetic_producer_surface() {
        let s = issue_spec();
        assert_eq!(s.subsystem, "issue");
        assert_eq!(s.type_, "issue");
        assert_eq!(
            s.acl_object_type, "issue",
            "acl_object_type defaults to the type"
        );
        assert!(!s.semantic, "issue is not semantically indexed");
        assert!(
            page_spec().semantic,
            "knowledge/page is semantically indexed"
        );
        assert!(s.struct_fields.contains_key("status"));
    }
}
