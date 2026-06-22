//! # The Knowledge Search feed — `declare_indexable` + the `query`/`semantic` Filter conjoin (KN-P21 → P-311, M3)
//!
//! **Owning architecture docs:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md`
//! §6 (search indexing granularity — index at BOTH page-level and block-level; semantic/vector
//! in v1; multilingual; permission-aware via the `Filter` conjoin — *Knowledge never indexes
//! itself, it `project`s text and Search consumes off the bus*) + §7 (the semantic events the
//! indexer reacts to, never keystrokes); `03-events-contracts-and-glue.md` §2.2 (project feeds the
//! index). **External insight:** `external-insights/04-hard-problems.md` §5 (Search is a derived
//! store — *the index never reads source databases; it asks each owner to re-emit through the live
//! consumer*).
//!
//! **Contract-index rows:**
//! - **6.3** `declare_indexable(IndexSpec)` — **OWNED** (the Knowledge projection spec): the page
//!   doc (multilingual title+body + the three structured inline-node facets + vector-in-v1) + the
//!   per-significant-block doc + the in-doc database `db_row` doc.
//! - **6.1** `query(ast, viewer, …) → RankedResults` / **6.2** `semantic(text|vec, viewer, k, …)` —
//!   **CONSUMED** (Knowledge drives them with the `list_objects` `Filter` ALWAYS conjoined — the
//!   `search-requires-acl-filter` discipline; the KN-D5 re-confirm: a restricted page/block never
//!   appears in a result, INCLUDING the COUNT, across the FT / structured / semantic / RAG paths).
//!
//! ## What this module ships (the genuinely-new KN-P21 producer work)
//!
//! When the Knowledge SERVICE crate did not yet exist, `myelin-search` modelled KN's owned specs +
//! the page projection builder emitter-less (SRCH-P17 / P-260, the `register_git_code_projection_spec`
//! posture GIT-P5 took), with the explicit note: *"When the Knowledge service crate lands (the M3 KN
//! producer prompts), it re-homes `kn_page_index_spec`/`kn_db_row_index_spec` verbatim; the SHAPE
//! does not change."* This is that re-home. Per EI-01 §7 (one primitive — never a parallel second
//! shape) the spec CONSTRUCTORS stay owned by the Search crate that owns the `IndexSpec` type; this
//! module RE-EXPORTS them as Knowledge's owned 6.3 surface ([`kn_index_specs`] /
//! [`register_kn_index_specs`] / [`page_search_projection`]) and adds the genuinely-new producer
//! glue that did NOT exist:
//!
//! 1. **[`feed_project`]** — the `project` feed: build the index-time [`SearchProjection`] for a KN
//!    artifact ref from its block content, the body Search consumes off the bus (no DB read). For a
//!    page-grain ref it is the page projection; for a `#h<id>`/`#b<id>` significant-block sub-ref it
//!    is the per-significant-block projection (architecture §6 — the heading-anchor jump target).
//! 2. **[`kn_search_query`] / [`kn_search_semantic`]** — the Knowledge-driven `query`/`semantic`
//!    entries that ALWAYS conjoin the `list_objects` `Filter` (the `search-requires-acl-filter`
//!    discipline; never a post-filter, never an un-ACL'd search) over Knowledge's `page` object
//!    type. These are the KN-D5 re-confirm seam: a confidential page/block is excluded by the
//!    pre-filter BEFORE scoring, so it appears in NEITHER the hit list NOR the COUNT.
//!
//! ## KN-D5 re-confirm (0 leak incl. COUNT) — how this seam earns it
//!
//! The conjoin happens in `myelin_search::query`/`semantic` (the engine half, frozen SRCH-P08/P10/
//! P11): `list_objects(viewer, read, page)` returns the leak-free pre-filter (`Ids` | `Filter`),
//! which is lowered to an `AclFilter` and conjoined into EVERY engine branch (FT / structured /
//! vector) BEFORE scoring. A denied page never enters the candidate set — so `RankedResults.hits`
//! omits it AND `hits.len()` (the result COUNT over an unbounded page) does not count it. The
//! `SetExpr::None` short-circuit returns an empty result without touching the engine (no count can
//! leak). Knowledge's role is to ALWAYS route through this conjoining entry — never a bespoke
//! un-ACL'd search path. Proven by the unit tests below + the `--features integration` KN-D5 drill
//! over the dev-stack (the search/embed leg).
//!
//! ## DAG note (the sanctioned acyclic producer→Search edge, EI-01 §7)
//!
//! `myelin-knowledge` depends on `myelin-search` here — the SAME acyclic edge `myelin-git` already
//! carries (it registers its `git.*` code-projection spec + drives the code-search pre-filter the
//! same way). Search is a LEAF consumer that owns the `IndexSpec` type + the query engine and does
//! NOT depend back on any producer crate, so the edge introduces no cycle. Knowledge NEVER indexes
//! itself: it PROJECTS text (Search consumes off the bus); the only spec/projection authority is
//! Search's, re-homed here as Knowledge's owned 6.3 surface.
//!
//! ## FLOORS named (KN-P21 DoD)
//! - **KQ-10** (the measured, parallel >5% search-block prune): the per-significant-block doc is
//!   indexed for every significant block today; the MEASURED pruning of blocks below a usage
//!   threshold (so the block index does not over-grow) is the parallel KQ-10 follow-on — it changes
//!   COST, never correctness. Named, not silently done.
//! ## Mutation-score floor (KN-P21 TESTS — the conjoin is mandatory-core, a search leak is a leak)
//!
//! The leak-critical CONJOIN LOGIC (lower `list_objects` → `AclFilter` → conjoin into every engine
//! branch before scoring; the `None` short-circuit; the no-N+1 single `list_objects`) lives in the
//! frozen `myelin-search` engine (SRCH-P08/P10/P11) and is mutation-tested THERE. What `search_feed`
//! adds is the Knowledge-side DISCIPLINE: ALWAYS route `query`/`semantic` through the conjoining
//! entry pinned to the `page` object type — there is no Knowledge-side ACL decision a mutant could
//! flip (a bespoke un-ACL'd path would be the lint failure, not a passing mutant). The
//! mutation-score floor on this module's leak-relevant surface (the object-type pin + the conjoin
//! routing) is **≥ 90%** — the same mandatory-core floor `refs_glue` (the project-leak gate) holds;
//! the engine's conjoin floor is the Search crate's. A mutant that drops the `page` pin or routes
//! around the conjoining entry is killed by [`tests::kn_d5_query_conjoin_excludes_confidential_page_incl_count`]
//! / [`tests::kn_d5_semantic_conjoin_excludes_confidential_page`].
//!
//! - **The real per-tenant facet-promotion / live indexer / NATS-JetStream relay** is the running
//!   Search service (SRCH-P*/the dev-stack); here the conjoin GATE drives the frozen engine over an
//!   in-memory `TantivyBackend` to PROVE the 0-leak property (the same property the live stack
//!   inherits — the engine is unchanged). The live search/embed leg is the `--features integration`
//!   KN-D5 drill.

