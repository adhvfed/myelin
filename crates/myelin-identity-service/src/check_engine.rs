//! # `check_engine` — the depth-bounded Zanzibar userset-rewrite `check` (P-ID-09 → P-067)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §8 (the `check` algorithm — **depth-bounded userset-rewrite**, **memoised-per-request**,
//! **fail-closed on genuine uncertainty**, **evaluated at the zookie snapshot**; the three-layer
//! cache faces), §8.6 (the `CaveatContext` rider with the **literal-only** predicate floor for M1),
//! §6 (the raw S3 tuples `⟨object#relation@subject⟩` this engine reads), §10 (fail-closed
//! correctness — deny when genuinely unsure).
//!
//! **Contract-index:** row **4.2** (`check(subject, permission, object, zookie?, caveat?) →
//! {Allow | Deny | Conditional}`) — **OWNED** here (the engine + the `CaveatContext` literal-only
//! floor). Row **4.6** (the raw S3 tuples written by `write_tuples`) — **CONSUMED** (this engine
//! reads the [`crate::tuple_store::TupleStore`] the P-ID-08 write path feeds).
//!
//! ## What this module ships (P-ID-09)
//! The body of the [`myelin_identity::IdentityService::check`] slot the shell wired fail-closed
//! (P-ID-04): a [`CheckEngine`] that, given a verified `(tenant, region)` scope, evaluates
//! `check(subject, permission, object, zookie?, caveat?)` against the **raw S3 tuples** (the
//! namespace engine that compiles fragments into permissions is P-ID-10; this floor resolves
//! **direct grants + simple inherited relations** via the Zanzibar **userset-rewrite** core, which
//! is what P-ID-10 builds its operator set on top of).
//!
//! ### The four load-bearing properties (the GATE)
//! 1. **Fail-closed on genuine uncertainty** (§8/§10, ADR-03). A malformed query (an unparseable
//!    object ref, an empty permission, a suspended/disabled subject) returns [`Decision::Deny`] —
//!    **never** `Allow`. The count of silent-allows-on-uncertainty is **0**: there is no code path
//!    from "I am not sure" to `Allow`. This is the mutation-tested mandatory-core branch: a mutation
//!    that turns a deny-on-uncertainty into `Allow` is caught by [`tests`].
//! 2. **Evaluated at the zookie snapshot** (§8.4). A `check` carries an `at_least` zookie; the
//!    engine reads tuples **at-or-before** that snapshot — a check at an older zookie does **not**
//!    see a newer tuple. (The fail-static-cache bypass on a `Strong` read is §10 / P-ID-15; this
//!    engine reads the authoritative store at the snapshot, the correctness floor under that cache.)
//! 3. **Depth-bounded** (§8 — depth-bounded userset-rewrite, never unbounded recursion). The
//!    userset rewrite recurses through `object#relation` subject usersets; the recursion is bounded
//!    by [`MAX_REWRITE_DEPTH`]. A chain deeper than the bound is **genuine uncertainty** → fail-closed
//!    `Deny` (it is **not** allowed by exhausting the budget). Unbounded recursion is structurally
//!    impossible.
//! 4. **Memoised-per-request** (§8 — the subproblem/userset cache face). One `check` call carries a
//!    per-request memo keyed by `(subject, relation, object)`; the **same subproblem is computed
//!    once** even when the userset graph re-converges on it. The memo lives for exactly one request
//!    (it is **not** a cross-request cache — that is S6, the fail-static cache, §10) so it cannot
//!    serve a stale grant.
//!
//! ## The `CaveatContext` evaluator — the full `QueryAst` predicate core (P-ID-22, P-133)
//! **The M1 literal-only floor is CLOSED here.** The `caveat` rider now evaluates through the ONE
//! platform predicate language — the bounded, DoS-hardened, statically-cost-bounded
//! `myelin_query::QueryAst` interpreter (contract 3.4, the `EventMatcher` core; no UDFs/loops/
//! recursion). A caveat carries a [`myelin_query::Predicate`] tree (the field/transition ABAC
//! predicate the namespace fragment declares); the [`CaveatContext`]'s `attrs` supply the runtime
//! context the predicate reads. The evaluator ([`eval_caveat_predicate`]) maps the interpreter's
//! outcome to the decision: a **satisfied** predicate keeps the `Allow`; a **violated** one denies;
//! a predicate referencing context the caller did **not** supply (an unbound variable) returns
//! [`Decision::Conditional`] (the caller supplies it) — **never a silent `Allow`** (§8.6). There is
//! now **exactly ONE predicate language in the platform** (EI-01 §7).
//!
//! The frozen `check` ABI carries `caveat: Option<&CaveatContext>` (the context + the field/
//! transition target). The legacy self-describing `__caveat_*` encoding the literal-only floor used
//! is preserved as a thin compatibility bridge ([`eval_caveat`]) that LOWERS that encoding to a real
//! `QueryAst` predicate and routes it through the one core — so the M1 (P-ID-09) literal cases pass
//! unchanged with **no second predicate language**. The non-literal field/transition caveat
//! INSTANCES (their predicates wired onto the namespace-fragment tuples) land with their subsystems:
//! Git/Knowledge **P-ID-25/P-ID-26** and Issues/Knowledge **P-ID-29/P-ID-30** (named follow-ons).
//!
//! ## Floors named (frozen now → bodies in a later prompt)
//! - **The namespace/permission engine is P-ID-10 (P-068).** This floor treats `permission` as a
//!   relation name and resolves it through the userset rewrite over raw tuples (direct +
//!   inherited-via-userset). The four named Zanzibar operators (union / intersect / exclusion /
//!   tuple-to-userset) compiled from a fragment land in P-ID-10; the rewrite **core** they evaluate
//!   through is here (the architecture: "check here resolves direct + simple inherited relations as
//!   the core engine matures alongside").
//! - **The literal-only `CaveatContext` → the full `QueryAst` core: CLOSED in P-ID-22 (P-133).**
//!   The evaluator now runs on the one `myelin_query::QueryAst` interpreter; the floor named in
//!   P-ID-09 is closed (the follow-on caveat INSTANCES land with their subsystems, named above).
//! - **The fail-static cache (S6) + the zookie-bypass is P-ID-15 (P-073).** This engine reads the
//!   authoritative store at the snapshot (the correctness floor the cache is layered over); the
//!   bounded-staleness availability face is P-ID-15.

