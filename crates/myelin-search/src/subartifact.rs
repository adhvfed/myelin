//! # `subartifact` — Sub-artifact-granular + content-anchored projections (SRCH-P19 / P-262, M3)
//!
//! **Owning architecture doc:** `search-and-indexing.md` §4.1 tail / §4.9 ask (sub-artifact-granular
//! projections; Git line-ranges content-anchored — the searchable span re-derived from the owner's
//! resolve, never a stale raw line number; KN replay page-subtree at block granularity; Git replay
//! per-blob/ref), §3.1 (the `doc_id` may carry a frozen `#sub` of any frozen kind).
//! **Reconciliation:** `00-reconciliation-decisions.md` change #7 / X-4 / OQ-D (the unified `#sub`
//! grammar + content-anchoring). **Contracts:** 5.7 (the `#sub` kinds on real sub-anchors; Git
//! line-ranges content-anchored), 2.6 (sub-artifact-granular replay).
//!
//! ## What SRCH-P19 ships here — the sub-artifact GRAIN over the FROZEN `#sub` grammar
//!
//! The M3 producer prompts already land the heavy machinery this rides:
//! - the [`crate::indexer::IncrementalIndexer`] ALREADY keys a `#sub`-anchored `doc_id` sub-precisely
//!   and pins the ACL on the `#sub`-stripped parent (`acl_object`, §3.1) — see
//!   `indexer::acl_object_of` / `sub_anchor_of`. SRCH-P19 does NOT re-build that; it CONFIRMS it and
//!   adds the GRAIN classifier + the content-anchored re-derive on top.
//! - the unified `#sub` grammar is FROZEN in `myelin_refs` ([`myelin_refs::sub_kind`] / [`Sub`] /
//!   [`SubKind`], contract 5.7). Search keys the grain off THAT — it does NOT re-implement the grammar
//!   (EI-01 §7: never a second parser). This module is the ONE place Search classifies a `#sub` into
//!   its index-time grain.
//! - the content-anchoring LADDER (`exact/rebased/partial/content_gone`) is the OWNER's `project`
//!   resolver (`myelin_refs_service::resolve_line_range`, REF-P17 / GIT producer). Search does NOT run
//!   it — the owner's `project(ref, viewer)` (5.6) RETURNS the re-derived span text, and Search indexes
//!   what the owner returned (the no-cross-db floor). [`ContentAnchoredSpan`] is the SHAPE of that
//!   owner-resolved span the projection builder consumes.
//!
//! ### The three sub-artifact grains (the prompt's enumerated set)
//!
//! 1. **Doc blocks (`b<id>` / `h<id>`)** — a Knowledge page's block/heading subtree at block
//!    granularity (contract 2.6: "KN page-subtree at block granularity"). [`block_subdoc_projection`]
//!    projects ONE block's analyzable text (the owner resolved the `#b<id>`/`#h<id>` sub-anchor and
//!    returned that block's content). The doc_id keeps the `#b<id>` so a hit resolves at block grain;
//!    the ACL pins on the parent page.
//! 2. **KN rows / fields (`row-` / `field-`)** — a row of an in-doc database, or a single field within
//!    a row. [`db_row_subdoc_projection`] / [`db_field_subdoc_projection`] project the row's / field's
//!    structured + full-text content. The doc_id keeps the `#row-`/`#field-`; the ACL pins on the
//!    host page.
//! 3. **Git line-ranges (`L<a>-L<b>`, CONTENT-ANCHORED)** — a span of a Git blob. The searchable span
//!    is **re-derived from the owner's resolve** ([`ContentAnchoredSpan`]), never a stale raw line
//!    number (§3.1/§4.9). [`line_range_subdoc_projection`] consumes the owner-resolved span (its
//!    re-derived `[start, end]` + the current span text) and projects it through the SAME code
//!    tokenizer + trigram index as a whole blob (SRCH-P18). On a force-push a scoped reindex re-drives
//!    the owner's `project`, which re-derives the span — so the index NEVER holds the stale line number.
//!
//! ## FLOOR named (SRCH-P19 DoD)
//! - **Sub-artifact granularity is the full SHAPE at M3.** The doc-block (`b`/`h`), KN row/field
//!   (`row-`/`field-`), and Git line-range (`L<a>-L<b>`) grains are exercised here. The OTHER `#sub`
//!   kinds in the frozen vocabulary — `comment-`/`thread-`/`message-` (Chat/Issues/Git review) and
//!   `check-`/`step-` (CI) — arrive M4 as those producers light up (SRCH-P20..P23). The GRAMMAR is the
//!   one frozen vocabulary NOW; the M3 producer corpus is Git + KN, so only Git + KN sub-anchors are
//!   index-exercised at M3. Named so the grammar is not mistaken for fully-exercised across all five
//!   producers. Greppable as [`M4ProducerSubAnchorFloor`].
//! - **The content-anchoring algorithm is the OWNER's** (`resolve_line_range`, REF-P17 / GIT-P25). Here
//!   Search consumes the owner-resolved span SHAPE ([`ContentAnchoredSpan`]) — the no-cross-db floor:
//!   Search re-indexes what the owner re-derived, never a raw line number it stored itself. The real
//!   Git line-range EMITTER (the receive-pack hook that re-emits the per-blob `*.snapshot` carrying the
//!   re-derived span) is GIT-P25 / P-287; here Search ships the projection BUILDER + the re-derive
//!   shape the emitter feeds, and the integration test drives the genuine builder over the owner's
//!   resolve.

