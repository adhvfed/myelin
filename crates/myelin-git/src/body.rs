//! # `body` — PR/review/comment bodies on the frozen `myelin-content` subset + the content-node →
//! `refs.edge.created` emission (GIT-P17 / P-278, M3-G3)
//!
//! This is the M3-G3 **content-bodies half** of Git hosting (the [`crate::lifecycle`] entities are the
//! domain half). It makes a Git PR/review/comment BODY a real [`myelin_content`] document — the FROZEN
//! markdown-subset string + the three positional structured inline nodes
//! ([`InlineNode::Mention`] / [`InlineNode::ArtifactRefNode`] / [`InlineNode::Embed`], contract 13.1,
//! X-2/OQ-B) — and emits the reference edges those structured nodes produce **uniformly via the
//! outbox** (`refs.edge.created`, contract 5.4). It REPLACES the GIT-P16 opaque [`crate::lifecycle::BodyRef`]
//! ciphertext-handle floor for the body content: a body now round-trips `render(parse(md)) === md` and
//! its structured nodes are the edge producers.
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `../../VISION.md` §2 (the cross-artifact reference graph — Git PRs/comments reference any
//!   artifact); §3 (content round-trips — the one editor render path).
//! - `03-events-contracts-and-glue.md` §1 (the inline comment thread bodies; the `git.comment.*` /
//!   `git.pr.*` events these edges ride the SAME transaction as) + `00-overview.md` §1.1 (the inline
//!   comment thread bodies are **single-author CAS** — a git PR/review body is single-author, the
//!   multi-author collab story is Knowledge's, not git's).
//! - `00-reconciliation-decisions.md` X-2 (the `myelin-content` taxonomy + the three content nodes
//!   byte-identical across Chat/Issues/Knowledge — and here Git).
//! - `contract-index.md` rows **5.4** (`refs.edge.created` from the mention/artifact_ref/embed content
//!   nodes — emitted by producers via the outbox; **no standalone edge-write API**), **13.1** (the
//!   `myelin-content` markdown-subset + `render(parse(md)) === md`).
//!
//! ## What this prompt (GIT-P17 / P-278) ships
//! 1. [`Body`] — a Git PR/review/comment body as a `myelin-content` document (the markdown-subset
//!    string + the positional [`InlineNode`] array). [`Body::parse`] re-derives the [`Inline`] AST
//!    through the ONE [`myelin_content::parse_inline`]; [`Body::render`] re-serialises through the ONE
//!    [`myelin_content::serialize_inline`] — so `render(parse(md)) === md` byte-identically (the
//!    KN-D2-class round-trip applied to git bodies). **Single-author CAS** ([`Body::cas_edit`]): a body
//!    edit is admitted only against the expected `revision` (a stale edit is rejected loudly — no
//!    silent last-writer-wins).
//! 2. [`extract_body_edges`] — extract EXACTLY ONE reference edge per structured ref node by **matching
//!    the structured enum variant**, NEVER a regex over the prose (the reliability guarantee, EI-04
//!    §2.4: a `@` in a code span is not an edge; a structured `mention` node is). The frozen X-2 uniform
//!    mapping: `mention → mentions`, `artifact_ref → links`, `embed → embeds`.
//! 3. [`emit_body_edges`] — emit one `refs.edge.created` per extracted edge **in the SAME outbox
//!    transaction** as the body's `git.pr.*` / `git.comment.*` content event (emit-iff-committed — no
//!    edge without its content, no content without its edge). The emitted event is the byte-identical
//!    shape the Refs edge-builder ([`myelin_refs`] consumer) ingests.
//!
//! ## Why a Git-OWNED producer half (EI-01 §7 — extend/reconcile, never duplicate)
//! The canonical content-node → edge extraction + emit seam already exists in the Refs SERVICE crate
//! (`myelin_refs_service::emit::{extract_edges, emit_edges}`, REF-P8 / P-157), and the Knowledge
//! producer drives it directly (`myelin_refs_service::kn_producer`). But **Git is a producer LEAF and
//! CANNOT depend on the Refs SERVICE crate** (the §2.9 acyclic DAG — a subsystem crate never depends on
//! a sibling service crate; the same constraint that made [`crate::lifecycle::CodeOwners::resolve`] the
//! "Git-owned half" of contract 4.9). So this module is the **Git-owned producer half** of contract
//! 5.4: it produces the **byte-identical** `refs.edge.created` event the Refs edge-builder consumes —
//! the SAME `source`/`target`/`rel`/`rel_class` payload + the SAME `edge:<source>-><target>` aggregate
//! (so an edge's create/remove sequence is per-aggregate ordered, EB-03) — over the SAME frozen
//! [`myelin_content::InlineNode`] taxonomy. The encoding equivalence with the Refs seam is PINNED by the
//! CDC (`tests/cdc_5_4_git_content_edges.rs`): a drift on either side fails the same CI job. There is no
//! second content model and no second edge wire-shape — Git reuses the frozen shapes and adds only the
//! Git source-URN construction (the PR/comment root the edges depart from).
//!
//! ## Named floors (VISION §3 / EI-01 §1)
//! - **The typed-edge LIFECYCLE mirror is GIT-P19, NOT here.** The `Closes <ISSUEKEY>` commit-trailer /
//!   PR-link `closes`/`relates` edges (the `rel_class='lifecycle'` TE-7 mirror, contract 5.5) are a
//!   DISTINCT producer from these content-node `mention`/`artifact_ref`/`embed` reference edges
//!   (`rel_class='reference'`). A `Closes ENG-1` written as PROSE in a body is NOT an edge here (only a
//!   structured `artifact_ref`/`embed`/`mention` node is) — the trailer-driven typed edge is the
//!   GIT-P19 follow-on over the typed PR-link table. Named so the content-node producer is not mistaken
//!   for the lifecycle mirror.
//! - **The per-subject-DEK body ciphertext-at-rest** (the body bytes encrypted under the subject DEK,
//!   contract 11.4 — `erasure = CryptoShred`) is the storage layer's; [`Body`] is the cleartext
//!   in-memory document the round-trip + extraction run over. The live OLTP store + the DEK seal/unseal
//!   ride the GIT-P20 store wiring (the SAME seam — this module is the document + extraction logic, the
//!   store is the persistence). Named so the cleartext document is not mistaken for the at-rest form.
//! - **Mutation floor (mandatory-core) — MEASURED ≥ 90%, met at 94%.** The content-node → edge path —
//!   the per-variant `rel`/`rel_class` mapping, the principal → pseudonymous `member`-URN target
//!   derivation, the one-edge-per-node invariant (0 edges for a plain-prose body), the same-tx outbox
//!   emit shape, the round-trip invariant, and the single-author CAS — is the mutation-tested core. The
//!   floor is stated + met: `cargo mutants -p myelin-git --file crates/myelin-git/src/body.rs` finds 17
//!   viable mutants, **16 caught (94%)** by the unit + chained-e2e + CDC tests; the SOLE survivor is
//!   `Body::empty -> Default::default()`, a **provable EQUIVALENT mutant** ([`Body::empty`] is *defined
//!   as* [`Body::default`], so no observation can distinguish them — a cargo-mutants false positive, not
//!   a test gap). A mutant that drops a variant, mis-maps a `rel`, emits outside the transaction, breaks
//!   the round-trip, or admits a stale CAS edit is caught. The world-scale corpus-under-load drill is a
//!   later band.

use myelin_content::{parse_inline, serialize_inline, Inline, InlineNode};
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType, OutboxTx,
    Result as BusResult, Visibility,
};
use myelin_identity::Principal;

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE BODY — a Git PR/review/comment body as a myelin-content document (single-author CAS)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A Git PR/review/comment **body** as a frozen [`myelin_content`] document (contract 13.1): the
/// markdown-subset string + the positional structured inline nodes
/// ([`InlineNode::Mention`] / [`InlineNode::ArtifactRefNode`] / [`InlineNode::Embed`]). This is the
/// cleartext in-memory document the round-trip + edge-extraction run over (the at-rest per-subject-DEK
/// ciphertext is the storage layer's — a named floor).
///
/// **Single-author CAS** (00-overview §1.1 — a git PR/review body is single-author): the body carries a
/// monotonic `revision`; an edit is admitted only against the expected revision ([`Body::cas_edit`]) —
/// a stale edit is rejected loudly (no silent last-writer-wins). The multi-author collaborative-edit
/// engine is Knowledge's (KN-1 CAS-floor → CRDT), never git's.
///
/// The `md` field is the canonical markdown-subset string; the `nodes` field is the positional
/// structured-node array (the i-th [`myelin_content::OBJ`] in `md` binds to `nodes[i]`, §2.2). The
/// invariant is `render(parse(md)) === md` — pinned by [`Body::round_trips`].
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Body {
    /// The canonical markdown-subset string (the body prose with `**bold**`/`*italic*`/`` `code` ``/
    /// `~~strike~~`/`[text](url)` + one [`myelin_content::OBJ`] per structured node).
    pub md: String,
    /// The positional structured inline nodes (the i-th node binds the i-th `OBJ` in `md`). These three
    /// node kinds are the uniform `refs.edge.created` producers ([`extract_body_edges`]).
    pub nodes: Vec<InlineNode>,
    /// The single-author CAS revision (monotonic; bumped on each admitted edit). A stale edit (against a
    /// prior revision) is rejected by [`Body::cas_edit`].
    pub revision: u64,
}