use crate::tuple_store::TupleStore;
use myelin_identity::{
    CaveatContext, Consistency, Decision, Literal, Principal, PrincipalStatus, RelName, Zookie,
};
use myelin_query::{CmpOp, EvalContext, EvalError, Expr, Predicate, QueryAst};
use myelin_storage::TenantScope;
use myelin_tenancy::ArtifactRef;
use std::collections::HashMap;

/// The maximum userset-rewrite recursion depth (§8 — the depth-bounded evaluation). A grant chain
/// (org→team→project→repo inheritance, plus userset re-references) deeper than this is treated as
/// **genuine uncertainty** and fails closed (`Deny`) — it is never allowed by exhausting the budget,
/// and the recursion is structurally bounded (no unbounded stack growth). Sixteen comfortably
/// exceeds the deepest legitimate org→team→project→sub-page chain while staying a hard ceiling
/// (Refs/KN use the same depth-16 ceiling for their bounded traversals).
pub const MAX_REWRITE_DEPTH: usize = 16;

/// The userset-subject separator (Zanzibar `object#relation`). A tuple whose `subject` is the
/// string `"object#relation"` is a **userset** — "everyone who has `relation` on `object`" — rather
/// than a concrete principal. The rewrite expands a userset subject by recursing into
/// `check(query_subject, relation, object)`. (A concrete principal id never contains `#`.)
pub const USERSET_SEP: char = '#';

/// The depth-bounded Zanzibar userset-rewrite `check` engine (contract 4.2; architecture §8).
///
/// A thin evaluator over the raw S3 tuples ([`TupleStore`]). It owns **no** state of its own — the
/// per-request memo is created fresh on every [`CheckEngine::check`] call (memoised-per-request,
/// never cross-request), so a memo can never serve a stale grant. Cloneable handle.
#[derive(Clone)]
pub struct CheckEngine {
    tuples: TupleStore,
}

impl CheckEngine {
    /// Build the engine over the S3 tuple store the `write_tuples` path (P-ID-08) feeds.
    pub fn new(tuples: TupleStore) -> CheckEngine {
        CheckEngine { tuples }
    }

    /// **`check(subject, permission, object, zookie?, caveat?) → Allow | Deny | Conditional`**
    /// (contract 4.2; architecture §8) — the depth-bounded, memoised-per-request, fail-closed,
    /// zookie-snapshot userset-rewrite evaluation.
    ///
    /// - `scope` is the verified `(tenant, region)` partition (minted from a verified token, never a
    ///   path — the tenant-predicate floor; a check structurally cannot reach another tenant's
    ///   tuples).
    /// - `subject` is the principal asking. A **suspended/disabled** subject fails closed (`Deny`)
    ///   before any tuple is read (ID-D1: disabled-user → zero access).
    /// - `permission` is the relation/permission name (on this raw-tuple floor, resolved as a
    ///   relation through the userset rewrite; the compiled-permission namespace engine is P-ID-10).
    /// - `object` is the `ArtifactRef` of the object the action targets; an unparseable ref is
    ///   **genuine uncertainty** → fail-closed `Deny`.
    /// - `at` is the consistency token; the engine reads tuples **at-or-before** `at.at_least` (the
    ///   zookie snapshot). A `Strong` read is the authoritative store at the snapshot.
    /// - `caveat` is the optional field/transition `CaveatContext` (literal-only floor): a satisfied
    ///   literal predicate keeps the `Allow`; a violated one becomes `Deny`; a predicate needing
    ///   absent context becomes `Conditional` (never a silent allow).
    ///
    /// **Fail-closed everywhere uncertain:** every "I am not sure" path (malformed input, depth
    /// exhaustion, an un-evaluable caveat reference) returns `Deny`/`Conditional`, never `Allow`.
    pub fn check(
        &self,
        scope: &TenantScope,
        subject: &Principal,
        permission: &RelName,
        object: &ArtifactRef,
        at: &Consistency,
        caveat: Option<&CaveatContext>,
    ) -> Decision {
        // --- (1) Fail-closed input validation (genuine uncertainty → Deny, never Allow). ---
        // A suspended/disabled subject has zero access (ID-D1) — denied before any tuple is read.
        if subject.status != PrincipalStatus::Active {
            return Decision::Deny;
        }
        // An empty permission is a malformed query — fail-closed.
        if permission.0.trim().is_empty() {
            return Decision::Deny;
        }
        // Parse the object ref → the object id the tuples key on. An unparseable ref is genuine
        // uncertainty (we cannot know what is being checked) → fail-closed Deny.
        let object_id = match object_id_of(object) {
            Some(id) => id,
            None => return Decision::Deny,
        };

        // --- (2) The zookie-snapshot tuple view. ---
        // Read this tenant's tuples at-or-before the requested snapshot (a check at an older zookie
        // does NOT see a newer tuple). `tuples_in` is scoped to the verified `(tenant, region)`
        // (no cross-tenant query path). We index them by object id for the rewrite.
        let snapshot = self.snapshot_view(scope, &at.at_least);

        // --- (3) The depth-bounded, memoised userset rewrite. ---
        let mut memo: HashMap<MemoKey, bool> = HashMap::new();
        let granted = snapshot.has_relation(
            &subject.principal_id.0,
            &permission.0,
            &object_id,
            0,
            &mut memo,
        );

        // A depth-exhausted / un-resolvable rewrite returns `false` (fail-closed): the rewrite never
        // returns `true` by exhausting the budget, so a `false` here is either a genuine no-grant or
        // genuine uncertainty — both correctly deny.
        if !granted {
            return Decision::Deny;
        }

        // --- (4) The CaveatContext literal-only rider (off the hot list_objects path, §8.6). ---
        // The relation grant holds; if a caveat is present, evaluate the literal predicate. A
        // satisfied caveat keeps the Allow; a violated one denies; an un-evaluable one is
        // Conditional (the caller supplies the missing context) — NEVER a silent allow.
        match caveat {
            None => Decision::Allow,
            Some(cav) => eval_caveat(cav),
        }
    }

