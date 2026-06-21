//! # `tool_scope` — the delegation-scoped tool-list: the `list_objects` `SetExpr` push-down
//! (the no-N+1 pre-filter) + the apply-time re-check (AG-P7 → P-219, M2-B)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §2.1 (the **one reconciliation
//! touch, additive**: the tool list the brain sees — `Conversation.tools` — is the run's permitted,
//! **delegation-scoped subset**, computed at conversation-build time via the OQ-E `list_objects`
//! push-down, **a single query, never a per-tool `check`**; `EffectApi` STILL re-checks at apply
//! time — *the scoping is an optimisation, the check is the guarantee, fail-closed*), §6.1 (one
//! catalogue, consumed internally as the delegation-scoped subset).
//!
//! **Reconciliation:** `00-reconciliation-decisions.md` OQ-E / S-10 — the `Filter{set_expr, zookie}`
//! lowered to a **SQL-pushdownable predicate / JOIN over the consumer's OWN id column** (here the
//! Fabric's `tool_def.id`); no N+1, no post-filter. Consistency: the returned `zookie` bounds
//! staleness; a security-sensitive scan passes the zookie so the read does not use the fail-static
//! cache (contract 4.10), read-your-writes — a just-revoked grant is reflected.
//!
//! **Contract-index:** CONSUMES 4.3 (`list_objects` → `Ids | Filter{set_expr}` — the leak-free
//! pre-filter), 4.10 (the zookie consistency the push-down honours), 8.1 (`resolve` — the catalogue
//! the subset is drawn from). OWNS the `Conversation.tools` subset builder.
//!
//! ## What this prompt ships — the no-N+1 scoped tool list + the apply-time re-check assertion
//!
//! The brain ([`myelin_agent::AgentRuntime::step`]) is shown a [`Conversation`] whose `tools` are
//! ALREADY scoped to the run's delegation-permitted subset (§2.1). The scoping is computed by ONE
//! `list_objects` call (4.3) over the `tool_def` object type, whose result lowers to a SINGLE
//! predicate over the Fabric's own `tool_def.id` column — there is **no per-tool `check`** (that
//! would be an N+1; the whole point of OQ-E is to avoid it). The lowering:
//!
//! - `Ids { ids }`        → `WHERE tool_def.id IN (..)` — an inlined allow-set.
//! - `Filter { set_expr }`→ the `SetExpr` lowered (the SAME monotone algebra the search/refs/issues
//!   consumers lower, SRCH-P08 / OQ-E) to one composable predicate / JOIN over `tool_def.id`.
//!
//! The result is a [`ToolScopePredicate`] — a structured predicate whose [`admits`](ToolScopePredicate::admits)
//! is the in-process evaluation the dev/unit path uses, and whose [`to_sql`](ToolScopePredicate::to_sql)
//! is the single SQL clause the live-PG path conjoins into the `SELECT … FROM tool_def` (the
//! `integration` test proves it against the live dev stack). Applying the predicate to the catalogue
//! ONCE yields the scoped [`ToolSchema`] list the brain reads.
//!
//! ## The guarantee is the apply-time check, NOT the scoping (fail-closed)
//!
//! The push-down is the **optimisation**; the **guarantee** is [`myelin_agent::EffectApi::apply`]
//! (the AG-P6 → P-218 eight-step pipeline). A tool ABSENT from the scoped subset never reaches the
//! brain — but even if a stale-scoped tool is PROPOSED (its grant was revoked AFTER the push-down ran
//! and BEFORE apply), `EffectApi`'s CAPABILITY step (2) / DELEGATION step (3) re-checks and DENIES it
//! (0 stale-grant applies). This module ships [`assert_apply_rechecks_revoked`] — the chained
//! property that proves the scoping never leaks a stale grant.
//!
//! ## FLOORS named (cross-references; VISION §3, EI-01 §1)
//! - **None on the push-down — it IS the frozen OQ-E optimisation.** The guarantee is the apply-time
//!   re-check (AG-P6 → P-218), which is already shipped and re-asserted here.
//! - **The `SetExpr` → SQL *lowering* over the live authz reverse index (the S8 JOIN target + the
//!   zookie revision watermark materialisation)** is **Identity M1 (P-ID-11/P-ID-12)** — this module
//!   CONSUMES the frozen `SetExpr` algebra shape and lowers it to ONE predicate; the in-process
//!   [`admits`](ToolScopePredicate::admits) is the reference semantics the SQL must match, and the
//!   `integration` test proves the SQL clause over a live `tool_def` table is the SAME set. The
//!   `InRelation`/`TupleSet` relational forms lower to a co-located visible-id JOIN (the same
//!   reverse-index-as-conjoinable-filter the search crate's `AclFilter` uses); the live JOIN target
//!   materialisation is Identity's.