use std::collections::BTreeMap;

use myelin_content::Block;
use myelin_query::{FieldType, FieldValue, OrderKey};
use myelin_refs::{sub_kind, Sub, SubKind};
use myelin_tenancy::ArtifactRef;

use crate::git_code_projection::{
    git_blob_search_projection, GitBlobProjectionInput, FACET_BLOB_OID, FACET_LANGUAGE, FACET_PATH,
};
use crate::indexer::SearchProjection;
use crate::kn_projection::page_search_projection;

/// The structured facet for a Git line-range's RE-DERIVED start line (the owner's resolve, never the
/// minted raw number). Carried so a hit can render "lines 42–88 (moved)" from the CURRENT resolve.
pub const FACET_LINE_START: &str = "line_start";
/// The structured facet for a Git line-range's RE-DERIVED end line (the owner's resolve).
pub const FACET_LINE_END: &str = "line_end";
/// The structured facet for the content-anchoring RESOLUTION STATE the owner reported
/// (`exact`/`rebased`/`partial`) — the §3.5 ladder state the searchable span was re-derived under, so a
/// hit can carry the `moved`/`outdated` flag at render time WITHOUT the index storing a raw line number.
pub const FACET_ANCHOR_STATE: &str = "anchor_state";

/// **The index-time GRAIN of a sub-artifact `doc_id`** — the classification of a `#sub` ref into the
/// granularity Search keys it at (SRCH-P19). Built from the FROZEN [`myelin_refs::sub_kind`] grammar
/// (contract 5.7) — Search does NOT re-implement the grammar (EI-01 §7). A bare root (no `#sub`) is
/// [`SubGrain::Root`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubGrain {
    /// A bare root artifact (no `#sub`) — indexed whole (the SRCH-P17/P18 root-doc grain).
    Root,
    /// A Knowledge block (`b<id>`) or heading (`h<id>`) — block-granular page subtree (contract 2.6).
    Block(String),
    /// A heading anchor (`h<id>`) — the heading-subtree grain (block-granular).
    Heading(String),
    /// A Knowledge db row (`row-<id>`) — the row grain.
    Row(String),
    /// A field within a row / issue (`field-<id>`) — the field grain.
    Field(String),
    /// A Git CONTENT-ANCHORED line range (`L<a>-L<b>`) — the searchable span re-derived from the
    /// owner's resolve, NEVER a stored raw line number (§3.1/§4.9). Carries the MINTED endpoints (the
    /// grammar's parsed `[start, end]`) only as the request the owner re-derives against — the indexed
    /// span comes from the owner's [`ContentAnchoredSpan`], not these numbers.
    LineRange {
        /// The minted 1-based start (the grammar endpoint — the resolve REQUEST, not the indexed line).
        minted_start: u64,
        /// The minted 1-based end (the grammar endpoint — the resolve request).
        minted_end: u64,
    },
    /// A `#sub` kind NOT exercised at M3 (`comment-`/`thread-`/`message-`/`check-`/`step-`) — the M4
    /// producer follow-on (SRCH-P20..P23). The grammar is frozen NOW; the GRAIN classifier admits it
    /// (carrying the [`SubKind`]) so the M4 producer wiring is a builder add, not a grammar change.
    M4Producer(SubKind),
}

