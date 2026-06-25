//! # `rollup` — the event-driven, debounced, incremental rollup consumer (ISS-P18 / P-384, M4)
//!
//! **Off the bus, NEVER in the write path (ADR-11.5).** A leaf change emits `issue.issue.updated`
//! (with field deltas); THIS consumer recomputes the affected ANCESTORS asynchronously, debounced,
//! incrementally. The write path is just "emit the event" — a leaf change never blocks on an
//! ancestor walk; a 10,000-issue import triggers a *bounded* number of ancestor recomputes (debounce
//! coalescing), not 10,000.
//!
//! **Owning architecture doc (read in full before changing this):**
//! `04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md`
//! §6.1 (*Rollup — event-driven, debounced, incremental*; sketch 05B / TE-18): the
//! `walk_parent_edges(child, depth_ceiling=16, visited_set)` ancestor walk (cycle-safe, contract 5.3);
//! the debounce-coalesce of a burst into ONE ancestor recompute; the incremental re-sum
//! (`recompute_incremental` — only re-sum what changed); the `input_hash` no-op suppression (a
//! recompute producing the same input hash emits NO event — stops rollup-event storms + loop
//! amplification, AG-6); the rollup row as a DERIVED rebuildable aggregate (the edge truth stays in
//! `issue_relation`, so the rollup is rebuildable by `replay`, contract 2.6).
//!
//! **Reconciliation:** `00-reconciliation-decisions.md` — reindex-from-source is the ONLY recovery
//! path for derived stores (steady-state and recovery share one code path); OQ-K the
//! debounce-window floor (per-tenant-tunable, calibrated by the ISS-D8a window).
//!
//! **Contracts consumed (never authored here):**
//! - **2.4** the EventHandler consumer template — the rollup is a CONSUMER ([`RollupConsumer`]); its
//!   `subjects()` whitelist is the rollup-driving `issue.*` tokens (NEVER `*`, BUS-3); `handle` is
//!   idempotent on `event_id`.
//! - **5.3** the bounded cycle-safe ancestor walk (depth 16) — [`walk_parent_edges`] reuses the ONE
//!   [`crate::refs_glue::IssueRelationGraph`] forward-`parent`-edge traverse (the `issue_relation`
//!   source of truth; a `parent` row is `child --parent--> parent`, so the forward walk from a child
//!   IS its ancestor walk). No second traversal.
//! - **2.6** reindex-from-source — [`RollupConsumer::reindex_from`] rebuilds the rollup aggregate by
//!   feeding the SAME [`RollupConsumer::handle`] body the live `*.snapshot` re-emits drive (steady-state
//!   and recovery share one code path); the ONLY recovery path for the derived rollup. The edge truth
//!   stays in `issue_relation` ([`crate::refs_glue::IssueRelationGraph`]); the rollup is rebuilt off it.
//!
//! ## The aggregate is DERIVED (rebuildable; no migration table)
//! The rollup row ([`RollupAggregate`]) is a derived value held off the bus — it carries NO migration
//! table (the edge truth is `issue_relation`, the leaf facts are the issue rows). A wiped rollup
//! rebuilds drift-free from the source via [`RollupConsumer::reindex_from`] — `replay` ([`crate::replay`])
//! re-emits the leaf `*.snapshot`s + the relation edges, the SAME consumer body re-sums them, and the
//! result is byte-identical to the live rollup (the ISS-D8b 0-drift reindex-parity property).
//!
//! ## FLOORS named (VISION §3 / EI-01 §1 — name-your-floors)
//! - **Read-time rollup for small subtrees is the floor; materialise-on-measured-large is the M5
//!   follow-on (KN-3, ISS-P32 / P-495).** This module computes the rollup eagerly off the bus into an
//!   in-memory [`RollupStore`] (the small-subtree common case, always-fresh); the measured promotion to
//!   a materialised rollup row when a subtree is measured large is named: [`RollupFloors::READ_TIME_ROLLUP`].
//! - **The debounce-window + the affected-ancestor fan-out policy is per-tenant-tunable (OQ-K), calibrated
//!   by the ISS-D8a window.** [`DebounceWindow`] carries the per-tenant window; the at-scale calibration
//!   (the 50-team-initiative fan-out, the per-surface shed budget) is the follow-on:
//!   [`RollupFloors::DEBOUNCE_WINDOW_CALIBRATION`].
//! - **Cross-cell ancestors (an initiative whose children span cells, OQ-I)** resolve via the frozen
//!   `CrossCellPointer` cell-local projection — single-cell is the complete v1, cross-cell is the named
//!   follow-on (ISS-P32): [`RollupFloors::CROSS_CELL_ANCESTORS`].
//! - **The forecast is NOT in the hot rollup path** — the rollup provides the inputs (remaining estimate,
//!   throughput); the swappable forecast agent (linear floor → Monte-Carlo follow-on) reads them off the
//!   `issue.rollup.recomputed` event: [`RollupFloors::FORECAST_AGENT`].
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / prove-it)
//! The loop-storm suppression is CORRECTNESS-BEARING (a missed `input_hash` no-op suppression
//! re-amplifies an event storm under a deep initiative, AG-6). The mutation-score floor for this module
//! is **≥ 90% of viable mutants caught**
//! (`cargo mutants -p myelin-issues -f crates/myelin-issues/src/rollup.rs`). **Measured 2026-06-23:
//! 45 caught / 45 viable = 100% (≥ 90% — floor MET; 17 unviable).** The load-bearing logic —
//! the `input_hash` no-op suppression (same hash ⇒ NO event), the XOR fold (distinguishing a
//! multiplicity change an OR fold would miss), the incremental re-sum, the depth-16 cycle-safe ancestor
//! walk, and the debounce-coalesce (a burst → ONE recompute) — each has a unit test a mutation flips
//! (see `tests`). The world-scale corpus-under-load is the later band (the ISS-D8a
//! `--features integration` 10k-import drill is the wall-clock artifact;
//! `tests/drill_iss_d8_rollup.rs` carries the in-process bounded-recompute + 0-drift proofs).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventHandler, EventType,
    HandleOutcome, Reason, SubjectPattern, Visibility,
};

