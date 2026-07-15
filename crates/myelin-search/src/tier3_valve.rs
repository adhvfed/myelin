//! # `tier3_valve` — the Issues Tier-3 board-escalation valve: byte-identical ACL pre-filter
//! (SRCH-P21 / P-339, M4)
//!
//! **Owning architecture doc:** `search-and-indexing.md` §4.2.4 (the Issues Tier-3
//! board-escalation valve — when a board's filtered scan goes OVER its OLTP budget, the board
//! compiles its query to a Search `query(ast, viewer)` that conjoins **the SAME `Filter{set_expr}`**
//! the OLTP board would have used, so the board and Search apply **byte-identical ACL pre-filter
//! semantics**; no leak, no N+1, on either tier), §4.2 (the permission-aware query pipeline — the ONE
//! `list_objects` conjoin), change #5 (the valve now unblocked, OQ-E frozen). **Contracts:** 6.1
//! `query` (the Tier-3 valve consumer wires here — Issues calls it), 4.3 the `SetExpr` (byte-identical
//! to the OLTP board's). The ENGINE is UNCHANGED (the prompt's DoD) — this is the consumer-side
//! escalation seam, not an engine change.
//!
//! ## What SRCH-P21 ships here — the consumer side of contract 6.1 (the OLTP-budget escalation seam)
//!
//! An Issues **board** runs a filtered scan over its OLTP store (the `myelin-issues` board path). That
//! scan has an **OLTP budget** (a row/cost ceiling, ISS-P14's three-tier escalation). When a board's
//! filtered scan goes OVER that budget — too many candidate rows for the OLTP tier to scan within its
//! latency budget — the board **escalates to Search**: it compiles the board query to a Search
//! `query(ast, viewer)` that conjoins **the SAME `Filter{set_expr}`** the OLTP board would have used.
//!
//! The crux property (the GATE): **byte-identical ACL pre-filter semantics across the two tiers.** The
//! OLTP board's row-visibility decision and the Search valve's posting-list pre-filter MUST admit the
//! **identical** set of documents for the **identical** `set_expr` — 0 leak divergence between the two
//! ACL pre-filters. A doc the OLTP board would hide must NEVER surface through the valve, and a doc the
//! OLTP board would show must surface through the valve (no spurious deny, no leak).
//!
//! ## How parity is GUARANTEED (no second lowering — the coherence crux, EI-01 §7)
//!
//! Byte-identical is not "two implementations that happen to agree" — it is **ONE lowering shared by
//! both tiers**. The valve lowers the board's `SetExpr` through the **SAME**
//! [`crate::pipeline::lower_set_expr`] the live permission-aware [`crate::query`] pipeline uses (the
//! function is `pub(crate)`; the valve does NOT re-implement the algebra). The resulting [`AclFilter`]
//! is the canonical ACL pre-filter:
//! - The **Search valve tier** conjoins that `AclFilter` into the engine query (the posting-list-level
//!   pre-filter, §4.2.1) via the real [`crate::query`] entry — hidden docs never enter the candidate
//!   set, never contribute to counts/IDF.
//! - The **OLTP board tier** decides each row's visibility by [`AclFilter::admits`] over the SAME
//!   lowered filter ([`oltp_board_admits`]). This is the byte-identical reference the OLTP board's row
//!   scan applies: the SAME membership/deny/boolean semantics, evaluated per row instead of at the
//!   posting list.
//!
//! Because both tiers derive their decision from the SAME `AclFilter` produced by the SAME lowering,
//! divergence is structurally impossible: there is no second `SetExpr` interpreter to drift. The valve
//! parity check ([the integration drill](../../tests/drill_srch_p21_tier3_valve_parity.rs)) runs a
//! board query through BOTH tiers and asserts byte-identical visible rows (0 leak divergence) — the
//! property the master-band ISS-D2 board-query-<1s gate relies on for a leak-equivalent escalation
//! path.
//!
//! ## The SetExpr is the board's, byte-identical to its OLTP shape (4.3)
//!
//! The valve does NOT compute a NEW reachable set — it carries the board's OWN `SetExpr` (the one the
//! board's OLTP scan would have conjoined, the SRCH-P09 lowering, 4.3) into Search verbatim. Issues'
//! `list_objects` answer for the board's viewer is the SAME `Filter{set_expr}` whether the board scans
//! OLTP or escalates to Search; the valve threads it through a [`BoardEscalationAuthz`] port so the
//! exact-same `set_expr` reaches Search's conjoin step (NOT a re-derived one — a re-derivation could
//! drift, which is the whole risk the valve exists to eliminate).
//!
//! ## FLOOR named (SRCH-P21 DoD)
//! - **The at-scale board-query latency under the surge** is **SRCH-P25**'s concern (the world-scale
//!   surge drill). The valve gives ISS-D2 a *leak-equivalent escalation path*; the at-scale latency of
//!   that path under the 30× surge is the named M5 follow-on. The valve is the FULL shape at M4 — no
//!   new floor in the valve itself; the latency hardening is downstream. Greppable as
//!   [`Tier3ValveSurgeFloor`].
//! - **No new engine change, no new mutation-core module** — the SRCH-P09 mutation floor (the `SetExpr`
//!   ACL conjoin decision logic in [`crate::pipeline::lower_set_expr`]) still holds on the valve path:
//!   the valve REUSES that exact lowering, so the same mutation tests that pin the conjoin logic pin
//!   the valve's pre-filter. This slice is consumer-side WIRING.
//! - **The live Issues board OLTP scan + its budget meter** live in `myelin-issues` (ISS-P14's
//!   three-tier escalation). Here the valve ships the Search-side consumer seam + the byte-identical
//!   reference the OLTP board applies; [`OltpBudget`] models the over-budget decision the board makes
//!   (the row-ceiling escalation trigger), so the seam is exercisable end-to-end without a live OLTP.

