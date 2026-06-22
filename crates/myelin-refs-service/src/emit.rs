//! The **edge-extraction emit seam** (REF-P8 / P-157; contract 5.4 EMIT side — the producer half).
//!
//! **Owning architecture doc:** `reference-graph.md` §4.1 (edge extraction → emit — CONFIRMED
//! unchanged, BUS-2): edges are born from two producers, both via the **outbox only** (no standalone
//! edge-write API). This module is producer #1: **content-node extraction (reference edges).** The
//! three structured inline nodes of `myelin-content` ([`InlineNode::Mention`] /
//! [`InlineNode::ArtifactRefNode`] / [`InlineNode::Embed`] — ADR-05; X-2/OQ-B freezes them
//! byte-identical across Chat/Issues/Knowledge) are the producers: the **same transaction** that
//! writes content emits one `refs.edge.created` per structured ref node
//! (`rel ∈ {mentions, links, embeds}`, `rel_class='reference'`). **External insight:**
//! `04-hard-problems.md` §2.4 (structured-node extraction, NOT a regex over prose — which is why
//! extraction is reliable, the guarantee). `01-process-and-quality-doctrine.md` §3 (prove-it).
//! **VISION §2** (Chat references any artifact). **Reconciliation:** `00-reconciliation-decisions.md`
//! X-2 (the three nodes byte-identical everywhere → the producer is uniform).
//!
//! ## What REF-P8 (P-157) ships — the EMIT side of 5.4
//! Given a `myelin-content` document (a slice of [`InlineNode`]) and the content event being written,
//! the seam:
//!
//! 1. **extracts** exactly one edge per structured ref node — [`extract_edges`] — by **matching the
//!    structured enum variant** (`Mention`/`ArtifactRefNode`/`Embed`), NEVER scanning prose. A node
//!    that carries no artifact reference produces no edge; N structured nodes → N edges.
//! 2. **emits** one `refs.edge.created` per extracted edge in the **SAME transaction** that writes the
//!    content — [`emit_edges`] — via [`myelin_events::OutboxTx::emit`]`(draft, cause =
//!    Some(content_event))`. Because the emit rides the ONE sanctioned outbox path (contract 2.2),
//!    there is **NO standalone edge-write API** (the `no-raw-publish` lint, P-019, is structurally
//!    satisfied: this module never calls a broker `publish`). The causality is correct-by-construction
//!    (P-S06): the correlation **root carries** from the content event, `causation_id = the content
//!    event`, and `depth = content_event.depth + 1` — the **loop-guard stamp** the AG-6 guard reads
//!    (the explicit assert/drill on the +1 stamp lands in **REF-P9 / P-158**; here it rides
//!    [`derive_envelope`] for free).
//!
//! ## Emit-iff-committed (REF-D7 producer-emit half, BUS-D4) — structural
//! [`emit_edges`] only ever calls [`OutboxTx::emit`], which **buffers** the row into the open
//! transaction; the row becomes durable **iff** the caller commits that transaction (the content
//! write + the edge events co-commit). If the content transaction aborts, the buffered edge rows are
//! dropped with the transaction — **no edge without its content, no content without its edge**. This
//! is the producer-emit half of REF-D7 (the ingest half — the builder consuming what this emits — is
//! [`crate::edge_builder`] / REF-P6); proven in `tests/cdc_ref_p8_emit_seam.rs` (the abort → 0 edges
//! chained test) and against the live dev-stack outbox in
//! `tests/integration_ref_p8_emit_seam.rs` (the `integration` feature: N nodes committed → N rows
//! visible to the relay; aborted → 0 rows).
//!
//! ## Floors named (VISION §3 / prompt DoD)
//! - **Producers are SYNTHETIC at M2.** This seam is exercised by a **test content writer**
//!   ([`emit_edges`] called from a fixture). The first REAL producers (Git diffs / Knowledge blocks /
//!   Chat messages writing actual content) land in **REF-P17 / REF-P18** (M3+). Named so the seam is
//!   not mistaken for live producer edges — the WIRING (the structured-node extraction + the
//!   same-tx outbox emit) is real; the CALLERS are synthetic until M3+.
//! - **The loop-guard causal-depth STAMP assert is REF-P9 / P-158.** The `depth = content.depth + 1`
//!   already rides [`derive_envelope`] correct-by-construction (the emit passes `cause =
//!   Some(content_event)`); REF-P9 adds the explicit depth-stamp drill + the depth-ceiling tripwire
//!   over THIS seam. Named so the depth correctness is not mistaken for the loop-guard gate.
//! - **Mutation floor (the extraction module).** The extraction decision logic — the per-variant
//!   `rel`/`rel_class` mapping, the principal→`member`-URN target derivation, the one-edge-per-node
//!   invariant (zero edges for an empty doc), the same-tx outbox emit shape — is the mutation-tested
//!   core. The floor is stated + met by the unit + chained + CDC tests below: every variant's mapping
//!   is asserted, the N→N count is asserted, and a mutant that drops a variant, mis-maps a `rel`, or
//!   emits outside the transaction is caught.

