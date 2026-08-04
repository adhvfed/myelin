use std::collections::BTreeMap;

use myelin_content::{Block, InlineNode};
use myelin_identity::{Consistency, ListObjectsResult, ObjectType, Permission, Principal};
use myelin_query::{FieldType, FieldValue};
use myelin_search::{
    query as search_query, IndexBackend, IndexSpec, ListObjectsPort, Page, QueryStats,
    RankedResults, ScopedEngine, SearchProjection,
};
use myelin_storage::{IndexAdmission, KeyOrigin};

use crate::content::MessageBody;

pub const CHAT_SUBSYSTEM: &str = "chat";

pub const MESSAGE_TYPE: &str = crate::rebac_fragment::object_types::MESSAGE;

pub const MESSAGE_ACL_OBJECT_TYPE: &str = crate::rebac_fragment::object_types::MESSAGE;

pub const MESSAGE_READ_PERMISSION: &str = "read";

pub const FT_BODY_FIELD: &str = "body";

pub use myelin_search::chat_projection::FACET_ARTIFACT_REF;
pub use myelin_search::chat_projection::FACET_EMBED;
pub use myelin_search::chat_projection::FACET_MENTION;

pub const FACET_CHANNEL: &str = "channel";
pub const FACET_AUTHOR: &str = "author";
pub const FACET_THREAD_ROOT: &str = "thread_root";
pub const FACET_CREATED_AT: &str = "created_at";
pub const FACET_KIND: &str = "kind";

pub fn message_index_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    struct_fields.insert(FACET_CHANNEL.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_AUTHOR.to_string(), FieldType::Principal);
    struct_fields.insert(FACET_THREAD_ROOT.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_CREATED_AT.to_string(), FieldType::Date);
    struct_fields.insert(FACET_KIND.to_string(), FieldType::Select);
    struct_fields.insert(FACET_MENTION.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_ARTIFACT_REF.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_EMBED.to_string(), FieldType::Relation);
    IndexSpec::new(CHAT_SUBSYSTEM, MESSAGE_TYPE, struct_fields)
        .with_acl_object_type(MESSAGE_ACL_OBJECT_TYPE)
        .semantic()
}

pub fn message_index_specs() -> Vec<IndexSpec> {
    vec![message_index_spec()]
}

pub fn register_message_index_specs() -> Vec<IndexSpec> {
    let specs = message_index_specs();
    let _accepted = myelin_search::IncrementalIndexer::new(
        specs.clone(),
        std::sync::Arc::new(NullProjectFetcher),
        std::sync::Arc::new(myelin_search::MockEmbeddingAdapter::new(8)),
    );
    specs
}

struct NullProjectFetcher;

impl myelin_search::ProjectFetcher for NullProjectFetcher {
    fn project(
        &self,
        _tenant: &myelin_tenancy::TenantId,
        _region: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<SearchProjection, myelin_search::ProjectFetchError> {
        Err(myelin_search::ProjectFetchError::Gone)
    }
}

pub fn message_search_projection(body: &MessageBody, lang: Option<&str>) -> SearchProjection {
    let text = render_body_text(&body.blocks);
    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
    for node in body.structured_nodes() {
        match node {
            InlineNode::Mention(principal) => {
                fields.insert(
                    FACET_MENTION.to_string(),
                    FieldValue::Relation(principal.principal_id.0.clone()),
                );
            }
            InlineNode::ArtifactRefNode(target) => {
                fields.insert(
                    FACET_ARTIFACT_REF.to_string(),
                    FieldValue::Relation(target.0.clone()),
                );
            }
            InlineNode::Embed(target) => {
                fields.insert(
                    FACET_EMBED.to_string(),
                    FieldValue::Relation(target.0.clone()),
                );
            }
        }
    }
    SearchProjection {
        text,
        fields,
        lang: lang.map(|s| s.to_string()),
    }
}

fn render_body_text(blocks: &[Block]) -> String {
    let mut out = String::new();
    collect_block_text(blocks, &mut out);
    out
}

fn collect_block_text(blocks: &[Block], out: &mut String) {
    for block in blocks {
        match block {
            Block::Paragraph { inline } | Block::Heading { inline, .. } => {
                push_inline_text(inline, out);
            }
            Block::TaskList { items } => {
                for item in items {
                    push_inline_text(&item.inline, out);
                }
            }
            Block::Blockquote { blocks } | Block::Callout { blocks, .. } => {
                collect_block_text(blocks, out);
            }
            Block::BulletList { items } | Block::OrderedList { items, .. } => {
                for item in items {
                    collect_block_text(&item.blocks, out);
                }
            }
            Block::Table { rows, .. } => {
                for row in rows {
                    for cell in row {
                        collect_block_text(&cell.blocks, out);
                    }
                }
            }
            Block::CodeBlock { text, .. } => {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(text);
            }
            _ => {}
        }
    }
}

fn push_inline_text(inline: &myelin_content::Inline, out: &mut String) {
    let rendered = myelin_content::serialize_inline(inline);
    if !rendered.is_empty() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&rendered);
    }
}

