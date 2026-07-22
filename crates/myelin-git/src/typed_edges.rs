//! # `typed_edges` — the typed-edge mirror: PR-link / commit-trailer lifecycle edges into the Refs
//! projection (GIT-P19 / P-280, M3-G3)
//!
//! This is the M3-G3 **typed-edge-mirror half** of Git hosting. As the PR lifecycle advances, a Git PR
//! emits **lifecycle edges** (`closes` / `relates`) into the Refs projection **via the outbox** — the
//! `rel_class='lifecycle'` TE-7 mirror (contract 5.5), DISTINCT from the content-node
//! `mention`/`artifact_ref`/`embed` REFERENCE edges ([`crate::body`], GIT-P17, `rel_class='reference'`):
//!
//! - a **`Closes <ISSUEKEY>` trailer** on a **merged** PR produces a **`closes`** lifecycle edge
//!   (PR → issue) — the auto-close linkage Issues reflects (arch §1.1 `issue.issue.closed` consume);
//! - a **PR-link** (a PR explicitly declares it RELATES to another artifact — another PR, an issue, a
//!   commit) produces a **`relates`** lifecycle edge (symmetric, both ends visible after the Refs
//!   mirror's inverse projection).
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `../../VISION.md` §2 (the cross-artifact reference graph — "producing edges from trailers/PR
//!   links" is the Git→Refs contribution, 00-overview §1.1 the cross-cutting table).
//! - `03-events-contracts-and-glue.md` §1 (the `git.pr.merged` / `git.pr.updated` lifecycle events
//!   these edges ride the SAME transaction as) + `00-overview.md` §1.1 (the PR lifecycle; Refs row:
//!   "producing edges from trailers/PR links").
//! - `contract-index.md` row **5.5** (the TE-7 typed-edge mirror — lifecycle edges
//!   `closes/blocks/.../relates` dual-homed: the producer subsystem is the source of truth, Refs holds
//!   the rebuildable projection + fixes the inverse pairing).
//! - `external-insights/01-process-and-quality-doctrine.md` §7 (keep contracts coherent — extend the
//!   existing producer seam, never duplicate the edge wire-shape).
//!
//! ## What this prompt (GIT-P19 / P-280) ships
//! 1. [`LifecycleRel`] — the two lifecycle rels Git's PR lifecycle PRODUCES (`closes`/`relates`), a
//!    SUBSET of the frozen §3.3 / contract-5.5 vocabulary (`closes/blocks/blocked_by/depends_on/parent/
//!    assigns/relates`). Git mints only `closes` (trailer) + `relates` (PR-link); the other lifecycle
//!    rels are Issues'/Knowledge's typed tables (REF-P18/REF-P20). The token strings are byte-identical
//!    to `myelin_refs_service::mirror::LifecycleRel::{Closes,Relates}` (the consumer vocabulary).
//! 2. [`parse_closes_trailers`] — extract the `Closes <ISSUEKEY>` trailers from a commit/PR message by
//!    matching the STRUCTURED trailer grammar (a trailing `Closes:`/`Closes ` line), NEVER a loose
//!    regex over the prose body (the reliability guarantee, EI-04 §2.4 — a `Closes` written mid-sentence
//!    is NOT a trailer; only a recognised trailer line on the merged PR is the lifecycle edge).
//! 3. [`extract_lifecycle_edges`] — given a merged PR's source URN + its `Closes` issue targets + its
//!    explicit PR-link targets, produce EXACTLY ONE lifecycle edge per linkage (a `closes` per trailer,
//!    a `relates` per PR-link; 0 duplicate, 0 missed). The target of a `closes` is the issue URN; the
//!    target of a `relates` is the linked artifact URN.
//! 4. [`emit_lifecycle_edges`] — emit one `refs.edge.created` (`rel_class='lifecycle'`) per extracted
//!    lifecycle edge **in the SAME outbox transaction** as the PR's `git.pr.merged` / `git.pr.updated`
//!    lifecycle event (emit-iff-committed — no lifecycle edge without its committed lifecycle
//!    transition). The emitted event is the byte-identical shape the Refs mirror consumer
//!    ([`myelin_refs_service::mirror::project_typed_event`]) ingests — the SAME
//!    `source`/`target`/`rel`/`rel_class` payload + the SAME `edge:<source>-><target>` aggregate.
//!
//! ## Why a Git-OWNED producer half (EI-01 §7 — extend/reconcile, never duplicate)
//! The canonical typed-edge mirror DISCIPLINE (the vocabulary, the inverse pairing, the both-directions
//! projection, drift reconvergence) already exists in the Refs SERVICE crate
//! (`myelin_refs_service::mirror`, REF-P14 / P-163). But **Git is a producer LEAF and CANNOT depend on
//! the Refs SERVICE crate** (the §2.9 acyclic DAG — the SAME constraint that made
//! [`crate::lifecycle::CodeOwners::resolve`] (4.9) and [`crate::body::extract_body_edges`] (5.4) the
//! "Git-owned half"). So this module is the **Git-owned producer half** of contract 5.5: it produces
//! the **byte-identical** `refs.edge.created` lifecycle event the Refs mirror consumes — the SAME `rel`
//! tokens (`closes`/`relates`), the SAME `rel_class='lifecycle'`, the SAME `edge:<source>-><target>`
//! aggregate (EB-03 per-aggregate ordering). The **Refs mirror is the source of the inverse pairing**:
//! Git emits ONLY the forward lifecycle event (`closes` PR→issue, `relates` PR→target); the Refs
//! consumer ([`mirror_edges`]) projects the forward edge AND its inverse (for `relates`, the symmetric
//! swap; `closes` has no frozen inverse token yet — REF-P18/REF-P20 floor). Git NEVER projects the
//! inverse itself (REF-3 — never invent a token; the typed table / producer emits forward, Refs
//! mirrors). The encoding equivalence with the Refs consumer is PINNED by the CDC
//! (`tests/cdc_5_5_git_lifecycle_edges.rs`): a drift on either side fails the same CI job.
//!
//! ## Named floors (VISION §3 / EI-01 §1)
//! - **The inverse projection is the Refs mirror's, not Git's.** Git emits the FORWARD lifecycle event
//!   only; the `closes`-inverse token (`closed_by`-class) is the §3.3 [`Inverse::None`] FLOOR
//!   (REF-P18/REF-P20 mint it). Named so the forward-only producer is not mistaken for the full
//!   dual-homed mirror — the BOTH-directions projection lands in the Refs consumer.
//! - **`blocks`/`depends_on`/`parent`/`assigns` are NOT Git-produced.** Those lifecycle rels live in the
//!   Issues `issue_relation` / Knowledge `db_relation` typed tables (REF-P18/REF-P20); Git's PR
//!   lifecycle produces only `closes` (trailer) + `relates` (PR-link). [`LifecycleRel`] is the Git
//!   SUBSET, not the whole §3.3 vocabulary. Named so this is not mistaken for the complete mirror.
//! - **The live PR-merge / PR-link transition wiring is GIT-P20/GIT-P22.** This module is the
//!   **extraction + emit seam** (the trailer parse + the edge derivation + the same-tx outbox emit),
//!   unit- and e2e-tested against the in-memory PR; the live OLTP store that fires it on a real
//!   `git.pr.merged` rides the merge-gate wiring (GIT-P20). Named so the seam is not mistaken for the
//!   live store.
//! - **Mutation floor (mandatory-core) — MEASURED ≥ 90%, met at 94%.** The typed-edge path — the
//!   per-rel `closes`/`relates` token mapping, the structured trailer parse (line-leading + delimited +
//!   keyless-reject + de-dup), the one-edge-per-linkage invariant (0 edges for a plain merge), the
//!   same-tx outbox emit shape, and the lifecycle `rel_class` discipline — is the mutation-tested core.
//!   `cargo mutants -p myelin-git --file crates/myelin-git/src/typed_edges.rs` finds **18 viable
//!   mutants, 17 caught (94%)**. The SOLE survivor is `len() < KW.len()` → `<=` in
//!   [`strip_closes_keyword`], a **provable EQUIVALENT mutant**: the guard only differs when
//!   `line.len() == 6` (exactly `"closes"`), and a 6-char line has no room for an issue key, so it
//!   yields zero edges under BOTH `<` and `<=` (and `split_at(6)` on a 6-char string never panics) — no
//!   observation can distinguish them (a cargo-mutants false positive, not a test gap). A mutant that
//!   mis-maps a rel, accepts a mid-sentence `closes`, admits a keyless trailer, drops the de-dup, emits
//!   outside the transaction, or mislabels the class is caught. The world-scale corpus-under-load drill
//!   is a later band.

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType, OutboxTx,
    Result as BusResult, Visibility,
};

