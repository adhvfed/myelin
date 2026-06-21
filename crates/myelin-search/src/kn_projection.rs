//! # `kn_projection` — Knowledge's `declare_indexable` IndexSpec + block/page projection (SRCH-P17 / P-260, M3)
//!
//! **Owning architecture doc:** `search-and-indexing.md` §3.1 (the structured inline nodes
//! mention/artifact_ref/embed are dependable facets), §4.6 (the read-time rollup/formula path —
//! INPUTS indexed, derived value never stored), §4.6.1 (the GIN-indexed JSONB facet scan FLOOR for KN
//! custom DB fields; the measured projection-feeder promotion is the M5 follow-on, SRCH-P27), §4.7
//! (the multilingual analyzers KN text uses). **Reconciliation:** `00-reconciliation-decisions.md`
//! X-2 (the three content nodes byte-identical; `code_block.text` raw), KN-3 (rollup/formula
//! read-time-computed never stored), OQ-C (the promotion threshold). **Contracts:** 6.3
//! (`declare_indexable` — KN's IndexSpec), 13.1 (the content taxonomy), 13.3 (the FieldType facets).
//!
//! ## What SRCH-P17 ships here — the KN slice of contract 6.3 (the engine is UNCHANGED)
//!
//! Git declares its `git.*` code-projection spec in `myelin-git` (it is a service crate that can
//! depend on Search — GIT-P5). **Knowledge has NO service crate yet** (the WASM-clean
//! `myelin-content` freeze crate must NOT depend on downstream Search — it is mid-tier, std-only,
//! `wasm32-unknown-unknown`-clean, and pulling Search in would drag `-gdpr`/`-substrate` across the
//! DAG tier line and break KN-D2). So KN's owned `declare_indexable` IndexSpec is modelled HERE — in
//! the Search consumer crate that owns the [`IndexSpec`] type and consumes the frozen
//! `myelin-content` taxonomy — exactly the wiring the prompt's DELIVERABLE places "in the
//! myelin-search service crate". When the Knowledge service crate lands (the M3 KN producer prompts),
//! it re-homes [`kn_page_index_spec`]/[`kn_db_row_index_spec`] verbatim; the SHAPE does not change.
//!
//! ### The two KN index specs (the page text + the in-doc database row)
//!
//! Knowledge indexes **two** searchable artifact types (contract 6.3, the KN bullet "block+page
//! multilingual + vector-in-v1 + JSONB struct"):
//!
//! 1. **`knowledge`/`page`** — a page (and its block subtree at block granularity, contract 2.6) of
//!    [`myelin_content::Block`] content. The full-text body is the page/block analyzable text
//!    (multilingual, §4.7 — the `lang` tag selects the analyzer chain, SRCH-P12). The **three
//!    structured inline nodes** (mention/artifact_ref/embed, [`myelin_content::InlineNode`]) are
//!    indexed as dependable structured FACETS (`mention`/`artifact_ref`/`embed`, §3.1) so a query can
//!    filter "pages that @mention alice" / "pages embedding this doc" reliably (a node-array walk,
//!    never a regex over prose). A page is **semantically indexed** (vector-in-v1 for semantic KN
//!    search, §4.5). `code_block.text` is carried RAW (X-2) — never markdown-parsed.
//!
//! 2. **`knowledge`/`db_row`** — a row in an in-doc database (`Block::DbView`). Its **custom DB
//!    fields** (the flexible-DB facets, e.g. `priority`/`owner`/`due`) are typed structured facets —
//!    served by the **GIN-indexed JSONB facet scan FLOOR** (§4.6.1). **`rollup`/`formula` fields are
//!    NOT stored** (KN-3): Search indexes their INPUTS (the relation target, the source fields), and a
//!    `Cmp` over a rollup/formula compiles to a post-fetch predicate the view computes at read time
//!    (`crate::compiler::FieldKind::ReadTime`). A row is NON-semantic (a structured record, not prose).
//!
//! ## The structured-inline-node facets (§3.1) — the dependable reference facets
//!
//! The three [`myelin_content::InlineNode`]s produce `refs.edge.created` uniformly (contract 13.1 /
//! 5.4); Search indexes them as structured facets keyed by [`FACET_MENTION`]/[`FACET_ARTIFACT_REF`]/
//! [`FACET_EMBED`]. They are [`FieldType::Relation`] (an ArtifactRef/principal-pseudonym token) — a
//! `Cmp`/`In` over them is a columnar equality on the reference token, never a full-text scan of
//! prose. [`page_search_projection`] walks [`myelin_content::Inline::structured_nodes`] to build them
//! — the SAME node-array walk Refs uses, so there is no second extraction path.
//!
//! ## FLOOR named (SRCH-P17 DoD)
//! - **The GIN-indexed JSONB facet scan for KN custom DB fields** ([`kn_db_row_index_spec`]'s
//!   structured facets): the generated/columnar projection-feeder index promoted per facet at > 5% of
//!   a collection's view executions is the **MEASURED M5 follow-on, SRCH-P27** (OQ-C). The GIN scan
//!   serves correctly meanwhile; promotion changes COST, never correctness. Named.
//! - **The real Knowledge SERVICE crate** that re-homes these specs + ships the live block-tree store
//!   and the push-time projection EMITTER is the M3 KN producer prompt cluster; here Search models the
//!   KN producer's owned spec shape against the frozen taxonomy (the same posture GIT-P5 took for git
//!   before its emitter, GIT-P25). The ENGINE is unchanged — this is producer-corpus WIRING.