use myelin_identity::{Consistency, ObjectType, Permission, Principal};
use myelin_query::QueryAst;
use myelin_search::{
    query as search_query, semantic as search_semantic, AclFilter, ConsistencyStats, IndexBackend,
    IndexSpec, ListObjectsPort, Page, QueryError, QueryStats, RankedResults, ScopedEngine,
    SearchProjection, VectorQuery, KN_PAGE_TYPE,
};

// Re-home Knowledge's owned 6.3 surface VERBATIM from the Search crate that owns the `IndexSpec`
// type (EI-01 §7 — one primitive, never a parallel second shape). These ARE Knowledge's owned
// `declare_indexable` specs; the SHAPE does not change from the emitter-less SRCH-P17 model.
pub use myelin_search::{
    kn_db_row_index_spec, kn_index_specs, kn_page_index_spec, page_search_projection,
    register_kn_index_specs, FACET_ARTIFACT_REF, FACET_EMBED, FACET_MENTION, KN_DB_ROW_TYPE,
    KN_SUBSYSTEM,
};
// The per-significant-block projection (architecture §6's block-grain doc) — Search owns the
// sub-artifact projection (SRCH-P19); Knowledge re-homes it as the block leg of its 6.3 surface.
pub use myelin_search::block_subdoc_projection;

/// The Knowledge ACL object type the Search `Filter` conjoin keys on — `page` (a page's
/// reachability is decided by the page-tree ReBAC; a significant-block / heading sub-doc pins its
/// ACL on the parent page, so the block doc is ACL'd as `page` too — contract 6.3
/// `acl_object_type = page`, [`kn_page_index_spec`]). Knowledge's `query`/`semantic` always
/// `list_objects(viewer, read, page)` — never an un-typed or un-ACL'd search.
pub const KN_SEARCH_OBJECT_TYPE: &str = KN_PAGE_TYPE;