/// **The frozen `refs.edge.created` event type (contract 5.4/5.5 — the emit-side token).** The ONLY
/// edge-creation event a lifecycle producer emits (the SAME event a content-node producer emits — the
/// `rel_class` field distinguishes lifecycle from reference). A named constant so drills assert against
/// the NAME, never a literal (EI-01 §3). Byte-identical to [`crate::body::REFS_EDGE_CREATED`] and
/// `myelin_refs_service::emit::REFS_EDGE_CREATED`.
pub const REFS_EDGE_CREATED: &str = "refs.edge.created";

/// **The frozen `rel_class` token a TYPED-EDGE (lifecycle) edge carries (contract 5.5 / Refs §3.2).** A
/// lifecycle mirror edge is ALWAYS `lifecycle` (NEVER `reference` — the two classes never alias; a
/// content-node edge is [`crate::body::REL_CLASS_REFERENCE`]). A `&'static str` constant so the drills
/// assert against the token, never a literal — and it is the byte-identical token the Refs mirror
/// stamps (`RelClass::Lifecycle.as_str()`).
pub const REL_CLASS_LIFECYCLE: &str = "lifecycle";

/// **The lifecycle relations Git's PR lifecycle PRODUCES** — a SUBSET of the frozen §3.3 / contract-5.5
/// vocabulary (`closes/blocks/blocked_by/depends_on/parent/assigns/relates`). Git mints exactly two:
/// `closes` (a `Closes <ISSUEKEY>` commit-trailer on a merged PR) and `relates` (an explicit PR-link).
/// The other lifecycle rels are the Issues/Knowledge typed tables' (REF-P18/REF-P20). The token strings
/// are byte-identical to `myelin_refs_service::mirror::LifecycleRel::{Closes,Relates}.as_str()` (the
/// CDC pins the equivalence) — Git produces the SAME wire tokens the Refs mirror ingests, it does not
/// author a second vocabulary. PII-free token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LifecycleRel {
    /// `closes` — a `Closes <ISSUEKEY>` trailer on a MERGED PR closes the issue (PR → issue,
    /// directional; the §3.3 inverse token is the [`Inverse::None`] floor — REF-P18/REF-P20 mint it).
    Closes,
    /// `relates` — an explicit PR-link (this PR relates to another artifact). SYMMETRIC: the Refs mirror
    /// projects the inverse as the same `relates` rel with the endpoints swapped (visible from both
    /// ends). Git emits only the forward; Refs mirrors the symmetric swap.
    Relates,
}