use std::collections::BTreeMap;

use myelin_content::{Block, Inline, InlineNode};
use myelin_query::{FieldType, FieldValue};

use crate::indexer::{IndexSpec, SearchProjection};

/// The subsystem token Knowledge declares its projections under (`knowledge` — the §6.4 / events
/// `knowledge.*` token family the indexer whitelists, [`crate::indexer::INDEXER_SUBJECT_PREFIXES`]).
pub const KN_SUBSYSTEM: &str = "knowledge";

/// Knowledge's page artifact type — a page of [`Block`] content (the canonical ref is
/// `myelin://<tenant>/knowledge/page/<id>`, sub-anchored `#b<id>`/`#h<id>` per the §5.7 grammar for a
/// block/heading sub-artifact doc, SRCH-P19).
pub const KN_PAGE_TYPE: &str = "page";

/// Knowledge's in-doc database ROW artifact type — a row of a `Block::DbView` flexible database (the
/// canonical ref is `myelin://<tenant>/knowledge/db_row/<db>:<row>`, sub-anchored `#row-<id>`/
/// `#field-<id>` per §5.7). Its custom DB fields are the GIN-scan JSONB facets (§4.6.1).
pub const KN_DB_ROW_TYPE: &str = "db_row";

/// The structured facet for an `@mention` ([`InlineNode::Mention`]) — the principal pseudonym token
/// the mention targets, indexed as a dependable reference facet (§3.1). [`FieldType::Relation`].
pub const FACET_MENTION: &str = "mention";
/// The structured facet for an inline `artifact_ref` ([`InlineNode::ArtifactRefNode`]) — the
/// referenced artifact's `ArtifactRef` token (§3.1). [`FieldType::Relation`].
pub const FACET_ARTIFACT_REF: &str = "artifact_ref";
/// The structured facet for an inline `embed` ([`InlineNode::Embed`]) — the embedded artifact's
/// `ArtifactRef` token (§3.1). [`FieldType::Relation`].
pub const FACET_EMBED: &str = "embed";

/// **Knowledge's page `declare_indexable` IndexSpec (contract 6.3 — the KN slice).** `knowledge`/
/// `page`, the three structured inline-node facets (mention/artifact_ref/embed, all
/// [`FieldType::Relation`]), **semantic** (vector-in-v1 for semantic KN search, §4.5). The
/// full-text body (the page/block analyzable text, multilingual) arrives at emit time in the
/// index-time [`SearchProjection::text`], not in the spec (the spec is the schema, the projection is
/// the row). `acl_object_type = page` (a page's reachability is decided by the page-tree ReBAC, the
/// KN `page` object type — and a block/heading sub-artifact doc pins its ACL on the parent page).
pub fn kn_page_index_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    // The three structured inline-node reference facets (§3.1) — dependable, columnar, never a
    // regex over prose. Each is a relation token (an ArtifactRef / a principal pseudonym).
    struct_fields.insert(FACET_MENTION.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_ARTIFACT_REF.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_EMBED.to_string(), FieldType::Relation);
    // A page is semantically indexed (vector-in-v1 for semantic KN search, §4.5).
    IndexSpec::new(KN_SUBSYSTEM, KN_PAGE_TYPE, struct_fields).semantic()
}