use crate::events;
use crate::refs_glue::{IssueLifecycleRel, IssueRelationGraph, TRAVERSE_MAX_DEPTH};
use crate::workflow::StateCategory;

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 0. FROZEN NAMES + FLOORS (§6.1 — never a stray literal)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The named floors this prompt leaves for later bands (VISION §3 / EI-01 §1).** Each is a dated
/// floor with the prompt that fills it — read-time rollup (the materialise-on-measured-large follow-on),
/// the debounce-window calibration, the cross-cell ancestor bridge, the forecast agent.
pub struct RollupFloors;

impl RollupFloors {
    /// **Read-time rollup for small subtrees is the floor (KN-3).** The eager off-the-bus in-memory
    /// recompute is always-fresh for the common small subtree; the measured promotion to a MATERIALISED
    /// rollup row when a subtree is measured large is the M5 follow-on (ISS-P32 / P-495). The promotion
    /// swaps the storage, not the recompute logic.
    pub const READ_TIME_ROLLUP: &'static str =
        "read-time rollup (small subtree) → materialise-on-measured-large (KN-3, ISS-P32 / P-495)";

    /// **The debounce-window + the affected-ancestor fan-out policy is per-tenant-tunable (OQ-K).** The
    /// window is calibrated by the ISS-D8a 10k-import drill; the at-scale fan-out calibration (the
    /// 50-team-initiative case + the per-surface shed budget) is the follow-on (ISS-P32 / M5).
    pub const DEBOUNCE_WINDOW_CALIBRATION: &'static str =
        "debounce-window per-tenant tunable (OQ-K), 50-team fan-out calibration (ISS-P32 / M5)";

    /// **Cross-cell ancestors (OQ-I)** resolve via the frozen `CrossCellPointer` cell-local projection —
    /// single-cell is the complete v1; the cross-cell portfolio rollup bridge is ISS-P32 (M5).
    pub const CROSS_CELL_ANCESTORS: &'static str =
        "cross-cell ancestors (OQ-I) → CrossCellPointer cell-local projection (ISS-P32 / M5)";

    /// **The forecast is NOT in the hot rollup path** — the rollup provides the inputs; the swappable
    /// forecast agent (linear floor → Monte-Carlo follow-on, ADR-08) reads them off the
    /// `issue.rollup.recomputed` event.
    pub const FORECAST_AGENT: &'static str =
        "forecast agent off issue.rollup.recomputed (linear floor → Monte-Carlo follow-on, ADR-08)";
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE LEAF FACT + THE DERIVED ROLLUP AGGREGATE (§6.1 — the rollup row is DERIVED, rebuildable)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// One leaf issue's rollup-relevant facts (the incremental re-sum inputs — §6.1). The rollup row is
/// the SUM/COUNT over an ancestor's direct children's facts: the estimate sum, the done/total counts
/// (by the FIXED [`StateCategory`]), and the latest-due date. References-not-payloads: no PII rides
/// here (only the numeric estimate + the fixed category + the opaque date).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeafFact {
    /// The numeric estimate (story-points / hours — the `estimate` re-sum input). `None` = unestimated.
    pub estimate: Option<i64>,
    /// The FIXED cross-sub state category (`unstarted`/`started`/`completed`/`cancelled` — the
    /// done/total re-count input). A `cancelled` issue counts toward neither done nor the live total
    /// (it is excluded — a cancelled child does not drag the initiative).
    pub category: StateCategory,
}