impl LifecycleRel {
    /// The frozen `rel` column token (`'closes' | 'relates'`, §3.2/§3.3 vocabulary). PII-free.
    /// Byte-identical to `myelin_refs_service::mirror::LifecycleRel::{Closes,Relates}.as_str()`.
    pub fn as_str(self) -> &'static str {
        match self {
            LifecycleRel::Closes => "closes",
            LifecycleRel::Relates => "relates",
        }
    }
}

/// **One extracted lifecycle edge** from a PR transition — the `(source, target, rel)` triple
/// (`rel_class = lifecycle`, always). The deterministic `edge_id = hash(tenant, source, target, rel)`
/// is the CONSUMER's (the Refs mirror derives it from the payload triple); here the producer ships the
/// triple. PII-free: `source`/`target` are opaque `ArtifactRef` URNs (the PR URN + the issue/linked
/// artifact URN). The inverse direction is the Refs mirror's (Git emits forward only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleEdge {
    /// The referencing side — the Git PR URN the lifecycle transition fired on (the `closes`/`relates`
    /// edge departs FROM the PR). The same for every edge in one transition.
    pub source: ArtifactRef,
    /// The referenced side — the artifact the linkage points at (a `closes`'s issue URN, or a
    /// `relates`'s linked artifact URN).
    pub target: ArtifactRef,
    /// The lifecycle relation this linkage produces (`closes`/`relates`).
    pub rel: LifecycleRel,
}