/// **Knowledge's in-doc database ROW `declare_indexable` IndexSpec (contract 6.3 — the KN slice).**
/// `knowledge`/`db_row`, the **custom DB fields** typed structured facets (the GIN-scan JSONB facet
/// FLOOR, §4.6.1) — here a representative set (`priority` select, `owner` principal, `due` date,
/// `order_key`). The `rollup`/`formula` fields are NOT facets here (KN-3): they are NEVER stored;
/// Search indexes their INPUTS and the view computes the derived value at read time (the
/// `crate::compiler::FieldKind::ReadTime` path). NON-semantic (a structured record, not prose).
/// `acl_object_type = page` (the row's reachability is its host page's; an in-doc DB lives in a page).
///
/// In production the custom-field set is the producer's per-database schema (a flexible DB has
/// per-collection fields); this returns the representative shape the SRCH-P17 drills exercise. The
/// GIN scan serves ANY declared facet correctly; the measured per-facet promotion to a generated
/// index is the M5 follow-on (SRCH-P27).
pub fn kn_db_row_index_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    // The custom DB fields (the JSONB GIN-scan facets, §4.6.1). A `Cmp`/`In` over any of these is a
    // typed columnar equality the GIN scan serves correctly (the FLOOR; promotion is M5/SRCH-P27).
    struct_fields.insert("priority".to_string(), FieldType::Select);
    struct_fields.insert("owner".to_string(), FieldType::Principal);
    struct_fields.insert("due".to_string(), FieldType::Date);
    // The columnar sort key (the LexoRank fractional index, 13.3 — byte-identical to Issues'/KN's).
    struct_fields.insert(crate::engine::ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);
    // A row is NOT vector-embedded (it is a structured record; semantic search is over page prose).
    IndexSpec::new(KN_SUBSYSTEM, KN_DB_ROW_TYPE, struct_fields)
}

/// Every KN index spec (the page + the db_row) — the set a Search indexer registers to consume the
/// real Knowledge corpus. The same set [`register_kn_index_specs`] proves Search ADMITS.
pub fn kn_index_specs() -> Vec<IndexSpec> {
    vec![kn_page_index_spec(), kn_db_row_index_spec()]
}

/// **Register Knowledge's index specs WITH Search (the GATE).** Builds [`kn_index_specs`] and proves
/// Search **accepts** them by admitting them into a live
/// [`IncrementalIndexer`](crate::indexer::IncrementalIndexer)'s per-tenant facet union without a
/// schema mismatch (the only honest definition of "accepted" — Search is the authority that admits).
/// Returns the specs that were accepted. Mirrors git's `register_git_code_projection_spec` (GIT-P5).
pub fn register_kn_index_specs() -> Vec<IndexSpec> {
    let specs = kn_index_specs();
    // Admit them into a real indexer's facet union (the build-time declare_indexable surface). A
    // facet-type collision or a malformed shape would panic at construction; it does not.
    let _accepted = crate::indexer::IncrementalIndexer::new(
        specs.clone(),
        std::sync::Arc::new(NullProjectFetcher),
        std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(16)),
    );
    specs
}

/// A do-nothing [`ProjectFetcher`](crate::indexer::ProjectFetcher) used ONLY to admit the KN specs
/// into a live indexer for the registration GATE (the SPEC half + the projection BUILDER ship here;
/// the real owner-`project` fetch is the Knowledge service crate's emitter). It never fetches —
/// registration does not index. Mirrors git's `NullProjectFetcher` (GIT-P5).
struct NullProjectFetcher;