use myelin_content::InlineNode;
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType, OutboxTx,
    Result, Visibility,
};
use myelin_identity::Principal;

use crate::edge_builder::RelClass;

/// The frozen edge relation token a structured content node produces (§4.1; the `rel` column
/// vocabulary `{mentions, links, embeds}`). PII-free token. Each of the three structured inline nodes
/// maps to **exactly one** rel — the uniform producer (X-2: byte-identical across Chat/Issues/KN).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeRel {
    /// `mention(Principal)` → `mentions` (the @-mention reference edge).
    Mentions,
    /// `artifact_ref(ArtifactRef)` → `links` (the inline reference edge).
    Links,
    /// `embed(ArtifactRef)` → `embeds` (the inline embed/unfurl reference edge).
    Embeds,
}

impl EdgeRel {
    /// The frozen `rel` column token (`'mentions' | 'links' | 'embeds'`, §4.1).
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeRel::Mentions => "mentions",
            EdgeRel::Links => "links",
            EdgeRel::Embeds => "embeds",
        }
    }
}

/// **The shared edge-aggregate-key convention `edge:<source>-><target>` (EB-03 ordering anchor).**
/// Every `refs.edge.*` event for ONE logical edge shares this aggregate, so an edge's
/// create → remove → create sequence is per-aggregate ordered (gap-free, in commit order). Used by
/// the producer seam (this module) so an edge's events cannot reorder relative to one another. The
/// consumer ([`crate::edge_builder`]) keys idempotency on the deterministic `edge_id` derived from the
/// SAME `(tenant, source, target, rel)`; the aggregate key is the ORDERING key, the `edge_id` is the
/// IDENTITY key. PII-free: opaque URNs only.
pub fn edge_aggregate_key(source: &ArtifactRef, target: &ArtifactRef) -> AggregateKey {
    AggregateKey(format!("edge:{}->{}", source.0, target.0))
}

/// **One extracted reference edge** (the output of structured-node extraction; the emit-side input).
/// Every reference-class edge born from a content node is `(source, target, rel)` with
/// `rel_class = Reference` (§4.1 — Refs-authoritative). The deterministic `edge_id =
/// hash(tenant, source, target, rel)` (the consumer side, [`crate::edge_id`]) is derived from these
/// three; here the producer ships the triple. PII-free: `source`/`target` are opaque `ArtifactRef`
/// URNs (a `mention`'s target is the PSEUDONYMOUS `member` URN, never a name — erasure-safe, §4.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeDraft {
    /// The referencing side — the content document this node lives in (the same for every node in one
    /// document). The full `#sub`-precise URN.
    pub source: ArtifactRef,
    /// The referenced side — the artifact the structured node points at (the `mention`'s principal
    /// `member` URN, or the `artifact_ref`/`embed` target URN).
    pub target: ArtifactRef,
    /// The relation token this node kind produces (`mentions`/`links`/`embeds`).
    pub rel: EdgeRel,
    /// Always [`RelClass::Reference`] for the content-node producer (§4.1; the TE-7 lifecycle mirror
    /// is the OTHER producer, [`crate::edge_builder`]'s typed-relation path).
    pub rel_class: RelClass,
}