use myelin_identity::{
    Consistency, ListObjectsResult, ObjectType, Permission, Principal, Result as AuthzResult,
    SetExpr, Zookie,
};
use myelin_query::QueryAst;

use crate::engine::{AclFilter, IndexBackend};
use crate::pipeline::{
    self, ListObjectsPort, Page, QueryError, QueryStats, RankedResults, RelationalLeaf,
    ReverseIndexAnswer, RevisionWatermark, ScopedEngine,
};

/// **The OLTP budget a board's filtered scan runs under (§4.2.4 / ISS-P14).** A board scans its OLTP
/// store within a row/cost ceiling; when the candidate count for the board's filter goes OVER the
/// ceiling, the OLTP tier cannot serve the scan within its latency budget and the board ESCALATES to
/// Search (the Tier-3 valve). This models that over-budget decision — the escalation TRIGGER — so the
/// valve seam is exercisable. The real budget meter (the live row counter) lives in `myelin-issues`;
/// the *decision* (`candidate_rows > max_rows`) is byte-identical.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OltpBudget {
    /// The maximum candidate rows the OLTP tier scans before the board escalates to Search. A board
    /// query whose candidate set exceeds this is OVER budget → the Tier-3 valve fires.
    pub max_rows: usize,
}

impl OltpBudget {
    /// The default board OLTP budget (the row ceiling the three-tier escalation uses before Tier-3).
    /// A board filtered scan over more candidate rows than this escalates to Search.
    pub const DEFAULT_MAX_ROWS: usize = 10_000;

    /// A budget with an explicit row ceiling.
    pub fn new(max_rows: usize) -> OltpBudget {
        OltpBudget { max_rows }
    }

    /// **Is a board scan with `candidate_rows` OVER budget (§4.2.4)?** When true, the board cannot
    /// serve the scan on the OLTP tier within its latency budget and the Tier-3 valve fires (escalate
    /// to Search with the SAME `Filter{set_expr}`). The decision is `candidate_rows > max_rows` — the
    /// SAME comparison the live board budget meter makes.
    pub fn is_over_budget(&self, candidate_rows: usize) -> bool {
        candidate_rows > self.max_rows
    }
}

impl Default for OltpBudget {
    fn default() -> OltpBudget {
        OltpBudget::new(OltpBudget::DEFAULT_MAX_ROWS)
    }
}