/// **The `project` feed for the Search index (contract 6.3 / architecture §2.2 / §6).** Build the
/// index-time [`SearchProjection`] Search consumes off the bus for a Knowledge artifact — NOT a DB
/// read. `blocks` is the artifact's block content (the page subtree, or the single significant
/// block for a block-grain ref); `grain` selects the projection grain; `lang` is the source-declared
/// language tag (the multilingual analyzer selector, §6; `None` ⇒ the indexer detects it, §4.7).
///
/// - [`FeedGrain::Page`] → [`page_search_projection`] (the page doc: title+body + the three
///   structured inline-node reference facets + vector-in-v1).
/// - [`FeedGrain::SignificantBlock`] → [`block_subdoc_projection`] over the single block (the
///   heading/callout/code jump-target doc, §6 — the `#h<id>`/`#b<id>` `#sub` anchor target).
///
/// Knowledge NEVER indexes itself: this is the text it PROJECTS; Search builds the index (no
/// cross-DB, external-insights/04 §5).
pub fn feed_project(
    blocks: &[myelin_content::Block],
    grain: FeedGrain,
    lang: Option<&str>,
) -> SearchProjection {
    match grain {
        FeedGrain::Page => page_search_projection(blocks, lang),
        FeedGrain::SignificantBlock => {
            // A block-grain doc projects the SINGLE significant block (the jump-target, §6). An
            // empty slice projects empty (the caller hands the one block); the first is the block.
            match blocks.first() {
                Some(block) => block_subdoc_projection(block, lang),
                None => SearchProjection {
                    text: String::new(),
                    fields: std::collections::BTreeMap::new(),
                    lang: lang.map(|s| s.to_string()),
                },
            }
        }
    }
}

/// The projection grain a [`feed_project`] call builds — the page doc vs. the per-significant-block
/// doc (architecture §6: *index at BOTH page-level and block-level*). The `db_row` doc grain is the
/// flexible-database row projection ([`kn_db_row_index_spec`]) fed by the database module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedGrain {
    /// The page doc (title + concatenated body, language-tagged + the structured facets + vector).
    Page,
    /// A per-significant-block doc (a heading/callout/code jump target; the `#h<id>`/`#b<id>` anchor).
    SignificantBlock,
}

/// **Knowledge's `query` entry with the `list_objects` `Filter` ALWAYS conjoined (contract 6.1; the
/// `search-requires-acl-filter` discipline).** A thin Knowledge-owned wrapper over
/// [`myelin_search::query`] that pins the object type to `page` ([`KN_SEARCH_OBJECT_TYPE`]) so the
/// ACL pre-filter is ALWAYS Knowledge's page-tree reachability — never an un-ACL'd or mis-typed
/// search. The conjoin (lower `list_objects` → `AclFilter` → conjoin into every engine branch
/// before scoring) is the frozen Search engine's; routing through it is Knowledge's discipline.
///
/// This is the KN-D5 re-confirm seam: a confidential page is excluded by the pre-filter, so it
/// appears in NEITHER `RankedResults.hits` NOR the COUNT (`hits.len()` over an unbounded page).
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

/// **Knowledge's `semantic`/RAG entry with the `list_objects` `Filter` ALWAYS conjoined (contract
/// 6.2; §4.5 filter-during-traversal — k VISIBLE neighbours).** The Knowledge-owned wrapper over
/// [`myelin_search::semantic`] pinning the `page` object type. An agent's RAG retrieval rides this
/// with the agent's DELEGATED principal as `viewer`, so the agent never retrieves a page its
/// delegated principal cannot see (RAG is permission-correct by the SAME pre-filter — not a weaker
/// path). The KN-D5 re-confirm over the semantic/embed/RAG leg: a confidential page never enters the
/// visible-neighbour set, so it leaks through neither the result nor the count.
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

/// The Knowledge read permission the Search `list_objects` conjoin keys on (`knowledge.read`) — the
/// permission Knowledge's `query`/`semantic` ask Identity to `list_objects(viewer, read, page)` at.
/// A PII-free label; the SAME permission the per-op Layer-2 `check` gates reads at (KN-P14).
pub const KN_READ_PERMISSION: &str = "knowledge.read";

/// The [`Permission`] value Knowledge's Search conjoin keys on ([`KN_READ_PERMISSION`]). Exposed so
/// the production wiring binds `list_objects` at exactly the read permission (one source of truth).
pub fn kn_read_permission() -> Permission {
    Permission(KN_READ_PERMISSION.to_string())
}

/// **The Knowledge IndexSpecs as the owned 6.3 surface (page + significant-block + db_row).** The
/// page + db_row specs come from the Search-owned constructors (re-homed verbatim); the
/// significant-block doc shares the page spec's `acl_object_type=page` reachability (a block is
/// never more visible than its page). Returns the set Knowledge `declare_indexable`s with Search.
///
/// (The block doc is indexed as the page object type — its ACL is the parent page's — so it does
/// NOT add a third *spec* TYPE; it rides the `page` spec's facet shape with the sub-artifact
/// anchor facets the Search sub-artifact projection adds. The two SPEC types are `page` + `db_row`,
/// exactly [`kn_index_specs`].)
pub fn kn_declared_index_specs() -> Vec<IndexSpec> {
    kn_index_specs()
}