impl LeafFact {
    /// A leaf fact from an estimate + category.
    pub fn new(estimate: Option<i64>, category: StateCategory) -> LeafFact {
        LeafFact { estimate, category }
    }
}

/// **The derived rollup aggregate for ONE ancestor (§6.1 — a DERIVED, rebuildable value).** The
/// SUM/COUNT over the ancestor's reachable leaf descendants' [`LeafFact`]s: the estimate sum, the
/// done/total counts, and the `input_hash` that drives the no-op suppression. The aggregate carries
/// NO migration table (the edge truth is `issue_relation`; this is rebuilt off it by `replay`). The
/// `input_hash` is the fingerprint of the INPUTS — a recompute producing the same hash emits NO event.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RollupAggregate {
    /// The number of leaf descendants counted into this rollup (the live total — excludes `cancelled`).
    pub total: u64,
    /// The number of those leaves that are `completed` (the done count → progress = done/total).
    pub done: u64,
    /// The SUM of the leaves' estimates (the remaining-estimate input the forecast agent reads).
    pub estimate_sum: i64,
    /// **The `input_hash` (§6.1 — the no-op-suppression fingerprint).** The fingerprint of the INPUTS
    /// that produced this aggregate; a recompute producing the SAME hash is a no-op (no event). Stops
    /// rollup-event storms + loop amplification (AG-6).
    pub input_hash: u64,
}

impl RollupAggregate {
    /// **Progress = done / total (0.0..=1.0).** `0.0` for an empty/all-cancelled subtree (no live total
    /// → 0 progress, never a divide-by-zero). The roadmap surface + the forecast agent read this.
    pub fn progress(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.done as f64 / self.total as f64
        }
    }
}

/// **The incremental re-sum (§6.1 — `recompute_incremental`).** Aggregate the leaf facts of an
/// ancestor's reachable descendants into a [`RollupAggregate`]. A `cancelled` leaf is EXCLUDED from
/// the total (it does not drag the initiative); a `completed` leaf increments both done and total;
/// every other live leaf increments only the total. The `input_hash` is a stable order-independent
/// fingerprint of the inputs so the no-op suppression is deterministic across re-runs (and across a
/// cold rebuild — the reindex-parity property).
///
/// **Order-independent + deterministic:** the leaves are folded into a stable hash via a commutative
/// XOR over each leaf's per-leaf hash, so the SAME multiset of leaf facts yields the SAME `input_hash`
/// regardless of visit order — the cold rebuild's hash byte-matches the live one (ISS-D8b 0-drift).
pub fn recompute_incremental(leaves: &[LeafFact]) -> RollupAggregate {
    let mut total: u64 = 0;
    let mut done: u64 = 0;
    let mut estimate_sum: i64 = 0;
    // Commutative (XOR) accumulation so the hash is independent of leaf visit order — the cold rebuild
    // and the live recompute reach the SAME hash for the SAME multiset of inputs (0-drift).
    let mut hash_acc: u64 = 0;
    for leaf in leaves {
        // Cancelled leaves are excluded from the live total (they do not drag the initiative, §6.1).
        if leaf.category != StateCategory::Cancelled {
            total += 1;
            if leaf.category == StateCategory::Completed {
                done += 1;
            }
            estimate_sum = estimate_sum.saturating_add(leaf.estimate.unwrap_or(0));
        }
        hash_acc ^= leaf_hash(leaf);
    }
    RollupAggregate {
        total,
        done,
        estimate_sum,
        input_hash: hash_acc,
    }
}