/// **A board's query as the valve carries it across the OLTP-budget escalation seam (§4.2.4).** The
/// board's full-text/structured `ast` (the same frozen [`QueryAst`] the OLTP board compiles + the
/// Search engine compiles — ONE AST, no second query language) PLUS the board's **ACL pre-filter
/// `set_expr`** (the SRCH-P09 `SetExpr` the OLTP board would have conjoined, 4.3 — byte-identical
/// whether the board scans OLTP or escalates) PLUS the consistency `zookie` the ACL was computed at.
///
/// The valve threads the SAME `set_expr` into Search's conjoin step (NOT a re-derived one). The board
/// is the authority for both halves — Search compiles the SAME `ast` and conjoins the SAME `set_expr`.
#[derive(Clone, Debug)]
pub struct BoardQuery {
    /// The board's query AST (the frozen [`QueryAst`] — the SAME shape the OLTP board compiles and the
    /// Search engine compiles). One query language across both tiers (§4.6 / 6.1).
    pub ast: QueryAst,
    /// The board's ACL pre-filter — the `SetExpr` the OLTP board would have conjoined (the SRCH-P09
    /// lowering, 4.3). Byte-identical whether the board scans OLTP or escalates to Search.
    pub set_expr: SetExpr,
    /// The consistency zookie the ACL pre-filter was computed at (contract 4.3). Threaded so the valve
    /// honours the SAME snapshot the OLTP board's filter rode (the reverse-index watermark, §4.2.3).
    pub zookie: Zookie,
}

impl BoardQuery {
    /// A board query carrying its AST + ACL `set_expr` + the zookie its ACL was computed at.
    pub fn new(ast: QueryAst, set_expr: SetExpr, zookie: Zookie) -> BoardQuery {
        BoardQuery {
            ast,
            set_expr,
            zookie,
        }
    }
}

/// **The `list_objects` port the valve hands Search to conjoin the BOARD's `set_expr` verbatim.**
/// Search's [`crate::query`] calls `list_objects` exactly once to get the ACL pre-filter; the valve's
/// port returns the board's OWN `Filter{set_expr}` (the byte-identical 4.3 answer — NOT a re-derived
/// reachable set). This is how the SAME `set_expr` the OLTP board would have conjoined reaches Search's
/// conjoin step: the valve does not re-compute reachability, it carries the board's filter across the
/// seam.
///
/// A [`reverse_resolver`](BoardEscalationAuthz::with_reverse_resolver) may be wired when the board's
/// `set_expr` contains relational forms (`InRelation`/`TupleSet`) — the SAME reverse-index JOIN the
/// live `list_objects` consumer uses (SRCH-P09); without it, a relational form is a loud `Unavailable`
/// (deny-when-unsure, ADR-03 — never a silent widen), exactly as the bounded-set port behaves.
pub struct BoardEscalationAuthz<'a> {
    set_expr: SetExpr,
    zookie: Zookie,
    expect_type: ObjectType,
    reverse_resolver: Option<&'a dyn ReverseResolver>,
}

/// **The reverse-index JOIN the valve delegates relational `SetExpr` leaves to (SRCH-P09).** When the
/// board's `set_expr` carries `InRelation`/`TupleSet`, the valve resolves those leaves through the
/// SAME per-tenant authz reverse index the live `list_objects` consumer JOINs against — so the
/// relational forms lower byte-identically on the valve path too. Narrowed to the relational-leaf
/// resolve so a board whose ACL is a pure bounded set needs no resolver at all.
pub trait ReverseResolver {
    /// Resolve ONE relational `SetExpr` leaf to its co-located visible-id set + the reverse-index
    /// revision it was served at (contract 4.10 — the valve checks the revision against the watermark,
    /// never reads stale; SRCH-P09).
    fn resolve(
        &self,
        subject: &Principal,
        form: &RelationalLeaf,
        required: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer>;
}

impl<'a> BoardEscalationAuthz<'a> {
    /// Carry a board's `Filter{set_expr}` (computed at `zookie`, for objects of `expect_type`) across
    /// the escalation seam. The bounded-set path (`All`/`None`/`Ids`/`NotIds` + their boolean
    /// composition) needs no reverse resolver; a relational leaf without one is a loud `Unavailable`.
    pub fn new(
        set_expr: SetExpr,
        zookie: Zookie,
        expect_type: ObjectType,
    ) -> BoardEscalationAuthz<'a> {
        BoardEscalationAuthz {
            set_expr,
            zookie,
            expect_type,
            reverse_resolver: None,
        }
    }

    /// Wire the reverse-index JOIN for a board whose `set_expr` carries relational leaves (SRCH-P09 —
    /// the SAME reverse index the live consumer JOINs against).
    pub fn with_reverse_resolver(
        mut self,
        resolver: &'a dyn ReverseResolver,
    ) -> BoardEscalationAuthz<'a> {
        self.reverse_resolver = Some(resolver);
        self
    }
}

