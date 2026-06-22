//! # `issues_projection` — Issues' `declare_indexable` IndexSpec + the FieldType facets + the
//! `order_key` columnar sort (SRCH-P20 / P-338, M4)
//!
//! **Owning architecture doc:** `search-and-indexing.md` §3.1 (the `FieldType`-typed facets; the
//! `order_key` LexoRank columnar fast-field for sort, byte-identical to Issues'/Knowledge's encoding),
//! §4.6.1 (the GIN-indexed JSONB facet scan custom-field path + the measured projection-feeder
//! promotion follow-on). **Contracts:** 6.3 (consume Issues' IndexSpec — the `declare_indexable`
//! shape), 13.3 (the `FieldType` facets + the `order_key`/LexoRank encoding).
//!
//! ## What SRCH-P20 ships here — the Issues slice of contract 6.3 (the engine is UNCHANGED)
//!
//! The M4 Issues producer (ISS-P04 / P-243, `myelin_issues::declares`) already declared its OWN
//! `declare_indexable` IndexSpec — the `issue.*` facets projection (the seven structured board/list/
//! search facets `state_category`/`priority`/`assignee`/`type_rank`/`project_id`/`cycle_id`/`rank`,
//! non-semantic, `acl_object_type = "issue"`). That spec is the producer-side authority. SRCH-P20 is
//! the **consumer side**: Search models the SAME consumed spec + ships the index-time **projection
//! BUILDER** ([`issue_search_projection`]) that turns an issue's facet values + title/body free-text
//! into the [`SearchProjection`] Search indexes, so the real Issues corpus is searchable end-to-end.
//!
//! **Coherence (EI-01 §7) — Search models, never re-defines, the Issues spec.** `myelin-issues`
//! depends on `myelin-search` (never the reverse — Issues is a producer above the Search consumer in
//! the §2.9 DAG), so Search CANNOT import `myelin_issues`. It models the byte-identical consumed spec
//! against the frozen [`IndexSpec`]/[`FieldType`] types — the EXACT posture [`crate::kn_projection`]
//! (KN) and [`crate::git_code_projection`] (Git) take for their producer corpora. A CDC-style parity
//! test pins the modelled facet SET + types against the producer's declared shape so a drift between
//! the two is caught at the plan layer, not in prod.
//!
//! ## The `order_key`/`rank` reconciliation — Search's columnar-sort convention (DOCUMENTED deviation)
//!
//! The Issues producer names its LexoRank facet **`rank`** (its board-domain name, `FACET_RANK` in
//! `myelin_issues::declares`), typed [`FieldType::OrderKey`]. Search's index document, however, has a
//! SINGLE conventional columnar fast-field for the order_key sort: [`crate::engine::ORDER_KEY_FIELD`]
//! (= `"order_key"`) — the dedicated `STRING|FAST` field the engine's
//! [`search_structured`](crate::IncrementalIndexer::search_structured) sorts on
//! (`order_by_string_fast_field(ORDER_KEY_FIELD, …)`). KN's `db_row` spec already uses `order_key`
//! verbatim for exactly this reason. The engine is UNCHANGED (the prompt's DoD), so for an issue to
//! sort by its LexoRank rank via the dedicated columnar fast-field, **the Search-consumed projection
//! emits the rank value under the `order_key` index-doc convention**, not under the producer's `rank`
//! domain name. The VALUE + ENCODING + TYPE are byte-identical (13.3 LexoRank); only the index-doc
//! facet KEY is Search's `order_key` convention vs Issues' `rank` board name. This is the documented
//! deviation (external-insights/01 §1): mapping the producer's domain facet name to the engine's one
//! columnar-sort fast-field, rather than weakening the engine to sort on an arbitrarily-named OrderKey
//! facet. [`issue_rank_facet_is_the_order_key_convention`] pins it; the value never changes.
//!
//! ## FLOOR named (SRCH-P20 DoD)
//! - **The GIN-indexed JSONB facet scan serves the Issues board facets** ([`issue_index_spec`]'s
//!   typed facets, §4.6.1): a custom-field/board facet query is served by the GIN scan correctly. The
//!   **measured projection-feeder promotion** per hot facet (a facet filtered in > 5% of a
//!   collection's view executions over a rolling window) to a generated/columnar index is the **M5
//!   follow-on, SRCH-P27** (OQ-C) — the owner of the per-facet frequency signal is Issues/KN, Search
//!   consumes it and decides promotion. The GIN scan serves CORRECTLY meanwhile; promotion changes
//!   COST, never correctness. Named so the GIN scan is not mistaken for the final-cost answer.
//!   Greppable as [`IssueGinScanProjectionFeederFloor`].
//! - **The Issues Tier-3 board-escalation valve** (the byte-identical ACL pre-filter, the OLTP-budget
//!   escalation seam) is the sibling slice **SRCH-P21 / P-339** — it conjoins the SetExpr ACL
//!   byte-identically over the facets THIS slice indexes. Named so the board-facet path is not
//!   mistaken for the escalation valve.
//! - **The real Issues Search projection EMITTER** (the live `project(ref, viewer)` that walks an
//!   issue + emits the per-doc projection through the outbox) is **ISS-P17**. Here Search ships the
//!   SPEC model + the projection BUILDER the emitter feeds; the integration test drives the genuine
//!   builder over a real Issues corpus.
//! - **No new mutation-core module** — the SRCH-P09 mutation floor (the SetExpr ACL conjoin decision
//!   logic) still holds on the Issues corpus; this slice is producer-corpus WIRING, the engine
//!   decision logic is unchanged.