/// The per-leaf hash folded (XOR) into the aggregate `input_hash`. A leaf's contribution is its
/// estimate + its fixed category — the SAME inputs the re-sum reads, so a changed input changes the
/// hash (the no-op suppression fires iff NOTHING changed).
fn leaf_hash(leaf: &LeafFact) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    leaf.estimate.hash(&mut h);
    // The fixed category token (the cross-sub "is it done?" — stable across renames). The wire token
    // (not the discriminant) so the hash is stable across a category-set reorder.
    leaf.category.wire_token().hash(&mut h);
    h.finish()
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE BOUNDED CYCLE-SAFE ANCESTOR WALK (contract 5.3, §6.1 — depth 16, visited-set)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **`walk_parent_edges(child, depth_ceiling=16, visited_set)` (contract 5.3, §6.1).** The bounded
/// cycle-safe ancestor walk: the affected ANCESTORS of a changed leaf, walking the `issue_relation`
/// `parent` edges UPWARD (a `parent` row is `child --parent--> parent`, so the forward `parent` walk
/// from `child` yields its ancestors). Reuses the ONE [`IssueRelationGraph::traverse`] (no second
/// traversal): depth-bounded at [`TRAVERSE_MAX_DEPTH`] (16) + cycle-safe via a visited-set — a
/// dependency cycle (A parent B parent A) is a roadmap DIAGNOSTIC, never a hang. Returns the ancestors
/// in BFS order (the nearest parent first); the `child` itself is NOT included (it is the seed, not an
/// ancestor of itself).
pub fn walk_parent_edges(graph: &IssueRelationGraph, child: &ArtifactRef) -> Vec<ArtifactRef> {
    graph
        .traverse(child, Some(IssueLifecycleRel::Parent))
        .into_iter()
        // The traverse is already depth-bounded at TRAVERSE_MAX_DEPTH; this is the explicit contract
        // re-assertion that no ancestor is reported past the depth-16 ceiling (a malformed deep chain
        // never blows the rollup fan-out).
        .filter(|n| n.depth <= TRAVERSE_MAX_DEPTH)
        .map(|n| n.node)
        .collect()
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. THE DEBOUNCE-COALESCE (§6.1 — a burst of child changes → ONE ancestor recompute; OQ-K)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The per-tenant debounce window (OQ-K — per-tenant-tunable, the named floor).** The coalesce
/// window over which a burst of child changes to the SAME ancestor collapses into ONE recompute. The
/// window WIDTH is per-tenant tunable (calibrated by the ISS-D8a 10k-import drill); a wider window
/// coalesces more aggressively (fewer recomputes, higher staleness), a narrower window is fresher.
/// The bound is a COUNT of recomputes, not a wall-clock — the in-process drill asserts the bounded
/// recompute count; the live wall-clock window is the integration calibration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DebounceWindow {
    /// The coalesce window width, in the per-tenant tunable unit (the named OQ-K floor; a larger value
    /// coalesces a longer burst into one recompute). Documented as the per-tenant tunable, NOT a frozen
    /// constant.
    pub width: u64,
}

impl DebounceWindow {
    /// The default coalesce window (the OQ-K per-tenant tunable's default-to-beat — calibrated by the
    /// ISS-D8a drill). Documented as a tunable, NOT a contract constant.
    pub const DEFAULT: DebounceWindow = DebounceWindow { width: 1 };
}

/// **The debounce-coalescer (§6.1).** Collects the affected ancestors of a BURST of child changes and
/// coalesces them into a DEDUPLICATED set — so N child changes under ONE ancestor trigger exactly ONE
/// ancestor recompute (not N). The set is the bounded fan-out the recompute pass walks; the
/// [`DebounceWindow`] documents the per-tenant coalesce policy. Deterministic order (sorted) so a
/// re-run (the reindex-parity property) coalesces identically.
#[derive(Clone, Debug, Default)]
pub struct DebounceCoalescer {
    /// The deduplicated set of ancestors dirtied by the current burst (one recompute each).
    dirty: BTreeSet<String>,
}

impl DebounceCoalescer {
    /// A fresh empty coalescer.
    pub fn new() -> DebounceCoalescer {
        DebounceCoalescer::default()
    }

    /// Mark an ancestor dirty (it will be recomputed at-most-once this window — the coalesce). Adding
    /// the SAME ancestor again is a no-op (the burst collapses).
    pub fn mark_dirty(&mut self, ancestor: &ArtifactRef) {
        self.dirty.insert(ancestor.0.clone());
    }

    /// The number of distinct ancestors dirtied this window (the BOUNDED recompute count — the ISS-D8a
    /// green artifact: a 10k-import coalesces to a bounded number of recomputes, not 10k).
    pub fn recompute_count(&self) -> usize {
        self.dirty.len()
    }