/// **The shared edge-aggregate-key convention `edge:<source>-><target>` (EB-03 ordering anchor).** Every
/// `refs.edge.*` event for ONE logical edge shares this aggregate, so an edge's create → remove → create
/// sequence is per-aggregate ordered (gap-free, in commit order). Byte-identical to
/// [`crate::body::edge_aggregate_key`] and `myelin_refs_service::emit::edge_aggregate_key` — Git's
/// lifecycle edges share the SAME ordering aggregate the content-node edges + the Refs mirror use (one
/// ordering key across producers). PII-free.
pub fn edge_aggregate_key(source: &ArtifactRef, target: &ArtifactRef) -> AggregateKey {
    AggregateKey(format!("edge:{}->{}", source.0, target.0))
}

/// **Parse the `Closes <ISSUEKEY>` trailers from a commit/PR message (STRUCTURED, not a loose regex).**
///
/// A trailer is a recognised key-value line — `Closes <ISSUEKEY>` or `Closes: <ISSUEKEY>` — at the
/// START of a line (the conventional git trailer position is the message FOOTER, but accepting any
/// line-leading trailer keeps the parse robust to multi-line bodies; a `Closes` written MID-sentence is
/// NOT a trailer and yields no edge — the reliability guarantee). Case-insensitive on the `Closes`
/// keyword. Multiple issue keys on one trailer line (comma- or whitespace-separated) each yield a key.
/// Returns the issue keys verbatim (e.g. `ENG-1`); the caller composes the issue URN
/// (`myelin://<tenant>/issue/issue/<ISSUEKEY>`).
///
/// A `Closes` with no following key is NOT a trailer (no silent empty edge). Duplicate keys across
/// lines are de-duplicated (a PR that says `Closes ENG-1` twice closes ENG-1 ONCE — 0 duplicate edges).
pub fn parse_closes_trailers(message: &str) -> Result<Vec<String>, TrailerParseError> {
    const MAX_MESSAGE_BYTES: usize = 64 * 1024;
    const MAX_KEYS: usize = 100;
    const MAX_KEY_BYTES: usize = 256;
    const MAX_TOTAL_KEY_BYTES: usize = 8 * 1024;
    if message.len() > MAX_MESSAGE_BYTES {
        return Err(TrailerParseError::LimitExceeded("message bytes"));
    }
    let mut keys: Vec<String> = Vec::new();
    let mut total_key_bytes = 0usize;
    for raw in message.lines() {
        let line = raw.trim();
        // Match a line-leading `Closes` (case-insensitive) followed by `:` or whitespace.
        let rest = match strip_closes_keyword(line) {
            Some(rest) => rest,
            None => continue,
        };
        // The remainder is the issue key(s): comma- or whitespace-separated. Each token is one key.
        for tok in rest.split([',', ' ', '\t']) {
            let key = tok.trim();
            if key.is_empty() {
                continue;
            }
            if !keys.iter().any(|k| k == key) {
                if key.len() > MAX_KEY_BYTES {
                    return Err(TrailerParseError::LimitExceeded("issue key bytes"));
                }
                if keys.len() >= MAX_KEYS {
                    return Err(TrailerParseError::LimitExceeded("issue key count"));
                }
                total_key_bytes = total_key_bytes
                    .checked_add(key.len())
                    .ok_or(TrailerParseError::LimitExceeded("total issue key bytes"))?;
                if total_key_bytes > MAX_TOTAL_KEY_BYTES {
                    return Err(TrailerParseError::LimitExceeded("total issue key bytes"));
                }
                keys.push(key.to_string());
            }
        }
    }
    Ok(keys)
}