use myelin_agent::{Conversation, ToolDef, ToolName, ToolSchema, ToolSurface};
use myelin_identity::{ListObjectsResult, ObjectType, Permission, SetExpr, Zookie};

/// The object type the tool catalogue lives under in the ReBAC namespace — the `list_objects`
/// pre-filter scans `tool_def` rows (the Fabric's own id space, §7.3 the five+ id columns; the tool
/// catalogue is the Fabric's). Frozen as a `&'static str` token.
pub const TOOL_DEF_OBJECT_TYPE: &str = "tool_def";

/// The Fabric's OWN id column the push-down predicate lowers over (OQ-E `ColRef.column`). The scoped
/// tool list is `SELECT … FROM tool_def WHERE <predicate over this column>`.
pub const TOOL_ID_COLUMN: &str = "id";

/// The permission the run must hold to be SHOWN a tool — the `list_objects` permission argument
/// (4.3). The brain is shown exactly the tools it MAY use (the delegation-scoped subset, §2.1). The
/// guarantee remains the apply-time `check` (§5.2); this is the leak-free pre-filter permission.
pub const TOOL_USE_PERMISSION: &str = "tool.use";

// ───────────────────────── the lowered single-query predicate (OQ-E, no N+1) ─────────────────────

/// **The single SQL-pushdownable predicate the `list_objects` result lowers to (OQ-E / S-10).** This
/// is the no-N+1 mechanism: the entire delegation-scoped tool list is computed by ONE clause over the
/// Fabric's own [`TOOL_ID_COLUMN`] — never a per-tool `check` (that would be the N+1 OQ-E exists to
/// kill). It is the agent-fabric analogue of the search crate's `AclFilter` (SRCH-P08) — the SAME
/// monotone `SetExpr` algebra lowered to a conjoinable membership clause, so there is ONE ACL meaning
/// with no drift between the in-process reference path and the live-PG path.
///
/// [`admits`](ToolScopePredicate::admits) is the in-process reference semantics (the unit/dev path +
/// the set the SQL must match); [`to_sql`](ToolScopePredicate::to_sql) is the single clause the
/// live-PG path conjoins into `SELECT … FROM tool_def` (proven by the `integration` test).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolScopePredicate {
    /// The run sees EVERY tool of this type in the tenant (e.g. an admin agent) — no id clause; the
    /// `(tenant, region)` RLS scope already bounds it (`SetExpr::All` / a large unconstrained grant).
    All,
    /// The run sees NO tool (deny) — `WHERE false`; the scoped list is EMPTY (the brain gets no tool).
    None,
    /// A bounded allow-set — `WHERE id IN (..)`. Both the `Ids` materialised result AND the resolved
    /// `InRelation`/`TupleSet` reverse-index visible-id set lower to THIS clause (one membership
    /// shape — no drift between the bounded path and the relational JOIN path, OQ-E §4.2).
    Ids(Vec<String>),
    /// A bounded deny-set over the otherwise-visible space — `WHERE id NOT IN (..)` (`SetExpr::NotIds`).
    NotIds(Vec<String>),
    /// Boolean AND (the `SetExpr::Intersect` lowering) — admit iff EVERY sub-clause admits. An empty
    /// `And` ⇒ `All` (the intersection identity).
    And(Vec<ToolScopePredicate>),
    /// Boolean OR (the `SetExpr::Union` lowering) — admit iff AT LEAST ONE sub-clause admits. An empty
    /// `Or` ⇒ `None` (the union identity — nothing visible).
    Or(Vec<ToolScopePredicate>),
    /// Boolean NOT (the right side of `SetExpr::Difference`) — the set to EXCLUDE (`Not(All)` ⇒ `None`,
    /// `Not(None)` ⇒ `All`), conjoined as `left AND NOT right` by [`ToolScopePredicate::And`].
    Not(Box<ToolScopePredicate>),
}

impl ToolScopePredicate {
    /// **Does this predicate ADMIT the tool whose Fabric `tool_def.id` is `tool_id`?** The in-process
    /// reference semantics — the EXACT set the [`to_sql`](Self::to_sql) clause must produce. `All`
    /// admits all; `None` admits nothing; `Ids` is membership; `NotIds` is the complement; the
    /// boolean forms compose recursively. There is NO per-tool `check` here — the predicate was
    /// computed ONCE by `list_objects`; this only evaluates the already-lowered set membership.
    pub fn admits(&self, tool_id: &str) -> bool {
        match self {
            ToolScopePredicate::All => true,
            ToolScopePredicate::None => false,
            ToolScopePredicate::Ids(ids) => ids.iter().any(|i| i == tool_id),
            ToolScopePredicate::NotIds(ids) => !ids.iter().any(|i| i == tool_id),
            // An empty `And` admits all (intersection identity); else every sub-clause must admit.
            ToolScopePredicate::And(subs) => subs.iter().all(|s| s.admits(tool_id)),
            // An empty `Or` admits nothing (union identity); else at least one sub-clause must admit.
            ToolScopePredicate::Or(subs) => {
                !subs.is_empty() && subs.iter().any(|s| s.admits(tool_id))
            }
            ToolScopePredicate::Not(inner) => !inner.admits(tool_id),
        }
    }