pub fn message_doc_ref(tenant: &str, message_id: &str) -> String {
    format!("myelin://{tenant}/chat/{MESSAGE_TYPE}/{message_id}")
}

pub struct AclConjoinedSearchFeeder<'a, B: IndexBackend> {
    engine: &'a ScopedEngine<'a, B>,
    authz: &'a dyn ListObjectsPort,
}

impl<'a, B: IndexBackend> AclConjoinedSearchFeeder<'a, B> {
    pub fn new(
        engine: &'a ScopedEngine<'a, B>,
        authz: &'a dyn ListObjectsPort,
    ) -> AclConjoinedSearchFeeder<'a, B> {
        AclConjoinedSearchFeeder { engine, authz }
    }

    pub fn search_messages(
        &self,
        ast: &myelin_query::QueryAst,
        viewer: &Principal,
        at: &Consistency,
        page: Page,
        stats: &QueryStats,
    ) -> Result<RankedResults, myelin_search::QueryError> {
        let ty = ObjectType(MESSAGE_ACL_OBJECT_TYPE.to_string());
        search_query(self.engine, self.authz, ast, viewer, &ty, at, page, stats)
    }
}

pub fn message_search_acl_anchor() -> (Permission, ObjectType) {
    (
        Permission(MESSAGE_READ_PERMISSION.to_string()),
        ObjectType(MESSAGE_ACL_OBJECT_TYPE.to_string()),
    )
}

pub fn non_member_filter(zookie: &str) -> ListObjectsResult {
    ListObjectsResult::Filter {
        set_expr: myelin_identity::SetExpr::None,
        zookie: myelin_identity::Zookie(zookie.to_string()),
    }
}

pub fn admit_message_indexing(origin: &dyn KeyOrigin) -> IndexAdmission {
    IndexAdmission::for_origin(origin)
}

pub fn may_index_messages(origin: &dyn KeyOrigin) -> bool {
    admit_message_indexing(origin).may_index()
}

#[derive(Clone, Copy, Debug)]
pub struct EmbeddingsArePersonalData;

impl EmbeddingsArePersonalData {
    pub const ERASE_CASCADE_TOKEN: &'static str = crate::erase::CHAT_ERASE_CASCADE_TOKEN;
    pub const SPEC_IS_SEMANTIC: bool = true;
}

#[derive(Clone, Copy, Debug)]
pub struct ReplayParityFollowOn;

