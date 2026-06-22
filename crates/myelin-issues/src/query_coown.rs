//! **Issues co-owns `myelin-query` byte-identical with Knowledge** (contract 13.3, X-3/OQ-C —
//! ISS-P02 / P-241).
//!
//! ## What this module is (and what it is NOT)
//! This is the **co-ownership seam**, not a second definition. The four shared shapes of the 13.3
//! primitive — the [`FieldType`] enum, the [`ViewSpec`] view-model, the [`QueryAst`] grammar, and
//! the `order_key`/LexoRank [`OrderKey`] codec — are **frozen byte-identical** in the shared
//! `myelin-query` crate (Knowledge leads the definitions, Issues co-owns; ADR-06). Issues **LINKS
//! those frozen definitions directly** — *the same bytes* Knowledge uses, never a re-implementation
//! (architecture `issue-tracker/01-tech-and-data-model.md` §1 + §3.0 + §4.4). This module **re-
//! exports** the shared shapes under the Issues crate's namespace so Issues code (the M4-I1+ board/
//! backlog/filter surfaces) references ONE name for ONE type — it does **not** redefine any of them
//! (EI-01 §7: never a second copy of a contract type). A re-export is the co-ownership: a rename on
//! either side is a single, workspace-wide change, and the [`cdc_13_3_issues_coown`] drift-killer
//! catches a divergence at once.
//!
//! ## What is frozen here vs. what lands later (floors named — VISION §3)
//! - **Frozen now (linked):** the field-type enum, the `ViewSpec`, the `QueryAst` predicate core,
//!   and the `order_key`/LexoRank codec — the byte-identical DEFINITIONS.
//! - **NO Issues data is written yet.** No issue row, no `rank` column, no board scan exists at
//!   ISS-P02. This slice freezes the co-owned *definitions* and proves byte-identity; the Issues
//!   OLTP spine (the `rank text` column, contract 13.3 §5) lands in **ISS-P05**.
//! - **Issues' AST→store compiler is NOT here.** Issues owns its own compiler — the `SetExpr`
//!   push-down lowering (leak-free, no N+1, no post-filter) — which lands in **ISS-P13** (the
//!   AST→OLTP-store compiler), with cost-bounding + the three-tier escalation in **ISS-P14** and the
//!   server-arbitrated `order_key` CAS reorder (the silent-clobber floor) in **ISS-P09**. The
//!   co-equal `ViewSpec` views (board/roadmap/backlog/table/calendar/cycle) land in **ISS-P16**.
//!   This module ships the linked definitions those prompts build their executor ON TOP OF — the
//!   "share the schema language and the view model, not the query planner" split (architecture §3.0).
//!
//! ## The byte-identity invariant (the drift-killer)
//! A unit/encoding mismatch that ships on ONE side calcifies (EI-01 §7). Because Issues links the
//! SAME crate, a `FieldType` rename, a `ViewSpec` field reorder, or an `order_key` midpoint/jitter
//! change is a compile/test failure on BOTH sides simultaneously. The
//! `tests/cdc_13_3_issues_coown.rs` CDC pair replays the shared [`CONFORMANCE_VECTOR`] through the
//! frozen [`OrderKey`] API *from the Issues crate* and serializes the shared [`ViewSpec`] /
//! [`FieldType`] set *from the Issues crate*, asserting **0 byte differences** against Knowledge's
//! frozen outputs — the dated green artifact of the §3.0 exit gate.

// The co-owned shared shapes — RE-EXPORTED (linked), never re-defined. Issues references THESE names.
pub use myelin_query::{
    // The field-type enum + its typed value + the LexoRank `order_key` codec primitives.
    field::{FieldType, FieldValue, Jitter, OrderKey},
    // The shared X-3 anti-drift conformance vector + the created_at+ULID tiebreak (authored once in
    // Knowledge; Issues replays the SAME fixture through its OWN call sites — the byte-identity proof).
    order_key::{tiebreak, ConformanceStep, RankOp},
    // The ONE predicate/query AST grammar (= the EventMatcher core, 3.4) + its textual parser.
    parse_query,
    // The frozen view-model.
    view::{FieldId, SortDir, SortSpec, ViewKind, ViewSpec},
    CmpOp,
    EvalContext,
    Expr,
    Predicate,
    QueryAst,
    CONFORMANCE_VECTOR,
};