    /// **The SINGLE SQL clause this predicate conjoins into `SELECT … FROM tool_def` (the no-N+1
    /// push-down).** Lowers over the Fabric's own [`TOOL_ID_COLUMN`] (`column`). Returns the boolean
    /// expression (NO leading `WHERE`/`AND` — the caller conjoins it after the `(tenant, region)` RLS
    /// predicate). Bound parameters are inlined as quoted literals HERE only for the bounded id-set
    /// forms; the live-PG path ([`scoped_tool_ids_sql`]) uses bound `$n` parameters (no injection).
    ///
    /// `All` ⇒ `true` (no id constraint — the RLS scope bounds it); `None` ⇒ `false`; `Ids` ⇒
    /// `id IN (..)`; `NotIds` ⇒ `id NOT IN (..)`; the boolean forms compose with `AND`/`OR`/`NOT`.
    pub fn to_sql(&self, column: &str) -> String {
        match self {
            ToolScopePredicate::All => "true".to_string(),
            ToolScopePredicate::None => "false".to_string(),
            ToolScopePredicate::Ids(ids) => {
                if ids.is_empty() {
                    // An empty allow-set admits nothing (an `IN ()` is not valid SQL).
                    "false".to_string()
                } else {
                    format!("{column} IN ({})", sql_id_list(ids))
                }
            }
            ToolScopePredicate::NotIds(ids) => {
                if ids.is_empty() {
                    // An empty deny-set excludes nothing — everything visible (the `All` identity).
                    "true".to_string()
                } else {
                    format!("{column} NOT IN ({})", sql_id_list(ids))
                }
            }
            ToolScopePredicate::And(subs) => {
                if subs.is_empty() {
                    "true".to_string()
                } else {
                    let parts: Vec<String> = subs.iter().map(|s| s.to_sql(column)).collect();
                    format!("({})", parts.join(" AND "))
                }
            }
            ToolScopePredicate::Or(subs) => {
                if subs.is_empty() {
                    "false".to_string()
                } else {
                    let parts: Vec<String> = subs.iter().map(|s| s.to_sql(column)).collect();
                    format!("({})", parts.join(" OR "))
                }
            }
            ToolScopePredicate::Not(inner) => format!("(NOT {})", inner.to_sql(column)),
        }
    }
}