impl SubGrain {
    /// **Classify a `#sub` ref into its index-time grain (SRCH-P19) — through the FROZEN
    /// [`myelin_refs::sub_kind`] grammar (contract 5.7).** A bare root is [`SubGrain::Root`]; a
    /// sub-anchored ref is classified by its frozen [`Sub`] kind. Search does NOT re-implement the
    /// grammar — it READS the kind off `myelin_refs` (EI-01 §7: one parser).
    pub fn classify(ref_: &ArtifactRef) -> SubGrain {
        match sub_kind(ref_) {
            None => SubGrain::Root,
            Some(Sub::Block(id)) => SubGrain::Block(id),
            Some(Sub::Heading(id)) => SubGrain::Heading(id),
            Some(Sub::Row(id)) => SubGrain::Row(id),
            Some(Sub::Field(id)) => SubGrain::Field(id),
            Some(Sub::LineRange { start, end }) => SubGrain::LineRange {
                minted_start: start,
                minted_end: end,
            },
            // The frozen vocabulary's other kinds: the M4 producer corpus (Chat/Issues/CI). The grammar
            // is frozen now; the grain classifier carries the kind so M4 is a builder add (the floor).
            Some(other) => SubGrain::M4Producer(other.kind()),
        }
    }

    /// The frozen [`SubKind`] this grain corresponds to (`None` for a bare [`SubGrain::Root`]). Lets a
    /// caller key the projection builder off the SAME frozen discriminator the grammar owns.
    pub fn sub_kind(&self) -> Option<SubKind> {
        match self {
            SubGrain::Root => None,
            SubGrain::Block(_) => Some(SubKind::Block),
            SubGrain::Heading(_) => Some(SubKind::Heading),
            SubGrain::Row(_) => Some(SubKind::Row),
            SubGrain::Field(_) => Some(SubKind::Field),
            SubGrain::LineRange { .. } => Some(SubKind::LineRange),
            SubGrain::M4Producer(k) => Some(*k),
        }
    }

    /// Whether this grain is exercised by an M3 producer (Git + KN). The M3 corpus is doc blocks
    /// (`b`/`h`), KN rows/fields (`row-`/`field-`), and Git line-ranges (`L<a>-L<b>`); the other frozen
    /// kinds are the named M4 follow-on (so the grammar is not mistaken for fully-exercised at M3).
    pub fn is_m3_exercised(&self) -> bool {
        !matches!(self, SubGrain::M4Producer(_))
    }
}

/// **The owner-RESOLVED content-anchored span (the SHAPE Search consumes — §3.1/§4.9).** A Git
/// `L<a>-L<b>` is content-anchored: the owner's `project(ref, viewer)` runs the §3.5 ladder
/// (`resolve_line_range`, REF-P17 / GIT producer) against the CURRENT blob and RETURNS this — the
/// re-derived `[start, end]` + the current span text + the resolution state. Search indexes WHAT THE
/// OWNER RETURNED (the no-cross-db floor); it never stores a raw line number or runs the ladder itself.
/// On a force-push a scoped reindex re-drives `project`, which re-derives this — so the index converges
/// on the new span, never a stale line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentAnchoredSpan {
    /// The blob's path (the structured facet + the searchable path tokens — same as a whole blob).
    pub path: String,
    /// The detected source-language tag (`rust`/`python`/…). Empty ⇒ unset.
    pub language: String,
    /// The content-addressed CURRENT blob oid (the blob the owner re-derived against).
    pub blob_oid: String,
    /// The RE-DERIVED 1-based start line (the owner's resolve — `exact`/`rebased`/`partial`), NEVER the
    /// minted raw number. On a force-push this is the new position the fingerprinted lines moved to.
    pub resolved_start: u64,
    /// The RE-DERIVED 1-based end line (the owner's resolve).
    pub resolved_end: u64,
    /// The CURRENT span text (the re-derived lines' raw code, X-2) the owner returned — tokenized +
    /// trigram-indexed like a whole blob. NEVER the minted text (the content moved/changed).
    pub span_text: String,
    /// The §3.5 ladder STATE the owner reported the span under (`exact`/`rebased`/`partial`). Carried as
    /// a facet so a hit renders the `moved`/`outdated` flag from the CURRENT resolve, never a stored line.
    pub anchor_state: AnchorState,
}

/// **The §3.5 content-anchoring ladder STATE the owner reported (the searchable arms).** `Exact`/
/// `Rebased`/`Partial` are the three states that re-derive a searchable span (a `ContentGone`/tombstone
/// span has no text to index — the indexer REMOVES the doc, the `*.erased`/gone path). Mirrors
/// `myelin_refs_service::LineRangeState`'s live arms (the owner's vocabulary) — Search carries the LABEL
/// the owner resolved under, not the algorithm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorState {
    /// The blob oid matched → the exact minted range (LIVE). The span is the minted lines, unmoved.
    Exact,
    /// The fingerprinted lines moved to a shifted position (3-way context) (MOVED). The span is the
    /// shifted range; a hit renders the `moved` flag.
    Rebased,
    /// Some anchored lines survive, some are gone (OUTDATED). The span is the surviving sub-range; a hit
    /// renders the `outdated` flag.
    Partial,
}