impl crate::indexer::ProjectFetcher for NullProjectFetcher {
    fn project(
        &self,
        _tenant: &myelin_tenancy::TenantId,
        _region: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<SearchProjection, crate::indexer::ProjectFetchError> {
        // The SPEC registration never fetches a projection (no emitter here). This is the
        // registration GATE — Search admits the schema — not the index path.
        Err(crate::indexer::ProjectFetchError::Gone)
    }
}

/// **Build a page's [`SearchProjection`] from its [`Block`] content (the index-time row, §4.1).**
/// This is the owner's `project(ref, viewer)` body Search consumes (contract 5.6) — NOT a DB read.
/// It produces:
/// - the analyzable full-text `text` (the page/block prose, with `code_block.text` carried RAW, X-2),
/// - the three structured inline-node reference facets (mention/artifact_ref/embed) walked from the
///   inline node arrays (a node-array walk, never a regex over prose, §2.2/§3.1),
/// - the `lang` tag (source-declared here; index-time detection is the indexer's fallback, §4.7).
///
/// In production the Knowledge service builds this from its block-tree store + the per-block
/// rendered-text refs; here it builds it from the frozen [`Block`] taxonomy directly — the same
/// shape the live store swaps in behind (the projection is the row, the store is the source).
pub fn page_search_projection(blocks: &[Block], lang: Option<&str>) -> SearchProjection {
    let mut text = String::new();
    let mut mentions: Vec<String> = Vec::new();
    let mut artifact_refs: Vec<String> = Vec::new();
    let mut embeds: Vec<String> = Vec::new();

    for block in blocks {
        collect_block(block, &mut text, &mut mentions, &mut artifact_refs, &mut embeds);
    }

    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
    // The three structured inline-node reference facets (§3.1). A facet is stamped ONLY when the
    // page actually carries that node kind (an absent facet is not indexed as empty — the columnar
    // shape only carries present references). v1 indexes the FIRST occurrence of each node kind as
    // the facet value (the dependable "does this page reference X" filter); the multi-value facet
    // (every mention) is the SRCH-P19 sub-artifact follow-on. The drill filters on the present node.
    if let Some(m) = mentions.first() {
        fields.insert(FACET_MENTION.to_string(), FieldValue::Relation(m.clone()));
    }
    if let Some(a) = artifact_refs.first() {
        fields.insert(FACET_ARTIFACT_REF.to_string(), FieldValue::Relation(a.clone()));
    }
    if let Some(e) = embeds.first() {
        fields.insert(FACET_EMBED.to_string(), FieldValue::Relation(e.clone()));
    }

    SearchProjection { text, fields, lang: lang.map(|s| s.to_string()) }
}

/// Recursively collect a block's analyzable text + its structured inline-node references. `code_block`
/// text is carried RAW (X-2) — appended verbatim, never markdown-parsed (Search tokenizes it with the
/// code tokenizer, not a language stemmer — but the BODY is raw here). The inline-node walk is the
/// SAME `structured_nodes()` array walk Refs uses (no second extraction path).
fn collect_block(
    block: &Block,
    text: &mut String,
    mentions: &mut Vec<String>,
    artifact_refs: &mut Vec<String>,
    embeds: &mut Vec<String>,
) {
    match block {
        Block::Paragraph { inline } | Block::Heading { inline, .. } => {
            collect_inline(inline, text, mentions, artifact_refs, embeds);
        }
        Block::BulletList { items } | Block::OrderedList { items, .. } => {
            for item in items {
                for b in &item.blocks {
                    collect_block(b, text, mentions, artifact_refs, embeds);
                }
            }
        }
        Block::TaskList { items } => {
            for item in items {
                collect_inline(&item.inline, text, mentions, artifact_refs, embeds);
            }
        }
        Block::Blockquote { blocks } | Block::Callout { blocks, .. } => {
            for b in blocks {
                collect_block(b, text, mentions, artifact_refs, embeds);
            }
        }
        Block::CodeBlock { text: code, .. } => {
            // RAW (X-2): the code body is appended verbatim, NOT markdown-parsed.
            push_text(text, code);
        }
        Block::Table { columns, rows } => {
            for col in columns {
                collect_inline(&col.header, text, mentions, artifact_refs, embeds);
            }
            for row in rows {
                for cell in row {
                    for b in &cell.blocks {
                        collect_block(b, text, mentions, artifact_refs, embeds);
                    }
                }
            }
        }
        Block::Toggle { summary, blocks } => {
            collect_inline(summary, text, mentions, artifact_refs, embeds);
            for b in blocks {
                collect_block(b, text, mentions, artifact_refs, embeds);
            }
        }
        Block::Image { alt, caption, .. } => {
            push_text(text, alt);
            if let Some(c) = caption {
                collect_inline(c, text, mentions, artifact_refs, embeds);
            }
        }
        // A structured `embed` block is a load-bearing reference node (§2.1) — index it as the embed
        // facet so "pages embedding this artifact" filters reliably.
        Block::Embed { reference, .. } => {
            embeds.push(reference.0.clone());
        }
        // db_view / sync_block / divider carry no page prose to index at the page-text grain (the
        // db_view's ROWS are the separate `db_row` index spec; sync_block resolves at render time).
        Block::DbView { .. } | Block::SyncBlock { .. } | Block::Divider => {}
    }
}

/// Collect one [`Inline`]'s plain text + its three structured node references (the node-array walk,
/// §2.2). The text is the concatenated run content (the serialized markdown-subset string carries the
/// placeholder; for the searchable body we want the prose runs, so we read the spans' text directly).
fn collect_inline(
    inline: &Inline,
    text: &mut String,
    mentions: &mut Vec<String>,
    artifact_refs: &mut Vec<String>,
    embeds: &mut Vec<String>,
) {
    for span in &inline.spans {
        if let myelin_content::Span::Text { text: run, .. } = span {
            push_text(text, run);
        }
    }
    // The structured nodes (a node-array walk — the SAME seam Refs reads, never a regex over prose).
    for node in inline.structured_nodes() {
        match node {
            InlineNode::Mention(principal) => mentions.push(principal.principal_id.0.clone()),
            InlineNode::ArtifactRefNode(r) => artifact_refs.push(r.0.clone()),
            InlineNode::Embed(r) => embeds.push(r.0.clone()),
        }
    }
}

/// Append a run of text to the searchable body with a separating space (so adjacent block/run prose
/// does not fuse into one token — the analyzer still tokenizes on whitespace).
fn push_text(text: &mut String, run: &str) {
    if !text.is_empty() && !run.is_empty() {
        text.push(' ');
    }
    text.push_str(run);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::IncrementalIndexer;
    use myelin_content::{parse_inline, Block, HeadingLevel};
    use myelin_events::ArtifactRef;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn mention(id: &str) -> InlineNode {
        InlineNode::Mention(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        ))
    }