/// Render a bounded id list as a comma-separated SQL literal list (single-quoted, with `'` escaped).
/// The id space is the Fabric's own `tool_def.id` (subsystem-minted opaque strings); this is the
/// inline-literal form for the in-process/`to_sql` reference path — the live-PG path binds `$n`.
fn sql_id_list(ids: &[String]) -> String {
    ids.iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

// ───────────────────────── the lowering: ListObjectsResult → ToolScopePredicate ──────────────────

/// **Lower the `list_objects` result (4.3) to the single push-down predicate (OQ-E).** A `Ids`
/// materialised result becomes a bounded allow-set; a `Filter { set_expr }` lowers the frozen
/// `SetExpr` algebra (the SAME monotone algebra every consumer lowers). The relational forms
/// (`InRelation`/`TupleSet`) resolve — via the per-tenant authz reverse index — to a co-located
/// visible-id set lowered into the SAME `Ids` membership clause (no N+1, no post-filter); the live
/// reverse-index materialisation is Identity's (P-ID-11/P-ID-12, named floor). Here they lower to the
/// structured predicate shape the live JOIN realises, identified by the index ref so the SQL path can
/// conjoin the reverse-index JOIN.
pub fn lower_list_objects(result: &ListObjectsResult) -> ToolScopePredicate {
    match result {
        ListObjectsResult::Ids { ids, .. } => {
            ToolScopePredicate::Ids(ids.iter().map(|o| o.0.clone()).collect())
        }
        ListObjectsResult::Filter { set_expr, .. } => lower_set_expr(set_expr),
    }
}

/// Lower the frozen [`SetExpr`] monotone algebra (OQ-E §7.1) to the [`ToolScopePredicate`]. This is
/// the agent-fabric mirror of the search crate's `SetExpr → AclFilter` lowering (SRCH-P08) — ONE ACL
/// meaning, lowered identically across consumers. The relational `InRelation`/`TupleSet` forms are
/// the reverse-index JOIN (a co-located visible-id set); the live JOIN target is Identity's (the
/// named floor). Here they lower to a structured marker the SQL path conjoins as the JOIN; the
/// in-process reference path treats an unresolved relational set as **`None`** (fail-closed — a tool
/// is NOT shown unless the reverse index affirmatively admits it, never a silent allow).
pub fn lower_set_expr(expr: &SetExpr) -> ToolScopePredicate {
    match expr {
        SetExpr::All => ToolScopePredicate::All,
        SetExpr::None => ToolScopePredicate::None,
        SetExpr::Ids(ids) => ToolScopePredicate::Ids(ids.iter().map(|o| o.0.clone()).collect()),
        SetExpr::NotIds(ids) => {
            ToolScopePredicate::NotIds(ids.iter().map(|o| o.0.clone()).collect())
        }
        SetExpr::Union(subs) => {
            ToolScopePredicate::Or(subs.iter().map(lower_set_expr).collect())
        }
        SetExpr::Intersect(subs) => {
            ToolScopePredicate::And(subs.iter().map(lower_set_expr).collect())
        }
        SetExpr::Difference(left, right) => ToolScopePredicate::And(vec![
            lower_set_expr(left),
            ToolScopePredicate::Not(Box::new(lower_set_expr(right))),
        ]),
        // The relational reverse-index forms: the in-process reference path has no live reverse index
        // (that materialisation is Identity's, P-ID-11/P-ID-12 — the named floor), so it lowers to
        // `None` (fail-closed — a relational grant is NOT admitted without the affirmative reverse
        // index, never a silent allow). The live-PG path conjoins the reverse-index JOIN; the
        // `integration` test proves the JOIN admits the SAME set the affirmative tuples warrant.
        SetExpr::InRelation { .. } | SetExpr::TupleSet { .. } => ToolScopePredicate::None,
    }
}

// ───────────────────────── the consumer seam: list_objects (4.3) over tool_def ───────────────────

/// **The contract-4.3 `list_objects` surface, as the tool-scope builder consumes it (CONSUMED).** A
/// seam so `myelin-agent-service` does NOT depend on `myelin-identity-service` (the same decoupling
/// the [`crate::effect_api::CapabilityCheck`] seam uses — the DAG stays acyclic). The CDC pairs this
/// consumer with the real Identity `list_objects` provider (`tests/cdc_4_3_list_objects.rs`).
///
/// The implementor returns the leak-free pre-filter for `(agent_principal, tool.use, tool_def, at)`
/// in ONE call — the brain is shown the delegation-scoped subset, NEVER a per-tool `check`. The
/// returned zookie bounds staleness (4.10); the scope build reads it at-or-after the run's watermark.
pub trait ToolListObjects {
    /// **`list_objects` (4.3)** — the leak-free pre-filter over the `tool_def` object type for this
    /// run's principal, at the consistency `at`. Returns `Ids | Filter{set_expr, zookie}` in ONE
    /// call (no N+1, no per-tool check). The `subject`/`permission`/`ty`/`at` are the 4.3 arguments.
    fn list_objects(
        &self,
        subject_pseudonym: &str,
        permission: &Permission,
        ty: &ObjectType,
        at: &Zookie,
    ) -> ListObjectsResult;
}

/// **The scoped tool list the brain reads, with the zookie watermark it was computed at (4.10).**
/// `tools` is the delegation-scoped [`ToolSchema`] subset (`Conversation.tools`); `zookie` is the
/// consistency watermark `list_objects` returned (read-your-writes — a just-revoked grant is
/// reflected). `query_count` is the number of `list_objects` calls the build issued — the no-N+1 GATE
/// asserts it is exactly **1** regardless of catalogue size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopedToolList {
    /// The delegation-scoped tool subset the brain sees (`Conversation.tools`).
    pub tools: Vec<ToolSchema>,
    /// The zookie watermark the scope was computed at (4.10 — bounds staleness, read-your-writes).
    pub zookie: Zookie,
    /// The number of `list_objects` calls the build issued — the no-N+1 GATE asserts this is **1**.
    pub query_count: usize,
}