/// The canonical `member` URN for a mentioned principal (`myelin://<tenant>/identity/member/<id>` —
/// the §6.2 `identity`/`member` token pair). The mention target is the principal's PSEUDONYMOUS
/// opaque `principal_id` as an `ArtifactRef`, NEVER the name — so a mention edge is erasure-safe (the
/// name lives behind Identity's pseudonym map, §4.6). The `region` is not part of the URN scope
/// (a ref is `tenant/subsystem/type/id`); the principal's tenant scopes the URN.
fn principal_member_ref(p: &Principal) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/identity/member/{}",
        p.tenant.0, p.principal_id.0
    ))
}

/// **Extract one reference edge per structured ref node (the §4.1 producer; structured, NOT regex).**
///
/// Given the content document's own URN (`source`) and its parsed structured inline nodes (`doc`),
/// produce exactly one [`EdgeDraft`] per node, by **matching the enum variant** — the reliability
/// guarantee (EI-04 §2.4): extraction reads structured nodes, never scans prose, so it cannot miss a
/// reference inside a code span or hallucinate one from a literal `@` in text. The mapping is the
/// frozen X-2 uniform producer:
///
/// - [`InlineNode::Mention`]`(principal)` → `(source, member-urn(principal), mentions)`;
/// - [`InlineNode::ArtifactRefNode`]`(target)` → `(source, target, links)`;
/// - [`InlineNode::Embed`]`(target)` → `(source, target, embeds)`.
///
/// A document with **no** structured ref nodes yields **zero** edges (a plain-prose message produces
/// no reference edges — the no-op case). N structured nodes → N edges, in document order.
pub fn extract_edges(source: &ArtifactRef, doc: &[InlineNode]) -> Vec<EdgeDraft> {
    doc.iter()
        .map(|node| {
            let (target, rel) = match node {
                InlineNode::Mention(principal) => {
                    (principal_member_ref(principal), EdgeRel::Mentions)
                }
                InlineNode::ArtifactRefNode(target) => (target.clone(), EdgeRel::Links),
                InlineNode::Embed(target) => (target.clone(), EdgeRel::Embeds),
            };
            EdgeDraft {
                source: source.clone(),
                target,
                rel,
                // The content-node producer is ALWAYS reference-class (§4.1); the lifecycle mirror is
                // the typed-relation producer ([`crate::edge_builder`]), not this seam.
                rel_class: RelClass::Reference,
            }
        })
        .collect()
}

/// The frozen `refs.edge.created` event type (§4.1; contract 5.4 — the emit-side token). The ONLY
/// edge-creation event a content-node producer emits. A named constant so drills assert against the
/// NAME, never a literal (EI-01 §3 observability / coherence).
pub const REFS_EDGE_CREATED: &str = "refs.edge.created";

