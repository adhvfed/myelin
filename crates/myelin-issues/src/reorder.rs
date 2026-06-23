//! # `reorder` — the server-arbitrated `order_key` CAS reorder (ISS-P09 / P-375, M4-I1)
//!
//! **The silent-clobber floor this module ships (EI-01 §2 / VISION §3 — name-your-floors):** a
//! drag-to-reorder is a compare-and-set on the moved issue's *last-seen* `order_key`+version. The
//! server bisects a new [`OrderKey`] (the frozen contract-13.3 codec — midpoint bisection + 2-char
//! jitter + 48-char rebalance trigger) and writes it under a **server-arbitrated CAS on the prior
//! version**. On a precondition miss the **LOSER is rejected and re-bases honestly** against the
//! current authoritative order — there is **no silent overwrite, no merge**. Two concurrent moves to
//! the same gap → one wins the CAS, the loser is returned the authoritative order and re-bases (the
//! CAS floor, KN-1 / EI-04 §2). Humans and agents share this ONE mechanism (server-arbitrated, not
//! client-trust — arch §5, agent parity).
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md`
//! §5 (the frozen `order_key` LexoRank + server-arbitrated CAS — the `reorder(...)` sketch: bisect
//! the new rank, `UPDATE … WHERE version = expected_version RETURNING`, `n == 0 ⇒ Conflict{...}` the
//! loser re-bases, then `OutboxTx::emit(issue.reordered, cause)`). The 48-char rebalance re-spaces
//! the collection's keys but **never reorders the displayed order** (§5 "Rebalance").
//!
//! **Contract-index rows (consumed here — built to the FROZEN shapes, never diverged):**
//! - **13.3 (executed)** — the [`OrderKey`]/LexoRank codec is the SHARED, byte-identical crate
//!   ([`myelin_query::field::OrderKey`], co-owned with Knowledge). This module does NOT re-define the
//!   encoding (EI-01 §7 — never a second definition); it CALLS [`OrderKey::rank_between`] (midpoint +
//!   2-char jitter) for a new rank, [`OrderKey::needs_rebalance`] for the 48-char trigger, and the
//!   [`myelin_query::order_key::tiebreak`] (`created_at`+ULID) for the equal-key total order. The
//!   ranking ENGINE (the CAS arbitration) is the new thing ISS-P09 builds on top of the frozen codec.
//! - **2.2 (consumed)** — the reorder write **co-commits its `issue.reordered` event** through the
//!   ONE sanctioned emit verb [`OutboxTx::emit`] on the SAME [`OutboxTransaction`] (the
//!   `no-raw-publish` lint holds — emit is the only path). Emit-iff-committed: a CAS that loses (or a
//!   rebalance that aborts) commits neither the rank nor the event (0 ghost / 0 lost). The aggregate
//!   is the issue (per-issue ordering, contract 2.3).
//!
//! ## FLOOR named (VISION §3) — ranking = `order_key` + server-arbitrated CAS; the move-CRDT follows
//! The conflict engine here is the **server-arbitrated CAS** (the loser re-bases). The **move-CRDT
//! (Yrs list / Fugue)** is the named M5 follow-on (**ISS-P32**), promoted only on *measured*
//! concurrent-reorder pain — and it **reuses the byte-identical [`OrderKey`]**: the promotion swaps
//! the conflict-resolution engine, NOT the data model (arch §5 "Floor → follow-on"). So there is no
//! deferred-and-unbuilt encoding here; the CRDT lands *over* this order model.
//!
//! ## The board store is the CAS-guarded state (a thin in-memory model of the live OLTP row)
//! [`BoardRanking`] is a per-issue `(order_key, version, created_at, ulid)` map keyed by the issue's
//! stored canonical id. In the live OLTP binding the CAS is the architecture's
//! `UPDATE issue SET rank = …, version = version + 1 WHERE id = ? AND version = ? RETURNING …` (the
//! `n == 0` precondition-miss is the loser); the in-memory store models exactly that conditional
//! update under its own lock. The same-tx co-commit MECHANISM is the shared
//! [`myelin_events::outbox`] (never re-implemented here).

use crate::events;
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventId, EventType, IdMinter,
    OutboxStore, OutboxTx, Visibility,
};
use myelin_query::field::{Jitter, OrderKey};
use myelin_query::order_key::tiebreak;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use crate::write_path::{issue_aggregate_key, issue_ref};

// ===========================================================================
// §1 — the CAS-guarded board ranking row + store
// ===========================================================================

/// One issue's ranking row — the state the server-arbitrated CAS guards. `order_key` is the frozen
/// LexoRank rank (contract 13.3); `version` is the optimistic-concurrency token the CAS asserts on
/// (`+1` per accepted reorder); `created_at`+`ulid` are the deterministic `tiebreak` total-order
/// secondaries for the (should-not-happen-with-jitter) equal-key case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankedIssue {
    /// The issue's stored canonical id (`<PROJECTKEY>-<seqno>`, ISS-P08) — the CAS object + the
    /// per-issue aggregate key segment.
    pub issue_id: String,
    /// The frozen LexoRank `order_key` (contract 13.3) — byte order IS sort order.
    pub order_key: OrderKey,
    /// The optimistic-concurrency version the CAS asserts on. A reorder carries the *last-seen*
    /// version; the server accepts iff it still matches, then bumps it by one. This is the
    /// silent-clobber guard — a stale version loses and re-bases.
    pub version: u64,
    /// The RFC-3339/ISO-8601 lexicographically-sortable creation timestamp (the first `tiebreak`
    /// secondary when two rows somehow share an `order_key`).
    pub created_at: String,
    /// The ULID id (the final `tiebreak` secondary — distinct per row, so the total order is
    /// decisive).
    pub ulid: String,
}

/// **The server-side board ranking store — the CAS-guarded state.** A per-issue
/// `(order_key, version, …)` map keyed by the issue's stored canonical id. In the live OLTP binding
/// this is the `issue` row's `rank`/`version` columns; the in-memory model performs exactly the
/// architecture's conditional `UPDATE … WHERE version = expected RETURNING` under its own lock.
///
/// This is NOT a second event store — the same-tx co-commit of the `issue.reordered` event runs
/// through the shared [`OutboxStore`] (EI-01 §7 — one outbox, never two). This store holds only the
/// ranking row the CAS arbitrates.
#[derive(Clone, Debug, Default)]
pub struct BoardRanking {
    /// issue_id → its ranking row. A bisecting reorder reads the neighbours from here; the CAS
    /// asserts + bumps the moved issue's version here, atomically with the outbox co-commit.
    rows: HashMap<String, RankedIssue>,
}

impl BoardRanking {
    /// A fresh, empty board.
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed (or replace) an issue's ranking row. Used at issue-create time (the first rank is
    /// [`OrderKey::rank_last`] after the current tail) and by tests/fixtures to set up a board.
    /// The version starts at 0 for a freshly-ranked issue.
    pub fn upsert(&mut self, row: RankedIssue) {
        self.rows.insert(row.issue_id.clone(), row);
    }

    /// The current ranking row for an issue, if present.
    pub fn get(&self, issue_id: &str) -> Option<&RankedIssue> {
        self.rows.get(issue_id)
    }

    /// The number of ranked issues on the board.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the board is empty.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// **The authoritative displayed order** — every ranked issue, sorted by the contract-13.3 total
    /// order (`order_key`, then `created_at`, then ULID). This is the order a CAS loser re-bases
    /// against and the order a rebalance MUST preserve. The byte order of the `order_key` IS the
    /// sort order; the `tiebreak` is the secondary for the equal-key case (jitter makes it rare, but
    /// the order is still total).
    pub fn displayed_order(&self) -> Vec<RankedIssue> {
        let mut all: Vec<RankedIssue> = self.rows.values().cloned().collect();
        all.sort_by(|a, b| {
            tiebreak(
                &a.order_key,
                &a.created_at,
                &a.ulid,
                &b.order_key,
                &b.created_at,
                &b.ulid,
            )
        });
        all
    }

    /// The neighbour `order_key`s of a position in the *current* displayed order: the rank
    /// immediately before `before_id` and immediately after `after_id` (either `None` ⇒ the
    /// list edge). This is the gap a reorder bisects into — read from the CURRENT authoritative
    /// state, so a re-basing loser always bisects against fresh neighbours.
    fn gap_for(
        &self,
        before_id: Option<&str>,
        after_id: Option<&str>,
    ) -> (Option<OrderKey>, Option<OrderKey>) {
        let lo = before_id
            .and_then(|id| self.rows.get(id))
            .map(|r| r.order_key.clone());
        let hi = after_id
            .and_then(|id| self.rows.get(id))
            .map(|r| r.order_key.clone());
        (lo, hi)
    }
}

// ===========================================================================
// §2 — the reorder request + outcome (the server-arbitrated CAS surface)
// ===========================================================================

/// A drag-to-reorder request: move `issue_id` into the gap between `before_id` and `after_id`,
/// asserting the issue's *last-seen* `expected_version`. `before_id`/`after_id` are the issue ids
/// the client dragged the moved issue between (either `None` ⇒ the list edge — drag to the very
/// front/back). The CAS is the silent-clobber guard: the move is accepted iff `expected_version`
/// still matches the server's row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReorderRequest {
    /// The issue being moved (the CAS object).
    pub issue_id: String,
    /// The issue immediately BEFORE the target slot in the client's view (`None` ⇒ move to front).
    pub before_id: Option<String>,
    /// The issue immediately AFTER the target slot in the client's view (`None` ⇒ move to back).
    pub after_id: Option<String>,
    /// The moved issue's last-seen `version` (the optimistic token the server CAS asserts on).
    pub expected_version: u64,
    /// The 2-char concurrency-safety jitter for the bisected rank (contract 13.3 — an EXPLICIT value,
    /// drawn from the caller's RNG via [`Jitter::random`], so two concurrent same-gap drags produce
    /// DISTINCT keys). Tests supply a fixed jitter for determinism.
    pub jitter: Jitter,
}

/// The outcome of a server-arbitrated reorder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReorderOutcome {
    /// The CAS WON: the new rank was written, the version bumped, and the `issue.reordered` event
    /// co-committed. Carries the new ranking row + the minted event id.
    Applied {
        /// The moved issue's new ranking row (the bisected `order_key`, version `expected + 1`).
        ranked: RankedIssue,
        /// The minted `event_id` of the co-committed `issue.reordered` event (contract 2.2).
        event_id: EventId,
        /// `true` iff the new `order_key` tripped the 48-char rebalance signal
        /// ([`OrderKey::needs_rebalance`]) — the owner should schedule a rebalance pass (a pure
        /// signal; the move still succeeded). The rebalance NEVER reorders the displayed order.
        needs_rebalance: bool,
    },
    /// The CAS LOST (a precondition miss — a concurrent move bumped the version first). **Nothing
    /// was written, nothing emitted** (0 silent clobber). The loser is returned the AUTHORITATIVE
    /// order + its current version so it can re-base honestly (the architecture's
    /// `Conflict{authoritative_order}`). The loser re-bases by re-issuing a reorder against the
    /// fresh state — never a silent overwrite, never a merge.
    Conflict {
        /// The authoritative current version of the moved issue (the loser re-bases against this).
        authoritative_version: u64,
        /// The authoritative displayed order the loser re-bases against (the full server state).
        authoritative_order: Vec<RankedIssue>,
    },
}

/// Why a reorder could not even be attempted (a loud, never-silent error — distinct from the
/// expected [`ReorderOutcome::Conflict`] which is the normal CAS-loss path).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReorderError {
    /// The moved issue is not on the board (a programming error / a stale client referencing a
    /// deleted issue). Nothing is written.
    UnknownIssue(String),
    /// The bisected rank did not land strictly between its neighbours (a codec invariant violation —
    /// should be impossible with the frozen [`OrderKey`] bisection; surfaced LOUDLY rather than
    /// silently corrupting the order).
    RankInvariant(String),
    /// The outbox co-commit of the `issue.reordered` event failed (the transaction is dropped → the
    /// rank is NOT written either; emit-iff-committed).
    Outbox(String),
}

impl std::fmt::Display for ReorderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReorderError::UnknownIssue(id) => write!(f, "reorder: unknown issue `{id}`"),
            ReorderError::RankInvariant(why) => {
                write!(f, "reorder: rank invariant violated: {why}")
            }
            ReorderError::Outbox(why) => write!(f, "reorder: outbox co-commit failed: {why}"),
        }
    }
}

impl std::error::Error for ReorderError {}

// ===========================================================================
// §3 — the server-arbitrated CAS reorder (the silent-clobber floor)
// ===========================================================================

/// **THE server-arbitrated `order_key` CAS reorder (ISS-P09 / P-375 — the prompt's headline).**
///
/// Mirrors the architecture §5 `reorder(...)` sketch exactly:
///
/// 1. **read the gap** — the neighbour `order_key`s of the target slot from the CURRENT
///    authoritative board state (so a re-basing loser bisects against FRESH neighbours, never stale
///    client state);
/// 2. **bisect a new rank** — [`OrderKey::rank_between`] (the frozen midpoint bisection + 2-char
///    jitter, contract 13.3) — `new_rank = order_key::between(before_rank, after_rank)`;
/// 3. **SERVER-ARBITRATED CAS** — accept the move iff the moved issue's stored `version` still equals
///    `req.expected_version` (the architecture's `WHERE version = expected_version`). On a miss
///    (`n == 0`) return [`ReorderOutcome::Conflict`] with the authoritative order + version — **the
///    loser re-bases, nothing written, nothing emitted** (0 silent clobber);
/// 4. **co-commit the event** — on a CAS win, write the new rank + bump the version, and
///    [`OutboxTx::emit`] the `issue.reordered` event on the SAME [`OutboxTransaction`] so the rank
///    delta and its event co-commit (contract 2.2; emit-iff-committed — an outbox failure drops the
///    whole transaction, so the rank is NOT applied either).
///
/// The 48-char rebalance is a pure SIGNAL on the returned [`ReorderOutcome::Applied`]
/// (`needs_rebalance`); the move always succeeds — the owner schedules a [`rebalance`] pass that
/// re-spaces the keys WITHOUT reordering the displayed order. Humans and agents call this ONE
/// mechanism (server-arbitrated, not client-trust — arch §5 agent parity).
pub fn reorder(
    board: &mut BoardRanking,
    store: &OutboxStore,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    req: &ReorderRequest,
    cause: Option<&myelin_events::EventEnvelope>,
) -> Result<ReorderOutcome, ReorderError> {
    // The moved issue must exist on the board (a stale client / a deleted issue is a loud error,
    // never a silent no-op).
    let current = board
        .get(&req.issue_id)
        .cloned()
        .ok_or_else(|| ReorderError::UnknownIssue(req.issue_id.clone()))?;

    // ── 3. SERVER-ARBITRATED CAS: a stale version LOSES — the loser re-bases (0 silent clobber). ──
    // This is the architecture's `WHERE version = expected_version` precondition. We check it BEFORE
    // bisecting/writing anything, so a loser writes NOTHING and emits NOTHING — it is returned the
    // authoritative order to re-base against. (Checking first also means a lost CAS does not even
    // consume a jitter / a bisection — the loss is cheap and side-effect-free.)
    if current.version != req.expected_version {
        return Ok(ReorderOutcome::Conflict {
            authoritative_version: current.version,
            authoritative_order: board.displayed_order(),
        });
    }

    // ── 1 + 2. read the gap from CURRENT state, bisect the new rank (frozen codec, contract 13.3) ──
    let (lo, hi) = board.gap_for(req.before_id.as_deref(), req.after_id.as_deref());
    let new_rank = OrderKey::rank_between(lo.as_ref(), hi.as_ref(), req.jitter);

    // The bisected rank MUST land strictly between its neighbours (the load-bearing LexoRank
    // invariant). The jitter suffix is order-preserving, so `lo < new_rank < hi` holds for the
    // frozen codec — assert it LOUDLY rather than silently corrupt the displayed order. (A bound
    // equal to the moved issue's own current key is excluded: a no-op gap is still strictly ordered.)
    if let Some(ref lo_k) = lo {
        if lo_k >= &new_rank {
            return Err(ReorderError::RankInvariant(format!(
                "new rank {new_rank} did not sort after its lower neighbour {lo_k}"
            )));
        }
    }
    if let Some(ref hi_k) = hi {
        if &new_rank >= hi_k {
            return Err(ReorderError::RankInvariant(format!(
                "new rank {new_rank} did not sort before its upper neighbour {hi_k}"
            )));
        }
    }

    let needs_rebalance = new_rank.needs_rebalance();

    // ── 4. co-commit: write the new rank + bump the version, emit issue.reordered in ONE TX ───────
    let tenant = ctx_base.tenant.0.clone();
    let object_ref = issue_ref(&tenant, &req.issue_id);
    let aggregate = issue_aggregate_key(0, &req.issue_id);

    let mut tx = store.begin(minter, ctx_base);
    // The "state change" half of the co-commit: the new rank + the bumped version (in the live OLTP
    // binding this is the `UPDATE issue SET rank = …, version = version + 1 WHERE … RETURNING`).
    tx.stage_state_change(format!(
        "issue {} reordered: rank {} -> {} (version {} -> {})",
        req.issue_id,
        current.order_key,
        new_rank,
        current.version,
        current.version + 1
    ));

    let draft = reorder_event_draft(
        &object_ref,
        &aggregate,
        &req.issue_id,
        &current.order_key,
        &new_rank,
        current.version + 1,
    );
    let event_id = tx
        .emit(draft, cause)
        .map_err(|e| ReorderError::Outbox(format!("{e:?}")))?;

    // COMMIT: the rank delta + the issue.reordered event become durable ATOMICALLY. An Err above
    // dropped `tx` WITHOUT commit → neither the rank nor the event was written (emit-iff-committed).
    tx.commit()
        .map_err(|e| ReorderError::Outbox(format!("{e:?}")))?;

    // The CAS won and the event committed: now (and only now) mutate the in-memory ranking row. In
    // the live OLTP binding the `UPDATE … RETURNING` and the outbox `INSERT` share one DB
    // transaction; the in-memory model commits the outbox first, then reflects the row — the same
    // observable atomicity (the board row is never advanced for a write that did not commit).
    let ranked = RankedIssue {
        issue_id: current.issue_id.clone(),
        order_key: new_rank,
        version: current.version + 1,
        created_at: current.created_at.clone(),
        ulid: current.ulid.clone(),
    };
    board.upsert(ranked.clone());

    Ok(ReorderOutcome::Applied {
        ranked,
        event_id,
        needs_rebalance,
    })
}

/// Build the `issue.reordered` [`EventDraft`] (references-not-payloads, contract 2.7 — the payload
/// carries the issue ref + the rank delta tokens, never an inline body). The aggregate is the issue
/// (per-issue ordering, contract 2.3). A reorder carries no free-text → `contains_personal_data` is
/// false and there is no `pii_key_ref` (the moved-rank delta is opaque ranking tokens).
fn reorder_event_draft(
    object: &ArtifactRef,
    aggregate: &AggregateKey,
    issue_id: &str,
    from_rank: &OrderKey,
    to_rank: &OrderKey,
    new_version: u64,
) -> EventDraft {
    EventDraft {
        type_: EventType(events::ISSUE_REORDERED.into()),
        subject: object.clone(),
        aggregate: aggregate.clone(),
        payload: serde_json::json!({
            "issue": object.0,
            "issue_local_id": issue_id,
            "from_rank": from_rank.as_str(),
            "to_rank": to_rank.as_str(),
            "version": new_version,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        // a rank delta carries no free-text PII (opaque ranking tokens, an opaque pseudonymous actor
        // on the envelope — never an inline body).
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

// ===========================================================================
// §4 — the 48-char rebalance (re-spaces the keys; NEVER reorders the displayed order)
// ===========================================================================

/// **The 48-char rebalance pass (contract 13.3 — the measured precision-exhaustion pathology).**
///
/// When a bisected `order_key` grows past [`OrderKey::needs_rebalance`] (48 chars), the bisection has
/// run out of headroom. The rebalance re-spaces the collection's keys onto fresh, short, evenly-
/// gapped ranks — but it **NEVER reorders the displayed order** (arch §5 "Rebalance"): it walks the
/// issues in their CURRENT [`BoardRanking::displayed_order`] and re-ranks them in that exact sequence
/// ([`OrderKey::rank_first`] then [`OrderKey::rank_last`] stepping forward), so the i-th issue stays
/// the i-th issue — same order, shorter keys.
///
/// Each re-ranked issue's `version` is bumped (the rebalance is a write — a concurrent reorder that
/// raced it loses its CAS and re-bases against the rebalanced state). The rebalance is idempotent on
/// a single pass and, per the architecture, a `myelin-flow` activity emitted via the outbox so views
/// resubscribe — that emit wiring is the M-flow integration; THIS function performs the order-
/// preserving re-spacing the activity drives. The returned vector is the new displayed order (which
/// MUST equal the old displayed order, issue-for-issue).
///
/// `jitters` supplies one [`Jitter`] per issue (in displayed order) so the re-spacing is
/// deterministic + reproducible (a live caller draws them from its RNG; tests supply fixed digits).
/// If fewer jitters than issues are supplied, the remainder reuse [`Jitter::ZERO`].
pub fn rebalance(board: &mut BoardRanking, jitters: &[Jitter]) -> Vec<RankedIssue> {
    let order = board.displayed_order();
    let mut prev: Option<OrderKey> = None;
    let mut out = Vec::with_capacity(order.len());
    for (i, issue) in order.iter().enumerate() {
        let jitter = jitters.get(i).copied().unwrap_or(Jitter::ZERO);
        // Step forward: the first issue ranks first; each subsequent issue ranks LAST after the
        // previous re-ranked key. This produces strictly increasing, short, evenly-gapped keys IN
        // THE SAME ORDER as the input (displayed order is preserved by construction).
        let new_key = match &prev {
            None => OrderKey::rank_first(jitter),
            Some(p) => OrderKey::rank_last(Some(p), jitter),
        };
        let ranked = RankedIssue {
            issue_id: issue.issue_id.clone(),
            order_key: new_key.clone(),
            // a rebalance is a write → bump the version (a racing reorder loses its CAS, re-bases).
            version: issue.version + 1,
            created_at: issue.created_at.clone(),
            ulid: issue.ulid.clone(),
        };
        board.upsert(ranked.clone());
        out.push(ranked);
        prev = Some(new_key);
    }
    out
}

/// Whether two displayed orders are identical issue-for-issue (the rebalance invariant: a rebalance
/// re-spaces the KEYS but the SEQUENCE of issue ids is unchanged). Used by the drill + the unit test
/// to prove the 48-char rebalance never reorders the displayed order.
pub fn same_displayed_sequence(a: &[RankedIssue], b: &[RankedIssue]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.issue_id == y.issue_id)
}

/// The deterministic total-order comparison of two ranking rows (the contract-13.3 `tiebreak` over
/// `order_key` → `created_at` → ULID). Exposed so a consumer can sort an arbitrary slice of
/// [`RankedIssue`]s by the authoritative order without going through a [`BoardRanking`].
pub fn cmp_ranked(a: &RankedIssue, b: &RankedIssue) -> Ordering {
    tiebreak(
        &a.order_key,
        &a.created_at,
        &a.ulid,
        &b.order_key,
        &b.created_at,
        &b.ulid,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{Actor, CausedBy, MonotonicMinter, Region, TenantId, Timestamp};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T10:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T10:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }

    /// A SHARED minter handle (one monotonic minter reused across multiple reorders in one test, so
    /// the minted event_ids do not collide — distinct events get distinct ids, the UNIQUE(event_id)
    /// invariant the outbox enforces).
    fn shared_minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }

    fn jit(a: usize, b: usize) -> Jitter {
        Jitter::from_ranks(a, b).expect("in-range jitter")
    }

    /// Seed a board with three evenly-spaced issues A < B < C (the spine of the reorder tests).
    fn seed_three() -> BoardRanking {
        let mut board = BoardRanking::new();
        let a = OrderKey::rank_first(jit(0, 0)); // U00
        let b = OrderKey::rank_last(Some(&a), jit(0, 0));
        let c = OrderKey::rank_last(Some(&b), jit(0, 0));
        board.upsert(RankedIssue {
            issue_id: "ENG-1".into(),
            order_key: a,
            version: 0,
            created_at: "2026-06-21T10:00:00Z".into(),
            ulid: "01A".into(),
        });
        board.upsert(RankedIssue {
            issue_id: "ENG-2".into(),
            order_key: b,
            version: 0,
            created_at: "2026-06-21T10:00:01Z".into(),
            ulid: "01B".into(),
        });
        board.upsert(RankedIssue {
            issue_id: "ENG-3".into(),
            order_key: c,
            version: 0,
            created_at: "2026-06-21T10:00:02Z".into(),
            ulid: "01C".into(),
        });
        board
    }

    fn ids(order: &[RankedIssue]) -> Vec<String> {
        order.iter().map(|r| r.issue_id.clone()).collect()
    }

    /// **A reorder that WINS the CAS writes the new rank, bumps the version, and co-commits
    /// `issue.reordered`.** Move ENG-3 to the front (before ENG-1): its new rank sorts first, its
    /// version is 1, and exactly one event co-committed at seq 0 for its aggregate.
    #[test]
    fn reorder_wins_cas_and_co_commits_event() {
        let mut board = seed_three();
        let store = OutboxStore::new();
        let req = ReorderRequest {
            issue_id: "ENG-3".into(),
            before_id: None,
            after_id: Some("ENG-1".into()),
            expected_version: 0,
            jitter: jit(1, 1),
        };
        let out = reorder(&mut board, &store, minter(), ctx_base(), &req, None)
            .expect("the reorder is attempted");
        let (ranked, event_id) = match out {
            ReorderOutcome::Applied {
                ranked,
                event_id,
                needs_rebalance,
            } => {
                assert!(
                    !needs_rebalance,
                    "a short key does not trip the 48-char rebalance"
                );
                (ranked, event_id)
            }
            ReorderOutcome::Conflict { .. } => panic!("the only writer must WIN the CAS"),
        };
        assert_eq!(ranked.version, 1, "an accepted reorder bumps the version");
        // ENG-3 now sorts first.
        assert_eq!(ids(&board.displayed_order()), ["ENG-3", "ENG-1", "ENG-2"]);
        // the issue.reordered event co-committed.
        assert_eq!(store.outbox_depth(), 1, "one issue.reordered co-committed");
        let row = store.row(&event_id).expect("the committed row is present");
        assert_eq!(row.envelope.type_.0, events::ISSUE_REORDERED);
        assert_eq!(row.seq, 0, "first event for the aggregate is seq 0");
    }

    /// **A STALE reorder LOSES the CAS — nothing written, nothing emitted (0 silent clobber).** Two
    /// writers both think ENG-2 is at version 0. The first wins (version → 1); the second, still
    /// holding version 0, loses: it gets the authoritative order + version back and re-bases. The
    /// loser's attempt wrote NO rank and emitted NO event.
    #[test]
    fn stale_reorder_loses_cas_zero_clobber() {
        let mut board = seed_three();
        let store = OutboxStore::new();

        // Writer 1 moves ENG-2 to the back (after ENG-3) — wins, version 0 -> 1.
        let w1 = ReorderRequest {
            issue_id: "ENG-2".into(),
            before_id: Some("ENG-3".into()),
            after_id: None,
            expected_version: 0,
            jitter: jit(1, 0),
        };
        let r1 = reorder(&mut board, &store, minter(), ctx_base(), &w1, None).unwrap();
        assert!(
            matches!(r1, ReorderOutcome::Applied { .. }),
            "writer 1 wins"
        );
        let order_after_w1 = ids(&board.displayed_order());
        let depth_after_w1 = store.outbox_depth();

        // Writer 2 still holds version 0 for ENG-2 — moves it to the FRONT. It LOSES (stale version).
        let w2 = ReorderRequest {
            issue_id: "ENG-2".into(),
            before_id: None,
            after_id: Some("ENG-1".into()),
            expected_version: 0, // stale!
            jitter: jit(2, 2),
        };
        let r2 = reorder(&mut board, &store, minter(), ctx_base(), &w2, None).unwrap();
        match r2 {
            ReorderOutcome::Conflict {
                authoritative_version,
                authoritative_order,
            } => {
                assert_eq!(
                    authoritative_version, 1,
                    "the loser is told the real version"
                );
                assert_eq!(
                    ids(&authoritative_order),
                    order_after_w1,
                    "the loser gets the real order"
                );
            }
            ReorderOutcome::Applied { .. } => panic!("the STALE writer must LOSE the CAS"),
        }
        // 0 silent clobber: the board order is UNCHANGED by the loser, and NO second event emitted.
        assert_eq!(
            ids(&board.displayed_order()),
            order_after_w1,
            "the loser wrote no rank"
        );
        assert_eq!(
            store.outbox_depth(),
            depth_after_w1,
            "the loser emitted no event"
        );
    }

    /// **The loser re-bases honestly against fresh state and then WINS.** After losing on a stale
    /// version, the loser re-issues with the authoritative version — and now succeeds. This is the
    /// "honest rollback then re-apply" the architecture's `Conflict{authoritative_order}` enables.
    #[test]
    fn loser_rebases_against_fresh_state_and_wins() {
        let mut board = seed_three();
        let store = OutboxStore::new();
        let m = shared_minter();
        // Writer 1 wins (ENG-2 version 0 -> 1).
        let w1 = ReorderRequest {
            issue_id: "ENG-2".into(),
            before_id: Some("ENG-3".into()),
            after_id: None,
            expected_version: 0,
            jitter: jit(1, 0),
        };
        reorder(&mut board, &store, Arc::clone(&m), ctx_base(), &w1, None).unwrap();

        // Writer 2 loses on stale version 0.
        let mut w2 = ReorderRequest {
            issue_id: "ENG-2".into(),
            before_id: None,
            after_id: Some("ENG-1".into()),
            expected_version: 0,
            jitter: jit(2, 2),
        };
        let authoritative_version =
            match reorder(&mut board, &store, Arc::clone(&m), ctx_base(), &w2, None).unwrap() {
                ReorderOutcome::Conflict {
                    authoritative_version,
                    ..
                } => authoritative_version,
                _ => panic!("stale writer loses"),
            };
        // Re-base: re-issue with the authoritative version — now WINS.
        w2.expected_version = authoritative_version;
        let r = reorder(&mut board, &store, Arc::clone(&m), ctx_base(), &w2, None).unwrap();
        assert!(
            matches!(r, ReorderOutcome::Applied { .. }),
            "the re-based writer wins"
        );
        assert_eq!(
            ids(&board.displayed_order())[0],
            "ENG-2",
            "ENG-2 is now at the front"
        );
    }

    /// **Two concurrent same-gap inserts produce DISTINCT ranks (the jitter's reason to exist).**
    /// Both writers win their CAS in turn (distinct issues), bisecting into overlapping gaps with
    /// DIFFERENT jitter → distinct keys, and the displayed order stays total (no collision).
    #[test]
    fn concurrent_same_gap_moves_produce_distinct_ranks() {
        let mut board = seed_three();
        let store = OutboxStore::new();
        let m = shared_minter();
        // Move ENG-1 between ENG-2 and ENG-3 (jitter A).
        let a = ReorderRequest {
            issue_id: "ENG-1".into(),
            before_id: Some("ENG-2".into()),
            after_id: Some("ENG-3".into()),
            expected_version: 0,
            jitter: jit(5, 5),
        };
        let ra = match reorder(&mut board, &store, Arc::clone(&m), ctx_base(), &a, None).unwrap() {
            ReorderOutcome::Applied { ranked, .. } => ranked,
            _ => panic!(),
        };
        // Now move ENG-3 into a nearby gap (jitter B) — a different key.
        let b = ReorderRequest {
            issue_id: "ENG-3".into(),
            before_id: Some("ENG-2".into()),
            after_id: Some("ENG-1".into()),
            expected_version: 0,
            jitter: jit(6, 6),
        };
        let rb = match reorder(&mut board, &store, Arc::clone(&m), ctx_base(), &b, None).unwrap() {
            ReorderOutcome::Applied { ranked, .. } => ranked,
            _ => panic!(),
        };
        assert_ne!(ra.order_key, rb.order_key, "distinct ranks via the jitter");
        // the displayed order is a strict total order (no two equal keys collide unbroken).
        let order = board.displayed_order();
        for w in order.windows(2) {
            assert!(
                cmp_ranked(&w[0], &w[1]) == Ordering::Less,
                "strictly increasing total order"
            );
        }
    }

    /// **A reorder against a deleted/unknown issue is a LOUD error, never a silent no-op.**
    #[test]
    fn unknown_issue_is_a_loud_error() {
        let mut board = seed_three();
        let store = OutboxStore::new();
        let req = ReorderRequest {
            issue_id: "ENG-404".into(),
            before_id: None,
            after_id: Some("ENG-1".into()),
            expected_version: 0,
            jitter: jit(0, 0),
        };
        let err = reorder(&mut board, &store, minter(), ctx_base(), &req, None).unwrap_err();
        assert!(matches!(err, ReorderError::UnknownIssue(_)));
        assert_eq!(store.outbox_depth(), 0, "no event for an unknown issue");
    }

    /// **The 48-char rebalance re-spaces the keys but NEVER reorders the displayed order.** Seed a
    /// board whose keys have grown long, rebalance, and assert: (a) the displayed SEQUENCE is
    /// identical issue-for-issue, (b) every new key is short (well under the 48-char trigger), (c)
    /// the new keys are strictly increasing in the same order.
    #[test]
    fn rebalance_preserves_displayed_order_with_short_keys() {
        // Three issues with long, adjacent keys (the precision-exhaustion pathology).
        let mut board = BoardRanking::new();
        let long_a = OrderKey::parse(format!("{}1", "V".repeat(40))).unwrap();
        let long_b = OrderKey::parse(format!("{}2", "V".repeat(40))).unwrap();
        let long_c = OrderKey::parse(format!("{}3", "V".repeat(40))).unwrap();
        board.upsert(RankedIssue {
            issue_id: "A".into(),
            order_key: long_a,
            version: 7,
            created_at: "t1".into(),
            ulid: "01A".into(),
        });
        board.upsert(RankedIssue {
            issue_id: "B".into(),
            order_key: long_b,
            version: 3,
            created_at: "t2".into(),
            ulid: "01B".into(),
        });
        board.upsert(RankedIssue {
            issue_id: "C".into(),
            order_key: long_c,
            version: 9,
            created_at: "t3".into(),
            ulid: "01C".into(),
        });
        let before = board.displayed_order();
        assert_eq!(ids(&before), ["A", "B", "C"]);

        let after = rebalance(&mut board, &[jit(0, 0), jit(0, 0), jit(0, 0)]);

        // (a) the displayed SEQUENCE is unchanged issue-for-issue.
        assert!(
            same_displayed_sequence(&before, &after),
            "rebalance must not reorder displayed order"
        );
        assert_eq!(ids(&board.displayed_order()), ["A", "B", "C"]);
        // (b) every new key is SHORT (the rebalance fixed the precision exhaustion).
        for r in &after {
            assert!(
                !r.order_key.needs_rebalance(),
                "{} rebalanced to a short key",
                r.issue_id
            );
            assert!(r.order_key.as_str().len() < 8, "a re-spaced key is short");
        }
        // (c) the new keys are strictly increasing in the SAME order.
        for w in after.windows(2) {
            assert!(
                w[0].order_key < w[1].order_key,
                "re-spaced keys strictly increase"
            );
        }
        // the rebalance bumped each version (a racing reorder loses its CAS).
        assert_eq!(board.get("A").unwrap().version, 8);
        assert_eq!(board.get("C").unwrap().version, 10);
    }

    /// **The rebalance is order-preserving even when the input is NOT pre-sorted by id** — it walks
    /// the displayed (rank) order, not the insertion order. Insert C, A, B out of order; rebalance;
    /// the sequence is the displayed-order sequence, re-spaced.
    #[test]
    fn rebalance_walks_displayed_order_not_insertion_order() {
        let mut board = BoardRanking::new();
        // displayed order by key will be A < B < C even though we insert C, A, B.
        board.upsert(RankedIssue {
            issue_id: "C".into(),
            order_key: OrderKey::parse("z").unwrap(),
            version: 0,
            created_at: "t3".into(),
            ulid: "3".into(),
        });
        board.upsert(RankedIssue {
            issue_id: "A".into(),
            order_key: OrderKey::parse("1").unwrap(),
            version: 0,
            created_at: "t1".into(),
            ulid: "1".into(),
        });
        board.upsert(RankedIssue {
            issue_id: "B".into(),
            order_key: OrderKey::parse("M").unwrap(),
            version: 0,
            created_at: "t2".into(),
            ulid: "2".into(),
        });
        let before = board.displayed_order();
        assert_eq!(ids(&before), ["A", "B", "C"]);
        let after = rebalance(&mut board, &[]);
        assert!(same_displayed_sequence(&before, &after));
        assert_eq!(ids(&after), ["A", "B", "C"]);
    }
}