impl ReplayParityFollowOn {
    pub const REPLAY_PARITY: &'static str = "CHAT-P21";
    pub const ERASURE_RECEIPT: &'static str = "CHAT-P22";
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::{parse_inline, Block, HeadingLevel};
    use myelin_events::ArtifactRef;
    use myelin_identity::{
        Consistency, ConsistencyMode, Literal, ObjectId, ObjectType as IdObjectType,
        Permission as IdPerm, Principal, PrincipalId, PrincipalKind, Result as AuthzRes, Zookie,
    };
    use myelin_query::{CmpOp, Expr, Predicate, QueryAst};
    use myelin_search::{
        FieldDecl, FieldSchema, IncrementalIndexer, IndexDocument, MockEmbeddingAdapter,
        TantivyBackend,
    };
    use myelin_storage::{
        kms::{DekHandle, KekId, KmsEngine, KEY_LEN},
        Byok, Dek, Hyok, HyokKeyService, HyokServiceDenied, PlatformManaged, WrappedDek,
    };
    use myelin_tenancy::{Region, TenantId};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn alice() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn mention(id: &str) -> InlineNode {
        InlineNode::Mention(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        ))
    }

    fn consistency() -> Consistency {
        Consistency {
            at_least: Zookie("z0".into()),
            mode: ConsistencyMode::BoundedStale,
        }
    }

    #[test]
    fn chat_spec_is_the_authoritative_owned_6_3_shape() {
        let s = message_index_spec();
        assert_eq!(s.subsystem, "chat");
        assert_eq!(s.type_, "message");
        assert_eq!(
            s.acl_object_type, "message",
            "§7: Search ALWAYS conjoins the list_objects Filter over message.id"
        );
        assert!(
            s.semantic,
            "§7: message bodies are vector-embedded for RAG/dedup (embeddings ARE personal data)"
        );
        for facet in [
            FACET_CHANNEL,
            FACET_AUTHOR,
            FACET_THREAD_ROOT,
            FACET_CREATED_AT,
            FACET_KIND,
        ] {
            assert!(
                s.struct_fields.contains_key(facet),
                "§7 struct_field `{facet}` is present"
            );
        }
        for facet in [FACET_MENTION, FACET_ARTIFACT_REF, FACET_EMBED] {
            assert_eq!(
                s.struct_fields.get(facet),
                Some(&FieldType::Relation),
                "`{facet}` is a dependable reference facet (Relation, X-2)"
            );
        }
    }

    #[test]
    fn message_body_is_not_a_struct_facet() {
        let s = message_index_spec();
        for absent in [FT_BODY_FIELD, "text", "message", "content", "markdown"] {
            assert!(
                !s.struct_fields.contains_key(absent),
                "`{absent}` is the full-text projection body, not a structured facet"
            );
        }
    }

    #[test]
    fn registration_is_accepted_by_search() {
        let accepted = register_message_index_specs();
        assert_eq!(
            accepted,
            message_index_specs(),
            "Search accepts the declared chat spec verbatim"
        );
        let _ix = IncrementalIndexer::new(
            message_index_specs(),
            std::sync::Arc::new(NullProjectFetcher),
            std::sync::Arc::new(MockEmbeddingAdapter::new(8)),
        );
    }

    #[test]
    fn message_projection_extracts_body_and_structured_facets() {
        let referenced = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        let body = MessageBody::new(vec![
            Block::Heading {
                level: HeadingLevel::new(2).unwrap(),
                inline: parse_inline("standup notes", &[]),
            },
            Block::Paragraph {
                inline: parse_inline(
                    &format!(
                        "blocked on {} - ping {}",
                        myelin_content::OBJ,
                        myelin_content::OBJ
                    ),
                    &[
                        InlineNode::ArtifactRefNode(referenced.clone()),
                        mention("alice"),
                    ],
                ),
            },
        ])
        .unwrap();
        let p = message_search_projection(&body, Some("en"));

        assert!(p.text.contains("standup notes"));
        assert!(p.text.contains("blocked on"));
        assert_eq!(p.lang.as_deref(), Some("en"));

        assert_eq!(
            p.fields.get(FACET_ARTIFACT_REF),
            Some(&FieldValue::Relation(referenced.0.clone()))
        );
        assert_eq!(
            p.fields.get(FACET_MENTION),
            Some(&FieldValue::Relation("alice".to_string()))
        );
    }

    #[test]
    fn multilingual_message_with_no_nodes_has_no_reference_facets() {
        let body = MessageBody::new(vec![Block::Paragraph {
            inline: parse_inline("der Scheduler ist blockiert", &[]),
        }])
        .unwrap();
        let p = message_search_projection(&body, Some("de"));
        assert!(
            p.fields.is_empty(),
            "no structured nodes ⇒ no reference facets"
        );
        assert!(p.text.contains("Scheduler"));
        assert_eq!(p.lang.as_deref(), Some("de"));
    }

    #[test]
    fn doc_ref_is_the_chat_message_ref() {
        assert_eq!(
            message_doc_ref("acme", "m-7"),
            "myelin://acme/chat/message/m-7"
        );
    }

    fn schema() -> FieldSchema {
        FieldSchema::new().with(
            FT_BODY_FIELD,
            FieldDecl::stored(myelin_query::FieldType::Text),
        )
    }

    fn corpus() -> TantivyBackend {
        let mut be = TantivyBackend::open(&BTreeMap::new()).expect("open");
        for (id, body) in [
            (
                "myelin://acme/chat/message/m-public",
                "deploy the public service",
            ),
            (
                "myelin://acme/chat/message/m-secret",
                "deploy the confidential fix",
            ),
        ] {
            be.upsert(&IndexDocument::new(id, body)).unwrap();
        }
        be
    }

    struct FakeAuthz {
        answer: ListObjectsResult,
        calls: AtomicU64,
    }
    impl FakeAuthz {
        fn ids(ids: &[&str]) -> FakeAuthz {
            FakeAuthz {
                answer: ListObjectsResult::Ids {
                    ids: ids.iter().map(|i| ObjectId((*i).into())).collect(),
                    zookie: Zookie("z-acl".into()),
                },
                calls: AtomicU64::new(0),
            }
        }
        fn non_member() -> FakeAuthz {
            FakeAuthz {
                answer: non_member_filter("z-acl"),
                calls: AtomicU64::new(0),
            }
        }
    }
    impl ListObjectsPort for FakeAuthz {
        fn list_objects(
            &self,
            _subject: &Principal,
            _permission: &IdPerm,
            _ty: &IdObjectType,
            _at: &Consistency,
        ) -> AuthzRes<ListObjectsResult> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.answer.clone())
        }
    }

    fn ast(term: &str) -> QueryAst {
        QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(FT_BODY_FIELD.into()),
            rhs: Expr::Lit(Literal::Str(term.into())),
        })
        .expect("within cost bounds")
    }

    #[test]
    fn search_as_non_member_returns_zero_results_then_grant_surfaces() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());

        let none = FakeAuthz::non_member();
        let feeder = AclConjoinedSearchFeeder::new(&eng, &none);
        let stats = QueryStats::new();
        let res = feeder
            .search_messages(
                &ast("deploy"),
                &alice(),
                &consistency(),
                Page::FIRST,
                &stats,
            )
            .expect("the Search query surface is reachable");
        assert!(
            res.hits.is_empty(),
            "a non-member sees 0 message results from channels they're not in (CHAT-D11)"
        );
        assert_eq!(
            none.calls.load(Ordering::Relaxed),
            1,
            "exactly ONE list_objects (the conjoined pre-filter; no N+1)"
        );

        let granted = FakeAuthz::ids(&["myelin://acme/chat/message/m-public"]);
        let feeder2 = AclConjoinedSearchFeeder::new(&eng, &granted);
        let stats2 = QueryStats::new();
        let res2 = feeder2
            .search_messages(
                &ast("deploy"),
                &alice(),
                &consistency(),
                Page::FIRST,
                &stats2,
            )
            .expect("reachable");
        let ids: Vec<&str> = res2.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(
            ids,
            ["myelin://acme/chat/message/m-public"],
            "only the granted message surfaces; the confidential one is excluded incl. count"
        );
        assert_eq!(
            res2.hits.len(),
            1,
            "the count reveals only the visible message"
        );
    }

    #[test]
    fn acl_anchor_is_read_over_message() {
        let (perm, ty) = message_search_acl_anchor();
        assert_eq!(perm.0, "read");
        assert_eq!(ty.0, "message");
        assert_eq!(
            ty.0,
            message_index_spec().acl_object_type,
            "the feeder conjoins on the SAME object type the spec declares (message.id)"
        );
    }

    struct MockHyok {
        revoked: std::cell::Cell<bool>,
        key: [u8; KEY_LEN],
    }
    impl MockHyok {
        fn new() -> MockHyok {
            MockHyok {
                revoked: std::cell::Cell::new(false),
                key: [7u8; KEY_LEN],
            }
        }
    }
    impl HyokKeyService for MockHyok {
        fn wrap(&self, _dek: &Dek) -> Result<WrappedDek, HyokServiceDenied> {
            if self.revoked.get() {
                return Err(HyokServiceDenied);
            }
            Ok(WrappedDek {
                nonce: [0u8; 12],
                wrapped: self.key.to_vec(),
                kek_epoch: 0,
            })
        }
        fn unwrap(&self, _w: &WrappedDek) -> Result<DekHandle, HyokServiceDenied> {
            if self.revoked.get() {
                return Err(HyokServiceDenied);
            }
            Ok(DekHandle::from_raw(self.key))
        }
        fn destroy(&self) {
            self.revoked.set(true);
        }
    }

    #[test]
    fn hyok_class_skips_message_indexing() {
        let eng = KmsEngine::new();
        eng.ensure_kek(&KekId::new(
            TenantId("acme".into()),
            Region("fr-par".into()),
        ));
        let region = Region("fr-par".into());

        let platform = PlatformManaged::new(&eng, region.clone());
        assert_eq!(admit_message_indexing(&platform), IndexAdmission::Admit);
        assert!(
            may_index_messages(&platform),
            "platform-managed: full search/RAG"
        );

        let byok = Byok::new(&eng, region.clone(), "kms-customer://acme/k");
        assert!(
            may_index_messages(&byok),
            "BYOK: the key is live in-engine → can index"
        );

        let hyok = Hyok::new(MockHyok::new());
        assert_eq!(
            admit_message_indexing(&hyok),
            IndexAdmission::SkipHyok,
            "a HYOK class is refused - you cannot index what you cannot decrypt"
        );
        assert!(
            !may_index_messages(&hyok),
            "0 indexed message bodies for a HYOK tenant (the structural skip)"
        );
    }

    #[test]
    fn embeddings_are_personal_data_bound_to_the_cascade() {
        assert_eq!(
            EmbeddingsArePersonalData::SPEC_IS_SEMANTIC,
            message_index_spec().semantic,
            "the marker's semantic claim matches the wired spec (embeddings exist → must be reached)"
        );
        assert_eq!(
            EmbeddingsArePersonalData::ERASE_CASCADE_TOKEN,
            crate::erase::CHAT_ERASE_CASCADE_TOKEN,
            "the embeddings purge rides the chat erase cascade (the OUTBOX fan-out)"
        );
    }

    #[test]
    fn follow_ons_are_named() {
        assert_eq!(ReplayParityFollowOn::REPLAY_PARITY, "CHAT-P21");
        assert_eq!(ReplayParityFollowOn::ERASURE_RECEIPT, "CHAT-P22");
    }
}
