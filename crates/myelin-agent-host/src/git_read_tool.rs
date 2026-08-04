//! # `git_read_tool` — the FIRST real, governed READ tool: a `Direct`-route Git check-status read.
//!
//! This is the v1 tool that makes the hosted agent able to **act**, not just reason: a real Luna run
//! that *invokes* a governed READ tool, sees the real tenant data it returned, and answers — metered
//! end-to-end. It ships two halves:
//!
//! 1. **[`git_check_status_read_tool_def`] / [`git_check_status_read_tool_schema`]** — the catalogue
//!    [`ToolDef`] (what the platform loop `validate_call`s the untrusted model arguments against —
//!    the security checkpoint) + the model-facing [`ToolSchema`] (what Luna is told it may call). The
//!    tool's `effect_kind = Read`, so [`route_of`](myelin_agent_service::exec::route_of)`(Read) ==
//!    Direct`: a permission-filtered subsystem READ, no sandbox, no plan-then-apply.
//! 2. **[`GitCheckStatusReadExecutor`]** — the `Direct` [`ToolExecutor`] the loop calls after a
//!    `ToolCall` passes `validate_call`. It dispatches the validated call to the REAL Git subsystem
//!    read [`PgCheckStatusProjection::rows_for_commit`], which reads Git's own `check_status`
//!    projection **through `with_tenant_tx`** (the MR-022 RLS convention: the transaction-scoped
//!    `myelin.tenant_id` / `myelin.region` GUCs bound every row) AND fail-closes on a cross-region
//!    scope — so a cross-tenant read is structurally impossible (Postgres RLS refuses it, not app
//!    code). It returns the real check rows as bounded TEXT the brain reads on its next turn.
//!
//! ## Why the Git check-status projection is the cleanest v1 READ
//! It is a genuinely tenant-scoped real subsystem read that is *self-contained*: it is built from a
//! [`SubstrateProvider`] + a runtime handle ALONE — no KMS engine, no ReBAC authorizer, no
//! projection-rebuild choreography (the Issues `view` read needs all three; the Knowledge reads need
//! ACL-joins + schemas). RLS (`with_tenant_tx`) + the region fail-closed check ARE the access control
//! for a `Direct` read on this floor; the delegation-scoped tool-list push-down that additionally
//! filters by `required_caps` is the named follow-on (AG-P7 → the `list_objects` SetExpr push-down).
//! `required_caps = ["pull"]` is the declarative repo-read permission (Git's 4.9 fragment); it is
//! metadata here, enforced structurally by RLS today and by AG-P7 next.
//!
//! ## Tenant scoping is from the TOKEN, never the model's arguments
//! The executor holds the run's [`TenantScope`] (built from the verified agent [`Principal`] — the
//! tenant and region come from the token, never a path). The untrusted model `arguments` supply only
//! the *what* to read (`repo` + `commit`), never the *whose* — the model can never widen the read past its own
//! tenant. A missing/denied row (including a cross-tenant commit RLS filtered to empty) is an
//! [`Err(ToolExecError)`] → the driving loop tears the run down fail-closed (already handled).
//!
//! ## The sync→async bridge
//! [`ToolExecutor::execute`] is sync; the subsystem read is async. `rows_for_commit` performs the
//! bridge INTERNALLY (the same `tokio::task::block_in_place` + `Handle::block_on` bridge
//! [`AgentWallet`](myelin_storage::agent_wallet::AgentWallet)'s per-turn debit uses), so the run must
//! be driven on a multi-thread Tokio runtime worker — exactly where the metered loop's wallet debit
//! already runs. No model / SDK / prompt string appears here (the `no-llm-in-platform` boundary):
//! this is platform read code, the vendor edge lives only in the crate's `HostModelClient`/Luna home.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use myelin_agent::{EffectKind, ToolCall, ToolDef, ToolName, ToolResult, ToolSchema};
use myelin_agent_service::{ToolExecError, ToolExecutor};
use myelin_git::check_status::GitOid;
use myelin_git::check_status_store::PgCheckStatusProjection;
use myelin_storage::{SubstrateProvider, TenantScope};

/// The canonical tool name (the catalogue key + the name Luna echoes back on a call).
pub const GIT_READ_CHECK_STATUS_TOOL: &str = "git.read_check_status";

/// The tool's contributing subsystem (an event-bus token, mirrors the other Git ToolDefs).
const GIT_SUBSYSTEM: &str = "git";

/// The forward-only ToolDef version this read tool registers at.
const GIT_READ_TOOL_VERSION: u32 = 1;

/// The ONE JSON-Schema string for the read's arguments — shared by the catalogue [`ToolDef`] (the
/// `validate_call` security checkpoint reads it) and the model-facing [`ToolSchema`] (Luna produces
/// arguments to satisfy it), so the two can never drift. `repo` + `commit` are the *what* to read;
/// the *whose* (tenant + region) is the run's token scope, never an argument.
const GIT_READ_CHECK_STATUS_SCHEMA: &str = r#"{"type":"object","required":["repo","commit"],"properties":{"repo":{"type":"string"},"commit":{"type":"string"}}}"#;