/// A single-author CAS conflict on a body edit — the edit's expected revision did not match the body's
/// current revision (a concurrent edit landed first). Loud + typed: a stale edit is NEVER silently
/// applied (no last-writer-wins). Carries `(expected, actual)` for the audit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CasConflict {
    /// The revision the edit expected the body to be at.
    pub expected: u64,
    /// The revision the body is actually at (a concurrent edit advanced it).
    pub actual: u64,
}

impl std::fmt::Display for CasConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "single-author CAS conflict: edit expected revision {} but the body is at {}",
            self.expected, self.actual
        )
    }
}

impl std::error::Error for CasConflict {}

impl Body {
    /// Build a new body (revision 0) from a markdown-subset string + its positional structured nodes.
    /// The caller supplies the canonical `md` (e.g. the editor's serialised form) + the `nodes` array;
    /// the round-trip invariant ([`Body::round_trips`]) holds iff `md` is canonical.
    pub fn new(md: impl Into<String>, nodes: Vec<InlineNode>) -> Body {
        Body { md: md.into(), nodes, revision: 0 }
    }

    /// An empty body (revision 0) — a PR/comment opened with no description yet. Round-trips trivially
    /// (the empty string serialises to itself, 0 nodes ⇒ 0 edges).
    pub fn empty() -> Body {
        Body::default()
    }