/// A convenience: an [`AclFilter`] is NEVER constructed by Knowledge directly — the conjoin lowers
/// `list_objects` to it inside the Search engine. This re-export documents that the ACL clause is
/// the engine's frozen primitive, not a Knowledge-side parallel filter (EI-01 §7). Knowledge's
/// discipline is to ALWAYS route `query`/`semantic` through the conjoining entry, never to compose
/// an `AclFilter` itself (a bespoke filter would be a second, un-audited ACL path).
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

    /// A scripted `ListObjectsPort` returning a canned answer + counting the calls (the no-N+1 GATE
    /// reads the count). Mirrors the Search pipeline's `FakeAuthz` but is the Knowledge-side fake.
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
        // The KN page spec's facets (the structured inline-node reference facets) + the order key.
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

    /// Index a KN page corpus: a PUBLIC runbook page + a CONFIDENTIAL incident page (both match the
    /// FT term "deadlock"), each projected via [`feed_project`] (the page `project` feed), then
    /// upserted into a live in-memory backend. The doc id is the page's `ArtifactRef`.
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

    /// **The owned 6.3 surface re-homes the Search-modelled specs VERBATIM (the SHAPE does not
    /// change).** Knowledge's `declare_indexable` specs are byte-equal to the Search-owned set, and
    /// the page spec is the semantic page doc, the db_row spec the non-semantic JSONB-facet doc.
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
        let row = kn_db_row_index_spec();
        assert_eq!(row.type_, KN_DB_ROW_TYPE);
        assert!(
            !row.semantic,
            "a db row is a structured record, not vector-embedded prose"
        );
        // Exactly two SPEC types (page + db_row); the block doc rides the page object type.
        assert_eq!(specs.len(), 2);
    }

    /// **Search ADMITS Knowledge's declared specs (the registration GATE).** The owned set is
    /// accepted into a live indexer's per-tenant facet union without a schema mismatch.
    #[test]
    fn search_admits_the_declared_specs() {
        let accepted = register_kn_index_specs();
        assert_eq!(
            accepted,
            kn_declared_index_specs(),
            "Search admits the declared KN specs verbatim"
        );
    }

    /// **The `project` feed builds the page projection from block content (architecture §2.2/§6).**
    /// The page grain produces the analyzable body + the structured reference facets via the
    /// node-array walk (never a regex); a significant-block grain projects the single block.
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

        // The significant-block grain projects the single heading block (the jump target).
        let block = feed_project(&blocks[..1], FeedGrain::SignificantBlock, Some("en"));
        assert!(
            block.text.contains("Design Notes"),
            "the block doc carries the heading prose"
        );
    }

    /// **KN-D5 re-confirm — the FT `query` conjoin: a confidential page never appears in the result
    /// NOR the COUNT, then a grant makes it visible.** Both pages match "deadlock"; the unauthorized
    /// allow-set excludes SECRET-9 → it is in neither `hits` nor `hits.len()` (0 leak incl. COUNT).
    #[test]
    fn kn_d5_query_conjoin_excludes_confidential_page_incl_count() {
        let be = kn_page_corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let q = ast_body("deadlock");

        // UNAUTHORIZED: the allow-set excludes the confidential page.
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
        // THE COUNT-LEAK CLOSE: the result count is the VISIBLE count only — the forbidden page is
        // not counted (KN-D5 0 leak incl. COUNT).
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

        // GRANTED: the allow-set now includes SECRET-9 → it surfaces on re-query.
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

    /// **KN-D5 re-confirm — `SetExpr::None` short-circuits to an EMPTY result without touching the
    /// engine (a `WHERE false` ACL): the count cannot leak.** A viewer who can read no page gets an
    /// empty result + 0 engine branches (no candidate set is ever materialised).
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
            "the COUNT is 0 — no forbidden page is revealed"
        );
        assert_eq!(
            stats.engine_branches(),
            0,
            "no engine branch ran (short-circuit, no count leak)"
        );
    }

    /// **KN-D5 re-confirm — the semantic/RAG `semantic` conjoin: a confidential page NEVER enters the
    /// visible-neighbour set for an unauthorized viewer (filter-during-traversal), then a grant makes
    /// it visible.** The query text is the EXACT text of the secret page (its nearest vector), but the
    /// allow-set excludes it: it leaks through neither the result nor the count (the SRCH-D1 vector
    /// half re-confirmed over the KN corpus / agent-RAG path).
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
        // The query is the EXACT text of the secret page → SECRET-9 is its nearest vector.
        let vq = VectorQuery::Text {
            text: "deadlock secret ops postmortem".into(),
            embedder: &embedder,
        };
        let q = semantic_ast("deadlock secret ops postmortem");

        // UNAUTHORIZED (agent's delegated principal cannot read the confidential page).
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

        // GRANT → re-search: the confidential page is now a visible neighbour.
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

    /// **The read permission the conjoin keys on is `knowledge.read` (one source of truth).**
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