use std::collections::BTreeMap;

use myelin_query::{FieldType, FieldValue, OrderKey};

use crate::engine::ORDER_KEY_FIELD;
use crate::indexer::{IndexSpec, SearchProjection};

/// The subsystem token Issues declares its projection under (`issue`) — byte-identical to
/// `myelin_issues::declares::ISSUE_SUBSYSTEM`. Search models it here because `myelin-issues` depends
/// on `myelin-search` (never the reverse), so Search cannot import Issues (the [`crate::kn_projection`]
/// posture — the shape, not a second contract).
pub const ISSUE_SUBSYSTEM: &str = "issue";

/// The artifact type Issues' facets projection indexes — an `issue` (the canonical `ENG-1421` row;
/// the projection feeder's derived indexable doc). The canonical ref is
/// `myelin://<tenant>/issue/issue/<PROJECTKEY>-<seqno>` (change #6 — the `<PROJECTKEY>-<seqno>` key is
/// the Search `doc_id`, never the `#1421` UI short form).
pub const ISSUE_TYPE: &str = "issue";

/// The ACL object type an issue doc's reachability filter pins on — the **`issue`** object itself
/// (an issue's reachability is decided by the issue object's own frozen ReBAC `view` permission with
/// the `- confidential` set-difference — there is NO parent ACL object here, UNLIKE git's blob→repo).
/// Byte-identical to `myelin_issues::declares` `acl_object_type`.
pub const ISSUE_ACL_OBJECT_TYPE: &str = "issue";

/// The structured-facet key for the FIXED state category (`backlog`/`started`/`done`/… — the
/// cross-project reporting invariant the board scan keys on; an exact-match [`FieldType::Select`]).
pub const FACET_STATE_CATEGORY: &str = "state_category";
/// The structured-facet key for the issue priority (the typed-core `priority`; an ordered
/// [`FieldType::Int`] so `priority >= P2` is a structured comparison).
pub const FACET_PRIORITY: &str = "priority";
/// The structured-facet key for the assignee — a *pseudonymous* principal id (erasure-safe, EI-04 §1;
/// a [`FieldType::Principal`] facet compared by equality only, never ordered).
pub const FACET_ASSIGNEE: &str = "assignee";
/// The structured-facet key for the denormalised type rank (sub-task=0 … initiative=3 — the
/// board↔roadmap partitioning facet; an ordered [`FieldType::Int`]).
pub const FACET_TYPE_RANK: &str = "type_rank";
/// The structured-facet key for the parent project (the Identity `project` authz scope + the
/// per-project board filter; a [`FieldType::Relation`] ref facet).
pub const FACET_PROJECT_ID: &str = "project_id";
/// The structured-facet key for the current cycle membership (the time-axis/burndown filter; a
/// [`FieldType::Relation`] ref facet — nullable, the row carries the denormalised cache).
pub const FACET_CYCLE_ID: &str = "cycle_id";