    /// **Parse the body into the [`Inline`] AST through the ONE [`myelin_content::parse_inline`].** The
    /// i-th [`myelin_content::OBJ`] in `md` binds to `nodes[i]` (positional, §2.2). This is the SAME
    /// parser the WASM editor + every other subsystem run — there is no git-local renderer (the
    /// two-divergent-renderers trap is eliminated structurally, EI-01 §7).
    pub fn parse(&self) -> Inline {
        parse_inline(&self.md, &self.nodes)
    }

    /// **Render the parsed AST back to the canonical markdown-subset string through the ONE
    /// [`myelin_content::serialize_inline`].** `body.render() === body.md` for any canonical body — the
    /// `render(parse(md)) === md` round-trip invariant (the KN-D2-class gate applied to git bodies).
    pub fn render(&self) -> String {
        serialize_inline(&self.parse())
    }

    /// **The frozen round-trip invariant `render(parse(md)) === md` (contract 13.1; the GATE).** `true`
    /// iff re-serialising the parsed body reproduces the canonical `md` byte-identically. A corpus of
    /// git bodies round-tripping at 100% is the CI gate ([`crate::body::tests`] +
    /// `tests/roundtrip_git_bodies.rs`).
    pub fn round_trips(&self) -> bool {
        self.render() == self.md
    }