/// A structured lifecycle trailer was refused before edge-key allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrailerParseError {
    /// A bounded message, key-count, single-key, or aggregate-key ceiling was exceeded.
    LimitExceeded(&'static str),
}

impl std::fmt::Display for TrailerParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LimitExceeded(kind) => write!(f, "Closes trailer {kind} limit exceeded"),
        }
    }
}

impl std::error::Error for TrailerParseError {}

/// Strip a line-leading `Closes` trailer keyword (case-insensitive), with an optional `:` and the
/// mandatory following whitespace, returning the remainder (the issue-key portion). Returns `None` if
/// the line does not START with the `Closes` keyword as a trailer (a mid-sentence `Closes` is not a
/// trailer). The keyword must be followed by `:` or whitespace — `Closesomething` is NOT a trailer.
fn strip_closes_keyword(line: &str) -> Option<&str> {
    const KW: &str = "closes";
    if line.len() < KW.len() {
        return None;
    }
    let (head, rest) = line.split_at(KW.len());
    if !head.eq_ignore_ascii_case(KW) {
        return None;
    }
    // The keyword must be delimited (followed by `:` or whitespace) — `Closesthebug` is NOT a trailer.
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    let trimmed = rest.trim_start();
    // There must have been at least one delimiter (a `:` we stripped, or leading whitespace we
    // trimmed). If `rest` is non-empty and we trimmed nothing and stripped no `:`, the keyword ran into
    // the next token (`Closesthebug`) — not a trailer.
    if rest.len() == trimmed.len() && !rest.is_empty() {
        // No `:` was stripped (rest still starts at the byte after `Closes`) and no whitespace trimmed.
        return None;
    }
    Some(trimmed)
}

/// **Extract one lifecycle edge per linkage on a merged PR (the contract-5.5 producer; structured, NOT
/// regex).**
///
/// Given the PR's own URN (`source` — the `git/pr/<repo>:<n>` root the edges depart from), the `closes`
/// issue TARGET URNs (each parsed from a `Closes <ISSUEKEY>` trailer via [`parse_closes_trailers`] +
/// the caller's issue-URN compose), and the explicit PR-link TARGET URNs, produce exactly one
/// [`LifecycleEdge`] per linkage:
///
/// - each `closes_targets` entry → `(source, target, closes)`;
/// - each `relates_targets` entry → `(source, target, relates)`.
///
/// A merged PR with **no** trailers and **no** PR-links yields **zero** lifecycle edges (the no-op
/// case — most PRs). N trailers + M links → N+M edges, in order (closes first, then relates). Git emits
/// the FORWARD edge only; the Refs mirror projects the inverse (the symmetric `relates` swap; `closes`
/// has the [`Inverse::None`] floor). This is the SUBSET of the §3.3 mirror discipline Git owns.
pub fn extract_lifecycle_edges(
    source: &ArtifactRef,
    closes_targets: &[ArtifactRef],
    relates_targets: &[ArtifactRef],
) -> Vec<LifecycleEdge> {
    let mut edges = Vec::with_capacity(closes_targets.len() + relates_targets.len());
    for target in closes_targets {
        edges.push(LifecycleEdge {
            source: source.clone(),
            target: target.clone(),
            rel: LifecycleRel::Closes,
        });
    }
    for target in relates_targets {
        edges.push(LifecycleEdge {
            source: source.clone(),
            target: target.clone(),
            rel: LifecycleRel::Relates,
        });
    }
    edges
}