/// Build the canonical `refs.edge.created` [`EventDraft`] for one extracted [`EdgeDraft`].
///
/// The references-not-payloads payload carries `source`/`target`/`rel`/`rel_class` (the consumer
/// [`crate::edge_builder`] reads exactly these; the deterministic `edge_id` is derived from
/// `tenant + source + target + rel`, so the producer ships the triple, not the id). The aggregate is
/// the `(source → target)` edge identity — the SAME convention the builder's edge events use — so
/// per-aggregate ordering (EB-03) holds for an edge's create/remove sequence. `contains_personal_data
/// = false`: every field is an opaque ref/token (the mention target is the PSEUDONYMOUS member URN,
/// not a name), so no inline-PII envelope key is needed (references-not-payloads, contract 2.7).
fn edge_event_draft(edge: &EdgeDraft) -> EventDraft {
    EventDraft {
        type_: EventType(REFS_EDGE_CREATED.into()),
        // The referencing side is the event subject (the content document that authored the edge).
        subject: edge.source.clone(),
        // The edge identity aggregate — `edge:<source>-><target>` (shared with the builder's events
        // so an edge's create/remove sequence is per-aggregate ordered, EB-03).
        aggregate: edge_aggregate_key(&edge.source, &edge.target),
        payload: serde_json::json!({
            "source": edge.source.0,
            "target": edge.target.0,
            "rel": edge.rel.as_str(),
            "rel_class": edge.rel_class.as_str(),
        }),
        // Refs is the CONTROLLER of the edge fact it authors (the reference graph is Refs-owned).
        data_role: DataRole::Controller,
        // An edge inherits the referencing content's internal visibility (a routing hint, never an
        // authz decision — Identity decides at resolve-time, §4.2). The default for derived index
        // events is Internal.
        visibility: Visibility::Internal,
        // References-not-payloads: opaque refs only, no inline PII, so no envelope key.
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// **Emit one `refs.edge.created` per structured ref node, IN THE SAME TRANSACTION as the content
/// write (the §4.1 producer seam; contract 5.4 emit side).**
///
/// `tx` is the OPEN outbox transaction the caller is writing the content into (the content row is
/// staged in `tx`); `content_event` is the content event being emitted in that SAME transaction (the
/// CAUSE — `*.created`/`*.updated` for the document). For each extracted [`EdgeDraft`], this calls
/// [`OutboxTx::emit`]`(draft, cause = Some(content_event))` — the ONE sanctioned emit verb (contract
/// 2.2; the `no-raw-publish` lint, P-019). Returns the minted [`EventId`]s in document order.
///
/// **Causality correct-by-construction (P-S06):** because `cause = Some(content_event)`, the
/// [`derive_envelope`](myelin_events::derive_envelope) derivation sets `correlation_id =
/// content_event.correlation_id` (the root carries), `causation_id = content_event.event_id`, and
/// `depth = content_event.depth + 1` (the loop-guard +1 stamp — the explicit drill is REF-P9). The
/// caller CANNOT typo a wrong parent: the causal triple is not on [`EventDraft`].
///
/// **Emit-iff-committed (REF-D7 producer half, BUS-D4):** `emit` BUFFERS the row into `tx`; it becomes
/// durable iff the caller commits `tx`. An aborted content transaction drops the buffered edge rows
/// with it — no edge without its content. This function performs NO commit (the caller owns the
/// transaction lifecycle — content + edges co-commit).
pub fn emit_edges(
    tx: &mut dyn OutboxTx,
    source: &ArtifactRef,
    doc: &[InlineNode],
    content_event: &EventEnvelope,
) -> Result<Vec<EventId>> {
    let edges = extract_edges(source, doc);
    let mut ids = Vec::with_capacity(edges.len());
    for edge in &edges {
        // The ONE sanctioned emit path (contract 2.2; no-raw-publish). `cause = Some(content_event)`
        // → the correlation root carries + causation = the content event + depth+1 (the loop-guard
        // stamp, REF-P9). The row is BUFFERED into `tx` — durable iff the caller commits (the content
        // write + these edges co-commit; emit-iff-committed, REF-D7 producer half).
        let id = tx.emit(edge_event_draft(edge), Some(content_event))?;
        ids.push(id);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn source_doc() -> ArtifactRef {
        ArtifactRef("myelin://acme/chat/message/m1".into())
    }

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-7".into()),
            PrincipalKind::Human,
            tenant(),
        )
    }

    // --- extraction: one edge per structured node, correct rel/rel_class ---

    /// **Each of the three structured nodes yields exactly one edge with the correct `rel`/`rel_class`
    /// and target.** This is the X-2 uniform producer mapping (mention→mentions,
    /// artifact_ref→links, embed→embeds), structured-node-driven (NOT regex). The mention's target is
    /// the PSEUDONYMOUS `member` URN (erasure-safe), never the name.
    #[test]
    fn each_node_kind_yields_one_edge_with_correct_rel_and_class() {
        let src = source_doc();
        let target = ArtifactRef("myelin://acme/knowledge/page/7c2".into());
        let doc = vec![
            InlineNode::Mention(principal()),
            InlineNode::ArtifactRefNode(target.clone()),
            InlineNode::Embed(target.clone()),
        ];
        let edges = extract_edges(&src, &doc);
        assert_eq!(edges.len(), 3, "N structured nodes → N edges");

        // mention → mentions, target = the pseudonymous member URN.
        assert_eq!(edges[0].rel, EdgeRel::Mentions);
        assert_eq!(edges[0].rel.as_str(), "mentions");
        assert_eq!(edges[0].rel_class, RelClass::Reference);
        assert_eq!(edges[0].source, src);
        assert_eq!(
            edges[0].target.0, "myelin://acme/identity/member/p-opaque-7",
            "mention target is the pseudonymous member URN, never the name"
        );

        // artifact_ref → links.
        assert_eq!(edges[1].rel, EdgeRel::Links);
        assert_eq!(edges[1].rel.as_str(), "links");
        assert_eq!(edges[1].target, target);

        // embed → embeds.
        assert_eq!(edges[2].rel, EdgeRel::Embeds);
        assert_eq!(edges[2].rel.as_str(), "embeds");
        assert_eq!(edges[2].target, target);
    }

    /// **A document with NO structured ref nodes yields ZERO edges** (a plain-prose message produces
    /// no reference edges — the no-op case; extraction is structured, not a regex that would
    /// false-positive on a literal `@` in text).
    #[test]
    fn document_with_no_ref_nodes_yields_zero_edges() {
        let edges = extract_edges(&source_doc(), &[]);
        assert!(edges.is_empty(), "no structured nodes → no edges");
    }

    /// **The edge event draft is `refs.edge.created` with the references-not-payloads triple and the
    /// shared `edge:<source>-><target>` aggregate** (so per-aggregate ordering holds for an edge's
    /// create/remove sequence, EB-03; and the consumer derives the deterministic `edge_id` from the
    /// payload triple). `contains_personal_data = false` (opaque refs only).
    #[test]
    fn edge_event_draft_is_refs_edge_created_with_the_triple() {
        let target = ArtifactRef("myelin://acme/knowledge/page/7c2#block-3".into());
        let edge = EdgeDraft {
            source: source_doc(),
            target: target.clone(),
            rel: EdgeRel::Embeds,
            rel_class: RelClass::Reference,
        };
        let draft = edge_event_draft(&edge);
        assert_eq!(draft.type_.0, "refs.edge.created");
        assert_eq!(
            draft.subject,
            source_doc(),
            "the subject is the referencing content"
        );
        assert_eq!(draft.payload["source"], "myelin://acme/chat/message/m1");
        assert_eq!(draft.payload["target"], target.0);
        assert_eq!(draft.payload["rel"], "embeds");
        assert_eq!(draft.payload["rel_class"], "reference");
        assert!(
            !draft.contains_personal_data,
            "references-not-payloads: no inline PII"
        );
        assert!(draft.pii_key_ref.is_none());
    }

    /// `REFS_EDGE_CREATED` is the frozen `refs.edge.created` token (the named constant the drills
    /// assert against, never a literal).
    #[test]
    fn refs_edge_created_token_is_frozen() {
        assert_eq!(REFS_EDGE_CREATED, "refs.edge.created");
    }
}