    /// The structured ref nodes of the body (the edge producers) — a node-array walk, NEVER a regex over
    /// the prose (the reliability guarantee, EI-04 §2.4). This is the seam [`extract_body_edges`] reads.
    pub fn structured_nodes(&self) -> &[InlineNode] {
        &self.nodes
    }

    /// **Apply a single-author CAS edit (00-overview §1.1).** Replace the body content + nodes IFF
    /// `expected_revision` matches the current `revision`; on success the revision is bumped and the new
    /// revision returned. On a stale edit returns [`CasConflict`] — the edit is NOT applied (no silent
    /// last-writer-wins; a concurrent editor's change is never clobbered). The multi-author merge is
    /// Knowledge's CRDT, never git's.
    pub fn cas_edit(
        &mut self,
        expected_revision: u64,
        md: impl Into<String>,
        nodes: Vec<InlineNode>,
    ) -> Result<u64, CasConflict> {
        if expected_revision != self.revision {
            return Err(CasConflict { expected: expected_revision, actual: self.revision });
        }
        self.md = md.into();
        self.nodes = nodes;
        self.revision += 1;
        Ok(self.revision)
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE CONTENT-NODE → refs.edge.created PRODUCER (contract 5.4 — Git-owned producer half)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The frozen `rel` column token a structured content node produces (contract 5.4 / Refs §4.1 — the
/// `{mentions, links, embeds}` vocabulary). PII-free token. Each of the three structured inline nodes
/// maps to **exactly one** rel — the uniform X-2 producer (byte-identical across Chat/Issues/Knowledge,
/// and here Git). Mirrors `myelin_refs_service::emit::EdgeRel` byte-for-byte (the encoding equivalence
/// is pinned by the CDC) — Git produces the SAME wire tokens the Refs consumer ingests, it does not
/// author a second vocabulary.
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
    /// The frozen `rel` column token (`'mentions' | 'links' | 'embeds'`, contract 5.4).
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeRel::Mentions => "mentions",
            EdgeRel::Links => "links",
            EdgeRel::Embeds => "embeds",
        }
    }
}

/// The frozen `rel_class` token a CONTENT-NODE edge carries (contract 5.4 / Refs §3.2). A content-node
/// reference edge is ALWAYS `reference` (Refs-authoritative); the `lifecycle` class is the TE-7
/// typed-edge mirror's (the GIT-P19 follow-on, a DISTINCT producer). A `&'static str` constant so the
/// drills assert against the token, never a literal — and it is the byte-identical token the Refs
/// edge-builder stamps for a `refs.edge.*` event (`RelClass::Reference.as_str()`).
pub const REL_CLASS_REFERENCE: &str = "reference";

/// The frozen `refs.edge.created` event type (contract 5.4 — the emit-side token). The ONLY edge-creation
/// event a content-node producer emits. A named constant so drills assert against the NAME, never a
/// literal (EI-01 §3). Byte-identical to `myelin_refs_service::emit::REFS_EDGE_CREATED`.
pub const REFS_EDGE_CREATED: &str = "refs.edge.created";

/// **One extracted reference edge** from a body's structured node — the `(source, target, rel)` triple
/// (`rel_class = reference`, always). The deterministic `edge_id = hash(tenant, source, target, rel)`
/// is the CONSUMER's (the Refs edge-builder derives it from the payload triple); here the producer ships
/// the triple. PII-free: `source`/`target` are opaque `ArtifactRef` URNs (a mention's target is the
/// PSEUDONYMOUS `member` URN, never a name — erasure-safe). Mirrors
/// `myelin_refs_service::emit::EdgeDraft` (the encoding equivalence is CDC-pinned).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyEdge {
    /// The referencing side — the Git PR/comment body URN this node lives in (the same for every node in
    /// one body). The full `#sub`-precise URN (e.g. a `#comment-<id>` sub-URN, [`crate::subs`]).
    pub source: ArtifactRef,
    /// The referenced side — the artifact the structured node points at (the mention's pseudonymous
    /// `member` URN, or the `artifact_ref`/`embed` target URN).
    pub target: ArtifactRef,
    /// The relation token this node kind produces (`mentions`/`links`/`embeds`).
    pub rel: EdgeRel,
}