/// The producer's (Issues' board-domain) name for the LexoRank rank facet (`rank` in
/// `myelin_issues::declares::FACET_RANK`). Search maps this to its index-doc
/// [`ORDER_KEY_FIELD`](crate::engine::ORDER_KEY_FIELD) columnar-sort convention — see the module-level
/// "the `order_key`/`rank` reconciliation" note. Carried as a const so the parity test can assert the
/// mapping explicitly (the value/encoding/type are byte-identical; only the index-doc KEY differs).
pub const ISSUE_PRODUCER_RANK_FACET: &str = "rank";

/// **Issues' `declare_indexable` IndexSpec (contract 6.3 — the Search-side consumed model).** Byte-
/// identical in facet TYPES to `myelin_issues::declares::issue_facets_projection_spec`: `subsystem =
/// "issue"`, `type = "issue"`, the seven structured board/list/search facets, **non-semantic** (Issues
/// is trigram-title + facet filter in v1, not vector-embedded — semantic embedding is the post-v1
/// follow-on), `acl_object_type = "issue"`.
///
/// The full-text body (`title` / props free-text / comment bodies) is NOT in the spec — it arrives at
/// emit time in the index-time [`SearchProjection::text`] ([`issue_search_projection`]). The spec is
/// the columnar schema; the projection is the row.
///
/// **The `order_key`/`rank` convention (DOCUMENTED, see module note):** Search declares the LexoRank
/// rank facet under [`ORDER_KEY_FIELD`](crate::engine::ORDER_KEY_FIELD) (= `"order_key"`), NOT the
/// producer's `rank` board name, so the engine's dedicated columnar order_key fast-field serves the
/// sort (the engine is UNCHANGED). The VALUE/ENCODING/TYPE are byte-identical (13.3 LexoRank). KN's
/// `db_row` spec uses `order_key` for exactly this reason.
pub fn issue_index_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    // The structured/columnar board/list/search facets, each at its frozen FieldType (13.3). The
    // full-text body (title/props/comment free-text) arrives via SearchProjection.text at emit time
    // (ISS-P17), NOT here — the spec is the columnar schema, the projection is the row.
    struct_fields.insert(FACET_STATE_CATEGORY.to_string(), FieldType::Select);
    struct_fields.insert(FACET_PRIORITY.to_string(), FieldType::Int);
    struct_fields.insert(FACET_ASSIGNEE.to_string(), FieldType::Principal);
    struct_fields.insert(FACET_TYPE_RANK.to_string(), FieldType::Int);
    struct_fields.insert(FACET_PROJECT_ID.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_CYCLE_ID.to_string(), FieldType::Relation);
    // The board rank — the LexoRank columnar fast-field for sort (13.3). Declared under Search's
    // ORDER_KEY_FIELD convention (= "order_key") so the engine's dedicated columnar order_key sort
    // serves it (the producer's domain name is `rank`; see the module-level reconciliation note).
    struct_fields.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);

    // An issue's reachability is decided by the issue object's own frozen ReBAC `view` permission
    // (with the `- confidential` set-difference) — there is no parent ACL object (UNLIKE git's
    // blob→repo), so acl_object_type == type_ == "issue". Made explicit so a future facet rename can
    // not silently drift the ACL anchor off the `issue` namespace.
    IndexSpec::new(ISSUE_SUBSYSTEM, ISSUE_TYPE, struct_fields)
        .with_acl_object_type(ISSUE_ACL_OBJECT_TYPE)
}