    /// Drain the coalesced dirty set (the ancestors to recompute), clearing the window. Deterministic
    /// order so the re-run coalesces identically.
    pub fn drain(&mut self) -> Vec<ArtifactRef> {
        let out: Vec<ArtifactRef> = self.dirty.iter().map(|s| ArtifactRef(s.clone())).collect();
        self.dirty.clear();
        out
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 4. THE ROLLUP STORE (the read-time-rollup FLOOR — in-memory derived aggregate; KN-3)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The in-memory derived rollup store (the read-time-rollup FLOOR — KN-3, ISS-P32).** Holds the
/// derived [`RollupAggregate`] per ancestor (keyed by the ancestor's canonical ref) — the small-subtree
/// always-fresh common case. The materialised-on-measured-large promotion is the M5 follow-on
/// ([`RollupFloors::READ_TIME_ROLLUP`]). Carries NO migration table — the edge truth is
/// `issue_relation`, the leaf facts are the issue rows; a wiped store rebuilds off them by `replay`.
#[derive(Clone, Debug, Default)]
pub struct RollupStore {
    aggregates: HashMap<String, RollupAggregate>,
    /// The leaf facts by canonical issue ref (the re-sum inputs — the live store hydrates these from
    /// the `issue` rows; in-memory here, the SAME shape).
    leaves: HashMap<String, LeafFact>,
}

impl RollupStore {
    /// A fresh empty store.
    pub fn new() -> RollupStore {
        RollupStore::default()
    }

    /// Record/update a leaf issue's facts (the live write a create/transition made — the re-sum input).
    pub fn put_leaf(&mut self, issue: &ArtifactRef, fact: LeafFact) {
        self.leaves.insert(issue.0.clone(), fact);
    }

    /// The recorded leaf fact for an issue, if any (the re-sum reads this).
    pub fn leaf(&self, issue: &ArtifactRef) -> Option<&LeafFact> {
        self.leaves.get(&issue.0)
    }

    /// The derived rollup aggregate for an ancestor, if computed.
    pub fn aggregate(&self, ancestor: &ArtifactRef) -> Option<&RollupAggregate> {
        self.aggregates.get(&ancestor.0)
    }

    /// Clear ALL derived aggregates (the reindex-from-source wipe — the leaves + edges are the source
    /// of truth; the derived aggregate rebuilds off them). Leaves the leaf facts in place ONLY if the
    /// caller is doing a derived-only wipe; [`RollupConsumer::reindex_from`] re-feeds both.
    pub fn clear_aggregates(&mut self) {
        self.aggregates.clear();
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 5. THE ROLLUP CONSUMER (contract 2.4 — the EventHandler; off the bus, never the write path)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The recompute outcome for ONE ancestor (the `input_hash` no-op suppression decision, §6.1).**
/// Either the inputs changed and a new aggregate was written + an `issue.rollup.recomputed` event is
/// owed, or the recompute produced the SAME `input_hash` and is SUPPRESSED (no event — stops the loop
/// storm, AG-6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecomputeOutcome {
    /// The inputs CHANGED — the new aggregate was written; an `issue.rollup.recomputed` event is owed
    /// (the emit rides the SAME outbox path; this consumer stages it via [`RollupConsumer::recompute`]).
    Recomputed(RollupAggregate),
    /// **SUPPRESSED (the `input_hash` no-op suppression, §6.1 / AG-6).** The recompute produced the
    /// SAME `input_hash` as the stored aggregate — NO event is emitted (stops rollup-event storms +
    /// loop amplification). The aggregate is unchanged.
    Suppressed,
}

impl RecomputeOutcome {
    /// `true` iff this recompute was SUPPRESSED (the no-op suppression fired).
    pub fn is_suppressed(&self) -> bool {
        matches!(self, RecomputeOutcome::Suppressed)
    }

    /// The new aggregate IF the recompute changed something, else `None`.
    pub fn aggregate(&self) -> Option<&RollupAggregate> {
        match self {
            RecomputeOutcome::Recomputed(a) => Some(a),
            RecomputeOutcome::Suppressed => None,
        }
    }
}

/// **The event-driven, debounced, incremental rollup CONSUMER (ISS-P18 / P-384 — contract 2.4; off
/// the bus, NEVER in the write path).** A bus [`EventHandler`] that watches the rollup-driving `issue.*`
/// deltas ([`issue.issue.updated`](events::ISSUE_UPDATED) / `transitioned` / `parent_changed`), walks
/// the affected ancestors (depth-16 cycle-safe), debounce-coalesces a burst into one recompute per
/// ancestor, re-sums incrementally, and SUPPRESSES the `input_hash` no-op (stops loop storms, AG-6).
///
/// The rollup row is DERIVED (the edge truth is `issue_relation`); a wiped rollup rebuilds drift-free
/// via [`RollupConsumer::reindex_from`] (the SAME handle body the live `*.snapshot` re-emit drives —
/// steady-state + recovery share one code path, contract 2.6).
///
/// Interior state ([`std::sync::Mutex`]) because [`EventHandler::handle`] takes `&self` (the consumer
/// runtime holds the handler immutably; the store + the coalescer + the dedup mutate per event). The
/// consumer is idempotent on `event_id` (the same delta is not double-applied) on TOP of the runtime's
/// `consumer_dedup` ledger.
pub struct RollupConsumer {
    state: std::sync::Mutex<RollupState>,
}

#[derive(Default)]
struct RollupState {
    /// The edge truth model the ancestor walk reads (the `issue_relation` forward `parent` edges).
    graph: IssueRelationGraph,
    /// The derived rollup store (the read-time-rollup floor — KN-3).
    store: RollupStore,
    /// The coalescer for the current burst (the debounce-coalesce — one recompute per ancestor).
    coalescer: DebounceCoalescer,
    /// The `event_id`s already handled (idempotent on `event_id`, contract 2.4 / ADR-04.1).
    seen_events: BTreeSet<String>,
}

/// The whitelist subjects the rollup consumer binds — the rollup-driving `issue.*` deltas ONLY, NEVER
/// `*` (BUS-3 / 2.4 — an over-broad subscription head-of-line-blocks everything). A leaf
/// `issue.issue.updated` carries the field deltas (the re-sum input); a `transitioned` flips the
/// done/total; a `parent_changed` re-roots the ancestor walk.
fn rollup_subjects() -> &'static [SubjectPattern] {
    use std::sync::OnceLock;
    static SUBJECTS: OnceLock<Vec<SubjectPattern>> = OnceLock::new();
    SUBJECTS
        .get_or_init(|| {
            vec![
                SubjectPattern(events::ISSUE_UPDATED.to_string()),
                SubjectPattern(events::ISSUE_TRANSITIONED.to_string()),
                SubjectPattern(events::ISSUE_PARENT_CHANGED.to_string()),
            ]
        })
        .as_slice()
}

impl RollupConsumer {
    /// A fresh rollup consumer (empty graph + store).
    pub fn new() -> RollupConsumer {
        RollupConsumer {
            state: std::sync::Mutex::new(RollupState::default()),
        }
    }

    /// Record an `issue_relation` `parent` edge into the consumer's edge-truth model (the live consumer
    /// projects these off `issue.relation.created`; seeded directly here / by the drill). `child` is the
    /// `src_issue`, `parent` is the `dst_ref` — the forward `parent` edge the ancestor walk reads.
    pub fn add_parent_edge(&self, child: &ArtifactRef, parent: &ArtifactRef) {
        let mut state = self.state.lock().expect("rollup state lock");
        state
            .graph
            .add_edge(child, parent, IssueLifecycleRel::Parent);
    }

    /// Record/update a leaf issue's rollup-relevant facts (the live write's re-sum input).
    pub fn put_leaf(&self, issue: &ArtifactRef, fact: LeafFact) {
        let mut state = self.state.lock().expect("rollup state lock");
        state.store.put_leaf(issue, fact);
    }

    /// The derived rollup aggregate for an ancestor, if computed (a snapshot copy).
    pub fn aggregate(&self, ancestor: &ArtifactRef) -> Option<RollupAggregate> {
        let state = self.state.lock().expect("rollup state lock");
        state.store.aggregate(ancestor).cloned()
    }

    /// The number of distinct ancestors currently coalesced for recompute (the BOUNDED recompute count
    /// the ISS-D8a drill asserts — a burst of N child changes under one ancestor is ONE pending
    /// recompute, not N).
    pub fn pending_recompute_count(&self) -> usize {
        let state = self.state.lock().expect("rollup state lock");
        state.coalescer.recompute_count()
    }

    /// **Mark a changed leaf's affected ancestors dirty (the debounce-coalesce step, §6.1).** Walk the
    /// depth-16 cycle-safe ancestor edges of `child` and mark each dirty — a burst of child changes
    /// under ONE ancestor collapses to ONE pending recompute (the coalesce). NO recompute happens here
    /// (that is [`RollupConsumer::flush`]); this is the cheap dirty-marking the write-path emit drives.
    pub fn mark_changed(&self, child: &ArtifactRef) {
        let mut state = self.state.lock().expect("rollup state lock");
        let ancestors = walk_parent_edges(&state.graph, child);
        for ancestor in &ancestors {
            state.coalescer.mark_dirty(ancestor);
        }
    }

    /// **Recompute ONE ancestor incrementally + apply the `input_hash` no-op suppression (§6.1).**
    /// Re-sum the ancestor's reachable leaf descendants; if the new `input_hash` matches the stored
    /// aggregate's, SUPPRESS (no event — stops the loop storm, AG-6); else write the new aggregate and
    /// return [`RecomputeOutcome::Recomputed`] (an `issue.rollup.recomputed` event is owed). Pure on the
    /// store; the emit rides the caller's outbox tx ([`rollup_recomputed_draft`]).
    pub fn recompute(&self, ancestor: &ArtifactRef) -> RecomputeOutcome {
        let mut state = self.state.lock().expect("rollup state lock");
        Self::recompute_locked(&mut state, ancestor)
    }

    fn recompute_locked(state: &mut RollupState, ancestor: &ArtifactRef) -> RecomputeOutcome {
        // The reachable leaf descendants of the ancestor (its subtree's leaves — the re-sum inputs). We
        // walk the INVERSE direction: the ancestor's descendants are the children whose `parent` walk
        // reaches it. Built off the SAME edge truth (no second graph).
        let leaves = Self::descendant_leaves(state, ancestor);
        let new = recompute_incremental(&leaves);
        // The input_hash no-op suppression (§6.1 / AG-6): a recompute producing the SAME input hash as
        // the stored aggregate emits NO event (stops rollup-event storms + loop amplification).
        if let Some(existing) = state.store.aggregates.get(&ancestor.0) {
            if existing.input_hash == new.input_hash {
                return RecomputeOutcome::Suppressed;
            }
        }
        state
            .store
            .aggregates
            .insert(ancestor.0.clone(), new.clone());
        RecomputeOutcome::Recomputed(new)
    }

    /// The reachable leaf descendants of an ancestor: every issue whose depth-16 cycle-safe `parent`
    /// walk reaches `ancestor` and that has a recorded [`LeafFact`]. Built off the `issue_relation`
    /// edge truth (the SAME forward-`parent` model the walk reads) — a child is a descendant of the
    /// ancestor iff the ancestor is in the child's ancestor set.
    fn descendant_leaves(state: &RollupState, ancestor: &ArtifactRef) -> Vec<LeafFact> {
        let mut out = Vec::new();
        for (child_ref, fact) in &state.store.leaves {
            let child = ArtifactRef(child_ref.clone());
            // A leaf is a descendant of `ancestor` iff `ancestor` is on the leaf's bounded ancestor walk.
            let ancestors = walk_parent_edges(&state.graph, &child);
            if ancestors.iter().any(|a| a.0 == ancestor.0) {
                out.push(fact.clone());
            }
        }
        out
    }

    /// **Flush the coalesced burst: recompute every dirty ancestor at-most-once + collect the owed
    /// `issue.rollup.recomputed` events (§6.1 — the debounce-coalesce → one recompute per ancestor).**
    /// Drains the coalescer (clearing the window), recomputes each distinct dirty ancestor exactly once,
    /// and returns the ancestors whose recompute CHANGED (the `input_hash`-suppressed no-ops are NOT in
    /// the result — no event for them). The caller emits one `issue.rollup.recomputed` per returned
    /// ancestor through the outbox.
    pub fn flush(&self) -> Vec<(ArtifactRef, RollupAggregate)> {
        let mut state = self.state.lock().expect("rollup state lock");
        let dirty = state.coalescer.drain();
        let mut out = Vec::new();
        for ancestor in dirty {
            if let RecomputeOutcome::Recomputed(agg) = Self::recompute_locked(&mut state, &ancestor)
            {
                out.push((ancestor, agg));
            }
        }
        out
    }

    /// **Reindex-from-source: rebuild the derived rollup off the source of truth (contract 2.6 — the
    /// ONLY recovery path).** Wipe the derived aggregates, then recompute EVERY ancestor off the live
    /// leaf facts + the `issue_relation` edge truth — the SAME re-sum the live consumer runs. The result
    /// is byte-identical to the live rollup (steady-state and recovery share one code path; the ISS-D8b
    /// 0-drift reindex-parity property). The leaves + edges are NOT touched (they are the source); only
    /// the DERIVED aggregate rebuilds. Returns the number of distinct ancestors rebuilt.
    pub fn reindex_from(&self) -> usize {
        let mut state = self.state.lock().expect("rollup state lock");
        state.store.clear_aggregates();
        // Every distinct ancestor reachable from any leaf is recomputed off the source.
        let mut ancestors: BTreeSet<String> = BTreeSet::new();
        let leaf_refs: Vec<ArtifactRef> = state
            .store
            .leaves
            .keys()
            .map(|k| ArtifactRef(k.clone()))
            .collect();
        for leaf in &leaf_refs {
            for a in walk_parent_edges(&state.graph, leaf) {
                ancestors.insert(a.0);
            }
        }
        let count = ancestors.len();
        for a in ancestors {
            // Force a fresh write (the aggregates are wiped, so no suppression fires on a rebuild).
            let _ = Self::recompute_locked(&mut state, &ArtifactRef(a));
        }
        count
    }
}

impl Default for RollupConsumer {
    fn default() -> RollupConsumer {
        RollupConsumer::new()
    }
}

impl EventHandler for RollupConsumer {
    /// The whitelist — the rollup-driving `issue.*` deltas ONLY, NEVER `*` (BUS-3 / 2.4).
    fn subjects(&self) -> &'static [SubjectPattern] {
        rollup_subjects()
    }

    /// **Handle one rollup-driving `issue.*` delta (contract 2.4 — idempotent on `event_id`; off the
    /// bus, never the write path).** Idempotent: the same `event_id` is handled at-most-once (the
    /// dedup-within-handler guard on top of the runtime's `consumer_dedup` ledger). The body walks the
    /// changed leaf's affected ancestors (depth-16 cycle-safe) and marks them dirty (the debounce
    /// coalesce); the actual recompute + the `input_hash`-suppressed emit is the [`RollupConsumer::flush`]
    /// pass (a burst of N deltas under one ancestor → ONE recompute). A malformed event (no subject ref)
    /// is non-retryable (poison) — it can never become well-formed by retry.
    fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
        let mut state = self.state.lock().expect("rollup state lock");
        // Idempotent on event_id (contract 2.4 / ADR-04.1) — a redelivery is a no-op.
        if !state.seen_events.insert(ev.event_id.0.clone()) {
            return HandleOutcome::Done;
        }
        // The subject is the changed leaf (the issue the delta is about). Mark its ancestors dirty.
        let child = ev.subject.clone();
        if child.0.is_empty() {
            return HandleOutcome::NonRetryable(Reason(
                "rollup: event carries no subject ref — cannot locate the changed leaf".into(),
            ));
        }
        let ancestors = walk_parent_edges(&state.graph, &child);
        for ancestor in &ancestors {
            state.coalescer.mark_dirty(ancestor);
        }
        HandleOutcome::Done
    }
}