/// **The shared edge-aggregate-key convention `edge:<source>-><target>` (EB-03 ordering anchor).** Every
/// `refs.edge.*` event for ONE logical edge shares this aggregate, so an edge's create → remove → create
/// sequence is per-aggregate ordered (gap-free, in commit order). Byte-identical to
/// `myelin_refs_service::emit::edge_aggregate_key` — Git's content-node edges share the SAME ordering
/// aggregate the Refs consumer + the Knowledge producer use (one ordering key across producers). PII-free.
pub fn edge_aggregate_key(source: &ArtifactRef, target: &ArtifactRef) -> AggregateKey {
    AggregateKey(format!("edge:{}->{}", source.0, target.0))
}

/// The canonical `member` URN for a mentioned principal (`myelin://<tenant>/identity/member/<id>` — the
/// §6.2 `identity`/`member` token pair). The mention target is the principal's PSEUDONYMOUS opaque
/// `principal_id` as an `ArtifactRef`, NEVER the name — so a mention edge is erasure-safe (the name
/// lives behind Identity's pseudonym map). Byte-identical to the Refs seam's `principal_member_ref`.
fn principal_member_ref(p: &Principal) -> ArtifactRef {
    ArtifactRef(format!("myelin://{}/identity/member/{}", p.tenant.0, p.principal_id.0))
}

/// **Extract one reference edge per structured ref node (the contract-5.4 producer; structured, NOT
/// regex).**
///
/// Given the body's own URN (`source` — the PR/comment `#sub`-precise root) and its structured inline
/// nodes (`nodes`), produce exactly one [`BodyEdge`] per node by **matching the enum variant** — the
/// reliability guarantee (EI-04 §2.4): extraction reads structured nodes, never scans prose, so a
/// literal `@alice` inside a code span or a `Closes ENG-1` written as prose is NOT an edge (only a
/// structured `mention`/`artifact_ref`/`embed` node is). The frozen X-2 uniform mapping:
///
/// - [`InlineNode::Mention`]`(principal)` → `(source, member-urn(principal), mentions)`;
/// - [`InlineNode::ArtifactRefNode`]`(target)` → `(source, target, links)`;
/// - [`InlineNode::Embed`]`(target)` → `(source, target, embeds)`.
///
/// A body with **no** structured ref nodes yields **zero** edges (a plain-prose comment — the no-op
/// case). N structured nodes → N edges, in body order. This is the SAME mapping
/// `myelin_refs_service::emit::extract_edges` runs (CDC-pinned); Git owns this half because it cannot
/// depend on the Refs service crate.
pub fn extract_body_edges(source: &ArtifactRef, nodes: &[InlineNode]) -> Vec<BodyEdge> {
    nodes
        .iter()
        .map(|node| {
            let (target, rel) = match node {
                InlineNode::Mention(principal) => (principal_member_ref(principal), EdgeRel::Mentions),
                InlineNode::ArtifactRefNode(target) => (target.clone(), EdgeRel::Links),
                InlineNode::Embed(target) => (target.clone(), EdgeRel::Embeds),
            };
            BodyEdge { source: source.clone(), target, rel }
        })
        .collect()
}