impl AnchorState {
    /// The facet label the [`FACET_ANCHOR_STATE`] carries (so a hit renders the flag from the CURRENT
    /// resolve). PII-free, stable token.
    pub const fn label(self) -> &'static str {
        match self {
            AnchorState::Exact => "exact",
            AnchorState::Rebased => "rebased",
            AnchorState::Partial => "partial",
        }
    }
}

/// **Build a Knowledge BLOCK sub-doc projection (`b<id>`/`h<id>`, block granularity — contract 2.6).**
/// The owner resolved the `#b<id>`/`#h<id>` sub-anchor and returned the block's content (here as the
/// resolved [`Block`] subtree); Search projects ONE block's analyzable text + its structured inline-node
/// facets (the SAME [`page_search_projection`] walk, at block grain). The doc_id keeps the `#b<id>` so a
/// hit resolves at block grain; the ACL pins on the parent page (the indexer's `acl_object`). This is
/// the page-subtree-at-block-granularity replay the §4.9 ask names — a single block, not the whole page.
pub fn block_subdoc_projection(block: &Block, lang: Option<&str>) -> SearchProjection {
    // A block sub-doc is the SAME projection shape as a page, scoped to one block's subtree (the owner
    // resolved the sub-anchor; Search projects that block). Reuse the ONE page projector at block grain
    // (no second walk — EI-01 §7); the structured inline-node facets (mention/artifact_ref/embed) are
    // extracted from THIS block's inline nodes.
    page_search_projection(std::slice::from_ref(block), lang)
}

/// **Build a Knowledge DB ROW sub-doc projection (`row-<id>`, the row grain).** The owner resolved the
/// `#row-<id>` sub-anchor and returned the row's typed cell values (its custom DB fields) + its rendered
/// full-text. Search projects the row's structured facets (the GIN-scan JSONB facets, §4.6.1) keyed by
/// field name + the row's full-text body. The doc_id keeps the `#row-<id>`; the ACL pins on the host
/// page. `rollup`/`formula` are NEVER stored (KN-3) — the owner's projection already excludes them.
pub fn db_row_subdoc_projection(
    fields: &BTreeMap<String, FieldValue>,
    full_text: &str,
    order_key: Option<OrderKey>,
) -> SearchProjection {
    let mut out: BTreeMap<String, FieldValue> = fields.clone();
    if let Some(ok) = order_key {
        out.insert(
            crate::engine::ORDER_KEY_FIELD.to_string(),
            FieldValue::OrderKey(ok),
        );
    }
    SearchProjection {
        text: full_text.to_string(),
        fields: out,
        lang: None,
    }
}

/// **Build a Knowledge DB FIELD sub-doc projection (`field-<id>`, the field grain).** The owner
/// resolved the `#field-<id>` sub-anchor and returned ONE field's typed value + its rendered text.
/// Search projects exactly that one facet + its full-text. The doc_id keeps the `#field-<id>`; the ACL
/// pins on the host page. This is the finest KN grain — "find the issue/row whose `priority` field is
/// `P0`" resolves to the field sub-doc, not the whole row.
pub fn db_field_subdoc_projection(
    field_name: &str,
    value: FieldValue,
    rendered_text: &str,
) -> SearchProjection {
    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
    fields.insert(field_name.to_string(), value);
    SearchProjection {
        text: rendered_text.to_string(),
        fields,
        lang: None,
    }
}