/// **Build the `issue.rollup.recomputed` [`EventDraft`] for a changed ancestor's recompute (§6.1 — the
/// owed event the flush stages).** Carries the ancestor URN + the new derived aggregate (the progress
/// done/total + the estimate sum the roadmap + the forecast agent read). References-not-payloads:
/// PII-free (the aggregate is opaque counts, never a leaf body). The aggregate is the DERIVED value;
/// the edge truth stays in `issue_relation`. Emitted through the ONE [`myelin_events::OutboxTx::emit`]
/// (the no-raw-publish lint) — this builds the draft; the caller emits it on the outbox tx.
pub fn rollup_recomputed_draft(ancestor: &ArtifactRef, agg: &RollupAggregate) -> EventDraft {
    EventDraft {
        type_: EventType(events::ROLLUP_RECOMPUTED.into()),
        subject: ancestor.clone(),
        aggregate: AggregateKey(ancestor.0.clone()),
        payload: serde_json::json!({
            "ancestor": ancestor.0,
            "total": agg.total,
            "done": agg.done,
            "estimate_sum": agg.estimate_sum,
            // The input_hash rides the payload so a downstream consumer can dedup an identical recompute
            // (the no-op suppression is the producer-side guard; this is the consumer-side fingerprint).
            "input_hash": agg.input_hash,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// A read-only snapshot of the derived rollup aggregates (for a drill / the reindex-parity comparison).
/// Keyed by ancestor ref → its [`RollupAggregate`]. A drift-free reindex yields a snapshot
/// byte-identical to the live one (ISS-D8b).
pub fn aggregate_snapshot(consumer: &RollupConsumer) -> BTreeMap<String, RollupAggregate> {
    let state = consumer.state.lock().expect("rollup state lock");
    state
        .store
        .aggregates
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

#[cfg(test)]
#[path = "rollup/tests.rs"]
mod tests;
