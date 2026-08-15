use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_content::{parse_inline, Block, EmbedDisplay, HeadingLevel, InlineNode, OBJ};
use myelin_identity::{Literal, Principal, PrincipalId, PrincipalKind};
use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate, QueryAst};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventId, EventType, Timestamp,
    Visibility,
};
use myelin_search::{
    compile, kn_page_index_spec, kn_row_index_spec, page_search_projection, AclFilter,
    EmbeddingAdapter, FieldDecl, FieldKind, FieldSchema, IncrementalIndexer, IndexSpec,
    MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher, SearchProjection, FACET_ARTIFACT_REF,
    FACET_EMBED, FACET_MENTION, FT_BODY_FIELD,
};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str, t: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(t.into()),
    )
}

#[derive(Default)]
struct KnFetcher {
    projections: Mutex<BTreeMap<String, SearchProjection>>,
}
impl KnFetcher {
    fn put(&self, ref_: &str, p: SearchProjection) {
        self.projections.lock().unwrap().insert(ref_.to_string(), p);
    }
}
impl ProjectFetcher for KnFetcher {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        match self.projections.lock().unwrap().get(&ref_.0) {
            Some(p) => Ok(p.clone()),
            None => Err(ProjectFetchError::Gone),
        }
    }
}

fn kn_event(id: &str, type_: &str, subject: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(viewer("platform", "acme")),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(format!("agg:{subject}")),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: true,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload: serde_json::json!({ "zookie": "zk-kn-1", "version": 1 }),
    }
}

fn event_in(id: &str, type_: &str, subject: &str, t: &str) -> EventEnvelope {
    let mut ev = kn_event(id, type_, subject);
    ev.tenant = TenantId(t.into());
    ev
}

fn kn_indexer(fetcher: Arc<KnFetcher>) -> IncrementalIndexer {
    IncrementalIndexer::new(
        vec![kn_page_index_spec(), kn_row_index_spec()],
        fetcher,
        Arc::new(MockEmbeddingAdapter::new(16)),
    )
}

fn fr_page_blocks() -> Vec<Block> {
    let issue = ArtifactRef("myelin://acme/issues/issue/ENG-7".into());
    let embedded = ArtifactRef("myelin://acme/knowledge/page/embedded-1".into());
    vec![
        Block::Heading {
            level: HeadingLevel::new(1).unwrap(),
            inline: parse_inline("Conception de l'ordonnanceur", &[]),
        },
        Block::Paragraph {
            inline: parse_inline(
                &format!("voir {OBJ} signaler à {OBJ}"),
                &[
                    InlineNode::ArtifactRefNode(issue),
                    InlineNode::Mention(viewer("alice", "acme")),
                ],
            ),
        },
        Block::CodeBlock {
            lang: Some("rust".into()),
            text: "fn ordonnanceur_interblocage() {}".into(),
        },
        Block::Embed {
            reference: embedded,
            display: EmbedDisplay::Card,
        },
    ]
}

fn en_page_blocks() -> Vec<Block> {
    vec![
        Block::Heading {
            level: HeadingLevel::new(1).unwrap(),
            inline: parse_inline("Scheduler Design", &[]),
        },
        Block::Paragraph {
            inline: parse_inline("the scheduler avoids deadlock at runtime", &[]),
        },
    ]
}

#[test]
fn kn_pages_index_multilingual_and_are_searchable() {
    let fr_ref = "myelin://acme/knowledge/page/fr-1";
    let en_ref = "myelin://acme/knowledge/page/en-1";
    let fetcher = Arc::new(KnFetcher::default());
    fetcher.put(
        fr_ref,
        page_search_projection(&fr_page_blocks(), Some("fr")),
    );
    fetcher.put(
        en_ref,
        page_search_projection(&en_page_blocks(), Some("en")),
    );
    let ix = kn_indexer(fetcher);

    ix.index(&kn_event("e-fr", "knowledge.page.created", fr_ref))
        .expect("index fr page");
    ix.index(&kn_event("e-en", "knowledge.page.created", en_ref))
        .expect("index en page");
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        2,
        "both KN pages are live"
    );

    let acl = AclFilter::ids([fr_ref, en_ref]);

    let fr_hits = ix
        .search_ft(&tenant(), &region(), &acl, "ordonnanceur", 10)
        .expect("fr search");
    assert!(
        fr_hits.iter().any(|h| h.doc_id == fr_ref),
        "the FR term finds the FR page"
    );
    assert!(
        !fr_hits.iter().any(|h| h.doc_id == en_ref),
        "the FR term does not find the EN page"
    );

    let en_hits = ix
        .search_ft(&tenant(), &region(), &acl, "deadlock", 10)
        .expect("en search");
    assert!(
        en_hits.iter().any(|h| h.doc_id == en_ref),
        "the EN term finds the EN page"
    );

    let code_hits = ix
        .search_ft(&tenant(), &region(), &acl, "ordonnanceur_interblocage", 10)
        .expect("code search");
    assert!(
        code_hits.iter().any(|h| h.doc_id == fr_ref),
        "the raw code_block body is indexed (X-2 - code is verbatim, not markdown-parsed)"
    );

    let embedder = MockEmbeddingAdapter::new(16);
    let q = embedder
        .embed("scheduler deadlock design")
        .expect("query embeds");
    let sem = ix
        .search_semantic(&tenant(), &region(), &acl, &q, 5)
        .expect("semantic search");
    assert!(
        !sem.is_empty(),
        "semantic KN search returns a visible passage (vector-in-v1, §4.5)"
    );
    assert!(
        sem.iter().all(|h| h.doc_id == fr_ref || h.doc_id == en_ref),
        "every semantic hit is a visible page"
    );
}