/// The Issues-side replay of the shared X-3 [`CONFORMANCE_VECTOR`]: drive each frozen [`RankOp`]
/// through the SAME co-owned [`OrderKey`] operations (`rank_first`/`rank_last`/`rank_between`) and
/// collect the produced base-62 keys. This is exactly what Issues' drag-rank executor (ISS-P09) will
/// do; running it here proves the co-owned encoding produces byte-identical keys from the Issues
/// crate. A drift in the shared midpoint/jitter rule makes this diverge from the vector's frozen
/// `expect` strings — caught immediately, on both sides.
pub fn issues_replay_conformance_vector() -> Vec<String> {
    CONFORMANCE_VECTOR.iter().map(run_step).collect()
}

/// Run one conformance step through the co-owned [`OrderKey`] API (the Issues call site).
fn run_step(step: &ConformanceStep) -> String {
    let key = match step.op {
        RankOp::First { jitter } => OrderKey::rank_first(jitter_of(jitter)),
        RankOp::Last { after, jitter } => {
            let after = after.map(parse_bound);
            OrderKey::rank_last(after.as_ref(), jitter_of(jitter))
        }
        RankOp::Between { lo, hi, jitter } => {
            let lo = lo.map(parse_bound);
            let hi = hi.map(parse_bound);
            OrderKey::rank_between(lo.as_ref(), hi.as_ref(), jitter_of(jitter))
        }
    };
    key.as_str().to_owned()
}

/// Parse a fixture bound through the co-owned [`OrderKey::parse`] (the bounds are hand-authored
/// in-alphabet base-62 strings; an out-of-alphabet bound is a fixture bug, panics loudly).
fn parse_bound(s: &str) -> OrderKey {
    OrderKey::parse(s).expect("a conformance-vector bound is a valid base-62 order_key")
}

/// Build a co-owned [`Jitter`] from two explicit ranks (fixture data is in-range by construction).
fn jitter_of((a, b): (usize, usize)) -> Jitter {
    Jitter::from_ranks(a, b).expect("conformance-vector jitter ranks are in 0..62")
}

/// The Issues-side construction of the canonical board [`ViewSpec`] — built from the co-owned shared
/// shapes (same `ViewKind`, same `QueryAst` grammar, same `FieldId`/`SortSpec`). Serializing THIS
/// and comparing to Knowledge's golden JSON is the view-model byte-identity proof. Issues builds the
/// SAME shape Knowledge's `provider_view` builds — neither side redefines `ViewSpec`.
pub fn issues_canonical_board_view() -> ViewSpec {
    ViewSpec {
        kind: ViewKind::Board,
        filter: parse_query("status == 'open' AND severity >= 3")
            .expect("the co-owned grammar compiles a well-formed Issues board filter"),
        group_by: Some(FieldId::new("status")),
        sort: vec![SortSpec {
            field: FieldId::new("priority"),
            dir: SortDir::Desc,
        }],
        visible: vec![FieldId::new("title"), FieldId::new("assignee")],
        order_field: FieldId::new("order_key"),
    }
}

/// The frozen field-type wire-id set, read from the co-owned enum on the Issues side. Issues' typed/
/// validated flexible fields (architecture §6.1 — the `field` scheme's `type: FieldType`) reference
/// THIS enum; the wire-id list is the byte-identical anchor both co-owners reconcile to.
pub fn issues_field_type_wire_ids() -> Vec<&'static str> {
    FieldType::all().iter().map(|t| t.wire_id()).collect()
}