/// **The `git.read_check_status` [`ToolDef`] (8.1) — the first governed READ tool.** `effect_kind =
/// Read` ⇒ the `Direct` route (a permission-filtered subsystem read; no sandbox, no plan-then-apply).
/// Non-side-effecting + never HITL-gated (a read mutates nothing). `required_caps = ["pull"]` is the
/// declarative Git repo-read permission (4.9); RLS (`with_tenant_tx`) is the structural enforcement
/// on this floor, the AG-P7 delegation-scoped tool-list push-down is the cap-filter follow-on.
pub fn git_check_status_read_tool_def() -> ToolDef {
    ToolDef {
        name: ToolName(GIT_READ_CHECK_STATUS_TOOL.to_string()),
        subsystem: GIT_SUBSYSTEM.to_string(),
        version: GIT_READ_TOOL_VERSION,
        input_schema: GIT_READ_CHECK_STATUS_SCHEMA.to_string(),
        required_caps: vec!["pull".to_string()],
        effect_kind: EffectKind::Read,
        side_effecting: false,
        requires_approval: false,
        exposed_over_mcp: false,
    }
}

/// **The model-facing [`ToolSchema`] for the read** (name + description + the SAME input schema the
/// catalogue [`ToolDef`] carries). The composition root turns this into the vendor tool spec Luna
/// sees, so the model knows the tool exists, what it does, and how to shape valid arguments.
pub fn git_check_status_read_tool_schema() -> ToolSchema {
    ToolSchema {
        name: ToolName(GIT_READ_CHECK_STATUS_TOOL.to_string()),
        description: "Read the current CI/external check-status rows Git recorded for a specific \
                      (repo, commit): each check context (e.g. ci/build, ci/test) and its state \
                      (success, failure, error, queued, in_progress, neutral, cancelled). Arguments: \
                      `repo` (the repo ref, e.g. myelin://<tenant>/git/repo/<id>) and `commit` (the \
                      commit OID). The read is scoped to your own tenant; you cannot read another \
                      tenant's data."
            .to_string(),
        input_schema: GIT_READ_CHECK_STATUS_SCHEMA.to_string(),
    }
}

/// **The `Direct`-route READ [`ToolExecutor`]: turn a VALIDATED `git.read_check_status` call into the
/// real Git subsystem read, tenant-scoped, fail-closed.**
///
/// Holds the live [`PgCheckStatusProjection`] (built from ONE [`SubstrateProvider`] + a runtime
/// handle — no KMS/ReBAC) and the run's [`TenantScope`] (tenant + region from the verified token). On
/// `execute` it parses the untrusted arguments for `repo` + `commit`, reads Git's `check_status`
/// projection through `with_tenant_tx` (RLS-scoped + region fail-closed), and returns the rows as
/// bounded TEXT — or an [`Err(ToolExecError)`] on a bad argument, a store fault, or a missing/denied
/// row (an empty result: the commit is absent OR filtered out by RLS as another tenant's — either way
/// the read fails closed and the loop tears the run down).
pub struct GitCheckStatusReadExecutor {
    projection: PgCheckStatusProjection,
    scope: TenantScope,
    /// How many times `execute` has been invoked — the "the tool was actually CALLED" witness a drill
    /// asserts on (the executor ran the real read at least once).
    invocations: AtomicUsize,
    /// The text of the last successful read (for a drill to inspect what the agent was handed).
    last_result: Mutex<Option<String>>,
}

impl GitCheckStatusReadExecutor {
    /// Build the executor over a [`SubstrateProvider`] + the run's verified [`TenantScope`].
    ///
    /// `provider` is the ordinary read lane and `admission_provider` the separately-bounded admission
    /// lane [`PgCheckStatusProjection`] wants (a read-only executor never takes an admission lock, so
    /// passing a clone of the same provider is correct). `runtime` is the multi-thread handle the
    /// projection's sync→async bridge blocks on — capture `tokio::runtime::Handle::current()` inside
    /// the run's runtime. `scope` MUST come from the run's verified token
    /// ([`TenantScope::from_verified_token`]) — the read is pinned to that tenant + region, never to a
    /// model argument.
    pub fn new(
        provider: SubstrateProvider,
        admission_provider: SubstrateProvider,
        runtime: tokio::runtime::Handle,
        scope: TenantScope,
    ) -> GitCheckStatusReadExecutor {
        GitCheckStatusReadExecutor {
            projection: PgCheckStatusProjection::production(provider, admission_provider, runtime),
            scope,
            invocations: AtomicUsize::new(0),
            last_result: Mutex::new(None),
        }
    }

    /// How many times the executor's `execute` ran (the tool-was-invoked witness).
    pub fn invocations(&self) -> usize {
        self.invocations.load(Ordering::SeqCst)
    }

    /// The TEXT of the last successful read (what the agent's next turn was handed), if any.
    pub fn last_result(&self) -> Option<String> {
        self.last_result.lock().expect("last_result lock").clone()
    }
}