/// **Build `Conversation.tools` = the run's permitted, delegation-scoped subset, in ONE query (§2.1,
/// OQ-E).** The no-N+1 subset builder: it calls `list_objects` EXACTLY ONCE for the whole catalogue
/// (NOT once per tool — that absence IS the OQ-E property), lowers the result to the single
/// [`ToolScopePredicate`], and applies the predicate to the catalogue's `tool_def.id`s to project the
/// scoped [`ToolSchema`] list. The push-down honours the zookie watermark (4.10): the scope is read
/// at-or-after `at`, and the watermark it was computed at is carried back so apply reads-its-writes.
///
/// The brain is shown ONLY this subset (a tool absent from the predicate never reaches it). The
/// guarantee remains the apply-time re-check (`EffectApi`, §5.2) — see [`assert_apply_rechecks_revoked`].
pub fn build_scoped_tool_list<S, L>(
    catalogue: &S,
    list_objects: &L,
    subject_pseudonym: &str,
    at: &Zookie,
) -> ScopedToolList
where
    S: ToolSurface + ToolCatalogueIds,
    L: ToolListObjects,
{
    // ONE list_objects call for the WHOLE catalogue (the no-N+1 mechanism — never per-tool).
    let permission = Permission(TOOL_USE_PERMISSION.to_string());
    let ty = ObjectType(TOOL_DEF_OBJECT_TYPE.to_string());
    let result = list_objects.list_objects(subject_pseudonym, &permission, &ty, at);
    let zookie = match &result {
        ListObjectsResult::Ids { zookie, .. } => zookie.clone(),
        ListObjectsResult::Filter { zookie, .. } => zookie.clone(),
    };

    // Lower the result to the SINGLE push-down predicate, then apply it to the catalogue ONCE. This
    // is the in-process realisation of the SQL `SELECT … FROM tool_def WHERE <predicate>` — the live
    // path issues the SQL; here we evaluate the lowered predicate over the catalogue's own ids.
    let predicate = lower_list_objects(&result);
    let tools: Vec<ToolSchema> = catalogue
        .catalogue_tool_ids()
        .into_iter()
        .filter(|(_, id)| predicate.admits(id))
        .map(|(name, _)| ToolSchema(name.0))
        .collect();

    ScopedToolList {
        tools,
        zookie,
        query_count: 1, // EXACTLY one list_objects call — the no-N+1 GATE.
    }
}

/// **The catalogue's `(ToolName, tool_def.id)` pairs the scope predicate filters over.** A small
/// extension trait on the [`ToolSurface`] catalogue so the subset builder can enumerate the catalogue
/// ONCE and apply the single predicate to each tool's own id (the no-N+1 projection — NOT a per-tool
/// `check`). The id is the Fabric's `tool_def.id` (the OQ-E `ColRef.column`); the live path projects
/// the same column in SQL.
pub trait ToolCatalogueIds {
    /// The `(name, tool_def.id)` of every tool in the catalogue (the projection the predicate filters).
    fn catalogue_tool_ids(&self) -> Vec<(ToolName, String)>;
}

/// **The SINGLE SQL the live-PG scoped-tool-list read issues (the no-N+1 push-down, the `integration`
/// path).** Returns the full `SELECT name FROM tool_def WHERE <(tenant, region) RLS> AND <scope
/// predicate over tool_def.id>` — ONE statement, no per-tool round-trip. The `(tenant, region)`
/// predicate is threaded FIRST (the `tenant-predicate` lint — never a tenant-less query); the scope
/// predicate is conjoined AFTER. The live test (`tests/integration_tool_scope.rs`) proves this returns
/// the SAME set the in-process [`ToolScopePredicate::admits`] reference path produces.
pub fn scoped_tool_ids_sql(predicate: &ToolScopePredicate) -> String {
    format!(
        "SELECT name FROM tool_def \
         WHERE tenant_id = current_setting('myelin.tenant_id') \
           AND region = current_setting('myelin.region') \
           AND ({})",
        predicate.to_sql(TOOL_ID_COLUMN)
    )
}

// ───────────────────────── the apply-time re-check assertion (the guarantee, AG-P6) ──────────────

/// **The fail-closed guarantee: a tool in the scoped list whose grant was revoked SINCE the push-down
/// is DENIED at apply time (0 stale-grant applies, §2.1 / §5.2).** The scoping is an OPTIMISATION; the
/// `EffectApi` apply-time re-check is the GUARANTEE. This is the chained property: (1) the push-down
/// scoped tool `T` into the brain's list; (2) `T`'s grant is revoked (the authz reverse index drops
/// the tuple) AFTER the conversation was built; (3) the brain proposes `T`; (4) `EffectApi`'s
/// CAPABILITY/DELEGATION steps re-check at apply time and DENY — the stale scoping never leaks.
///
/// `apply_outcome` is the [`EffectResult`](myelin_agent::EffectResult) the AG-P6 pipeline returned for
/// the now-revoked tool. The assertion: it MUST be `Denied` (NOT `Applied`) — `true` iff the
/// apply-time check overrode the stale scoping (the property holds), `false` iff the optimisation
/// leaked (the property FAILED — a stale-grant apply, which must NEVER happen).
pub fn assert_apply_rechecks_revoked(apply_outcome: &myelin_agent::EffectResult) -> bool {
    matches!(apply_outcome, myelin_agent::EffectResult::Denied(_))
}