impl ListObjectsPort for BoardEscalationAuthz<'_> {
    fn list_objects(
        &self,
        _subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        // The valve only ever escalates a READ board scan for the board's object type. A mismatch is a
        // mis-wired caller (the board escalated the wrong query) — surfaced loudly, never silently
        // widened (the seam carries the board's OWN filter; it must be the board's own type/read).
        if permission != &Permission(pipeline::READ_PERMISSION.to_string()) {
            return Err(myelin_identity::AuthzError::Unavailable(format!(
                "the Tier-3 valve escalates only a `read` board scan; got permission `{}` \
                 (the valve carries the board's own ACL pre-filter — it never widens the permission)",
                permission.0
            )));
        }
        if ty != &self.expect_type {
            return Err(myelin_identity::AuthzError::Unavailable(format!(
                "the Tier-3 valve escalated a board scan for type `{}` but Search asked for type `{}` \
                 (the seam carries the board's own `set_expr` — a type mismatch is a mis-wired board)",
                self.expect_type.0, ty.0
            )));
        }
        // The board's OWN filter, byte-identical to its OLTP shape (4.3) — NOT a re-derived set.
        Ok(ListObjectsResult::Filter {
            set_expr: self.set_expr.clone(),
            zookie: self.zookie.clone(),
        })
    }

    fn resolve_relation(
        &self,
        subject: &Principal,
        form: &RelationalLeaf,
        required: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        match self.reverse_resolver {
            // The SAME reverse-index JOIN the live consumer uses (SRCH-P09) — the relational leaf
            // lowers byte-identically on the valve path too.
            Some(r) => r.resolve(subject, form, required),
            // No resolver wired: a relational leaf cannot be resolved — loud Unavailable, never a
            // silent widen (deny-when-unsure, ADR-03), exactly as the default bounded-set port behaves.
            None => Err(myelin_identity::AuthzError::Unavailable(
                "the Tier-3 valve was handed a relational `SetExpr` leaf but no reverse-index \
                 resolver is wired — a relational form cannot be resolved (deny-when-unsure, ADR-03; \
                 wire `with_reverse_resolver` for a board whose ACL carries InRelation/TupleSet)"
                    .into(),
            )),
        }
    }
}

/// **THE TIER-3 BOARD-ESCALATION VALVE (contract 6.1 / §4.2.4) — the consumer side.** An over-budget
/// Issues board escalates its filtered scan to Search: it compiles the board query to a Search
/// `query(ast, viewer)` that conjoins **the SAME `Filter{set_expr}`** the OLTP board would have used
/// (carried verbatim through [`BoardEscalationAuthz`]). Search lowers that `set_expr` through the SAME
/// [`crate::pipeline::lower_set_expr`] the live pipeline uses and conjoins it at the posting-list level
/// — so the board and Search apply **byte-identical ACL pre-filter semantics**. No leak, no N+1, on
/// either tier.
///
/// - `engine` — the viewer's-tenant [`ScopedEngine`] (the partition key; cross-tenant 0, SRCH-D3).
/// - `board` — the [`BoardQuery`] (the board's `ast` + its `set_expr` + the zookie).
/// - `viewer` — the **verified** board viewer (the tenant is `viewer.tenant`, never a path).
/// - `ty` — the board's object type (`issue`); the seam asserts Search asks for THIS type.
/// - `at` — the read consistency (the zookie watermark the reverse-index JOIN honours, §4.2.3).
/// - `reverse_resolver` — the SRCH-P09 reverse-index JOIN for a relational `set_expr` (None for a
///   pure bounded-set board ACL — then a relational leaf would be a loud `Unavailable`).
///
/// Returns the SAME [`RankedResults`] the live [`crate::query`] returns — the valve adds NO new query
/// path, it is the live pipeline driven with the board's filter (the engine is UNCHANGED).
#[allow(clippy::too_many_arguments)]
pub fn escalate_to_search<B: IndexBackend>(
    engine: &ScopedEngine<'_, B>,
    board: &BoardQuery,
    viewer: &Principal,
    ty: &ObjectType,
    at: &Consistency,
    page: Page,
    stats: &QueryStats,
    reverse_resolver: Option<&dyn ReverseResolver>,
) -> Result<RankedResults, QueryError> {
    // The valve hands Search the board's OWN filter through the escalation port — Search's conjoin
    // step (the ONE `list_objects` call) receives the byte-identical `set_expr` (4.3), and lowers it
    // through the SAME `lower_set_expr` the live pipeline uses. There is NO second lowering.
    let mut authz =
        BoardEscalationAuthz::new(board.set_expr.clone(), board.zookie.clone(), ty.clone());
    if let Some(r) = reverse_resolver {
        authz = authz.with_reverse_resolver(r);
    }
    // The real, unchanged permission-aware query entry (6.1) — the valve is a CONSUMER, not a new path.
    pipeline::query(engine, &authz, &board.ast, viewer, ty, at, page, stats)
}