#[test]
fn kn_structured_inline_node_facets_filter_correctly() {
    let fr_ref = "myelin://acme/knowledge/page/fr-1";
    let plain_ref = "myelin://acme/knowledge/page/plain-1";
    let fetcher = Arc::new(KnFetcher::default());
    fetcher.put(
        fr_ref,
        page_search_projection(&fr_page_blocks(), Some("fr")),
    );
    fetcher.put(
        plain_ref,
        page_search_projection(&en_page_blocks(), Some("en")),
    );
    let ix = kn_indexer(fetcher);
    ix.index(&kn_event("e-fr", "knowledge.page.created", fr_ref))
        .expect("index");
    ix.index(&kn_event("e-pl", "knowledge.page.created", plain_ref))
        .expect("index");

    let acl = AclFilter::ids([fr_ref, plain_ref]);

    let m = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl,
            FACET_MENTION,
            &FieldValue::Relation("alice".into()),
            10,
        )
        .expect("mention facet scan");
    assert_eq!(m.len(), 1, "exactly the page mentioning alice");
    assert_eq!(m[0].doc_id, fr_ref);

    let a = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl,
            FACET_ARTIFACT_REF,
            &FieldValue::Relation("myelin://acme/issues/issue/ENG-7".into()),
            10,
        )
        .expect("artifact_ref facet scan");
    assert_eq!(a.len(), 1, "exactly the page referencing ENG-7");
    assert_eq!(a[0].doc_id, fr_ref);

    let e = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl,
            FACET_EMBED,
            &FieldValue::Relation("myelin://acme/knowledge/page/embedded-1".into()),
            10,
        )
        .expect("embed facet scan");
    assert_eq!(e.len(), 1, "exactly the page embedding embedded-1");
    assert_eq!(e[0].doc_id, fr_ref);
}

#[test]
fn kn_db_row_custom_field_query_via_gin_scan() {
    let row_a = "myelin://acme/knowledge/row/tasks:1";
    let row_b = "myelin://acme/knowledge/row/tasks:2";
    let fetcher = Arc::new(KnFetcher::default());
    let mut fa = BTreeMap::new();
    fa.insert("priority".to_string(), FieldValue::Select("high".into()));
    fa.insert("owner".to_string(), FieldValue::Principal("alice".into()));
    fa.insert("due".to_string(), FieldValue::Date("2026-07-01".into()));
    fa.insert(
        "order_key".to_string(),
        FieldValue::OrderKey(OrderKey::bisect(None, None)),
    );
    let mut fb = BTreeMap::new();
    fb.insert("priority".to_string(), FieldValue::Select("low".into()));
    fb.insert("owner".to_string(), FieldValue::Principal("bob".into()));
    fb.insert("due".to_string(), FieldValue::Date("2026-08-01".into()));
    fb.insert(
        "order_key".to_string(),
        FieldValue::OrderKey(OrderKey::bisect(None, None)),
    );
    fetcher.put(
        row_a,
        SearchProjection {
            text: "ship the scheduler".into(),
            fields: fa,
            lang: None,
        },
    );
    fetcher.put(
        row_b,
        SearchProjection {
            text: "write the docs".into(),
            fields: fb,
            lang: None,
        },
    );
    let ix = kn_indexer(fetcher);
    ix.index(&kn_event("r-a", "knowledge.row.updated", row_a))
        .expect("index row a");
    ix.index(&kn_event("r-b", "knowledge.row.updated", row_b))
        .expect("index row b");

    let acl = AclFilter::ids([row_a, row_b]);
    let hits = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl,
            "priority",
            &FieldValue::Select("high".into()),
            10,
        )
        .expect("custom-field GIN scan");
    assert_eq!(
        hits.len(),
        1,
        "exactly the high-priority row (the GIN-scan custom field)"
    );
    assert_eq!(hits[0].doc_id, row_a);
}