/// **Build a Git CONTENT-ANCHORED line-range sub-doc projection (`L<a>-L<b>`, §3.1/§4.9).** The owner's
/// `project(ref, viewer)` ran the §3.5 ladder against the CURRENT blob and returned a
/// [`ContentAnchoredSpan`] (the RE-DERIVED `[start, end]` + the current span text + the resolution
/// state). Search projects the span text through the SAME code tokenizer + trigram index as a whole
/// blob ([`git_blob_search_projection`], SRCH-P18) — so a symbol/path/literal/substring query over the
/// span works identically — and stamps the RE-DERIVED endpoints + the anchor state as structured facets
/// (so a hit renders "lines 60–106 (moved)" from the CURRENT resolve).
///
/// **The content-anchoring invariant (the SRCH-P19 gate):** the index NEVER stores a raw line number.
/// The endpoints stamped here are the owner's RE-DERIVED resolve, not the minted ones; the span text is
/// the CURRENT lines, not the minted ones. On a force-push a scoped reindex re-drives the owner's
/// `project`, which re-derives this span — so the rebuilt projection carries the NEW span, never a stale
/// line. (The minted endpoints in the `#sub` grammar are the resolve REQUEST, lowered by the owner.)
pub fn line_range_subdoc_projection(span: &ContentAnchoredSpan) -> SearchProjection {
    // Reuse the ONE Git code projector (SRCH-P18) over the RE-DERIVED span text — the span is tokenized
    // + trigram-indexed exactly like a whole blob, so code search v1 works at line-range grain. The
    // owner already re-derived the span (the no-cross-db floor): Search indexes what `project` returned.
    let mut projection = git_blob_search_projection(&GitBlobProjectionInput {
        path: span.path.clone(),
        language: span.language.clone(),
        text: span.span_text.clone(),
        literals: Vec::new(),
        commit_message: String::new(),
        blob_oid: span.blob_oid.clone(),
    });

    // Stamp the RE-DERIVED endpoints + the anchor state as structured facets — NEVER the minted raw
    // numbers. A hit renders the span position + the moved/outdated flag from the CURRENT resolve. The
    // blob projector already stamped path/language/blob_oid; ADD the line-range facets.
    projection.fields.insert(
        FACET_LINE_START.to_string(),
        FieldValue::Text(span.resolved_start.to_string()),
    );
    projection.fields.insert(
        FACET_LINE_END.to_string(),
        FieldValue::Text(span.resolved_end.to_string()),
    );
    projection.fields.insert(
        FACET_ANCHOR_STATE.to_string(),
        FieldValue::Text(span.anchor_state.label().to_string()),
    );
    projection
}

/// **The git line-range sub-doc's structured facets — the line-range grain's columnar shape (§4.9).**
/// A `L<a>-L<b>` sub-doc carries the whole-blob facets (path/language/blob_oid) PLUS the re-derived
/// line-range facets (`line_start`/`line_end`/`anchor_state`). A Search indexer that consumes Git
/// line-range sub-docs declares THIS facet union (so the re-derived endpoints + state are typed
/// columnar fields). Each is [`FieldType::Text`] (the endpoints are stored as their decimal string so a
/// query can range/equality-filter; the engine types them columnar-byte-identically, 13.3).
pub fn line_range_subdoc_facets() -> BTreeMap<String, FieldType> {
    let mut f: BTreeMap<String, FieldType> = BTreeMap::new();
    f.insert(FACET_PATH.to_string(), FieldType::Text);
    f.insert(FACET_LANGUAGE.to_string(), FieldType::Text);
    f.insert(FACET_BLOB_OID.to_string(), FieldType::Text);
    f.insert(FACET_LINE_START.to_string(), FieldType::Text);
    f.insert(FACET_LINE_END.to_string(), FieldType::Text);
    f.insert(FACET_ANCHOR_STATE.to_string(), FieldType::Text);
    f
}