/// **The byte-identical reference the OLTP board's row scan applies (§4.2.4) — the parity anchor.**
/// Lower the board's `set_expr` through the SAME [`crate::pipeline::lower_set_expr`] the Search valve
/// (and the live pipeline) uses, then decide each candidate row's visibility by [`AclFilter::admits`]
/// over the resulting filter. This is the OLTP board's ACL pre-filter expressed as a per-row predicate
/// — the SAME membership/deny/boolean semantics Search conjoins at the posting-list level, applied row
/// by row instead.
///
/// Returns the visible subset of `candidate_rows` (the rows the OLTP board would surface), in input
/// order. Because the filter is produced by the SAME lowering Search's valve uses, the visible set is
/// **byte-identical** to the valve's visible set for the SAME `set_expr` over the SAME corpus — the
/// 0-leak-divergence property the parity drill proves.
///
/// `subject`/`reverse_resolver` are threaded for a relational `set_expr` (the SRCH-P09 JOIN); a pure
/// bounded-set board ACL needs neither. A relational leaf with no resolver is a loud error (the OLTP
/// board could not resolve it either — deny-when-unsure, never a silent widen).
pub fn oltp_board_admits(
    set_expr: &SetExpr,
    candidate_rows: &[String],
    subject: &Principal,
    zookie: &Zookie,
    reverse_resolver: Option<&dyn ReverseResolver>,
) -> Result<Vec<String>, QueryError> {
    let acl = board_acl_filter(set_expr, subject, zookie, reverse_resolver)?;
    Ok(candidate_rows
        .iter()
        // A candidate row is a single id (the OLTP board scans by one key); pass it as BOTH doc_id
        // and acl_object so the two-field membership reduces to the row's membership — byte-identical
        // to the valve for the SAME set_expr (the parity property).
        .filter(|row| acl.admits(row, row))
        .cloned()
        .collect())
}

/// **Lower the board's `set_expr` to the canonical [`AclFilter`] (the ONE lowering, shared with the
/// live pipeline).** Both the OLTP board reference ([`oltp_board_admits`]) and — via Search's
/// `list_objects` conjoin — the Search valve derive their ACL pre-filter from THIS function, so there
/// is no second `SetExpr` interpreter to drift. Exposed so a parity test can assert the OLTP board's
/// filter is the SAME object the valve conjoins (byte-identical, not merely equal-by-coincidence).
///
/// The relational JOIN honours the revision watermark derived from the board's `zookie` (§4.2.3 — a
/// reverse-index revision below the watermark is a loud [`QueryError::StaleReverseIndex`], never read
/// stale), exactly as the live pipeline does.
pub fn board_acl_filter(
    set_expr: &SetExpr,
    subject: &Principal,
    zookie: &Zookie,
    reverse_resolver: Option<&dyn ReverseResolver>,
) -> Result<AclFilter, QueryError> {
    // The watermark the reverse-index JOIN must honour — the SAME zookie→revision decoding the live
    // pipeline uses (`watermark_from_zookie`), so the valve's watermark is byte-identical to the
    // pipeline's. The OLTP board ran its scan at the SAME snapshot.
    let required = pipeline::watermark_from_zookie(&zookie.0);
    // A throwaway stats sink — the OLTP-board reference does not feed the live no-N+1 telemetry (that
    // is the Search valve's `query` call); it only needs the lowered filter.
    let stats = QueryStats::new();
    let port = BoundedSetOnly { reverse_resolver };
    pipeline::lower_set_expr(set_expr, subject, &port, &required, &stats)
}

/// A minimal [`ListObjectsPort`] that exists ONLY so [`board_acl_filter`] can drive
/// [`crate::pipeline::lower_set_expr`] (which needs a port to resolve relational leaves). It never
/// answers `list_objects` (the board's filter is already known); it delegates a relational leaf to the
/// wired [`ReverseResolver`] (the SRCH-P09 JOIN) or fails loudly when none is wired.
struct BoundedSetOnly<'a> {
    reverse_resolver: Option<&'a dyn ReverseResolver>,
}