/// Build the canonical `refs.edge.created` [`EventDraft`] for one extracted [`LifecycleEdge`].
///
/// The references-not-payloads payload carries `source`/`target`/`rel`/`rel_class` (the Refs mirror
/// reads exactly these; the deterministic `edge_id` is derived from `tenant + source + target + rel`,
/// so the producer ships the triple, not the id). The aggregate is the `edge:<source>-><target>`
/// identity — the SAME convention the content-node producer + the Refs mirror use — so per-aggregate
/// ordering (EB-03) holds for a lifecycle edge's create/remove sequence. `rel_class='lifecycle'` (NEVER
/// `reference`). `contains_personal_data = false`: every field is an opaque ref/token (the PR URN + the
/// issue/linked URN), so no inline-PII envelope key is needed (references-not-payloads, contract 2.7).
fn edge_event_draft(edge: &LifecycleEdge) -> EventDraft {
    EventDraft {
        type_: EventType(REFS_EDGE_CREATED.into()),
        // The referencing side (the PR the lifecycle transition fired on) is the event subject.
        subject: edge.source.clone(),
        aggregate: edge_aggregate_key(&edge.source, &edge.target),
        payload: serde_json::json!({
            "source": edge.source.0,
            "target": edge.target.0,
            "rel": edge.rel.as_str(),
            // THE typed-edge discipline: a lifecycle mirror edge is ALWAYS lifecycle-class (§3.2/§3.3).
            "rel_class": REL_CLASS_LIFECYCLE,
        }),
        // Refs is the CONTROLLER of the edge fact it authors (the reference graph is Refs-owned) — the
        // SAME role the content-node + the Refs mirror producer stamp.
        data_role: DataRole::Controller,
        // A derived index event's default visibility is Internal (a routing hint, never an authz
        // decision — Identity decides at resolve-time).
        visibility: Visibility::Internal,
        // References-not-payloads: opaque refs only, no inline PII, so no envelope key.
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// **Emit one `refs.edge.created` (`rel_class='lifecycle'`) per lifecycle linkage, IN THE SAME
/// TRANSACTION as the PR's lifecycle event (the contract-5.5 producer seam — Git-owned half).**
///
/// `tx` is the OPEN outbox transaction the caller is writing the PR's `git.pr.merged` / `git.pr.updated`
/// lifecycle event into (the PR state change + the lifecycle event are staged in `tx`); `lifecycle_event`
/// is that lifecycle event (the CAUSE). For each extracted [`LifecycleEdge`], this calls
/// [`OutboxTx::emit`]`(draft, cause = Some(lifecycle_event))` — the ONE sanctioned emit verb (contract
/// 2.2; the `no-raw-publish` lint, P-019). There is **NO standalone edge-write API** — the lifecycle
/// edges are emitted from the lifecycle transition only. Returns the minted [`EventId`]s (closes first,
/// then relates).
///
/// **Causality correct-by-construction (P-S06):** because `cause = Some(lifecycle_event)`, the envelope
/// derivation sets `correlation_id = lifecycle_event.correlation_id` (the root carries), `causation_id =
/// lifecycle_event.event_id`, and `depth = lifecycle_event.depth + 1` (the loop-guard stamp). The caller
/// CANNOT typo a wrong parent: the causal triple is not on [`EventDraft`].
///
/// **Emit-iff-committed (the silent-data-loss floor, GIT-D9-class):** `emit` BUFFERS the row into `tx`;
/// it becomes durable iff the caller commits `tx`. An aborted PR-merge drops the buffered lifecycle-edge
/// rows with it — **no lifecycle edge without its committed transition** (the PR transition + the edge
/// events co-commit). This function performs NO commit (the caller owns the transaction lifecycle — the
/// SAME discipline [`crate::body::emit_body_edges`] + [`crate::receive_pack`] use).
pub fn emit_lifecycle_edges(
    tx: &mut dyn OutboxTx,
    source: &ArtifactRef,
    closes_targets: &[ArtifactRef],
    relates_targets: &[ArtifactRef],
    lifecycle_event: &EventEnvelope,
) -> BusResult<Vec<EventId>> {
    let edges = extract_lifecycle_edges(source, closes_targets, relates_targets);
    let mut ids = Vec::with_capacity(edges.len());
    for edge in &edges {
        // The ONE sanctioned emit path (contract 2.2; no-raw-publish). `cause = Some(lifecycle_event)` →
        // the correlation root carries + causation = the lifecycle event + depth+1. The row is BUFFERED
        // into `tx` — durable iff the caller commits (the PR transition + these edges co-commit).
        let id = tx.emit(edge_event_draft(edge), Some(lifecycle_event))?;
        ids.push(id);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr_source() -> ArtifactRef {
        crate::project::git_pr_ref("acme", "repo7", 42)
    }

    fn issue(key: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://acme/issue/issue/{key}"))
    }

    // ── 1. trailer parse: structured, not a loose regex over prose ─────────────────────────────────

    /// **A `Closes <ISSUEKEY>` trailer line is parsed; a mid-sentence `Closes` is NOT.** The structured
    /// trailer grammar (a line-leading `Closes`) — a `Closes` written in the prose body yields no key.
    #[test]
    fn closes_trailer_is_line_leading_not_mid_sentence() {
        let msg = "Fix the charge bug\n\nThis closes a long-standing race.\nCloses ENG-1\n";
        let keys = parse_closes_trailers(msg).unwrap();
        assert_eq!(
            keys,
            vec!["ENG-1".to_string()],
            "only the trailer line, not the prose `closes`"
        );
    }

    /// The `Closes:` colon form + case-insensitivity + multiple keys on one line + de-duplication.
    #[test]
    fn closes_trailer_colon_caseless_multikey_dedup() {
        let msg = "title\n\ncloses: ENG-1, ENG-2\nCLOSES ENG-2\nCloses ENG-3\n";
        let keys = parse_closes_trailers(msg).unwrap();
        assert_eq!(
            keys,
            vec![
                "ENG-1".to_string(),
                "ENG-2".to_string(),
                "ENG-3".to_string()
            ],
            "colon/caseless/multikey parsed; the duplicate ENG-2 is de-duplicated (0 dup)"
        );
    }

    /// **A bare `Closes` with no key is NOT a trailer (no silent empty edge); `Closesthebug` is not a
    /// keyword.** The keyword must be delimited and followed by at least one key.
    #[test]
    fn closes_without_key_or_undelimited_is_not_a_trailer() {
        assert!(
            parse_closes_trailers("Closes\n").unwrap().is_empty(),
            "a bare `Closes` yields no key"
        );
        assert!(
            parse_closes_trailers("Closes   \n").unwrap().is_empty(),
            "`Closes` + whitespace only, no key"
        );
        assert!(
            parse_closes_trailers("Closesthebug now\n")
                .unwrap()
                .is_empty(),
            "an undelimited `Closesthebug` is NOT the trailer keyword"
        );
    }

    #[test]
    fn closes_trailer_parser_bounds_message_keys_and_key_bytes() {
        let exact_keys = (0..100)
            .map(|index| format!("Closes ENG-{index}\n"))
            .collect::<String>();
        assert_eq!(parse_closes_trailers(&exact_keys).unwrap().len(), 100);
        assert!(parse_closes_trailers(&(exact_keys + "Closes ENG-100\n")).is_err());

        let exact_key = "x".repeat(256);
        assert_eq!(
            parse_closes_trailers(&format!("Closes {exact_key}"))
                .unwrap()
                .len(),
            1
        );
        assert!(parse_closes_trailers(&format!("Closes {}", "x".repeat(257))).is_err());
        assert!(parse_closes_trailers(&"x".repeat(64 * 1024 + 1)).is_err());

        let aggregate_over = (0..33)
            .map(|index| format!("Closes {index:03}{}\n", "x".repeat(253)))
            .collect::<String>();
        assert!(parse_closes_trailers(&aggregate_over).is_err());
    }

    // ── 2. extraction: one edge per linkage, correct rel/target ────────────────────────────────────

    /// **Each trailer → exactly one `closes` edge (PR→issue); each PR-link → exactly one `relates`
    /// edge.** N trailers + M links → N+M edges, closes first. 0 duplicate, 0 missed.
    #[test]
    fn each_linkage_yields_one_lifecycle_edge_with_correct_rel_and_target() {
        let src = pr_source();
        let closes = vec![issue("ENG-1"), issue("ENG-2")];
        let relates = vec![crate::project::git_pr_ref("acme", "repo7", 7)];
        let edges = extract_lifecycle_edges(&src, &closes, &relates);
        assert_eq!(
            edges.len(),
            3,
            "2 trailers + 1 PR-link → exactly 3 lifecycle edges"
        );

        // the closes edges come first, PR→issue, rel=closes.
        assert_eq!(edges[0].rel, LifecycleRel::Closes);
        assert_eq!(edges[0].rel.as_str(), "closes");
        assert_eq!(edges[0].source, src);
        assert_eq!(edges[0].target, issue("ENG-1"));
        assert_eq!(edges[1].rel, LifecycleRel::Closes);
        assert_eq!(edges[1].target, issue("ENG-2"));

        // the relates edge, PR→linked PR, rel=relates.
        assert_eq!(edges[2].rel, LifecycleRel::Relates);
        assert_eq!(edges[2].rel.as_str(), "relates");
        assert_eq!(edges[2].source, src);
        assert_eq!(
            edges[2].target,
            crate::project::git_pr_ref("acme", "repo7", 7)
        );
    }

    /// **A merged PR with no trailers and no PR-links yields ZERO lifecycle edges** (the no-op case —
    /// most PRs). The lifecycle producer is silent on a plain merge.
    #[test]
    fn merged_pr_without_linkage_yields_zero_edges() {
        let edges = extract_lifecycle_edges(&pr_source(), &[], &[]);
        assert!(
            edges.is_empty(),
            "a plain merge with no trailer/link produces 0 lifecycle edges"
        );
    }

    /// **The edge event draft is `refs.edge.created` with the references-not-payloads triple + the shared
    /// `edge:<source>-><target>` aggregate + `rel_class = lifecycle` (NOT reference).** This is the
    /// byte-identical shape the Refs mirror ingests (CDC-pinned). `contains_personal_data = false`.
    #[test]
    fn edge_event_draft_is_refs_edge_created_lifecycle_class() {
        let src = pr_source();
        let target = issue("ENG-1");
        let edge = LifecycleEdge {
            source: src.clone(),
            target: target.clone(),
            rel: LifecycleRel::Closes,
        };
        let draft = edge_event_draft(&edge);
        assert_eq!(draft.type_.0, "refs.edge.created");
        assert_eq!(draft.subject, src, "the subject is the referencing PR");
        assert_eq!(draft.payload["source"], src.0);
        assert_eq!(draft.payload["target"], target.0);
        assert_eq!(draft.payload["rel"], "closes");
        assert_eq!(
            draft.payload["rel_class"], "lifecycle",
            "a lifecycle mirror edge is lifecycle-class"
        );
        assert_eq!(draft.aggregate.0, format!("edge:{}->{}", src.0, target.0));
        assert!(
            !draft.contains_personal_data,
            "references-not-payloads: no inline PII"
        );
        assert!(draft.pii_key_ref.is_none());
        assert_eq!(draft.data_role, DataRole::Controller);
    }

    /// The frozen tokens are exactly the Refs mirror wire tokens (the names anchor X-5; no second
    /// vocabulary). `relates` is symmetric, `closes` directional — the Refs mirror owns the inverse.
    #[test]
    fn frozen_tokens_match_the_refs_mirror_wire_shape() {
        assert_eq!(REFS_EDGE_CREATED, "refs.edge.created");
        assert_eq!(REL_CLASS_LIFECYCLE, "lifecycle");
        assert_eq!(LifecycleRel::Closes.as_str(), "closes");
        assert_eq!(LifecycleRel::Relates.as_str(), "relates");
        // the lifecycle class is NEVER the reference class (the two never alias).
        assert_ne!(REL_CLASS_LIFECYCLE, crate::body::REL_CLASS_REFERENCE);
    }

    /// **The lifecycle edge aggregate is the SAME `edge:<source>-><target>` convention the content-node
    /// producer uses** (one ordering key across producers — EB-03). A lifecycle edge and a content edge
    /// for the SAME (source,target) share an aggregate, so their create/remove sequence is ordered.
    #[test]
    fn lifecycle_edge_shares_the_content_edge_aggregate_convention() {
        let src = pr_source();
        let target = issue("ENG-1");
        assert_eq!(
            edge_aggregate_key(&src, &target),
            crate::body::edge_aggregate_key(&src, &target),
            "the lifecycle + content producers share ONE edge-aggregate convention"
        );
    }
}