/// Every Issues index spec (the one `issue` type) — the set a Search indexer registers to consume the
/// real Issues corpus. Mirrors [`crate::kn_projection::kn_index_specs`] (the set
/// [`register_issue_index_specs`] proves Search ADMITS).
pub fn issue_index_specs() -> Vec<IndexSpec> {
    vec![issue_index_spec()]
}

/// **Register Issues' index spec WITH Search (the GATE).** Builds [`issue_index_specs`] and proves
/// Search **accepts** it by admitting it into a live
/// [`IncrementalIndexer`](crate::indexer::IncrementalIndexer)'s per-tenant facet union without a
/// schema mismatch (the only honest definition of "accepted" — Search is the authority that admits).
/// Returns the specs that were accepted. Mirrors KN's [`crate::kn_projection::register_kn_index_specs`]
/// and git's `register_git_index_specs`.
pub fn register_issue_index_specs() -> Vec<IndexSpec> {
    let specs = issue_index_specs();
    // Admit them into a real indexer's facet union (the build-time declare_indexable surface). A
    // facet-type collision or a malformed shape would panic at construction; it does not.
    let _accepted = crate::indexer::IncrementalIndexer::new(
        specs.clone(),
        std::sync::Arc::new(NullProjectFetcher),
        std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
    );
    specs
}

/// A do-nothing [`ProjectFetcher`](crate::indexer::ProjectFetcher) used ONLY to admit the Issues specs
/// into a live indexer for the registration GATE (the SPEC half + the projection BUILDER ship here;
/// the real owner-`project` fetch is the ISS-P17 emitter). It never fetches — registration does not
/// index. Mirrors KN's `NullProjectFetcher`.
struct NullProjectFetcher;

