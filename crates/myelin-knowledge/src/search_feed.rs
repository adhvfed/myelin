use myelin_identity::{Consistency, ObjectType, Permission, Principal};
use myelin_query::QueryAst;
use myelin_search::{
    query as search_query, semantic as search_semantic, AclFilter, ConsistencyStats, IndexBackend,
    IndexSpec, ListObjectsPort, Page, QueryError, QueryStats, RankedResults, ScopedEngine,
    SearchProjection, VectorQuery, KN_PAGE_TYPE,
};

pub use myelin_search::block_subdoc_projection;
pub use myelin_search::{
    kn_index_specs, kn_page_index_spec, kn_row_index_spec, page_search_projection,
    FACET_ARTIFACT_REF, FACET_EMBED, FACET_MENTION, KN_ROW_TYPE, KN_SUBSYSTEM,
};

pub const KN_SEARCH_OBJECT_TYPE: &str = KN_PAGE_TYPE;

pub fn feed_project(
    blocks: &[myelin_content::Block],
    grain: FeedGrain,
    lang: Option<&str>,
) -> SearchProjection {
    match grain {
        FeedGrain::Page => page_search_projection(blocks, lang),
        FeedGrain::SignificantBlock => match blocks.first() {
            Some(block) => block_subdoc_projection(block, lang),
            None => SearchProjection {
                text: String::new(),
                fields: std::collections::BTreeMap::new(),
                lang: lang.map(|s| s.to_string()),
            },
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedGrain {
    Page,
    SignificantBlock,
}

#[allow(clippy::too_many_arguments)]
pub fn kn_search_query<B: IndexBackend>(
    engine: &ScopedEngine<'_, B>,
    identity: &dyn ListObjectsPort,
    ast: &QueryAst,
    viewer: &Principal,
    at: &Consistency,
    page: Page,
    stats: &QueryStats,
) -> Result<RankedResults, QueryError> {
    search_query(
        engine,
        identity,
        ast,
        viewer,
        &ObjectType(KN_SEARCH_OBJECT_TYPE.to_string()),
        at,
        page,
        stats,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn kn_search_semantic<B: IndexBackend>(
    engine: &ScopedEngine<'_, B>,
    identity: &dyn ListObjectsPort,
    ast: &QueryAst,
    viewer: &Principal,
    at: &Consistency,
    vec: &VectorQuery<'_>,
    page: Page,
    stats: &QueryStats,
    cstats: &ConsistencyStats,
) -> Result<RankedResults, QueryError> {
    search_semantic(
        engine,
        identity,
        None,
        ast,
        viewer,
        &ObjectType(KN_SEARCH_OBJECT_TYPE.to_string()),
        at,
        vec,
        page,
        stats,
        cstats,
    )
}

pub const KN_READ_PERMISSION: &str = "knowledge.read";

pub fn kn_read_permission() -> Permission {
    Permission(KN_READ_PERMISSION.to_string())
}

pub fn kn_declared_index_specs() -> Vec<IndexSpec> {
    kn_index_specs()
}

pub type SearchAclFilter = AclFilter;

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::{parse_inline, Block, EmbedDisplay, HeadingLevel, InlineNode};
    use myelin_identity::{
        ConsistencyMode, ListObjectsResult, Literal, ObjectId, PrincipalId, PrincipalKind,
        Result as AuthzResult, SetExpr, Zookie,
    };
    use myelin_query::{CmpOp, Expr, Predicate};
    use myelin_search::{
        EmbeddingAdapter, IndexDocument, MockEmbeddingAdapter, TantivyBackend, FT_BODY_FIELD,
        ORDER_KEY_FIELD, SEMANTIC_FIELD,
    };
    use myelin_tenancy::{ArtifactRef, TenantId};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn viewer() -> Principal {
        Principal::stub(
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn consistency() -> Consistency {
        Consistency {
            at_least: Zookie("z0".into()),
            mode: ConsistencyMode::BoundedStale,
        }
    }

    fn ast_body(term: &str) -> QueryAst {
        QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(FT_BODY_FIELD.into()),
            rhs: Expr::Lit(Literal::Str(term.into())),
        })
        .expect("within cost bounds")
    }

    fn semantic_ast(term: &str) -> QueryAst {
        QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(SEMANTIC_FIELD.into()),
            rhs: Expr::Lit(Literal::Str(term.into())),
        })
        .expect("within cost bounds")
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
        fn filter(set_expr: SetExpr) -> FakeAuthz {
            FakeAuthz {
                answer: ListObjectsResult::Filter {
                    set_expr,
                    zookie: Zookie("z-acl".into()),
                },
                calls: AtomicU64::new(0),
            }
        }
    }
    impl ListObjectsPort for FakeAuthz {
        fn list_objects(
            &self,
            _subject: &Principal,
            _permission: &Permission,
            _ty: &ObjectType,
            _at: &Consistency,
        ) -> AuthzResult<ListObjectsResult> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.answer.clone())
        }
    }

    fn facet_decl() -> BTreeMap<String, myelin_query::FieldType> {
        let mut m = BTreeMap::new();
        m.insert(FACET_MENTION.to_string(), myelin_query::FieldType::Relation);
        m.insert(
            FACET_ARTIFACT_REF.to_string(),
            myelin_query::FieldType::Relation,
        );
        m.insert(FACET_EMBED.to_string(), myelin_query::FieldType::Relation);
        m.insert(
            ORDER_KEY_FIELD.to_string(),
            myelin_query::FieldType::OrderKey,
        );
        m
    }

    fn schema() -> myelin_search::FieldSchema {
        myelin_search::FieldSchema::new()
            .with(
                FT_BODY_FIELD,
                myelin_search::FieldDecl::stored(myelin_query::FieldType::Text),
            )
            .with(
                ORDER_KEY_FIELD,
                myelin_search::FieldDecl::stored(myelin_query::FieldType::OrderKey),
            )
    }

    fn kn_page_corpus() -> TantivyBackend {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let mut upsert = |id: &str, body: &str| {
            let blocks = vec![Block::Paragraph {
                inline: parse_inline(body, &[]),
            }];
            let proj = feed_project(&blocks, FeedGrain::Page, Some("en"));
            let k = myelin_query::OrderKey::bisect(None, None);
            let mut d = IndexDocument::new(id, &proj.text)
                .with_field(ORDER_KEY_FIELD, myelin_query::FieldValue::OrderKey(k));
            for (name, value) in proj.fields {
                d = d.with_field(&name, value);
            }
            be.upsert(&d).unwrap();
        };
        upsert(
            "myelin://acme/knowledge/page/PUB-1",
            "deadlock in the scheduler runbook",
        );
        upsert(
            "myelin://acme/knowledge/page/SECRET-9",
            "deadlock secret incident postmortem",
        );
        be
    }

    #[test]
    fn owned_6_3_specs_re_home_the_search_shapes_verbatim() {
        let specs = kn_declared_index_specs();
        assert_eq!(
            specs,
            kn_index_specs(),
            "the re-homed owned set is byte-equal (no parallel shape)"
        );
        let page = kn_page_index_spec();
        assert_eq!(page.subsystem, KN_SUBSYSTEM);
        assert_eq!(page.type_, KN_PAGE_TYPE);
        assert_eq!(page.acl_object_type, "page");
        assert!(
            page.semantic,
            "the page doc is semantically indexed (vector-in-v1, §6)"
        );
        let row = kn_row_index_spec();
        assert_eq!(row.type_, KN_ROW_TYPE);
        assert!(
            !row.semantic,
            "a db row is a structured record, not vector-embedded prose"
        );
        assert_eq!(specs.len(), 2);
    }

    #[test]
    fn feed_project_builds_page_and_block_projections() {
        let referenced = ArtifactRef("myelin://acme/issues/issue/ENG-1".into());
        let blocks = vec![
            Block::Heading {
                level: HeadingLevel::new(1).unwrap(),
                inline: parse_inline("Design Notes", &[]),
            },
            Block::Paragraph {
                inline: parse_inline(
                    &format!("see {}", myelin_content::OBJ),
                    &[InlineNode::ArtifactRefNode(referenced.clone())],
                ),
            },
            Block::Embed {
                reference: ArtifactRef("myelin://acme/knowledge/page/99".into()),
                display: EmbedDisplay::Card,
            },
        ];
        let page = feed_project(&blocks, FeedGrain::Page, Some("en"));
        assert!(page.text.contains("Design Notes"));
        assert_eq!(page.lang.as_deref(), Some("en"));
        assert_eq!(
            page.fields.get(FACET_ARTIFACT_REF),
            Some(&myelin_query::FieldValue::Relation(referenced.0.clone())),
            "the structured reference facet is extracted via the node-array walk"
        );

        let block = feed_project(&blocks[..1], FeedGrain::SignificantBlock, Some("en"));
        assert!(
            block.text.contains("Design Notes"),
            "the block doc carries the heading prose"
        );
    }

    #[test]
    fn kn_d5_query_conjoin_excludes_confidential_page_incl_count() {
        let be = kn_page_corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let q = ast_body("deadlock");

        let unauth = FakeAuthz::ids(&["myelin://acme/knowledge/page/PUB-1"]);
        let stats = QueryStats::new();
        let res = kn_search_query(
            &eng,
            &unauth,
            &q,
            &viewer(),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(
            ids,
            ["myelin://acme/knowledge/page/PUB-1"],
            "the confidential page is excluded by the pre-filter (no leak)"
        );
        assert_eq!(
            res.hits.len(),
            1,
            "the COUNT reveals neither the existence nor the number of forbidden pages"
        );
        assert_eq!(
            unauth.calls.load(Ordering::Relaxed),
            1,
            "exactly ONE list_objects (no N+1)"
        );

        let granted = FakeAuthz::ids(&[
            "myelin://acme/knowledge/page/PUB-1",
            "myelin://acme/knowledge/page/SECRET-9",
        ]);
        let stats2 = QueryStats::new();
        let res2 = kn_search_query(
            &eng,
            &granted,
            &q,
            &viewer(),
            &consistency(),
            Page::FIRST,
            &stats2,
        )
        .expect("query after grant");
        assert_eq!(
            res2.hits.len(),
            2,
            "after grant both pages are visible (and counted)"
        );
    }

    #[test]
    fn kn_d5_none_short_circuits_no_count_leak() {
        let be = kn_page_corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::filter(SetExpr::None);
        let stats = QueryStats::new();
        let res = kn_search_query(
            &eng,
            &authz,
            &ast_body("deadlock"),
            &viewer(),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        assert!(res.hits.is_empty(), "None ⇒ empty result");
        assert_eq!(
            res.hits.len(),
            0,
            "the COUNT is 0 - no forbidden page is revealed"
        );
        assert_eq!(
            stats.engine_branches(),
            0,
            "no engine branch ran (short-circuit, no count leak)"
        );
    }

    #[test]
    fn kn_d5_semantic_conjoin_excludes_confidential_page() {
        let embedder = MockEmbeddingAdapter::new(16);
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let mut emb = |id: &str, body: &str| {
            let v = embedder.embed(body).expect("non-empty body embeds");
            let k = myelin_query::OrderKey::bisect(None, None);
            let d = IndexDocument::new(id, body)
                .with_field(ORDER_KEY_FIELD, myelin_query::FieldValue::OrderKey(k))
                .with_embedding(v, embedder.model_ref());
            be.upsert(&d).unwrap();
        };
        emb(
            "myelin://acme/knowledge/page/PUB-1",
            "deadlock in the scheduler runbook",
        );
        emb(
            "myelin://acme/knowledge/page/SECRET-9",
            "deadlock secret ops postmortem",
        );

        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let vq = VectorQuery::Text {
            text: "deadlock secret ops postmortem".into(),
            embedder: &embedder,
        };
        let q = semantic_ast("deadlock secret ops postmortem");

        let unauth = FakeAuthz::ids(&["myelin://acme/knowledge/page/PUB-1"]);
        let stats = QueryStats::new();
        let cstats = ConsistencyStats::new();
        let res = kn_search_semantic(
            &eng,
            &unauth,
            &q,
            &viewer(),
            &consistency(),
            &vq,
            Page::FIRST,
            &stats,
            &cstats,
        )
        .expect("semantic");
        let ids: std::collections::BTreeSet<&str> =
            res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            !ids.contains("myelin://acme/knowledge/page/SECRET-9"),
            "the confidential page NEVER surfaces in the semantic/RAG result (KN-D5 vector half: 0 leak)"
        );
        assert_eq!(
            stats.list_objects_calls(),
            1,
            "exactly ONE list_objects (no N+1 on the semantic path)"
        );

        let granted = FakeAuthz::ids(&[
            "myelin://acme/knowledge/page/PUB-1",
            "myelin://acme/knowledge/page/SECRET-9",
        ]);
        let stats2 = QueryStats::new();
        let cstats2 = ConsistencyStats::new();
        let res2 = kn_search_semantic(
            &eng,
            &granted,
            &q,
            &viewer(),
            &consistency(),
            &vq,
            Page::FIRST,
            &stats2,
            &cstats2,
        )
        .expect("semantic after grant");
        let ids2: std::collections::BTreeSet<&str> =
            res2.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            ids2.contains("myelin://acme/knowledge/page/SECRET-9"),
            "after grant the page is a visible neighbour"
        );
    }

    #[test]
    fn read_permission_is_knowledge_read() {
        assert_eq!(KN_READ_PERMISSION, "knowledge.read");
        assert_eq!(kn_read_permission(), Permission("knowledge.read".into()));
        assert_eq!(
            KN_SEARCH_OBJECT_TYPE, "page",
            "the conjoin keys on the page object type"
        );
    }
}