impl ListObjectsPort for BoundedSetOnly<'_> {
    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        // `lower_set_expr` is called with the board's `set_expr` directly — `list_objects` is never
        // reached on this path. A call here would be a bug (the reference lowers a known filter).
        Err(myelin_identity::AuthzError::Unavailable(
            "board_acl_filter lowers a known board `set_expr` directly — list_objects is not part of \
             the OLTP-board reference path"
                .into(),
        ))
    }

    fn resolve_relation(
        &self,
        subject: &Principal,
        form: &RelationalLeaf,
        required: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        match self.reverse_resolver {
            Some(r) => r.resolve(subject, form, required),
            None => Err(myelin_identity::AuthzError::Unavailable(
                "the OLTP-board reference was handed a relational `SetExpr` leaf but no reverse-index \
                 resolver is wired (deny-when-unsure, ADR-03)"
                    .into(),
            )),
        }
    }
}

/// **FLOOR (named) — the at-scale board-query latency under the surge → SRCH-P25.** A greppable
/// zero-sized marker: the Tier-3 valve gives ISS-D2 a *leak-equivalent escalation path* (byte-identical
/// ACL pre-filter, the full shape at M4). The **at-scale latency of that escalation path under the 30×
/// world-scale surge** is the M5 follow-on **SRCH-P25** — the surge changes the valve's LATENCY budget,
/// never its leak-equivalence. Named so the M4 valve is not mistaken for the world-scale-hardened path.
#[derive(Clone, Copy, Debug)]
pub struct Tier3ValveSurgeFloor;