impl crate::indexer::ProjectFetcher for NullProjectFetcher {
    fn project(
        &self,
        _tenant: &myelin_tenancy::TenantId,
        _region: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<SearchProjection, crate::indexer::ProjectFetchError> {
        // The SPEC registration never fetches a projection (no emitter here — ISS-P17). This is the
        // registration GATE — Search admits the schema — not the index path.
        Err(crate::indexer::ProjectFetchError::Gone)
    }
}

/// **The index-time inputs of an issue's [`SearchProjection`] (the owner's `project(ref, viewer)` body
/// Search consumes, contract 5.6).** In production the Issues service builds these by walking the
/// issue (the title + props free-text + comment bodies for `body`, and the typed facet values from the
/// issue's typed core). Here the builder takes them directly — the same shape the live store swaps in
/// behind ([`issue_search_projection`] is the projection BUILDER the ISS-P17 emitter feeds).
#[derive(Clone, Debug, Default)]
pub struct IssueProjectionInput {
    /// The issue's searchable free-text body (title + props free-text + comment bodies) — the
    /// full-text inverted shape source (13.1). Analyzed multilingual (`lang`) at index time (§4.7).
    pub body: String,
    /// The FIXED state category (`backlog`/`started`/`done`/… — the [`FACET_STATE_CATEGORY`] facet).
    pub state_category: Option<String>,
    /// The issue priority (the ordered [`FACET_PRIORITY`] `Int` facet).
    pub priority: Option<i64>,
    /// The assignee pseudonym (the [`FACET_ASSIGNEE`] `Principal` facet — equality only, erasure-safe).
    pub assignee: Option<String>,
    /// The denormalised type rank (sub-task=0 … initiative=3 — the [`FACET_TYPE_RANK`] `Int` facet).
    pub type_rank: Option<i64>,
    /// The parent project ref (the [`FACET_PROJECT_ID`] `Relation` facet).
    pub project_id: Option<String>,
    /// The current cycle ref, if any (the [`FACET_CYCLE_ID`] `Relation` facet — nullable).
    pub cycle_id: Option<String>,
    /// The board rank — the LexoRank fractional index (13.3). Emitted under Search's
    /// [`ORDER_KEY_FIELD`](crate::engine::ORDER_KEY_FIELD) convention so the columnar sort serves it.
    pub rank: Option<OrderKey>,
    /// The analyzer-selection language tag (§3.1; the per-language analyzer chain reads it). `None`
    /// lets the indexer's pass-through detector set it.
    pub lang: Option<String>,
}

/// **Build an issue's [`SearchProjection`] from its index-time inputs (§4.1).** This is the owner's
/// `project(ref, viewer)` body Search consumes (contract 5.6) — NOT a DB read. It produces:
/// - the analyzable full-text `body` (title + props + comment free-text),
/// - the seven typed structured facets (only those present are stamped — the columnar shape only
///   carries present values; an absent nullable facet like `cycle_id` is simply not indexed),
/// - the LexoRank rank under Search's [`ORDER_KEY_FIELD`](crate::engine::ORDER_KEY_FIELD) convention so
///   the engine's dedicated columnar order_key fast-field sorts the board (§3.1).
///
/// In production the Issues service builds this from its typed core + per-issue rendered text; here it
/// builds it from the [`IssueProjectionInput`] directly — the projection is the row, the store is the
/// source. Every facet is typed to match [`issue_index_spec`]'s declaration (a type/value mismatch
/// would be rejected by the indexer; this builder only ever emits the declared types).
pub fn issue_search_projection(input: &IssueProjectionInput) -> SearchProjection {
    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();

    if let Some(sc) = &input.state_category {
        fields.insert(
            FACET_STATE_CATEGORY.to_string(),
            FieldValue::Select(sc.clone()),
        );
    }
    if let Some(p) = input.priority {
        fields.insert(FACET_PRIORITY.to_string(), FieldValue::Int(p));
    }
    if let Some(a) = &input.assignee {
        fields.insert(FACET_ASSIGNEE.to_string(), FieldValue::Principal(a.clone()));
    }
    if let Some(tr) = input.type_rank {
        fields.insert(FACET_TYPE_RANK.to_string(), FieldValue::Int(tr));
    }
    if let Some(pid) = &input.project_id {
        fields.insert(
            FACET_PROJECT_ID.to_string(),
            FieldValue::Relation(pid.clone()),
        );
    }
    if let Some(cid) = &input.cycle_id {
        fields.insert(
            FACET_CYCLE_ID.to_string(),
            FieldValue::Relation(cid.clone()),
        );
    }
    // The board rank → the order_key columnar fast-field convention (the documented `rank`→`order_key`
    // mapping; the LexoRank value/encoding is byte-identical, only the index-doc key is the convention).
    if let Some(rank) = &input.rank {
        fields.insert(
            ORDER_KEY_FIELD.to_string(),
            FieldValue::OrderKey(rank.clone()),
        );
    }

    SearchProjection {
        text: input.body.clone(),
        fields,
        lang: input.lang.clone(),
    }
}

/// **FLOOR (named) — the GIN-indexed JSONB facet scan → the measured projection-feeder promotion.**
/// A greppable zero-sized marker: the Issues board facets ([`issue_index_spec`]'s typed facets) are
/// served by the GIN-indexed JSONB facet scan (§4.6.1). The **measured projection-feeder promotion**
/// per hot facet (a facet filtered in > 5% of a collection's view executions over a rolling window) to
/// a generated/columnar index is the **M5 follow-on, SRCH-P27** (OQ-C) — the owner of the per-facet
/// frequency signal is Issues/KN, Search consumes it and decides promotion. The GIN scan serves
/// CORRECTLY meanwhile; promotion changes COST, never correctness.
#[derive(Clone, Copy, Debug)]
pub struct IssueGinScanProjectionFeederFloor;

impl IssueGinScanProjectionFeederFloor {
    /// The follow-on prompt that promotes a hot Issues facet from the GIN scan to a generated index.
    pub const PROMOTION_FOLLOW_ON: &'static str = "SRCH-P27";
    /// The sibling slice that wires the Issues Tier-3 board-escalation valve over these facets.
    pub const TIER3_VALVE_FOLLOW_ON: &'static str = "SRCH-P21";
    /// The Issues producer emitter that ships the live `project(ref)` feeding this builder.
    pub const ISSUES_EMITTER_FOLLOW_ON: &'static str = "ISS-P17";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::IncrementalIndexer;