/// **The named SRCH-P19 FLOOR (the gap-report entry, recorded in code per the prior-prompt convention —
/// e.g. [`crate::git_code_projection::ScipLsifFindUsagesFloor`]).** Sub-artifact granularity is the
/// full SHAPE at M3; what is NAMED, not index-exercised at M3:
///
/// - **The `comment-`/`thread-`/`message-` `#sub` kinds** (Chat/Issues/Git-review sub-anchors) and the
///   **`check-`/`step-` kinds** (CI) are the M4 producer corpus (SRCH-P20..P23). The unified `#sub`
///   grammar is the ONE frozen vocabulary NOW ([`myelin_refs::SubKind`]); [`SubGrain::classify`] admits
///   every kind, but only Git + KN sub-anchors are index-EXERCISED at M3 (the M3 producer corpus). The
///   M4 wiring is a projection-BUILDER add (a `comment_subdoc_projection`, a `step_subdoc_projection`),
///   never a grammar change. Named so the grammar is not mistaken for fully-exercised across all five
///   producers.
/// - **The content-anchoring ALGORITHM is the owner's** (`myelin_refs_service::resolve_line_range`,
///   REF-P17 / GIT-P25). Search consumes the owner-resolved [`ContentAnchoredSpan`] SHAPE — the
///   no-cross-db floor: Search re-indexes what the owner re-derived, never a raw line number. The real
///   Git line-range EMITTER (the receive-pack hook re-emitting the `*.snapshot` with the re-derived
///   span) is GIT-P25 / P-287.
///
/// A doc-only zero-sized marker so the floor is greppable + linkable in code.
#[derive(Debug, Clone, Copy)]
pub struct M4ProducerSubAnchorFloor;

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::{parse_inline, HeadingLevel, InlineNode};
    use myelin_events::ArtifactRef as EvArtifactRef;

    fn ref_(s: &str) -> ArtifactRef {
        ArtifactRef(s.to_string())
    }

    /// **The grain classifier reads the FROZEN `#sub` grammar — doc blocks, KN rows/fields, Git
    /// line-ranges resolve at the right grain (the prompt's core GATE).** Each kind is classified
    /// through `myelin_refs::sub_kind` (one parser), never a Search-local re-parse.
    #[test]
    fn classifies_each_m3_sub_grain_through_the_frozen_grammar() {
        // A bare root → Root (whole-doc grain).
        assert_eq!(
            SubGrain::classify(&ref_("myelin://acme/knowledge/page/42")),
            SubGrain::Root
        );
        // A doc block (b<id>) → block granularity.
        assert_eq!(
            SubGrain::classify(&ref_("myelin://acme/knowledge/page/42#b9")),
            SubGrain::Block("9".into())
        );
        // A heading (h<id>) → heading-subtree grain.
        assert_eq!(
            SubGrain::classify(&ref_("myelin://acme/knowledge/page/42#hintro")),
            SubGrain::Heading("intro".into())
        );
        // A KN db row (row-<id>) → the row grain.
        assert_eq!(
            SubGrain::classify(&ref_("myelin://acme/knowledge/db_row/tasks:r7#row-r7")),
            SubGrain::Row("r7".into())
        );
        // A field (field-<id>) → the field grain.
        assert_eq!(
            SubGrain::classify(&ref_(
                "myelin://acme/knowledge/db_row/tasks:r7#field-priority"
            )),
            SubGrain::Field("priority".into())
        );
        // A Git CONTENT-ANCHORED line range (L<a>-L<b>) → carries the MINTED endpoints (the resolve
        // request), NOT a stored indexed line.
        assert_eq!(
            SubGrain::classify(&ref_("myelin://acme/git/blob/repo:main:src/x.rs#L42-L88")),
            SubGrain::LineRange {
                minted_start: 42,
                minted_end: 88
            }
        );
    }

    /// **Every M3 grain reports its frozen `SubKind` + is M3-exercised; the M4 kinds are admitted but
    /// named NOT-M3 (the floor).** The grammar is the one frozen vocabulary now; the M4 producer corpus
    /// (Chat/Issues/CI sub-anchors) is the named follow-on.
    #[test]
    fn m3_grains_are_exercised_and_m4_kinds_are_named_floor() {
        for (r, kind) in [
            ("myelin://acme/knowledge/page/1#b2", SubKind::Block),
            ("myelin://acme/knowledge/page/1#hx", SubKind::Heading),
            ("myelin://acme/knowledge/db_row/d:r#row-r", SubKind::Row),
            ("myelin://acme/knowledge/db_row/d:r#field-f", SubKind::Field),
            (
                "myelin://acme/git/blob/repo:main:x.rs#L1-L9",
                SubKind::LineRange,
            ),
        ] {
            let g = SubGrain::classify(&ref_(r));
            assert_eq!(g.sub_kind(), Some(kind), "{r} reports its frozen SubKind");
            assert!(
                g.is_m3_exercised(),
                "{r} is an M3-exercised grain (Git + KN)"
            );
        }

        // A Chat message / CI step sub-anchor: the grammar admits it (frozen vocabulary) but it is the
        // NAMED M4 producer follow-on — classify carries the kind, not index-exercised at M3.
        let chat = SubGrain::classify(&ref_("myelin://acme/chat/channel/c#message-m1"));
        assert_eq!(chat, SubGrain::M4Producer(SubKind::Message));
        assert!(
            !chat.is_m3_exercised(),
            "a Chat message sub-anchor is the M4 floor (named)"
        );
        let ci = SubGrain::classify(&ref_("myelin://acme/ci/run/r#step-3"));
        assert_eq!(ci, SubGrain::M4Producer(SubKind::Step));
        assert!(
            !ci.is_m3_exercised(),
            "a CI step sub-anchor is the M4 floor (named)"
        );
    }

    /// **A doc-block sub-doc projects ONE block at block granularity (contract 2.6) — its text + its
    /// structured inline-node facets, not the whole page.** The owner resolved the `#b<id>`; Search
    /// projects that block.
    #[test]
    fn block_subdoc_projects_one_block_at_block_grain() {
        let referenced = EvArtifactRef("myelin://acme/issues/issue/ENG-7".into());
        let block = Block::Paragraph {
            inline: parse_inline(
                &format!("the deadlock fix references {}", myelin_content::OBJ),
                &[InlineNode::ArtifactRefNode(referenced.clone())],
            ),
        };
        let p = block_subdoc_projection(&block, Some("en"));
        assert!(
            p.text.contains("deadlock fix"),
            "the block's prose is the searchable body"
        );
        assert_eq!(p.lang.as_deref(), Some("en"));
        // The structured inline-node facet is extracted at block grain (the node-array walk).
        assert_eq!(
            p.fields.get(crate::kn_projection::FACET_ARTIFACT_REF),
            Some(&FieldValue::Relation(referenced.0.clone()))
        );
    }

    /// **A heading sub-doc (`h<id>`) projects the heading's content at block granularity.**
    #[test]
    fn heading_subdoc_projects_at_block_grain() {
        let block = Block::Heading {
            level: HeadingLevel::new(2).unwrap(),
            inline: parse_inline("Scheduler internals", &[]),
        };
        let p = block_subdoc_projection(&block, Some("en"));
        assert!(p.text.contains("Scheduler internals"));
    }

    /// **A KN db ROW sub-doc projects the row's typed facets + the order_key + its full-text (the row
    /// grain).** `rollup`/`formula` are excluded by the owner's projection (KN-3) — Search indexes what
    /// the owner returned.
    #[test]
    fn db_row_subdoc_projects_the_row_grain() {
        let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
        fields.insert("priority".into(), FieldValue::Select("P0".into()));
        fields.insert(
            "owner".into(),
            FieldValue::Principal("u-1-pseudonym".into()),
        );
        let ok = OrderKey::parse("hzzzzz").expect("a base-62 LexoRank key");
        let p = db_row_subdoc_projection(&fields, "the row about a P0 incident", Some(ok.clone()));
        assert_eq!(
            p.fields.get("priority"),
            Some(&FieldValue::Select("P0".into()))
        );
        assert!(p.fields.contains_key("owner"));
        assert_eq!(
            p.fields.get(crate::engine::ORDER_KEY_FIELD),
            Some(&FieldValue::OrderKey(ok)),
            "the columnar sort key is carried (13.3)"
        );
        assert!(
            p.text.contains("P0 incident"),
            "the row's full-text is searchable"
        );
    }

    /// **A KN db FIELD sub-doc projects exactly one field + its rendered text (the field grain — the
    /// finest KN sub-grain).**
    #[test]
    fn db_field_subdoc_projects_the_field_grain() {
        let p =
            db_field_subdoc_projection("priority", FieldValue::Select("P0".into()), "priority: P0");
        assert_eq!(p.fields.len(), 1, "exactly the one resolved field");
        assert_eq!(
            p.fields.get("priority"),
            Some(&FieldValue::Select("P0".into()))
        );
        assert!(p.text.contains("P0"));
    }

    fn exact_span() -> ContentAnchoredSpan {
        ContentAnchoredSpan {
            path: "src/scheduler/deadlock.rs".into(),
            language: "rust".into(),
            blob_oid: "oid-v1".into(),
            resolved_start: 42,
            resolved_end: 45,
            span_text: "fn detectDeadlock(graph: &WaitForGraph) -> bool {\n    \
                        graph.has_cycle()\n}"
                .into(),
            anchor_state: AnchorState::Exact,
        }
    }

    /// **A Git line-range sub-doc projects the RE-DERIVED span through the code tokenizer + trigram
    /// index (SRCH-P18) — symbol/path/substring search works at line-range grain — and stamps the
    /// re-derived endpoints + anchor state (NEVER a raw line number).**
    #[test]
    fn line_range_subdoc_projects_the_resolved_span_content_anchored() {
        let p = line_range_subdoc_projection(&exact_span());
        let toks: std::collections::BTreeSet<&str> = p.text.split(' ').collect();
        // The span is code-tokenized like a whole blob (camel split + whole identifier + operator).
        assert!(toks.contains("detect"), "the span is code-tokenized");
        assert!(toks.contains("deadlock"));
        assert!(
            toks.contains("detectdeadlock"),
            "whole identifier kept (exact-identifier hit)"
        );
        assert!(toks.contains("->"), "the operator survives at span grain");
        assert_eq!(p.lang.as_deref(), Some("code"));
        // The RE-DERIVED endpoints are stamped as facets — the owner's resolve, NOT a raw line number.
        assert_eq!(
            p.fields.get(FACET_LINE_START),
            Some(&FieldValue::Text("42".into()))
        );
        assert_eq!(
            p.fields.get(FACET_LINE_END),
            Some(&FieldValue::Text("45".into()))
        );
        assert_eq!(
            p.fields.get(FACET_ANCHOR_STATE),
            Some(&FieldValue::Text("exact".into()))
        );
        // The whole-blob facets are present too (path/language/blob_oid).
        assert_eq!(
            p.fields.get(FACET_PATH),
            Some(&FieldValue::Text("src/scheduler/deadlock.rs".into()))
        );
    }

    /// **Content-anchoring re-derive: a force-push that REBASES the span (the fingerprinted lines move)
    /// re-derives the searchable projection at the NEW position — the index never holds the stale line
    /// number.** This models the owner's resolve returning a shifted `[start, end]` (the §3.5 `rebased`
    /// state) + the current span text; Search indexes the re-derived span, not the minted one.
    #[test]
    fn force_push_rebase_re_derives_the_span_never_a_stale_line() {
        // BEFORE: the span anchored at lines 42–45 (the minted position).
        let before = line_range_subdoc_projection(&exact_span());
        assert_eq!(
            before.fields.get(FACET_LINE_START),
            Some(&FieldValue::Text("42".into()))
        );

        // A force-push moved the fingerprinted block to lines 60–63 (the owner's `resolve_line_range`
        // returned `Rebased { new_start: 60, .. }`). The owner re-derived the span; Search re-indexes it.
        let after_span = ContentAnchoredSpan {
            blob_oid: "oid-v2-after-force-push".into(),
            resolved_start: 60,
            resolved_end: 63,
            anchor_state: AnchorState::Rebased,
            // The same fingerprinted content (it moved, not changed) — the current span text.
            ..exact_span()
        };
        let after = line_range_subdoc_projection(&after_span);

        // The RE-DERIVED endpoints are the NEW position — never the stale 42–45.
        assert_eq!(
            after.fields.get(FACET_LINE_START),
            Some(&FieldValue::Text("60".into())),
            "the span re-derives to the shifted position (content-anchored, not positional)"
        );
        assert_eq!(
            after.fields.get(FACET_LINE_END),
            Some(&FieldValue::Text("63".into()))
        );
        assert_eq!(
            after.fields.get(FACET_ANCHOR_STATE),
            Some(&FieldValue::Text("rebased".into())),
            "the hit renders the `moved` flag from the CURRENT resolve"
        );
        // The blob oid is the NEW blob (the force-push target) — the index tracks the current blob.
        assert_eq!(
            after.fields.get(FACET_BLOB_OID),
            Some(&FieldValue::Text("oid-v2-after-force-push".into()))
        );
        // The searchable code body is still correct (the same identifiers, re-derived).
        let toks: std::collections::BTreeSet<&str> = after.text.split(' ').collect();
        assert!(
            toks.contains("detectdeadlock"),
            "the span content is still searchable post-rebase"
        );
    }

    /// **A PARTIAL (outdated) span re-derives to the surviving sub-range, flagged `partial`.** Some
    /// anchored lines were edited away; the owner returns the surviving sub-range; Search indexes it.
    #[test]
    fn partial_span_re_derives_to_the_surviving_sub_range() {
        let partial = ContentAnchoredSpan {
            resolved_start: 42,
            resolved_end: 43,
            anchor_state: AnchorState::Partial,
            span_text: "fn detectDeadlock(graph: &WaitForGraph) -> bool {".into(),
            ..exact_span()
        };
        let p = line_range_subdoc_projection(&partial);
        assert_eq!(
            p.fields.get(FACET_LINE_END),
            Some(&FieldValue::Text("43".into()))
        );
        assert_eq!(
            p.fields.get(FACET_ANCHOR_STATE),
            Some(&FieldValue::Text("partial".into()))
        );
    }

    /// **The line-range facet union is the whole-blob facets + the re-derived line-range facets.** A
    /// Search indexer consuming Git line-range sub-docs declares THIS typed columnar shape.
    #[test]
    fn line_range_facet_union_is_blob_plus_re_derived_line_facets() {
        let f = line_range_subdoc_facets();
        for facet in [
            FACET_PATH,
            FACET_LANGUAGE,
            FACET_BLOB_OID,
            FACET_LINE_START,
            FACET_LINE_END,
            FACET_ANCHOR_STATE,
        ] {
            assert_eq!(
                f.get(facet),
                Some(&FieldType::Text),
                "`{facet}` is a typed columnar facet"
            );
        }
        assert_eq!(
            f.len(),
            6,
            "exactly the blob facets + the three re-derived line-range facets"
        );
    }

    /// The named floor marker is constructible (the greppable gap-report entry).
    #[test]
    fn the_named_floor_is_constructible() {
        let _floor = M4ProducerSubAnchorFloor;
    }
}