impl ToolExecutor for GitCheckStatusReadExecutor {
    fn execute(&self, def: &ToolDef, call: &ToolCall) -> Result<ToolResult, ToolExecError> {
        // Record that the executor was reached (the tool-was-invoked witness) BEFORE any fallible
        // work — a validation abort or an empty read both count as "the tool ran".
        self.invocations.fetch_add(1, Ordering::SeqCst);

        // Defensive: this is the `Direct` READ executor — it serves ONLY `effect_kind = Read` tools
        // (route_of(Read) == Direct). A non-read def reaching here is a wiring bug; fail closed rather
        // than read under a mis-routed tool. (The loop already `validate_call`d the arguments against
        // `def.input_schema`; we do not re-validate the schema, only the routing invariant.)
        if def.effect_kind != EffectKind::Read {
            return Err(ToolExecError::Failed(format!(
                "git.read_check_status executor received a non-Read tool `{}` ({:?}) — the Direct \
                 read route serves only Read tools (fail-closed)",
                def.name.0, def.effect_kind
            )));
        }

        // Parse the UNTRUSTED model arguments for the read key (`repo`, `commit`). These are the
        // *what* to read; the *whose* (tenant + region) is `self.scope` from the verified token.
        let repo = call
            .arguments
            .get("repo")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolExecError::Failed(
                    "git.read_check_status requires a string `repo` argument (fail-closed)".into(),
                )
            })?;
        let commit = call
            .arguments
            .get("commit")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolExecError::Failed(
                    "git.read_check_status requires a string `commit` argument (fail-closed)".into(),
                )
            })?;

        // THE REAL SUBSYSTEM READ — Git's own `check_status` projection, through `with_tenant_tx`
        // (RLS: the tenant/region GUCs bound every row) + region fail-closed. A cross-tenant commit
        // is filtered to empty by Postgres RLS, never returned. The sync→async bridge is internal to
        // `rows_for_commit` (block_in_place + block_on), so this sync `execute` composes cleanly on
        // the run's multi-thread worker (the same place the per-turn wallet debit blocks).
        let rows = self
            .projection
            .rows_for_commit(&self.scope, repo, &GitOid(commit.to_string()))
            .map_err(|e| {
                ToolExecError::Failed(format!("git check-status subsystem read failed: {e}"))
            })?;

        // Fail-closed on a missing/denied row: an empty result means the commit is absent OR was
        // filtered out by RLS as another tenant's — either way there is nothing this run may read, so
        // abort the run rather than answer over an empty read.
        if rows.is_empty() {
            return Err(ToolExecError::Failed(format!(
                "no check status recorded for commit `{commit}` in repo `{repo}` — the commit is \
                 absent or outside this run's tenant scope (fail-closed)"
            )));
        }

        // Format the rows into bounded, PII-free TEXT the brain reads on its next turn: each check
        // context and its state (plus the supersession attempt, trust posture, and cost-settled
        // bookend — all references/labels, never log bytes).
        let mut parts = Vec::with_capacity(rows.len());
        for row in &rows {
            parts.push(format!(
                "{} = {:?} (run attempt {}, {:?}, cost_settled={})",
                row.context.policy_token(),
                row.state,
                row.run_attempt,
                row.trust_tier,
                row.cost_settled,
            ));
        }
        let text = format!(
            "check status for commit {commit} in repo {repo}: {}",
            parts.join("; ")
        );

        *self.last_result.lock().expect("last_result lock") = Some(text.clone());
        Ok(ToolResult(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent_service::exec::{route_of, ToolRoute};

    /// **The read tool is a `Direct`-route Read: `effect_kind = Read`, non-side-effecting, never
    /// gated, and `route_of(Read) == Direct`** (so the loop routes it to this Direct executor, not
    /// the sandbox / plan-then-apply).
    #[test]
    fn read_tool_def_is_a_direct_route_read() {
        let def = git_check_status_read_tool_def();
        assert_eq!(def.name, ToolName(GIT_READ_CHECK_STATUS_TOOL.into()));
        assert_eq!(def.effect_kind, EffectKind::Read);
        assert!(!def.side_effecting, "a read mutates nothing");
        assert!(!def.requires_approval, "a read is never HITL-gated");
        assert_eq!(route_of(def.effect_kind), ToolRoute::Direct);
        assert_eq!(def.required_caps, vec!["pull".to_string()]);
    }

    /// **The catalogue [`ToolDef`] and the model-facing [`ToolSchema`] share ONE name + input
    /// schema** (they cannot drift — the model produces arguments to the exact schema the security
    /// checkpoint validates them against).
    #[test]
    fn tool_def_and_schema_share_name_and_input_schema() {
        let def = git_check_status_read_tool_def();
        let schema = git_check_status_read_tool_schema();
        assert_eq!(def.name, schema.name);
        assert_eq!(def.input_schema, schema.input_schema);
        assert!(
            schema.description.contains("check-status"),
            "the model reads a real description"
        );
        // The shared schema is valid JSON declaring the two required string arguments.
        let parsed: serde_json::Value = serde_json::from_str(&def.input_schema).unwrap();
        assert_eq!(parsed["type"], "object");
        assert_eq!(parsed["required"][0], "repo");
        assert_eq!(parsed["required"][1], "commit");
    }
}