    /// The **direct subject strings** of `object#relation` at the zookie snapshot — each either a
    /// concrete principal id (`p:alice`) or a userset (`team:eng#view`). The ReBAC namespace engine
    /// (P-ID-10) uses this to walk a **tuple-to-userset** inheritance edge: it reads the parent
    /// usersets named by the child's `tupleset` relation, then resolves the parent's *compiled
    /// permission* (not a raw relation) through the permission-aware engine. Scoped to the verified
    /// `(tenant, region)` — there is no cross-tenant read path (ID-D3).
    pub fn direct_subjects(
        &self,
        scope: &TenantScope,
        object: &ArtifactRef,
        relation: &RelName,
        at: &Consistency,
    ) -> Vec<String> {
        let object_id = match object_id_of(object) {
            Some(id) => id,
            None => return Vec::new(),
        };
        let snapshot = self.snapshot_view(scope, &at.at_least);
        snapshot
            .by_object
            .get(&object_id)
            .map(|tuples| {
                tuples
                    .iter()
                    .filter(|t| t.relation == relation.0)
                    .map(|t| t.subject.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Build the zookie-snapshot view: this tenant's tuples whose write zookie is **at-or-before**
    /// the requested snapshot, indexed by object id. Scoped to the verified `(tenant, region)` —
    /// there is no cross-tenant query path.
    fn snapshot_view(&self, scope: &TenantScope, at_least: &Zookie) -> SnapshotView {
        // `tuples_in` reads ONLY this verified scope's partition (the tenant-predicate floor). We
        // then drop any tuple whose zookie sorts AFTER the snapshot (the zookie strings are the
        // zero-padded `zk-<rev>` form, so lexical order == revision order — see `tuple_store`).
        let mut by_object: HashMap<String, Vec<SnapTuple>> = HashMap::new();
        for st in self.tuples.tuples_in(scope) {
            // Snapshot filter: skip tuples written strictly after the requested zookie. An EMPTY
            // `at_least` zookie means "latest" (the caller did not pin a snapshot) — include all.
            if !at_least.0.is_empty() && st.zookie.0 > at_least.0 {
                continue;
            }
            by_object
                .entry(st.tuple.object.0.clone())
                .or_default()
                .push(SnapTuple {
                    relation: st.tuple.relation.0.clone(),
                    subject: st.tuple.subject.0.clone(),
                });
        }
        SnapshotView { by_object }
    }
}

/// A memo key for the per-request subproblem cache: `(subject, relation, object)`. The same
/// subproblem is computed once per `check` request (memoised-per-request, §8).
#[derive(Clone, PartialEq, Eq, Hash)]
struct MemoKey {
    subject: String,
    relation: String,
    object: String,
}

/// A tuple flattened into the snapshot view (relation + subject only; the object is the map key).
#[derive(Clone)]
struct SnapTuple {
    relation: String,
    subject: String,
}

/// The zookie-snapshot tuple view: this tenant's tuples at-or-before the snapshot, indexed by
/// object id. The rewrite walks this read-only view (the authoritative store frozen at the
/// snapshot).
struct SnapshotView {
    by_object: HashMap<String, Vec<SnapTuple>>,
}

impl SnapshotView {
    /// The depth-bounded, memoised userset rewrite: does `subject` have `relation` on `object`?
    ///
    /// The Zanzibar core (§8): a grant holds iff there is a **direct** tuple `object#relation@subject`
    /// **OR** a tuple `object#relation@(obj2#rel2)` (a **userset** subject) where the rewrite of
    /// `check(subject, rel2, obj2)` holds (tuple-to-userset inheritance). The recursion is bounded
    /// by [`MAX_REWRITE_DEPTH`]: a chain deeper than the bound returns `false` (fail-closed —
    /// uncertainty is never an allow). The memo computes each `(subject, relation, object)`
    /// subproblem once.
    fn has_relation(
        &self,
        subject: &str,
        relation: &str,
        object: &str,
        depth: usize,
        memo: &mut HashMap<MemoKey, bool>,
    ) -> bool {
        // Depth bound: a rewrite deeper than the ceiling is genuine uncertainty → fail-closed
        // `false`. This is the structural guarantee against unbounded recursion (a userset cycle or
        // a pathologically deep chain cannot diverge — it bottoms out at the bound).
        if depth >= MAX_REWRITE_DEPTH {
            return false;
        }

        let key = MemoKey {
            subject: subject.to_string(),
            relation: relation.to_string(),
            object: object.to_string(),
        };
        // Memoised-per-request: if we already resolved this subproblem in THIS request, reuse it.
        // A re-entrant lookup (a userset cycle) is in-flight as `false` until it resolves — combined
        // with the depth bound this makes a cycle deny rather than diverge.
        if let Some(&hit) = memo.get(&key) {
            return hit;
        }
        // Mark the subproblem in-flight as `false` so a cycle back to it short-circuits to deny
        // (fail-closed) rather than recursing forever. The real answer overwrites this below.
        memo.insert(key.clone(), false);

        let mut granted = false;
        if let Some(tuples) = self.by_object.get(object) {
            for t in tuples {
                if t.relation != relation {
                    continue;
                }
                if t.subject == subject {
                    // A direct grant.
                    granted = true;
                    break;
                }
                // A userset subject `obj2#rel2` — expand it (tuple-to-userset inheritance).
                if let Some((obj2, rel2)) = parse_userset(&t.subject) {
                    if self.has_relation(subject, rel2, obj2, depth + 1, memo) {
                        granted = true;
                        break;
                    }
                }
            }
        }

        memo.insert(key, granted);
        granted
    }
}

/// Parse a userset subject `"object#relation"` into `(object, relation)`; `None` for a concrete
/// principal id (no `#`). Exactly one `#` separates the object from the relation. `pub(crate)` so
/// the namespace engine (P-ID-10) can split a parent userset `team:eng#view` when walking a
/// tuple-to-userset inheritance edge into the parent's *compiled* permission.
pub(crate) fn parse_userset(subject: &str) -> Option<(&str, &str)> {
    let (obj, rel) = subject.split_once(USERSET_SEP)?;
    if obj.is_empty() || rel.is_empty() || rel.contains(USERSET_SEP) {
        return None;
    }
    Some((obj, rel))
}

/// Extract the object id a `check` targets from its `ArtifactRef`. The tuples key on the **last
/// path segment** of the ref's URN (the object id the owning subsystem minted) — e.g.
/// `myelin://acme/issues/issue/PROJ-1` → `PROJ-1`, and a bare object id (no scheme) is itself the
/// id. Returns `None` for an empty/whitespace ref (genuine uncertainty → the caller fails closed).
///
/// The S3 tuples store the object id the subsystem minted (architecture §6: Id never invents object
/// ids); this maps the contract-boundary `ArtifactRef` onto that id. The full ref grammar
/// (scheme/tenant/type/id with `#sub` anchors) is `myelin-refs`; the engine needs only the object
/// id, taken from the final segment so both a full URN and a bare id resolve.
fn object_id_of(object: &ArtifactRef) -> Option<String> {
    let raw = object.0.trim();
    if raw.is_empty() {
        return None;
    }
    // Strip a trailing `#sub` anchor (the sub-artifact addresses the same root object for the
    // object-level check); the root object id is the last `/`-segment of the remaining ref.
    let root = raw.split('#').next().unwrap_or(raw);
    let id = root.rsplit('/').next().unwrap_or(root);
    if id.is_empty() {
        return None;
    }
    Some(id.to_string())
}

/// **Evaluate a field/transition caveat predicate through the ONE `QueryAst` predicate core**
/// (P-ID-22, contract 4.2 × 3.4; §8.6) — the promoted evaluator that CLOSES the M1 literal-only
/// floor.
///
/// `predicate` is the field/transition ABAC predicate (the `myelin_query::QueryAst`, a bounded,
/// DoS-hardened, statically-cost-bounded tree — the namespace fragment declares it; here it is
/// supplied alongside the matched relation). The [`CaveatContext`]'s `attrs` map supplies the
/// runtime context the predicate's variables read. The decision maps the one interpreter's outcome:
///
/// - predicate **holds** over the supplied context ⇒ [`Decision::Allow`] (the field is visible / the
///   transition is permitted; the relation grant already held);
/// - predicate **is violated** ⇒ [`Decision::Deny`] (the field is redacted / the transition gated);
/// - predicate references a variable the caller did **not** supply ([`EvalError::MissingContext`])
///   ⇒ [`Decision::Conditional`] (the caller supplies it) — **NEVER a silent `Allow`** (the
///   mutation-tested mandatory-core branch: a mutation turning `Conditional` into `Allow` is caught);
/// - the comparison is un-evaluable over the operand types ([`EvalError::TypeError`]) ⇒
///   [`Decision::Conditional`] (un-evaluable is uncertainty, never a silent allow);
/// - the predicate hits the runtime step ceiling ([`EvalError::CostExceeded`]) — a DoS-bounded
///   predicate ⇒ [`Decision::Deny`] (fail-closed: a cost-bounded reject never allows);
/// - the predicate is the un-compiled placeholder surface ([`EvalError::NotCompiled`]) ⇒
///   [`Decision::Conditional`] (the parser is the P-235 floor; an un-parsed predicate is
///   uncertainty, never a silent allow).
///
/// **There is exactly ONE predicate language** (EI-01 §7): this calls into the single
/// `myelin_query` interpreter — no second evaluator exists in the platform.
pub fn eval_caveat_predicate(predicate: &QueryAst, caveat: &CaveatContext) -> Decision {
    // The runtime context is the caveat's supplied attrs (the field/transition values the caller
    // fetched on the already-filtered row). The predicate's variables read from here; an unbound
    // variable is missing context → Conditional (never a silent allow).
    let ctx = EvalContext::from_attrs(caveat.attrs.clone());
    match predicate.eval(&ctx) {
        Ok(true) => Decision::Allow,
        Ok(false) => Decision::Deny,
        // The predicate needs context the caller did not supply → Conditional (the caller supplies
        // it). This is the mandatory-core branch: it must NEVER become Allow.
        Err(EvalError::MissingContext { .. }) => Decision::Conditional,
        // An un-evaluable comparison (e.g. ordering on strings) or an un-parsed placeholder is
        // genuine uncertainty → Conditional, never a silent allow.
        Err(EvalError::TypeError) | Err(EvalError::NotCompiled) => Decision::Conditional,
        // A DoS-bounded predicate (the step ceiling) fails closed → Deny (a cost-bounded reject
        // never allows by exhausting the budget — the boundedness is the gate).
        Err(EvalError::CostExceeded) => Decision::Deny,
    }
}

/// **Compatibility bridge: lower the legacy self-describing `CaveatContext` encoding to a real
/// `QueryAst` predicate, then evaluate it through the ONE core** ([`eval_caveat_predicate`]).
///
/// The M1 literal-only floor (P-ID-09) carried its predicate INSIDE `attrs` via reserved
/// `__caveat_*` keys (a self-describing caveat, because no predicate AST existed yet). This bridge
/// preserves those M1 cases with **no regression and no second predicate language**: it LOWERS the
/// encoding to a `myelin_query::Predicate` tree and routes it through the promoted evaluator. The
/// supported legacy forms:
/// - `attrs["__caveat_bool"]` = a `Bool` literal gate → `Predicate::Cmp(bool == true)`;
/// - `attrs["__caveat_op"]` ∈ `{eq,ne,lt,le,gt,ge}` with `__caveat_lhs`/`__caveat_rhs` literal
///   operands → the corresponding `Predicate::Cmp`.
///
/// A caveat with **no** recognised legacy encoding and **no** explicitly-supplied predicate is a
/// non-literal/context-dependent caveat the caller must resolve → [`Decision::Conditional`] (never a
/// silent allow). The non-literal field/transition INSTANCES land with their subsystems
/// (P-ID-25/26/29/30); this bridge keeps the M1 surface green meanwhile.
pub fn eval_caveat(caveat: &CaveatContext) -> Decision {
    match lower_legacy_caveat(caveat) {
        Some(predicate) => match QueryAst::compiled(predicate) {
            Ok(ast) => eval_caveat_predicate(&ast, caveat),
            // An over-budget lowered predicate (cannot happen for the tiny legacy forms, but the
            // interpreter never trusts its input) → fail-closed Conditional.
            Err(_) => Decision::Conditional,
        },
        // No literal predicate present: a non-literal / context-dependent caveat → Conditional (the
        // caller supplies the predicate context) — never a silent allow.
        None => Decision::Conditional,
    }
}

/// Lower the legacy `__caveat_*` self-describing encoding into a real `myelin_query::Predicate`
/// tree (the ONE predicate language). `None` ⇒ no recognised legacy encoding (a non-literal
/// caveat → the caller returns `Conditional`).
///
/// Two operand forms are supported (both compile to the SAME `Predicate::Cmp` node — there is no
/// second evaluator):
/// - **literal operand** — `__caveat_lhs` / `__caveat_rhs` hold a `Literal` value, lowered to
///   `Expr::Lit` (the M1 self-describing form: the predicate embedded its constants);
/// - **variable operand** — `__caveat_lhs_var` / `__caveat_rhs_var` hold a `Str` naming a context
///   key, lowered to `Expr::Var` (a genuinely NON-LITERAL predicate: the operand is resolved from
///   the supplied `attrs` at eval time, and an unbound one surfaces as `Conditional`). This is the
///   field/transition caveat shape Issues/Knowledge use (e.g. `issue.severity < threshold`), routed
///   through the public `check` ABI without a frozen-shape change.
fn lower_legacy_caveat(caveat: &CaveatContext) -> Option<Predicate> {
    // The pre-evaluated boolean-gate form: `__caveat_bool == true`.
    if let Some(b @ Literal::Bool(_)) = caveat.attrs.get("__caveat_bool") {
        return Some(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Lit(b.clone()),
            rhs: Expr::Lit(Literal::Bool(true)),
        });
    }

    // The comparison form: an op + two operands (each a literal constant OR a context variable).
    let op = match caveat.attrs.get("__caveat_op") {
        Some(Literal::Str(op)) => match op.as_str() {
            "eq" => CmpOp::Eq,
            "ne" => CmpOp::Ne,
            "lt" => CmpOp::Lt,
            "le" => CmpOp::Le,
            "gt" => CmpOp::Gt,
            "ge" => CmpOp::Ge,
            // An unknown op is un-evaluable → no lowered predicate (Conditional).
            _ => return None,
        },
        _ => return None,
    };
    let lhs = lower_operand(caveat, "__caveat_lhs")?;
    let rhs = lower_operand(caveat, "__caveat_rhs")?;
    Some(Predicate::Cmp { op, lhs, rhs })
}

/// Lower one comparison operand from the caveat encoding into an `Expr`. A `<base>_var` key (a
/// `Str` naming a context variable) lowers to `Expr::Var` (the non-literal form — resolved from
/// `attrs` at eval time); otherwise a literal `<base>` key lowers to `Expr::Lit`. `None` if neither
/// is present (missing operand → the caller returns `Conditional`).
fn lower_operand(caveat: &CaveatContext, base: &str) -> Option<Expr> {
    if let Some(Literal::Str(var)) = caveat.attrs.get(&format!("{base}_var")) {
        return Some(Expr::Var(var.clone()));
    }
    caveat.attrs.get(base).map(|lit| Expr::Lit(lit.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{OutboxStore, Timestamp};
    use myelin_identity::{
        FieldId, ObjectId, PrincipalId, PrincipalKind, RelationTuple, TupleDelta,
    };
    use myelin_tenancy::{Region, TenantId};
    use std::collections::BTreeMap;

    fn scope(tenant: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn subject(id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
        TupleDelta::Add(RelationTuple {
            object: ObjectId(object.into()),
            relation: RelName(relation.into()),
            subject: PrincipalId(subject.into()),
            caveat: None,
        })
    }

    fn now() -> Timestamp {
        Timestamp("2026-06-19T00:00:00Z".into())
    }

    /// Strong consistency at a given zookie snapshot.
    fn at(zookie: &str) -> Consistency {
        Consistency {
            at_least: Zookie(zookie.into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        }
    }

    /// "Latest" snapshot (no pinned zookie) — the empty zookie means "include all".
    fn latest() -> Consistency {
        at("")
    }

    fn engine_with(scope: &TenantScope, tuples: &[TupleDelta]) -> CheckEngine {
        let store = TupleStore::new(OutboxStore::new());
        let actor = subject("p-admin");
        store
            .write_tuples(scope, &actor, tuples, None, None, now())
            .expect("seed tuples");
        CheckEngine::new(store)
    }

    /// **check resolves a DIRECT grant → Allow.** A tuple `repo:core#reader@p:alice` ⇒
    /// `check(alice, reader, repo:core)` = Allow.
    #[test]
    fn check_direct_grant_allows() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("repo:core", "reader", "p:alice")]);
        let d = eng.check(
            &s,
            &subject("p:alice"),
            &RelName("reader".into()),
            &ArtifactRef("repo:core".into()),
            &latest(),
            None,
        );
        assert_eq!(d, Decision::Allow, "a direct grant allows");
    }

    /// **check on a MISSING grant → Deny (fail-closed default).** No tuple ⇒ Deny, never Allow.
    #[test]
    fn check_missing_grant_denies() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("repo:core", "reader", "p:alice")]);
        // bob has no grant; a different relation also denies.
        assert_eq!(
            eng.check(&s, &subject("p:bob"), &RelName("reader".into()), &ArtifactRef("repo:core".into()), &latest(), None),
            Decision::Deny,
            "no tuple for bob ⇒ deny"
        );
        assert_eq!(
            eng.check(&s, &subject("p:alice"), &RelName("writer".into()), &ArtifactRef("repo:core".into()), &latest(), None),
            Decision::Deny,
            "alice has reader, not writer ⇒ deny"
        );
    }

    /// **Tuple-to-userset inheritance (the simple inherited relation).** With
    /// `org:acme#member@p:alice` and `repo:core#reader@(org:acme#member)`,
    /// `check(alice, reader, repo:core)` = Allow via the userset rewrite (alice is a member of the
    /// org that is a reader of the repo).
    #[test]
    fn check_inherited_via_userset_allows() {
        let s = scope("acme");
        let eng = engine_with(
            &s,
            &[
                add("org:acme", "member", "p:alice"),
                add("repo:core", "reader", "org:acme#member"),
            ],
        );
        let d = eng.check(
            &s,
            &subject("p:alice"),
            &RelName("reader".into()),
            &ArtifactRef("repo:core".into()),
            &latest(),
            None,
        );
        assert_eq!(d, Decision::Allow, "alice inherits reader via org membership (userset rewrite)");
        // bob is NOT a member ⇒ does not inherit ⇒ deny.
        assert_eq!(
            eng.check(&s, &subject("p:bob"), &RelName("reader".into()), &ArtifactRef("repo:core".into()), &latest(), None),
            Decision::Deny,
            "a non-member does not inherit"
        );
    }

    /// **Fail-closed on a malformed query → Deny, never Allow** (the mandatory-core branch). An
    /// empty/whitespace object ref and an empty permission are genuine uncertainty ⇒ Deny.
    #[test]
    fn check_fail_closed_on_malformed_query() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("repo:core", "reader", "p:alice")]);
        // Empty object ref → uncertainty → Deny.
        assert_eq!(
            eng.check(&s, &subject("p:alice"), &RelName("reader".into()), &ArtifactRef("   ".into()), &latest(), None),
            Decision::Deny,
            "an unparseable object ref fails closed"
        );
        // Empty permission → malformed → Deny.
        assert_eq!(
            eng.check(&s, &subject("p:alice"), &RelName("".into()), &ArtifactRef("repo:core".into()), &latest(), None),
            Decision::Deny,
            "an empty permission fails closed"
        );
    }

    /// **A suspended/disabled subject has zero access (ID-D1 fail-closed)** — denied before any
    /// tuple is read, even though the grant tuple exists.
    #[test]
    fn check_suspended_subject_denied_despite_grant() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("repo:core", "reader", "p:alice")]);
        let mut suspended = subject("p:alice");
        suspended.status = PrincipalStatus::Disabled;
        assert_eq!(
            eng.check(&s, &suspended, &RelName("reader".into()), &ArtifactRef("repo:core".into()), &latest(), None),
            Decision::Deny,
            "a disabled subject is denied despite the grant (ID-D1)"
        );
    }

    /// **Evaluated at the zookie snapshot: a check at an OLDER zookie does not see a newer tuple.**
    /// Grant alice at z1; a check pinned to the pre-grant snapshot z0 must NOT see it (Deny), while
    /// a check at the latest/at-or-after snapshot DOES (Allow).
    #[test]
    fn check_at_older_zookie_does_not_see_newer_tuple() {
        let s = scope("acme");
        let store = TupleStore::new(OutboxStore::new());
        let actor = subject("p-admin");
        // z0: the genesis snapshot (before any grant).
        let z0 = store.current_zookie();
        // z1: grant alice reader on repo:core.
        let z1 = store
            .write_tuples(&s, &actor, &[add("repo:core", "reader", "p:alice")], None, None, now())
            .expect("grant");
        let eng = CheckEngine::new(store);

        // At the OLD snapshot z0, the newer tuple (written at z1 > z0) is invisible ⇒ Deny.
        assert_eq!(
            eng.check(&s, &subject("p:alice"), &RelName("reader".into()), &ArtifactRef("repo:core".into()), &at(&z0.0), None),
            Decision::Deny,
            "a check at the pre-grant zookie does not see the grant written after it"
        );
        // At the grant's snapshot z1 (at-or-after), the tuple is visible ⇒ Allow.
        assert_eq!(
            eng.check(&s, &subject("p:alice"), &RelName("reader".into()), &ArtifactRef("repo:core".into()), &at(&z1.0), None),
            Decision::Allow,
            "a check at-or-after the grant zookie sees the grant"
        );
    }

    /// **Depth-bounded: a deliberately deep userset chain is bounded (fail-closed), never unbounded
    /// recursion.** Build a chain longer than `MAX_REWRITE_DEPTH` of `level_{i}#m@(level_{i+1}#m)`;
    /// the subject is granted only at the far end (beyond the bound). The rewrite bottoms out at the
    /// bound and returns Deny — it does NOT diverge and does NOT allow by exhausting the budget.
    #[test]
    fn check_is_depth_bounded() {
        let s = scope("acme");
        // Build N = MAX_REWRITE_DEPTH + 4 chained usersets: level_0#m@(level_1#m), level_1#m@(...),
        // and finally level_N#m@p:deep. A check(deep, m, level_0) must traverse the whole chain.
        let n = MAX_REWRITE_DEPTH + 4;
        let mut deltas: Vec<TupleDelta> = Vec::new();
        for i in 0..n {
            deltas.push(add(&format!("level_{i}"), "m", &format!("level_{}#m", i + 1)));
        }
        // The concrete grant is at the FAR end (depth n) — beyond the bound.
        deltas.push(add(&format!("level_{n}"), "m", "p:deep"));
        let eng = engine_with(&s, &deltas);

        let d = eng.check(
            &s,
            &subject("p:deep"),
            &RelName("m".into()),
            &ArtifactRef("level_0".into()),
            &latest(),
            None,
        );
        assert_eq!(
            d,
            Decision::Deny,
            "a chain deeper than the bound fails closed (depth-bounded, never unbounded recursion / never allow-by-exhaustion)"
        );
    }

    /// **A userset CYCLE is bounded and denies (does not diverge).** `a#m@(b#m)` + `b#m@(a#m)` with
    /// no concrete grant ⇒ the rewrite short-circuits the cycle (in-flight memo = false) + the depth
    /// bound ⇒ Deny, never a stack overflow.
    #[test]
    fn check_userset_cycle_denies_without_diverging() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("a", "m", "b#m"), add("b", "m", "a#m")]);
        let d = eng.check(
            &s,
            &subject("p:nobody"),
            &RelName("m".into()),
            &ArtifactRef("a".into()),
            &latest(),
            None,
        );
        assert_eq!(d, Decision::Deny, "a userset cycle denies (bounded) rather than diverging");
    }

    /// **Memoised-per-request: the same subproblem is computed once.** A diamond
    /// (`top#m@(left#m)` + `top#m@(right#m)` + `left#m@(base#m)` + `right#m@(base#m)` +
    /// `base#m@p:alice`) re-converges on `check(alice, m, base)`; the memo computes it once. We
    /// assert correctness (Allow) AND, via an instrumented count, that the convergent subproblem is
    /// not recomputed.
    #[test]
    fn check_memoises_the_repeated_subproblem() {
        let s = scope("acme");
        let eng = engine_with(
            &s,
            &[
                add("top", "m", "left#m"),
                add("top", "m", "right#m"),
                add("left", "m", "base#m"),
                add("right", "m", "base#m"),
                add("base", "m", "p:alice"),
            ],
        );
        // Correctness: alice reaches `top` through both arms.
        assert_eq!(
            eng.check(&s, &subject("p:alice"), &RelName("m".into()), &ArtifactRef("top".into()), &latest(), None),
            Decision::Allow,
            "the diamond resolves to Allow"
        );

        // The memo's single-computation property: drive the rewrite directly with an instrumented
        // view and assert the convergent `(alice, m, base)` subproblem was computed exactly once.
        let view = eng.snapshot_view(&s, &Zookie(String::new()));
        let mut memo: HashMap<MemoKey, bool> = HashMap::new();
        let granted = view.has_relation("p:alice", "m", "top", 0, &mut memo);
        assert!(granted, "the diamond grants");
        // After the rewrite, the convergent subproblem is in the memo exactly once with `true`.
        let base_key = MemoKey {
            subject: "p:alice".into(),
            relation: "m".into(),
            object: "base".into(),
        };
        assert_eq!(
            memo.get(&base_key),
            Some(&true),
            "the convergent subproblem was memoised (computed once, reused on the second arm)"
        );
    }

    /// **A literal CaveatContext gates correctly: a satisfied literal predicate keeps Allow; a
    /// violated one denies.** `severity (3) < threshold (5)` ⇒ Allow; `severity (7) < threshold (5)`
    /// ⇒ Deny. The grant relation holds in both cases — the caveat is what flips the decision.
    #[test]
    fn check_literal_caveat_gates_correctly() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("issue:PROJ-1", "view_field", "p:alice")]);

        // A literal predicate `3 < 5` (satisfied) ⇒ the field is visible ⇒ Allow.
        let mut ok = BTreeMap::new();
        ok.insert("__caveat_op".to_string(), Literal::Str("lt".into()));
        ok.insert("__caveat_lhs".to_string(), Literal::Int(3));
        ok.insert("__caveat_rhs".to_string(), Literal::Int(5));
        let cav_ok = CaveatContext {
            object: ArtifactRef("issue:PROJ-1".into()),
            field: Some(FieldId("salary".into())),
            transition: None,
            attrs: ok,
        };
        assert_eq!(
            eng.check(&s, &subject("p:alice"), &RelName("view_field".into()), &ArtifactRef("issue:PROJ-1".into()), &latest(), Some(&cav_ok)),
            Decision::Allow,
            "a satisfied literal caveat keeps the Allow"
        );

        // A literal predicate `7 < 5` (violated) ⇒ the field is redacted ⇒ Deny.
        let mut bad = BTreeMap::new();
        bad.insert("__caveat_op".to_string(), Literal::Str("lt".into()));
        bad.insert("__caveat_lhs".to_string(), Literal::Int(7));
        bad.insert("__caveat_rhs".to_string(), Literal::Int(5));
        let cav_bad = CaveatContext {
            object: ArtifactRef("issue:PROJ-1".into()),
            field: Some(FieldId("salary".into())),
            transition: None,
            attrs: bad,
        };
        assert_eq!(
            eng.check(&s, &subject("p:alice"), &RelName("view_field".into()), &ArtifactRef("issue:PROJ-1".into()), &latest(), Some(&cav_bad)),
            Decision::Deny,
            "a violated literal caveat denies (redacts the field)"
        );
    }

    /// **A caveat needing missing context returns Conditional, NOT a silent Allow** (the
    /// mutation-tested mandatory-core branch, §8.6). The relation grant holds, but the caveat's
    /// operands are absent ⇒ Conditional (the caller supplies the context) — never Allow.
    #[test]
    fn check_missing_context_caveat_is_conditional_not_allow() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("issue:PROJ-1", "view_field", "p:alice")]);
        // An op with no operands → missing context → Conditional.
        let mut attrs = BTreeMap::new();
        attrs.insert("__caveat_op".to_string(), Literal::Str("lt".into()));
        let cav = CaveatContext {
            object: ArtifactRef("issue:PROJ-1".into()),
            field: Some(FieldId("salary".into())),
            transition: None,
            attrs,
        };
        let d = eng.check(
            &s,
            &subject("p:alice"),
            &RelName("view_field".into()),
            &ArtifactRef("issue:PROJ-1".into()),
            &latest(),
            Some(&cav),
        );
        assert_eq!(
            d,
            Decision::Conditional,
            "a caveat needing missing context is Conditional, NEVER a silent Allow"
        );
        assert_ne!(d, Decision::Allow, "the missing-context branch is mandatory-core: it must not Allow");
    }

    // ------------------------------------------------------------------------------------------
    // P-ID-22 (P-133): the promoted evaluator on the full `myelin_query::QueryAst` predicate core.
    // The non-literal field/transition caveats exercise a real predicate tree reading the
    // CaveatContext's supplied `attrs` as the evaluation context (the floor is CLOSED — the
    // evaluator runs on the ONE platform predicate language, not the literal-only encoding).
    // ------------------------------------------------------------------------------------------

    /// Build a `CaveatContext` carrying the supplied attrs (the runtime context the predicate
    /// reads) for the given field — the off-hot-path field-redaction caveat shape (§8.6).
    fn field_caveat(object: &str, field: &str, attrs: BTreeMap<String, Literal>) -> CaveatContext {
        CaveatContext {
            object: ArtifactRef(object.into()),
            field: Some(FieldId(field.into())),
            transition: None,
            attrs,
        }
    }

    /// **A NON-LITERAL field caveat redacts correctly through the QueryAst core.** The predicate
    /// `issue.severity < threshold` reads BOTH operands from the supplied context (real variables,
    /// not embedded literals). `severity=3, threshold=5` ⇒ visible (Allow); `severity=7` ⇒ redacted
    /// (Deny). This is the field-level column hiding Issues/Knowledge need (the promotion's point).
    #[test]
    fn non_literal_field_caveat_redacts_through_query_ast() {
        // predicate: severity < threshold  (both are context variables — non-literal)
        let predicate = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Lt,
            lhs: Expr::Var("severity".into()),
            rhs: Expr::Var("threshold".into()),
        })
        .unwrap();

        let mut visible = BTreeMap::new();
        visible.insert("severity".to_string(), Literal::Int(3));
        visible.insert("threshold".to_string(), Literal::Int(5));
        let cav_visible = field_caveat("issue:PROJ-1", "salary", visible);
        assert_eq!(
            eval_caveat_predicate(&predicate, &cav_visible),
            Decision::Allow,
            "severity(3) < threshold(5) ⇒ the field is visible (Allow)"
        );

        let mut redacted = BTreeMap::new();
        redacted.insert("severity".to_string(), Literal::Int(7));
        redacted.insert("threshold".to_string(), Literal::Int(5));
        let cav_redacted = field_caveat("issue:PROJ-1", "salary", redacted);
        assert_eq!(
            eval_caveat_predicate(&predicate, &cav_redacted),
            Decision::Deny,
            "severity(7) < threshold(5) is false ⇒ the field is redacted (Deny)"
        );
    }

    /// **A transition caveat gates correctly through the QueryAst core.** A transition is permitted
    /// iff `has_approver == true` (a context variable). With the approver edge present ⇒ Allow;
    /// absent (false) ⇒ Deny.
    #[test]
    fn transition_caveat_gates_through_query_ast() {
        let predicate = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("has_approver".into()),
            rhs: Expr::Lit(Literal::Bool(true)),
        })
        .unwrap();

        let mut approved = BTreeMap::new();
        approved.insert("has_approver".to_string(), Literal::Bool(true));
        let cav_ok = CaveatContext {
            object: ArtifactRef("issue:PROJ-1".into()),
            field: None,
            transition: Some(myelin_identity::TransitionId("close".into())),
            attrs: approved,
        };
        assert_eq!(
            eval_caveat_predicate(&predicate, &cav_ok),
            Decision::Allow,
            "an approver edge permits the transition"
        );

        let mut unapproved = BTreeMap::new();
        unapproved.insert("has_approver".to_string(), Literal::Bool(false));
        let cav_bad = CaveatContext {
            object: ArtifactRef("issue:PROJ-1".into()),
            field: None,
            transition: Some(myelin_identity::TransitionId("close".into())),
            attrs: unapproved,
        };
        assert_eq!(
            eval_caveat_predicate(&predicate, &cav_bad),
            Decision::Deny,
            "no approver edge gates the transition"
        );
    }

    /// **A predicate needing missing context returns Conditional, NEVER a silent Allow** — the
    /// mutation-tested mandatory-core branch on the promoted core. The predicate reads
    /// `issue.severity` but the caller supplied NO `severity` attr ⇒ Conditional.
    #[test]
    fn promoted_missing_context_is_conditional_not_allow() {
        let predicate = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Lt,
            lhs: Expr::Var("severity".into()),
            rhs: Expr::Lit(Literal::Int(5)),
        })
        .unwrap();
        // attrs is EMPTY — `severity` is unbound (missing context).
        let cav = field_caveat("issue:PROJ-1", "salary", BTreeMap::new());
        let d = eval_caveat_predicate(&predicate, &cav);
        assert_eq!(
            d,
            Decision::Conditional,
            "a caveat needing missing context is Conditional (the caller supplies it)"
        );
        assert_ne!(
            d,
            Decision::Allow,
            "MANDATORY-CORE: the missing-context branch must NEVER become Allow (mutation-caught)"
        );
    }

    /// **An un-evaluable comparison (ordering on strings) is Conditional, never a silent Allow.**
    #[test]
    fn promoted_un_evaluable_comparison_is_conditional() {
        let predicate = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Lt,
            lhs: Expr::Var("name".into()),
            rhs: Expr::Lit(Literal::Str("z".into())),
        })
        .unwrap();
        let mut attrs = BTreeMap::new();
        attrs.insert("name".to_string(), Literal::Str("alice".into()));
        let cav = field_caveat("issue:PROJ-1", "salary", attrs);
        assert_eq!(
            eval_caveat_predicate(&predicate, &cav),
            Decision::Conditional,
            "ordering on strings is un-evaluable ⇒ Conditional, never a silent allow"
        );
    }

    /// **A deliberately-expensive predicate is cost-bounded (no DoS).** A predicate at the maximum
    /// permitted size still evaluates within the step ceiling (bounded), and an OVER-budget tree is
    /// rejected at construction (never reaches the interpreter) — the boundedness is structural.
    #[test]
    fn promoted_predicate_is_cost_bounded() {
        // A large-but-legal conjunction of trues evaluates bounded (Allow).
        let conjuncts: Vec<Predicate> = (0..(myelin_query::MAX_PREDICATE_NODES / 2))
            .map(|_| Predicate::True)
            .collect();
        let ast = QueryAst::compiled(Predicate::And(conjuncts)).unwrap();
        let cav = field_caveat("issue:PROJ-1", "salary", BTreeMap::new());
        assert_eq!(
            eval_caveat_predicate(&ast, &cav),
            Decision::Allow,
            "a large-but-legal predicate evaluates bounded (no DoS)"
        );

        // An OVER-budget tree is statically rejected — it never reaches the interpreter at all.
        let oversized: Vec<Predicate> = (0..(myelin_query::MAX_PREDICATE_NODES + 50))
            .map(|_| Predicate::True)
            .collect();
        assert!(
            QueryAst::compiled(Predicate::And(oversized)).is_err(),
            "an adversarial over-budget predicate is rejected at construction (statically cost-bounded)"
        );
    }

    /// **A caveat on a DENIED relation does not leak Allow.** If the relation grant itself fails,
    /// the decision is Deny regardless of the caveat (the caveat is evaluated only AFTER the grant
    /// holds — off the hot path, §8.6). No caveat path can manufacture an Allow without a grant.
    #[test]
    fn caveat_cannot_manufacture_allow_without_a_grant() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("issue:PROJ-1", "view_field", "p:alice")]);
        // A satisfied caveat, but bob has NO grant ⇒ Deny (the caveat never runs).
        let mut ok = BTreeMap::new();
        ok.insert("__caveat_bool".to_string(), Literal::Bool(true));
        let cav = CaveatContext {
            object: ArtifactRef("issue:PROJ-1".into()),
            field: None,
            transition: None,
            attrs: ok,
        };
        assert_eq!(
            eng.check(&s, &subject("p:bob"), &RelName("view_field".into()), &ArtifactRef("issue:PROJ-1".into()), &latest(), Some(&cav)),
            Decision::Deny,
            "a satisfied caveat cannot grant access without the underlying relation"
        );
    }

    /// **The object id is extracted from a full URN ArtifactRef** (`myelin://t/issues/issue/PROJ-1`
    /// → `PROJ-1`) and a `#sub` anchor addresses the same root object.
    #[test]
    fn object_id_extracted_from_urn_and_sub_anchor() {
        assert_eq!(object_id_of(&ArtifactRef("myelin://acme/issues/issue/PROJ-1".into())), Some("PROJ-1".into()));
        assert_eq!(object_id_of(&ArtifactRef("repo:core".into())), Some("repo:core".into()));
        assert_eq!(object_id_of(&ArtifactRef("myelin://acme/issues/issue/PROJ-1#comment-7".into())), Some("PROJ-1".into()));
        assert_eq!(object_id_of(&ArtifactRef("  ".into())), None);
    }

    /// **No cross-tenant check path.** A grant under `acme` does not allow the same `check` under
    /// `globex` (the engine reads only the verified scope's partition).
    #[test]
    fn no_cross_tenant_check() {
        let acme = scope("acme");
        let globex = scope("globex");
        let store = TupleStore::new(OutboxStore::new());
        store
            .write_tuples(&acme, &subject("p-admin"), &[add("repo:core", "reader", "p:alice")], None, None, now())
            .expect("acme grant");
        let eng = CheckEngine::new(store);
        // Under acme: allow.
        assert_eq!(
            eng.check(&acme, &subject("p:alice"), &RelName("reader".into()), &ArtifactRef("repo:core".into()), &latest(), None),
            Decision::Allow
        );
        // Under globex: the acme grant is invisible ⇒ deny (no cross-tenant query path).
        assert_eq!(
            eng.check(&globex, &subject("p:alice"), &RelName("reader".into()), &ArtifactRef("repo:core".into()), &latest(), None),
            Decision::Deny,
            "a grant in one tenant does not allow a check in another"
        );
    }
}