impl Tier3ValveSurgeFloor {
    /// The M5 follow-on that hardens the valve's at-scale board-query latency under the surge.
    pub const SURGE_FOLLOW_ON: &'static str = "SRCH-P25";
    /// The master-band board-query gate the valve gives a leak-equivalent escalation path.
    pub const SUPPORTED_GATE: &'static str = "ISS-D2";
    /// The byte-identical `SetExpr` lowering both tiers share (no second interpreter — the coherence
    /// crux). The valve REUSES this; the mutation floor on it (SRCH-P09) covers the valve's pre-filter.
    pub const SHARED_LOWERING: &'static str = "pipeline::lower_set_expr (SRCH-P09)";
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{ObjectId, PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn subject() -> Principal {
        Principal::stub(
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn zk(s: &str) -> Zookie {
        Zookie(s.into())
    }

    fn oid(s: &str) -> ObjectId {
        ObjectId(s.into())
    }

    /// **The OLTP-budget escalation decision is `candidate_rows > max_rows`.** Under budget ⇒ the
    /// board serves on the OLTP tier (no valve); over budget ⇒ the Tier-3 valve fires.
    #[test]
    fn over_budget_triggers_escalation() {
        let budget = OltpBudget::new(100);
        assert!(!budget.is_over_budget(100), "exactly at budget is NOT over");
        assert!(!budget.is_over_budget(50), "under budget stays on OLTP");
        assert!(
            budget.is_over_budget(101),
            "over budget escalates to Search"
        );
        assert_eq!(OltpBudget::default().max_rows, OltpBudget::DEFAULT_MAX_ROWS);
    }

    /// **The bounded-set board ACL lowers byte-identically through the SHARED lowering.** An `Ids`
    /// allow-set and a `NotIds` deny-set both lower to the SAME `AclFilter` the live pipeline produces;
    /// the OLTP board's `admits` over that filter is the byte-identical reference the valve conjoins.
    #[test]
    fn bounded_set_board_acl_admits_byte_identically() {
        // Allow-set: only A and C are visible.
        let allow = SetExpr::Ids(vec![oid("A"), oid("C")]);
        let rows = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let visible = oltp_board_admits(&allow, &rows, &subject(), &zk("z@0"), None).unwrap();
        assert_eq!(visible, vec!["A".to_string(), "C".to_string()]);

        // The SAME lowering produces the SAME AclFilter the valve conjoins (no second interpreter).
        let acl = board_acl_filter(&allow, &subject(), &zk("z@0"), None).unwrap();
        assert_eq!(acl, AclFilter::Ids(vec!["A".into(), "C".into()]));
        assert!(acl.admits("A", "A") && !acl.admits("B", "B") && acl.admits("C", "C"));
    }

    /// **A `Difference` board ACL (visible-under-left MINUS reachable-under-right) lowers to the SAME
    /// `left AND NOT right` the live pipeline composes — the boolean composition is byte-identical.**
    #[test]
    fn difference_board_acl_is_left_and_not_right() {
        // Left = {A, B, C}; Right (excluded, e.g. `- confidential`) = {B}. Visible = {A, C}.
        let set_expr = SetExpr::Difference(
            Box::new(SetExpr::Ids(vec![oid("A"), oid("B"), oid("C")])),
            Box::new(SetExpr::Ids(vec![oid("B")])),
        );
        let rows = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let visible = oltp_board_admits(&set_expr, &rows, &subject(), &zk("z@0"), None).unwrap();
        assert_eq!(
            visible,
            vec!["A".to_string(), "C".to_string()],
            "the confidential B is excluded by the set-difference (the `- confidential` shape)"
        );
        let acl = board_acl_filter(&set_expr, &subject(), &zk("z@0"), None).unwrap();
        assert!(acl.admits("A", "A") && !acl.admits("B", "B") && acl.admits("C", "C"));
    }

    /// **A relational board ACL with NO reverse resolver is a loud error (deny-when-unsure, never a
    /// silent widen).** The OLTP board could not resolve a relational leaf without the reverse index
    /// either — the reference fails loudly, exactly as the live pipeline does.
    #[test]
    fn relational_without_resolver_is_loud_not_widened() {
        let set_expr = SetExpr::InRelation {
            relation: myelin_identity::RelName("viewer".into()),
            via_column: myelin_identity::ColRef {
                table: "issue".into(),
                column: "id".into(),
            },
        };
        let err = board_acl_filter(&set_expr, &subject(), &zk("z@5"), None).unwrap_err();
        assert!(
            matches!(err, QueryError::Authz(_)),
            "a relational leaf with no reverse resolver is a loud Authz error, never a silent widen"
        );
    }

    /// **The relational board ACL JOINs through the wired resolver + honours the watermark (SRCH-P09).**
    /// With a resolver serving a fresh-enough revision, the relational leaf lowers to the visible-id
    /// set; a STALE revision is a loud `StaleReverseIndex` (never read stale).
    #[test]
    fn relational_with_resolver_joins_and_honours_watermark() {
        struct Resolver {
            visible: Vec<String>,
            revision: u64,
        }
        impl ReverseResolver for Resolver {
            fn resolve(
                &self,
                _s: &Principal,
                _f: &RelationalLeaf,
                _required: &RevisionWatermark,
            ) -> AuthzResult<ReverseIndexAnswer> {
                Ok(ReverseIndexAnswer {
                    object_ids: self.visible.clone(),
                    revision: RevisionWatermark(self.revision),
                })
            }
        }
        let set_expr = SetExpr::TupleSet {
            index: myelin_identity::AuthzIndexRef("authz_visible".into()),
        };

        // Fresh: the reverse index serves revision 10 >= the z@10 watermark → the JOIN resolves {A}.
        let fresh = Resolver {
            visible: vec!["A".into()],
            revision: 10,
        };
        let acl = board_acl_filter(&set_expr, &subject(), &zk("z@10"), Some(&fresh)).unwrap();
        assert_eq!(acl, AclFilter::Ids(vec!["A".into()]));

        // Stale: revision 5 < the z@10 watermark → a loud StaleReverseIndex (never read stale).
        let stale = Resolver {
            visible: vec!["A".into()],
            revision: 5,
        };
        let err = board_acl_filter(&set_expr, &subject(), &zk("z@10"), Some(&stale)).unwrap_err();
        assert!(
            matches!(
                err,
                QueryError::StaleReverseIndex {
                    required: 10,
                    served: 5,
                    ..
                }
            ),
            "a reverse-index revision below the watermark is refused (the new-enemy problem)"
        );
    }

    /// **The floor marker names the at-scale surge follow-on (SRCH-P25) + the shared lowering.**
    #[test]
    fn floor_marker_names_the_surge_follow_on() {
        assert_eq!(Tier3ValveSurgeFloor::SURGE_FOLLOW_ON, "SRCH-P25");
        assert_eq!(Tier3ValveSurgeFloor::SUPPORTED_GATE, "ISS-D2");
        assert!(Tier3ValveSurgeFloor::SHARED_LOWERING.contains("lower_set_expr"));
    }
}