    /// **The KN page spec is Knowledge's owned 6.3 shape.** Pins every field — a rename of a Search
    /// `IndexSpec` field, or a drift in the structured-node facet set, breaks the registrant here.
    #[test]
    fn page_spec_is_kn_owned_6_3_shape() {
        let s = kn_page_index_spec();
        assert_eq!(s.subsystem, "knowledge");
        assert_eq!(s.type_, "page");
        assert_eq!(s.acl_object_type, "page", "a page's reachability is the page-tree's");
        assert!(s.semantic, "a page is semantically indexed (vector-in-v1, §4.5)");
        // The three structured inline-node reference facets (§3.1), all Relation.
        assert_eq!(s.struct_fields.len(), 3);
        for facet in [FACET_MENTION, FACET_ARTIFACT_REF, FACET_EMBED] {
            assert_eq!(
                s.struct_fields.get(facet),
                Some(&FieldType::Relation),
                "`{facet}` is a dependable reference facet (Relation)"
            );
        }
    }

    /// **The KN db_row spec is the JSONB GIN-scan facet shape (§4.6.1) — rollup/formula are NOT
    /// stored facets (KN-3).** The custom DB fields are typed facets; the order_key is the columnar
    /// sort. Non-semantic.
    #[test]
    fn db_row_spec_is_the_gin_scan_facet_shape() {
        let s = kn_db_row_index_spec();
        assert_eq!(s.subsystem, "knowledge");
        assert_eq!(s.type_, "db_row");
        assert!(!s.semantic, "a db row is a structured record, not vector-embedded prose");
        // The custom DB fields (the GIN-scan facets) + the order_key sort.
        assert_eq!(s.struct_fields.get("priority"), Some(&FieldType::Select));
        assert_eq!(s.struct_fields.get("owner"), Some(&FieldType::Principal));
        assert_eq!(s.struct_fields.get("due"), Some(&FieldType::Date));
        assert_eq!(
            s.struct_fields.get(crate::engine::ORDER_KEY_FIELD),
            Some(&FieldType::OrderKey)
        );
        // rollup/formula are NOT stored (KN-3) — they are never facets in the spec.
        assert!(!s.struct_fields.contains_key("rollup"));
        assert!(!s.struct_fields.contains_key("formula"));
    }

