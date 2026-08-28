pub mod analysis;
pub mod compiler;
pub mod consistency;
pub mod engine;
pub mod fusion;
pub mod indexer;
pub mod pipeline;
pub mod reindex;
pub mod tier3_valve;
pub mod vector;

pub use compiler::{
    compile, render, CompileError, CompiledPlan, ConjoinedPlan, FieldDecl, FieldKind, FieldSchema,
    FtClause, PostFetchPredicate, Sort, StructuredClause, VectorBranch, FT_BODY_FIELD,
    SEMANTIC_FIELD, SORT_FIELD,
};
pub use consistency::{
    disposition, fail_static_bypass, stale_candidates, BoundedCheckPort, CandidateDisposition,
    ConsistencyStats,
};
pub use engine::{
    AclFilter, Hit, IndexBackend, IndexDocument, IndexError, SubjectMatcher, TantivyBackend,
    DEFAULT_SUBJECT_LOCATOR_FACETS, ORDER_KEY_FIELD,
};
pub use fusion::{fuse_with_k, reciprocal_rank_fusion, FusedHit, RankedList, RRF_K};
pub use indexer::{
    EmbeddingAdapter, IncrementalIndexer, IndexEventError, IndexSpec, MockEmbeddingAdapter,
    ProjectFetchError, ProjectFetcher, SearchProjection, INDEXER_CONSUMER,
    INDEXER_SUBJECT_PREFIXES,
};
pub use pipeline::{
    query, query_consistent, semantic, ListObjectsPort, Page, QueryError, QueryStats, RankedResult,
    RankedResults, RelationalLeaf, ReverseIndexAnswer, RevisionWatermark, ScopedEngine,
    VectorQuery, READ_PERMISSION,
};
pub use reindex::{
    ReindexCursorStore, ReindexError, ReindexJob, ReindexProgress, SearchReindexer,
    DEFAULT_BATCH_CAP, DEFAULT_MAX_IN_FLIGHT_PER_TENANT,
};
pub use tier3_valve::{
    board_acl_filter, escalate_to_search, oltp_board_admits, BoardEscalationAuthz, BoardQuery,
    OltpBudget, ReverseResolver,
};
pub use vector::{Embedding, HnswVectorIndex, ModelRef, VectorHit, VectorRecord};

pub const DOC_ID: &str = "doc_id";

pub const TENANT: &str = "tenant";

pub const REGION: &str = "region";

pub const INDEXED_ZOOKIE: &str = "indexed_zookie";

pub const VERSION: &str = "version";

pub const LANG: &str = "lang";

pub const CONTAINS_PERSONAL_DATA: &str = "contains_personal_data";

pub const DATA_ROLE: &str = "data_role";

pub const VISIBILITY: &str = "visibility";

pub const PII_KEY_REF: &str = "pii_key_ref";

pub const INDEX_DOC_ANCHORS: [&str; 9] = [
    DOC_ID,
    TENANT,
    REGION,
    INDEXED_ZOOKIE,
    VERSION,
    LANG,
    CONTAINS_PERSONAL_DATA,
    DATA_ROLE,
    VISIBILITY,
];

pub const ENVELOPE_DERIVED_ANCHORS: [&str; 5] = [
    TENANT,
    REGION,
    CONTAINS_PERSONAL_DATA,
    DATA_ROLE,
    VISIBILITY,
];

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        ArtifactRef, DataRole, EventEnvelope, PiiKeyRef, Region, TenantId, Visibility,
    };

    fn sample_envelope() -> EventEnvelope {
        use myelin_events::{Actor, AggregateKey, CorrelationId, EventId, EventType, Timestamp};
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        EventEnvelope {
            event_id: EventId("01J0".into()),
            type_: EventType("search.doc.indexed".into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            subject: ArtifactRef("myelin://acme/issues/issue/ENG-1421".into()),
            aggregate: AggregateKey("issue:ENG-1421".into()),
            causation_id: None,
            correlation_id: CorrelationId("01J0".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: true,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: Some(PiiKeyRef("kms://acme/3/subject:u42".into())),
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            payload: serde_json::json!({ "ref": "myelin://acme/issues/issue/ENG-1421" }),
        }
    }

    #[test]
    fn index_doc_anchors_match_the_frozen_envelope() {
        let env = sample_envelope();

        let json = serde_json::to_value(&env).expect("envelope serialises");
        let obj = json.as_object().expect("envelope is a JSON object");

        for anchor in ENVELOPE_DERIVED_ANCHORS {
            assert!(
                obj.contains_key(anchor),
                "index-doc anchor `{anchor}` no longer matches a frozen EventEnvelope field - \
                 the envelope (contract 2.1) was renamed; reconcile the Search index-doc anchor \
                 (EI-01 §7, names/units drift caught up front, not in prod)"
            );
        }

        assert!(
            obj.contains_key("subject"),
            "the index `doc_id` is the envelope `subject` ArtifactRef key (5.1) - the envelope \
             `subject` field was renamed; reconcile DOC_ID"
        );
        let subject = &env.subject;
        assert_eq!(
            subject.0,
            obj["subject"]
                .as_str()
                .expect("subject serialises as a string"),
            "doc_id is the ArtifactRef key string verbatim (architecture §3.1)"
        );

        for gdpr in [CONTAINS_PERSONAL_DATA, DATA_ROLE, VISIBILITY, PII_KEY_REF] {
            assert!(
                obj.contains_key(gdpr),
                "personal-data routing anchor `{gdpr}` must match a frozen EventEnvelope field so \
                 any future durable index can preserve the source event's handling metadata"
            );
        }
    }

    #[test]
    fn the_named_anchor_set_is_the_prompt_enumerated_core() {
        assert_eq!(
            INDEX_DOC_ANCHORS.len(),
            9,
            "the named-anchor core is exactly nine"
        );
        let mut sorted = INDEX_DOC_ANCHORS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 9, "no index-doc anchor name is duplicated");

        for required in [DOC_ID, TENANT, REGION, INDEXED_ZOOKIE, VERSION, LANG] {
            assert!(
                INDEX_DOC_ANCHORS.contains(&required),
                "the prompt-named anchor `{required}` is missing from INDEX_DOC_ANCHORS"
            );
        }
    }
}