#[test]
fn kn_rollup_field_is_read_time_not_a_stored_indexed_value() {
    let spec: IndexSpec = kn_row_index_spec();
    let mut schema = FieldSchema::new().with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text));
    for (name, ty) in &spec.struct_fields {
        schema = schema.with(name.clone(), FieldDecl::stored(*ty));
    }
    schema = schema.with("rollup_total", FieldDecl::read_time(FieldType::Int));

    assert_eq!(schema.get("priority").unwrap().kind, FieldKind::Stored);
    assert_eq!(
        schema.get("rollup_total").unwrap().kind,
        FieldKind::ReadTime
    );

    let ast = QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Gt,
        lhs: Expr::Var("rollup_total".into()),
        rhs: Expr::Lit(Literal::Int(10)),
    })
    .expect("within cost bounds");
    let plan = compile(&ast, &schema).expect("compiles");
    assert_eq!(
        plan.post_fetch.len(),
        1,
        "the rollup Cmp lowers to a post-fetch predicate"
    );
    assert_eq!(plan.post_fetch[0].field, "rollup_total");
    assert!(
        plan.structured.is_empty(),
        "the rollup derived value is NEVER a stored/indexed structured clause (KN-3)"
    );

    let stored_ast = QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var("priority".into()),
        rhs: Expr::Lit(Literal::Str("high".into())),
    })
    .expect("within cost bounds");
    let stored_plan = compile(&stored_ast, &schema).expect("compiles");
    assert!(
        stored_plan.post_fetch.is_empty(),
        "a stored field is not post-fetch"
    );
    assert_eq!(
        stored_plan.structured.len(),
        1,
        "a stored custom field IS a structured clause"
    );
}

#[test]
fn srch_d1_private_kn_page_never_leaks() {
    let visible = "myelin://acme/knowledge/page/visible-1";
    let private = "myelin://acme/knowledge/page/private-secret";
    let fetcher = Arc::new(KnFetcher::default());
    let secret_blocks = vec![Block::Paragraph {
        inline: parse_inline("classified zarquon deadlock plan", &[]),
    }];
    let visible_blocks = vec![Block::Paragraph {
        inline: parse_inline("public zarquon overview", &[]),
    }];
    fetcher.put(visible, page_search_projection(&visible_blocks, Some("en")));
    fetcher.put(private, page_search_projection(&secret_blocks, Some("en")));
    let ix = kn_indexer(fetcher);
    ix.index(&kn_event("v", "knowledge.page.created", visible))
        .expect("index visible");
    ix.index(&kn_event("p", "knowledge.page.created", private))
        .expect("index private");
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        2,
        "both pages are indexed"
    );

    let acl_unauth = AclFilter::ids([visible]);

    let hits = ix
        .search_ft(&tenant(), &region(), &acl_unauth, "zarquon", 10)
        .expect("ft");
    assert_eq!(
        hits.len(),
        1,
        "0 count-leak: exactly the one visible page (hidden page never counted)"
    );
    assert_eq!(hits[0].doc_id, visible);
    assert!(
        !hits.iter().any(|h| h.doc_id == private),
        "0 leak: the private page never surfaces"
    );

    let embedder = MockEmbeddingAdapter::new(16);
    let q = embedder
        .embed("classified zarquon deadlock plan")
        .expect("embeds");
    let sem = ix
        .search_semantic(&tenant(), &region(), &acl_unauth, &q, 5)
        .expect("semantic");
    assert!(
        !sem.iter().any(|h| h.doc_id == private),
        "0 RAG/vector leak: the private page is never a visible neighbour"
    );

    let acl_granted = AclFilter::ids([visible, private]);
    let granted = ix
        .search_ft(&tenant(), &region(), &acl_granted, "zarquon", 10)
        .expect("ft granted");
    assert_eq!(
        granted.len(),
        2,
        "after the grant BOTH pages surface (the rejection was the ACL, not a deny)"
    );
    assert!(
        granted.iter().any(|h| h.doc_id == private),
        "the granted page now appears"
    );
}

#[test]
fn srch_d3_cross_tenant_kn_pages_do_not_leak() {
    let acme_page = "myelin://acme/knowledge/page/shared-name";
    let evil_page = "myelin://evil/knowledge/page/shared-name";
    let fetcher = Arc::new(KnFetcher::default());
    fetcher.put(
        acme_page,
        page_search_projection(&en_page_blocks(), Some("en")),
    );
    fetcher.put(
        evil_page,
        page_search_projection(&en_page_blocks(), Some("en")),
    );
    let ix = kn_indexer(fetcher);
    ix.index(&event_in("a", "knowledge.page.created", acme_page, "acme"))
        .expect("index acme");
    ix.index(&event_in("e", "knowledge.page.created", evil_page, "evil"))
        .expect("index evil");

    let acme_t = TenantId("acme".into());
    let evil_t = TenantId("evil".into());

    let acme_hits = ix
        .search_ft(
            &acme_t,
            &region(),
            &AclFilter::ids([acme_page]),
            "scheduler",
            10,
        )
        .expect("acme search");
    assert!(
        acme_hits.iter().any(|h| h.doc_id == acme_page),
        "acme sees its own page"
    );

    let cross = ix
        .search_ft(
            &acme_t,
            &region(),
            &AclFilter::ids([evil_page]),
            "scheduler",
            10,
        )
        .expect("cross-tenant search");
    assert!(
        cross.is_empty(),
        "0 cross-tenant: acme's index holds none of evil's pages"
    );

    let evil_hits = ix
        .search_ft(
            &evil_t,
            &region(),
            &AclFilter::ids([acme_page]),
            "scheduler",
            10,
        )
        .expect("evil search");
    assert!(
        evil_hits.is_empty(),
        "0 cross-tenant: evil's index holds none of acme's pages"
    );
}