    /// **Search ACCEPTS both KN specs (the GATE).** Search admits them into a live indexer's
    /// per-tenant facet union without a schema mismatch — the accepted set is byte-equal to the
    /// declared set.
    #[test]
    fn registration_is_accepted_by_search() {
        let accepted = register_kn_index_specs();
        assert_eq!(accepted, kn_index_specs(), "Search accepts the declared KN specs verbatim");
        // And a live indexer over them opens (the facet union is consistent across the two specs).
        let _ix = IncrementalIndexer::new(
            kn_index_specs(),
            std::sync::Arc::new(NullProjectFetcher),
            std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(16)),
        );
    }

    /// **The page projection walks the block tree + the structured inline nodes.** The full-text body
    /// carries the prose (and raw code), and the three reference facets are extracted via the
    /// node-array walk (never a regex over prose).
    #[test]
    fn page_projection_extracts_text_and_structured_facets() {
        let referenced = ArtifactRef("myelin://acme/issues/issue/ENG-1".into());
        let embedded = ArtifactRef("myelin://acme/knowledge/page/99".into());
        let blocks = vec![
            Block::Heading {
                level: HeadingLevel::new(1).unwrap(),
                inline: parse_inline("Design Notes", &[]),
            },
            Block::Paragraph {
                inline: parse_inline(
                    &format!("see {} and ping {}", myelin_content::OBJ, myelin_content::OBJ),
                    &[InlineNode::ArtifactRefNode(referenced.clone()), mention("alice")],
                ),
            },
            Block::CodeBlock { lang: Some("rust".into()), text: "let x = scheduler_deadlock();".into() },
            Block::Embed { reference: embedded.clone(), display: myelin_content::EmbedDisplay::Card },
        ];
        let p = page_search_projection(&blocks, Some("en"));

        // The full-text body carries the prose AND the raw code (X-2: code is verbatim).
        assert!(p.text.contains("Design Notes"));
        assert!(p.text.contains("scheduler_deadlock"), "raw code body is indexed (X-2)");
        assert_eq!(p.lang.as_deref(), Some("en"));

        // The structured reference facets are extracted via the node-array walk.
        assert_eq!(
            p.fields.get(FACET_ARTIFACT_REF),
            Some(&FieldValue::Relation(referenced.0.clone()))
        );
        assert_eq!(
            p.fields.get(FACET_MENTION),
            Some(&FieldValue::Relation("alice".to_string()))
        );
        assert_eq!(
            p.fields.get(FACET_EMBED),
            Some(&FieldValue::Relation(embedded.0.clone())),
            "the structured embed block is a dependable embed facet"
        );
    }

    /// **A page with no structured nodes carries no reference facets** (the columnar shape only holds
    /// present references — an absent facet is not indexed as empty).
    #[test]
    fn page_with_no_nodes_has_no_reference_facets() {
        let blocks = vec![Block::Paragraph { inline: parse_inline("plain prose only", &[]) }];
        let p = page_search_projection(&blocks, None);
        assert!(p.fields.is_empty(), "no structured nodes ⇒ no reference facets");
        assert!(p.text.contains("plain prose"));
        assert!(p.lang.is_none(), "no source-declared lang ⇒ the indexer detects it (§4.7)");
    }
}