    /// **The Issues spec is the consumed 6.3 shape.** Pins every facet + type + the acl_object_type. A
    /// rename of a Search `IndexSpec` field, or a drift in the facet set/types, breaks the registrant.
    #[test]
    fn issue_spec_is_the_consumed_6_3_shape() {
        let s = issue_index_spec();
        assert_eq!(s.subsystem, "issue");
        assert_eq!(s.type_, "issue");
        assert_eq!(
            s.acl_object_type, "issue",
            "an issue's reachability is its own ReBAC `view` (no parent ACL object)"
        );
        assert!(
            !s.semantic,
            "Issues is trigram-title + facet filter in v1, not vector-embedded"
        );
        // The seven structured board/list/search facets, each at its frozen FieldType (13.3).
        assert_eq!(s.struct_fields.len(), 7, "exactly the seven issue facets");
        assert_eq!(
            s.struct_fields.get(FACET_STATE_CATEGORY),
            Some(&FieldType::Select)
        );
        assert_eq!(s.struct_fields.get(FACET_PRIORITY), Some(&FieldType::Int));
        assert_eq!(
            s.struct_fields.get(FACET_ASSIGNEE),
            Some(&FieldType::Principal)
        );
        assert_eq!(s.struct_fields.get(FACET_TYPE_RANK), Some(&FieldType::Int));
        assert_eq!(
            s.struct_fields.get(FACET_PROJECT_ID),
            Some(&FieldType::Relation)
        );
        assert_eq!(
            s.struct_fields.get(FACET_CYCLE_ID),
            Some(&FieldType::Relation)
        );
        assert_eq!(
            s.struct_fields.get(ORDER_KEY_FIELD),
            Some(&FieldType::OrderKey),
            "the board rank is the order_key columnar fast-field for sort (13.3)"
        );
    }

    /// **The full-text body is NOT a structured facet.** `title` / props free-text / comment bodies
    /// arrive at emit time in `SearchProjection.text` (ISS-P17), so they must be absent from
    /// `struct_fields` (the schema is the columnar half, not the body).
    #[test]
    fn fulltext_body_is_not_a_struct_facet() {
        let s = issue_index_spec();
        for absent in ["title", "body", "props", "comment", "description"] {
            assert!(
                !s.struct_fields.contains_key(absent),
                "`{absent}` is full-text projection body, not a structured facet"
            );
        }
    }

    /// **The `rank`→`order_key` reconciliation is explicit (DOCUMENTED deviation).** The producer's
    /// board-domain rank facet name is `rank`; Search's index-doc declares it under the
    /// `ORDER_KEY_FIELD` (`order_key`) convention so the engine's dedicated columnar order_key sort
    /// serves it. The spec carries `order_key` (not `rank`); the value/encoding/type are byte-identical
    /// LexoRank. Pins the mapping so a future edit can not silently re-introduce a `rank`-named facet
    /// the engine would NOT sort on.
    #[test]
    fn issue_rank_facet_is_the_order_key_convention() {
        let s = issue_index_spec();
        assert_eq!(
            ISSUE_PRODUCER_RANK_FACET, "rank",
            "the producer's board-domain rank facet name is `rank` (myelin_issues::declares::FACET_RANK)"
        );
        assert_eq!(
            ORDER_KEY_FIELD, "order_key",
            "Search's index-doc order_key columnar-sort convention"
        );
        assert!(
            s.struct_fields.contains_key(ORDER_KEY_FIELD),
            "Search declares the rank under the order_key convention so the columnar sort serves it"
        );
        assert!(
            !s.struct_fields.contains_key(ISSUE_PRODUCER_RANK_FACET),
            "Search does NOT declare a `rank`-named facet — the engine sorts only on `order_key`"
        );
        assert_eq!(
            s.struct_fields.get(ORDER_KEY_FIELD),
            Some(&FieldType::OrderKey),
            "the order_key facet is byte-identical LexoRank (13.3), only the index-doc key is the convention"
        );
    }