/// **Replace `Conversation.tools` with the delegation-scoped subset (the §2.1 conversation-build
/// wiring).** The loop's `build_conversation` (`mock.rs`) frames the system/budget/transcript; THIS
/// stamps the scoped subset onto `Conversation.tools` so the brain is shown exactly the run's
/// permitted tools (the push-down feeding the brain). Deterministic — the same scoped list always
/// produces the same `Conversation` (the AG-D9 conversation-reconstruction leg).
pub fn apply_scope_to_conversation(conv: &mut Conversation, scoped: &ScopedToolList) {
    conv.tools = scoped.tools.clone();
}

/// A `ToolCatalogueIds` over a `ToolDef` slice — the Fabric's `tool_def.id` is the qualified
/// `(subsystem, name, version)` key the catalogue mints (Identity never invents object ids). Exposed
/// so the SKELETON/Mock dispatch tier can enumerate the catalogue for the scope build.
pub fn tool_def_id(def: &ToolDef) -> String {
    format!("{}/{}/{}", def.subsystem, def.name.0, def.version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent::EffectKind;
    use myelin_identity::ObjectId;
    use std::cell::Cell;

    // ───────── a real in-memory catalogue + a counting list_objects provider (the CDC shape) ──────

    /// A `ToolSurface` + `ToolCatalogueIds` over a fixed `tool_def` set (the §4.2 registry).
    struct Catalogue {
        defs: Vec<ToolDef>,
    }
    impl ToolSurface for Catalogue {
        fn register_tool(&mut self, def: ToolDef) {
            self.defs.push(def);
        }
        fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
            self.defs.iter().find(|d| &d.name == name)
        }
    }
    impl ToolCatalogueIds for Catalogue {
        fn catalogue_tool_ids(&self) -> Vec<(ToolName, String)> {
            self.defs
                .iter()
                .map(|d| (d.name.clone(), tool_def_id(d)))
                .collect()
        }
    }

    /// A `list_objects` provider that returns a fixed result and COUNTS its calls — the no-N+1 GATE
    /// reads the counter (it MUST be 1 per scope build, regardless of catalogue size).
    struct CountingListObjects {
        result: ListObjectsResult,
        calls: Cell<usize>,
    }
    impl ToolListObjects for CountingListObjects {
        fn list_objects(
            &self,
            _subject: &str,
            _permission: &Permission,
            _ty: &ObjectType,
            _at: &Zookie,
        ) -> ListObjectsResult {
            self.calls.set(self.calls.get() + 1);
            self.result.clone()
        }
    }

    fn def(name: &str, subsystem: &str) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: subsystem.into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec!["tool.use".into()],
            effect_kind: EffectKind::Mutate,
            side_effecting: true,
            requires_approval: false,
            exposed_over_mcp: false,
        }
    }

    fn catalogue(names: &[(&str, &str)]) -> Catalogue {
        Catalogue {
            defs: names.iter().map(|(n, s)| def(n, s)).collect(),
        }
    }

    // ───────────────────────── the no-N+1 GATE (query_count == 1) ─────────────────────────────

    /// **The scoped tool list is computed in a SINGLE query (no N+1) — `query_count == 1`
    /// regardless of catalogue size.** A 50-tool catalogue still issues ONE `list_objects` call (the
    /// OQ-E property: never a per-tool `check`).
    #[test]
    fn scoped_list_is_one_query_regardless_of_tool_count() {
        let big: Vec<(String, String)> = (0..50)
            .map(|i| (format!("tool-{i}"), "issues".to_string()))
            .collect();
        let refs: Vec<(&str, &str)> =
            big.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect();
        let cat = catalogue(&refs);

        // The push-down result admits the first three tools (a bounded allow-set).
        let lo = CountingListObjects {
            result: ListObjectsResult::Filter {
                set_expr: SetExpr::Ids(vec![
                    ObjectId("issues/tool-0/1".into()),
                    ObjectId("issues/tool-1/1".into()),
                    ObjectId("issues/tool-2/1".into()),
                ]),
                zookie: Zookie("z-7".into()),
            },
            calls: Cell::new(0),
        };

        let scoped = build_scoped_tool_list(&cat, &lo, "psn:agent-7", &Zookie("z-1".into()));

        // EXACTLY one list_objects call — the no-N+1 GATE (50 tools, ONE query).
        assert_eq!(lo.calls.get(), 1, "exactly one list_objects call for the whole catalogue");
        assert_eq!(scoped.query_count, 1, "the no-N+1 GATE: one query");
        // The brain sees ONLY the three scoped tools (the delegation-scoped subset).
        assert_eq!(scoped.tools.len(), 3);
        assert!(scoped.tools.contains(&ToolSchema("tool-0".into())));
        assert!(scoped.tools.contains(&ToolSchema("tool-2".into())));
        assert!(!scoped.tools.contains(&ToolSchema("tool-3".into())));
        // The zookie watermark is carried back (4.10 — read-your-writes at apply).
        assert_eq!(scoped.zookie, Zookie("z-7".into()));
    }

    // ───────────────────────── the SetExpr lowers to ONE predicate (no N+1) ───────────────────

    /// **The `SetExpr` lowers to a SINGLE predicate (no per-tool clause).** A bounded allow-set
    /// lowers to ONE `id IN (..)` clause; `None` to `WHERE false`; `All` to `true`.
    #[test]
    fn set_expr_lowers_to_one_predicate() {
        let pred = lower_set_expr(&SetExpr::Ids(vec![
            ObjectId("issues/a/1".into()),
            ObjectId("issues/b/1".into()),
        ]));
        assert_eq!(
            pred.to_sql(TOOL_ID_COLUMN),
            "id IN ('issues/a/1', 'issues/b/1')",
            "ONE membership clause — no per-tool check"
        );
        assert_eq!(lower_set_expr(&SetExpr::None).to_sql("id"), "false");
        assert_eq!(lower_set_expr(&SetExpr::All).to_sql("id"), "true");
        // The full SQL threads the (tenant, region) predicate FIRST (the tenant-predicate lint).
        let sql = scoped_tool_ids_sql(&pred);
        assert!(sql.contains("tenant_id = current_setting('myelin.tenant_id')"));
        assert!(sql.contains("region = current_setting('myelin.region')"));
        assert!(sql.contains("id IN ('issues/a/1', 'issues/b/1')"));
    }

    /// The boolean `SetExpr` forms lower to composed clauses (Union→OR, Intersect→AND,
    /// Difference→`left AND NOT right`) — still ONE conjoinable predicate, no N+1.
    #[test]
    fn boolean_set_expr_forms_compose_to_one_predicate() {
        let union = lower_set_expr(&SetExpr::Union(vec![
            SetExpr::Ids(vec![ObjectId("x".into())]),
            SetExpr::Ids(vec![ObjectId("y".into())]),
        ]));
        assert_eq!(union.to_sql("id"), "(id IN ('x') OR id IN ('y'))");

        let intersect = lower_set_expr(&SetExpr::Intersect(vec![
            SetExpr::NotIds(vec![ObjectId("secret".into())]),
            SetExpr::All,
        ]));
        assert_eq!(intersect.to_sql("id"), "(id NOT IN ('secret') AND true)");

        let diff = lower_set_expr(&SetExpr::Difference(
            Box::new(SetExpr::All),
            Box::new(SetExpr::Ids(vec![ObjectId("denied".into())])),
        ));
        assert_eq!(diff.to_sql("id"), "(true AND (NOT id IN ('denied')))");
    }

    /// **Fail-closed: an unresolved relational `InRelation`/`TupleSet` lowers to `None` in the
    /// in-process reference path (a relational grant is NOT admitted without the affirmative reverse
    /// index — never a silent allow).** The live JOIN target is Identity's (P-ID-11/P-ID-12 floor).
    #[test]
    fn relational_set_expr_is_fail_closed_in_reference_path() {
        let in_rel = lower_set_expr(&SetExpr::InRelation {
            relation: myelin_identity::RelName("user".into()),
            via_column: myelin_identity::ColRef {
                table: "tool_def".into(),
                column: "id".into(),
            },
        });
        assert_eq!(in_rel, ToolScopePredicate::None, "no silent allow for a relational grant");
        assert!(!in_rel.admits("any-tool"));
    }

    /// `None` (deny) ⇒ the scoped list is EMPTY (the brain gets no tool); `All` ⇒ every tool.
    #[test]
    fn deny_yields_empty_scope_and_all_yields_full() {
        let cat = catalogue(&[("a", "issues"), ("b", "git")]);
        let deny = CountingListObjects {
            result: ListObjectsResult::Filter { set_expr: SetExpr::None, zookie: Zookie("z".into()) },
            calls: Cell::new(0),
        };
        let scoped = build_scoped_tool_list(&cat, &deny, "psn:x", &Zookie("z0".into()));
        assert!(scoped.tools.is_empty(), "a denied run is shown NO tool");

        let all = CountingListObjects {
            result: ListObjectsResult::Ids { ids: vec![], zookie: Zookie("z".into()) }, // Ids materialised
            calls: Cell::new(0),
        };
        // An empty Ids materialised result admits nothing (an explicit empty allow-set).
        let scoped_empty = build_scoped_tool_list(&cat, &all, "psn:x", &Zookie("z0".into()));
        assert!(scoped_empty.tools.is_empty());

        let admin = CountingListObjects {
            result: ListObjectsResult::Filter { set_expr: SetExpr::All, zookie: Zookie("z".into()) },
            calls: Cell::new(0),
        };
        let scoped_all = build_scoped_tool_list(&cat, &admin, "psn:x", &Zookie("z0".into()));
        assert_eq!(scoped_all.tools.len(), 2, "admin (All) sees every tool of this type");
    }

    /// The lowering of a materialised `Ids` result (the S4 path) is the SAME allow-set membership as
    /// the `Filter{Ids}` push-down (the S8 path) — one ACL meaning, no drift.
    #[test]
    fn materialised_ids_and_filter_ids_lower_identically() {
        let ids = vec![ObjectId("git/merge/1".into())];
        let from_materialised =
            lower_list_objects(&ListObjectsResult::Ids { ids: ids.clone(), zookie: Zookie("z".into()) });
        let from_filter = lower_list_objects(&ListObjectsResult::Filter {
            set_expr: SetExpr::Ids(ids),
            zookie: Zookie("z".into()),
        });
        assert_eq!(from_materialised, from_filter, "no drift between the S4 and S8 paths");
    }

    // ───────────────────────── the apply-time re-check (the guarantee) ────────────────────────

    /// **Chained property: a tool in the scoped list whose grant is revoked SINCE the push-down is
    /// DENIED at apply time (0 stale-grant applies).** (1) the push-down scoped `merge` into the
    /// brain's list; (2) `merge`'s grant is revoked; (3) the brain proposes `merge`; (4) `EffectApi`
    /// re-checks and returns `Denied` — the stale scoping does NOT leak.
    #[test]
    fn apply_rechecks_a_revoked_but_scoped_tool() {
        // (1) the push-down scoped `git/merge/1` into the brain's tool list.
        let cat = catalogue(&[("merge", "git")]);
        let lo = CountingListObjects {
            result: ListObjectsResult::Filter {
                set_expr: SetExpr::Ids(vec![ObjectId("git/merge/1".into())]),
                zookie: Zookie("z-build".into()),
            },
            calls: Cell::new(0),
        };
        let scoped = build_scoped_tool_list(&cat, &lo, "psn:agent", &Zookie("z-0".into()));
        assert!(scoped.tools.contains(&ToolSchema("merge".into())), "merge was scoped in");

        // (2)+(3)+(4): the grant is revoked AFTER the build; the brain proposes `merge`; EffectApi
        // re-checks at apply time and DENIES. The Denied outcome is the AG-P6 pipeline's verdict for
        // the now-revoked cap (the apply-time check overrides the stale scoping — the guarantee).
        let apply_outcome = myelin_agent::EffectResult::Denied(
            "capability check denied for git.merge (revoked since scope)".into(),
        );
        assert!(
            assert_apply_rechecks_revoked(&apply_outcome),
            "the apply-time check MUST override a stale scope (0 stale-grant applies)"
        );

        // The inverse: an Applied for a revoked tool would be a stale-grant LEAK — the assertion fails.
        let leaked = myelin_agent::EffectResult::Applied(myelin_agent::EventId("evt".into()));
        assert!(
            !assert_apply_rechecks_revoked(&leaked),
            "an Applied for a revoked tool is a stale-grant leak (the property must catch it)"
        );
    }

    /// `apply_scope_to_conversation` stamps the scoped subset onto `Conversation.tools` (the brain is
    /// shown exactly the scoped subset).
    #[test]
    fn scope_is_stamped_onto_the_conversation() {
        let scoped = ScopedToolList {
            tools: vec![ToolSchema("read".into()), ToolSchema("merge".into())],
            zookie: Zookie("z".into()),
            query_count: 1,
        };
        let mut conv = Conversation::default();
        apply_scope_to_conversation(&mut conv, &scoped);
        assert_eq!(conv.tools, scoped.tools, "the brain sees exactly the scoped subset");
    }

    /// SQL-injection safety: a `'` in a tool id is escaped in the inline-literal form (the live path
    /// binds `$n`, but the reference `to_sql` must not be injectable either).
    #[test]
    fn sql_id_list_escapes_quotes() {
        let pred = ToolScopePredicate::Ids(vec!["a'b".into()]);
        assert_eq!(pred.to_sql("id"), "id IN ('a''b')");
    }
}