/// Build the canonical `refs.edge.created` [`EventDraft`] for one extracted [`BodyEdge`].
///
/// The references-not-payloads payload carries `source`/`target`/`rel`/`rel_class` (the Refs
/// edge-builder reads exactly these; the deterministic `edge_id` is derived from `tenant + source +
/// target + rel`, so the producer ships the triple, not the id). The aggregate is the
/// `edge:<source>-><target>` identity — the SAME convention the Refs consumer + Knowledge producer use —
/// so per-aggregate ordering (EB-03) holds for an edge's create/remove sequence. `contains_personal_data
/// = false`: every field is an opaque ref/token (the mention target is the PSEUDONYMOUS member URN, not
/// a name), so no inline-PII envelope key is needed (references-not-payloads, contract 2.7).
fn edge_event_draft(edge: &BodyEdge) -> EventDraft {
    EventDraft {
        type_: EventType(REFS_EDGE_CREATED.into()),
        // The referencing side is the event subject (the Git body that authored the edge).
        subject: edge.source.clone(),
        aggregate: edge_aggregate_key(&edge.source, &edge.target),
        payload: serde_json::json!({
            "source": edge.source.0,
            "target": edge.target.0,
            "rel": edge.rel.as_str(),
            "rel_class": REL_CLASS_REFERENCE,
        }),
        // Refs is the CONTROLLER of the edge fact it authors (the reference graph is Refs-owned) — the
        // SAME role the Refs/Knowledge producer stamps (the edge fact is Refs', not git's processor
        // posture on repo content).
        data_role: DataRole::Controller,
        // An edge inherits the referencing content's internal visibility (a routing hint, never an authz
        // decision — Identity decides at resolve-time). The default for derived index events is Internal.
        visibility: Visibility::Internal,
        // References-not-payloads: opaque refs only, no inline PII, so no envelope key.
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// **Emit one `refs.edge.created` per structured ref node, IN THE SAME TRANSACTION as the body's content
/// write (the contract-5.4 producer seam — Git-owned half).**
///
/// `tx` is the OPEN outbox transaction the caller is writing the body's `git.pr.*` / `git.comment.*`
/// content event into (the body row + the content event are staged in `tx`); `content_event` is that
/// content event (the CAUSE). For each extracted [`BodyEdge`], this calls
/// [`OutboxTx::emit`]`(draft, cause = Some(content_event))` — the ONE sanctioned emit verb (contract
/// 2.2; the `no-raw-publish` lint, P-019). There is **NO standalone edge-write API** — the edges are
/// emitted from the content nodes only. Returns the minted [`EventId`]s in body order.
///
/// **Causality correct-by-construction (P-S06):** because `cause = Some(content_event)`, the envelope
/// derivation sets `correlation_id = content_event.correlation_id` (the root carries), `causation_id =
/// content_event.event_id`, and `depth = content_event.depth + 1` (the loop-guard stamp). The caller
/// CANNOT typo a wrong parent: the causal triple is not on [`EventDraft`].
///
/// **Emit-iff-committed (the silent-data-loss floor, GIT-D9-class):** `emit` BUFFERS the row into `tx`;
/// it becomes durable iff the caller commits `tx`. An aborted body write drops the buffered edge rows
/// with it — **no edge without its body, no body without its edge** (the body content + the edge events
/// co-commit). This function performs NO commit (the caller owns the transaction lifecycle — the same
/// discipline [`crate::receive_pack`] uses for `git.ref.updated`).
pub fn emit_body_edges(
    tx: &mut dyn OutboxTx,
    source: &ArtifactRef,
    nodes: &[InlineNode],
    content_event: &EventEnvelope,
) -> BusResult<Vec<EventId>> {
    let edges = extract_body_edges(source, nodes);
    let mut ids = Vec::with_capacity(edges.len());
    for edge in &edges {
        // The ONE sanctioned emit path (contract 2.2; no-raw-publish). `cause = Some(content_event)` →
        // the correlation root carries + causation = the content event + depth+1. The row is BUFFERED
        // into `tx` — durable iff the caller commits (the body write + these edges co-commit).
        let id = tx.emit(edge_event_draft(edge), Some(content_event))?;
        ids.push(id);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::OBJ;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    /// A Git PR-comment body source URN (a `#comment-<id>` sub-URN, the referencing side).
    fn comment_source() -> ArtifactRef {
        crate::subs::mint_pr_comment("acme", "repo7", 42, "cAbc").unwrap()
    }

    fn alice() -> Principal {
        Principal::stub(PrincipalId("p-opaque-alice".into()), PrincipalKind::Human, tenant())
    }

    // ── 1. round-trip: render(parse(md)) === md on git bodies (contract 13.1) ──────────────────────

    /// **`render(parse(md)) === md` byte-identical on a marked-up git body** (the KN-D2-class round-trip
    /// applied to git content). A body mixing every mark + a structured node round-trips exactly.
    #[test]
    fn body_round_trips_byte_identical() {
        let body = Body::new(
            format!("**bold** and *italic* with `code` and a {OBJ} mention"),
            vec![InlineNode::Mention(alice())],
        );
        assert!(body.round_trips(), "render(parse(md)) must === md");
        assert_eq!(body.render(), body.md);
    }

    /// An empty body round-trips (a PR opened with no description) and yields zero edges.
    #[test]
    fn empty_body_round_trips_and_has_no_edges() {
        let body = Body::empty();
        assert!(body.round_trips());
        assert!(extract_body_edges(&comment_source(), body.structured_nodes()).is_empty());
    }

    /// **`round_trips()` returns FALSE on a NON-canonical body** (a `*` left unescaped as literal text
    /// is re-emitted escaped, so `render(parse(md)) != md`). This kills the `round_trips -> true` mutant:
    /// the round-trip invariant is a real check, not a constant — a non-canonical `md` is NOT a fixed
    /// point. (The canonical form `a\*b` IS a fixed point; the un-escaped `a*b` is not.)
    #[test]
    fn round_trips_is_false_on_a_non_canonical_body() {
        // `a*b` — a single `*` that opens no mark; the serializer re-emits it escaped as `a\*b`, so the
        // raw (non-canonical) `md` does NOT round-trip. The canonical `a\*b` DOES.
        let non_canonical = Body::new("a*b", vec![]);
        assert!(!non_canonical.round_trips(), "a non-canonical body must NOT round-trip");
        assert_eq!(non_canonical.render(), "a\\*b", "the serializer canonicalises the literal `*`");
        assert!(Body::new("a\\*b", vec![]).round_trips(), "the canonical form IS a fixed point");
    }

    /// **The CAS-conflict Display names the conflicting revisions** (the loud, auditable surface — a
    /// stale edit is never silently coerced; the message carries `(expected, actual)`). Kills the
    /// `fmt -> Ok(default)` mutant.
    #[test]
    fn cas_conflict_display_names_the_revisions() {
        let msg = CasConflict { expected: 0, actual: 3 }.to_string();
        assert!(msg.contains('0') && msg.contains('3'), "the message names both revisions: {msg}");
        assert!(msg.to_lowercase().contains("cas"), "the message is the CAS-conflict surface: {msg}");
    }

    /// **Single-author CAS: a stale edit is rejected loudly (no last-writer-wins).** An edit against the
    /// wrong expected revision returns [`CasConflict`] and does NOT mutate the body.
    #[test]
    fn body_cas_edit_rejects_stale_revision() {
        let mut body = Body::new("v0", vec![]);
        assert_eq!(body.revision, 0);
        // a fresh edit at revision 0 is admitted and bumps to 1.
        assert_eq!(body.cas_edit(0, "v1", vec![]).unwrap(), 1);
        assert_eq!(body.md, "v1");
        // a stale edit (still expecting revision 0) is rejected — body unchanged.
        let conflict = body.cas_edit(0, "v2-stale", vec![]).unwrap_err();
        assert_eq!(conflict, CasConflict { expected: 0, actual: 1 });
        assert_eq!(body.md, "v1", "a rejected CAS edit does NOT mutate the body");
        assert_eq!(body.revision, 1);
    }

    // ── 2. extraction: one edge per structured node, correct rel/target (contract 5.4) ─────────────

    /// **Each of the three structured nodes yields exactly one edge with the correct `rel` and target**
    /// (the X-2 uniform producer mapping, structured-node-driven NOT regex). The mention's target is the
    /// PSEUDONYMOUS `member` URN (erasure-safe), never the name.
    #[test]
    fn each_node_kind_yields_one_edge_with_correct_rel_and_target() {
        let src = comment_source();
        let page = ArtifactRef("myelin://acme/knowledge/page/7c2".into());
        let issue = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        let nodes = vec![
            InlineNode::Mention(alice()),
            InlineNode::ArtifactRefNode(issue.clone()),
            InlineNode::Embed(page.clone()),
        ];
        let edges = extract_body_edges(&src, &nodes);
        assert_eq!(edges.len(), 3, "N structured nodes → N edges (1 per node)");

        // mention → mentions, target = the pseudonymous member URN.
        assert_eq!(edges[0].rel, EdgeRel::Mentions);
        assert_eq!(edges[0].rel.as_str(), "mentions");
        assert_eq!(edges[0].source, src);
        assert_eq!(
            edges[0].target.0, "myelin://acme/identity/member/p-opaque-alice",
            "mention target is the pseudonymous member URN, never the name"
        );

        // artifact_ref → links.
        assert_eq!(edges[1].rel, EdgeRel::Links);
        assert_eq!(edges[1].rel.as_str(), "links");
        assert_eq!(edges[1].target, issue);

        // embed → embeds.
        assert_eq!(edges[2].rel, EdgeRel::Embeds);
        assert_eq!(edges[2].rel.as_str(), "embeds");
        assert_eq!(edges[2].target, page);
    }

    /// **A `Closes ENG-1` written as PROSE produces ZERO edges** — extraction is structured, never a
    /// regex over prose (the GIT-P19 typed-edge mirror is the DISTINCT trailer producer, not this one).
    /// Only a structured node is an edge.
    #[test]
    fn prose_closes_trailer_is_not_a_content_edge() {
        let body = Body::new("Closes ENG-1 and fixes the bug.", vec![]);
        assert!(body.round_trips());
        let edges = extract_body_edges(&comment_source(), body.structured_nodes());
        assert!(edges.is_empty(), "a prose `Closes` is NOT a content edge (that is GIT-P19's mirror)");
    }

    /// **The edge event draft is `refs.edge.created` with the references-not-payloads triple + the shared
    /// `edge:<source>-><target>` aggregate + `rel_class = reference`.** This is the byte-identical shape
    /// the Refs edge-builder ingests (CDC-pinned). `contains_personal_data = false` (opaque refs only).
    #[test]
    fn edge_event_draft_is_refs_edge_created_with_the_triple() {
        let src = comment_source();
        let target = ArtifactRef("myelin://acme/knowledge/page/7c2#block-3".into());
        let edge = BodyEdge { source: src.clone(), target: target.clone(), rel: EdgeRel::Embeds };
        let draft = edge_event_draft(&edge);
        assert_eq!(draft.type_.0, "refs.edge.created");
        assert_eq!(draft.subject, src, "the subject is the referencing body");
        assert_eq!(draft.payload["source"], src.0);
        assert_eq!(draft.payload["target"], target.0);
        assert_eq!(draft.payload["rel"], "embeds");
        assert_eq!(draft.payload["rel_class"], "reference");
        assert_eq!(draft.aggregate.0, format!("edge:{}->{}", src.0, target.0));
        assert!(!draft.contains_personal_data, "references-not-payloads: no inline PII");
        assert!(draft.pii_key_ref.is_none());
        assert_eq!(draft.data_role, DataRole::Controller);
    }

    /// The frozen tokens are exactly the Refs wire tokens (the names anchor X-5; no second vocabulary).
    #[test]
    fn frozen_tokens_match_the_refs_wire_shape() {
        assert_eq!(REFS_EDGE_CREATED, "refs.edge.created");
        assert_eq!(REL_CLASS_REFERENCE, "reference");
        assert_eq!(EdgeRel::Mentions.as_str(), "mentions");
        assert_eq!(EdgeRel::Links.as_str(), "links");
        assert_eq!(EdgeRel::Embeds.as_str(), "embeds");
    }
}