    /// **Search ACCEPTS the Issues spec (the GATE).** Search admits it into a live indexer's per-tenant
    /// facet union without a schema mismatch — the accepted set is byte-equal to the declared set.
    #[test]
    fn registration_is_accepted_by_search() {
        let accepted = register_issue_index_specs();
        assert_eq!(
            accepted,
            issue_index_specs(),
            "Search accepts the declared Issues spec verbatim"
        );
        // And a live indexer over it opens (the facet union is consistent).
        let _ix = IncrementalIndexer::new(
            issue_index_specs(),
            std::sync::Arc::new(NullProjectFetcher),
            std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
        );
    }

    /// **The projection builder stamps exactly the present typed facets + the order_key rank.** Every
    /// facet is typed to match the spec; absent nullable facets (here `cycle_id`) are simply not
    /// indexed (the columnar shape only carries present values). The body is the searchable free-text.
    #[test]
    fn projection_builds_typed_facets_and_order_key() {
        let input = IssueProjectionInput {
            body: "scheduler deadlock at runtime".into(),
            state_category: Some("started".into()),
            priority: Some(2),
            assignee: Some("psn:alice".into()),
            type_rank: Some(1),
            project_id: Some("myelin://acme/issue/project/ENG".into()),
            cycle_id: None, // an absent nullable facet — not indexed as empty.
            rank: Some(OrderKey::bisect(None, None)),
            lang: Some("en".into()),
        };
        let p = issue_search_projection(&input);

        assert_eq!(p.text, "scheduler deadlock at runtime");
        assert_eq!(p.lang.as_deref(), Some("en"));

        assert_eq!(
            p.fields.get(FACET_STATE_CATEGORY),
            Some(&FieldValue::Select("started".into()))
        );
        assert_eq!(p.fields.get(FACET_PRIORITY), Some(&FieldValue::Int(2)));
        assert_eq!(
            p.fields.get(FACET_ASSIGNEE),
            Some(&FieldValue::Principal("psn:alice".into()))
        );
        assert_eq!(p.fields.get(FACET_TYPE_RANK), Some(&FieldValue::Int(1)));
        assert_eq!(
            p.fields.get(FACET_PROJECT_ID),
            Some(&FieldValue::Relation(
                "myelin://acme/issue/project/ENG".into()
            ))
        );
        // The absent nullable cycle_id is NOT indexed.
        assert!(
            !p.fields.contains_key(FACET_CYCLE_ID),
            "an absent nullable facet is not indexed as empty"
        );
        // The rank is stamped under the order_key convention (not `rank`).
        assert!(
            matches!(p.fields.get(ORDER_KEY_FIELD), Some(FieldValue::OrderKey(_))),
            "the rank is stamped under the order_key columnar-sort convention"
        );
        assert!(
            !p.fields.contains_key(ISSUE_PRODUCER_RANK_FACET),
            "the projection never stamps a `rank`-named facet"
        );

        // Every stamped facet's type matches the spec's declaration (the indexer would reject a
        // mismatch; this builder only ever emits the declared types).
        let spec = issue_index_spec();
        for (name, value) in &p.fields {
            assert_eq!(
                value.field_type(),
                *spec.struct_fields.get(name).expect("facet is declared"),
                "facet `{name}` value type matches its spec declaration"
            );
        }
    }

    /// **The floor markers name the follow-ons (SRCH-P27 promotion / SRCH-P21 valve / ISS-P17 emitter).**
    #[test]
    fn floor_markers_name_the_follow_ons() {
        assert_eq!(
            IssueGinScanProjectionFeederFloor::PROMOTION_FOLLOW_ON,
            "SRCH-P27"
        );
        assert_eq!(
            IssueGinScanProjectionFeederFloor::TIER3_VALVE_FOLLOW_ON,
            "SRCH-P21"
        );
        assert_eq!(
            IssueGinScanProjectionFeederFloor::ISSUES_EMITTER_FOLLOW_ON,
            "ISS-P17"
        );
    }
}
