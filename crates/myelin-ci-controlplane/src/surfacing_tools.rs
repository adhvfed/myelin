//! # `surfacing_tools` — the CI agent-facing `ToolDef` registrations (CI-P26 / P-369, M4)
//!
//! The FOURTH half of CI's cross-fabric surfacing: CI registers its agent-facing actions into the
//! ONE permissioned [`ToolSurface`](myelin_agent::ToolSurface) with the **FROZEN X-6
//! `requires_approval` defaults** (arch
//! `04-subsystem-architectures/continuous-integration/architecture/04-views-cli-and-api.md` §3;
//! `05-refined-shared-systems-architecture/00-reconciliation-decisions.md` §X-6).
//!
//! ## The frozen X-6 table (arch 04 §3 — never weakened)
//! | ToolDef | effect_kind | side-effecting | `requires_approval` |
//! |---|---|---|---|
//! | `ci.run` / `run_pipeline` (non-prod) | mutate | yes | **no** (cheap, reversible, metered) |
//! | `ci.cancel_run` | mutate | yes | **no** (low-risk, reversible) |
//! | `ci.retry_run` | mutate | yes | **no** (reserve-gated; bumps `run_attempt`) |
//! | `ci.read_log` / `ci.read_run` | read | no | **no** (ACL-checked read — RAG/triage input) |
//! | `ci.validate` / `ci.plan` | read | no | **no** (shift-left; no runner spend) |
//! | `ci.deploy` (protected env) | mutate | yes | **YES** (consequential → the HITL approval card) |
//! | `ci.approve_deploy` | mutate | yes | **YES** (privileged; an agent cannot self-approve) |
//! | `ci.rollback` (prod) | mutate | yes | **YES** (reversibility, but prod rollback is consequential) |
//! | `ci.write_secret` | mutate | yes | **YES** (audit-critical) |
//!
//! **`ToolHands::exec` is NOT in this table** — it is the runner itself (the `kind=agent` job, the
//! deepest unification, X-6 / 05 §HP-5), never a side-effecting tool.
//!
//! ## Reconciliation with the existing CI ToolDefs (EI-01 §7 coherence — survey-first)
//! The Fabric consumer crate (`myelin_agent_service::ci_tools`, CI-P24 / P-347) already shipped FOUR
//! of these — `deploy` / `approve_deploy` / `write_secret` (gated) + `run_pipeline` (not gated) —
//! wired through the `seed_requires_approval` + no-silent-loosening machinery. CI-P26 does NOT
//! duplicate them: this module is the **CI-OWNED X-6 classification + the COMPLETE def list** (the
//! genuinely-new read/run/cancel/retry/validate/plan/rollback rows the frozen table names but the
//! consumer crate had not yet enumerated). CI is the authority on which of ITS actions are
//! consequential (X-6 — a frozen contract value, not a Fabric product call); the Fabric
//! `requires_approval_default` seed table is RECONCILED against THIS owned classification by the
//! CI-side CDC (`crates/myelin-agent-service/tests/cdc_8_1_ci_x6_tooldefs.rs`) — a drift on either
//! side is a test break. The `required_caps` here name CI's frozen ReBAC permissions
//! (`run.trigger` / `environment.deploy` / `ci_project.administer` / `run.view`) — the SAME
//! permissions the consumer crate's `required_caps` builders consume from
//! `myelin_identity_service::ci_fragment`, restated as the canonical CI strings (this is a producer
//! LEAF that cannot depend on the Identity SERVICE crate — the §2.9 acyclic DAG; the SHAPE is the
//! wire contract, proven by the CDC).

use myelin_agent::{EffectKind, ToolDef, ToolName, ToolSurface};

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 0. FROZEN NAMES (the X-6 table keys + the CI ReBAC permissions — never a stray literal)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The CI subsystem token (the catalogue key half + the X-6 lookup key). Named once.
pub const CI_SUBSYSTEM: &str = "ci";

/// The ToolDef version the CI agent tools register at (forward-only; the catalogue key is
/// `(subsystem, name, version)`). v1 is the first frozen shape — the SAME version
/// [`myelin_agent_service::ci_tools::CI_TOOL_VERSION`] uses (no per-module drift).
pub const CI_TOOL_VERSION: u32 = 1;

// The CI ReBAC permission names the `required_caps` pin on (4.9 — the canonical CI fragment strings,
// restated because the producer leaf cannot depend on the Identity service crate; the CDC pins them).
/// `run.trigger` — the cap a run start / cancel / retry / validate / plan checks (the run fragment).
pub const CAP_RUN_TRIGGER: &str = "run.trigger";
/// `run.view` — the cap a `read_run` / `read_log` checks (the run-visibility fragment).
pub const CAP_RUN_VIEW: &str = "run.view";
/// `environment.deploy` — the cap a deploy / approve_deploy / rollback checks (the environment fragment).
pub const CAP_ENVIRONMENT_DEPLOY: &str = "environment.deploy";
/// `ci_project.administer` — the cap a `write_secret` checks (the ci_project fragment).
pub const CAP_CI_PROJECT_ADMINISTER: &str = "ci_project.administer";

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE FROZEN X-6 requires_approval CLASSIFICATION (CI is the owner — arch 04 §3)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The complete CI agent-tool name set (the X-6 table rows, arch 04 §3), in catalogue order.** The
/// single source of truth every registration + CDC iterates. `ci.run` and `ci.run_pipeline` are BOTH
/// present (the X-6 table names `ci.run / run_pipeline` as one row, two surface names — `run` is the
/// generic start, `run_pipeline` the non-prod alias; both not gated).
pub const CI_TOOL_NAMES: &[&str] = &[
    // not-gated (cheap/reversible/read)
    "run",
    "run_pipeline",
    "cancel_run",
    "retry_run",
    "read_log",
    "read_run",
    "validate",
    "plan",
    // gated (consequential/privileged)
    "deploy",
    "approve_deploy",
    "rollback",
    "write_secret",
];

/// **The FROZEN X-6 `requires_approval` default for a CI tool (arch 04 §3 — CI owns this
/// classification).** `true` (gated) for the consequential/privileged actions
/// (`deploy`/`approve_deploy`/`rollback`/`write_secret`); `false` for the cheap/reversible/read
/// actions (`run`/`run_pipeline`/`cancel_run`/`retry_run`/`read_log`/`read_run`/`validate`/`plan`).
/// An UNKNOWN CI tool fails CLOSED to gated (`true`) — a new consequential action is added HERE, never
/// silently un-gated. This is the CI-side authority the Fabric `requires_approval_default` seed table
/// is reconciled against (the CDC pins byte-equality).
pub fn ci_requires_approval_default(tool: &str) -> bool {
    match tool {
        // gated (yes) — consequential / irreversible / privileged.
        "deploy" | "approve_deploy" | "rollback" | "write_secret" => true,
        // not gated (no) — cheap / reversible / metered / ACL-checked read.
        "run" | "run_pipeline" | "cancel_run" | "retry_run" | "read_log" | "read_run"
        | "validate" | "plan" => false,
        // fail-closed: an unrecognised CI action is gated until the frozen table is extended HERE.
        _ => true,
    }
}

/// The `effect_kind` of a CI tool (arch 04 §3). The reads (`read_log`/`read_run`/`validate`/`plan`)
/// are [`EffectKind::Read`]; everything else is a [`EffectKind::Mutate`] (routed through
/// `EffectApi::apply` — plan-then-apply, never a direct mutation).
pub fn ci_effect_kind(tool: &str) -> EffectKind {
    match tool {
        "read_log" | "read_run" | "validate" | "plan" => EffectKind::Read,
        _ => EffectKind::Mutate,
    }
}

/// `true` iff the CI tool is side-effecting (the reads are not). Mirrors [`ci_effect_kind`]: a
/// `Read` tool is not side-effecting; a `Mutate` tool is.
pub fn ci_side_effecting(tool: &str) -> bool {
    !matches!(ci_effect_kind(tool), EffectKind::Read)
}

/// The frozen `required_caps` for a CI tool (4.9 — the CI ReBAC fragment permissions). Deploy /
/// approve / rollback check `environment.deploy`; a secret write checks `ci_project.administer`; a
/// read checks `run.view`; a run lifecycle op checks `run.trigger`.
pub fn ci_required_caps(tool: &str) -> Vec<String> {
    let cap = match tool {
        "deploy" | "approve_deploy" | "rollback" => CAP_ENVIRONMENT_DEPLOY,
        "write_secret" => CAP_CI_PROJECT_ADMINISTER,
        "read_log" | "read_run" => CAP_RUN_VIEW,
        // run / run_pipeline / cancel_run / retry_run / validate / plan — the run-trigger gate.
        _ => CAP_RUN_TRIGGER,
    };
    vec![cap.to_string()]
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE COMPLETE CI ToolDef SET (contract 8.1 — the OWNED registration)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// Build one CI [`ToolDef`] from its name, stamping the FROZEN X-6 classification
/// ([`ci_requires_approval_default`] / [`ci_effect_kind`] / [`ci_side_effecting`]) + the 4.9
/// `required_caps`. The `requires_approval` is SEEDED from the frozen table — never hand-set — so a
/// def cannot silently un-gate a consequential action.
pub fn ci_tool_def(tool: &str) -> ToolDef {
    ToolDef {
        name: ToolName(tool.to_string()),
        subsystem: CI_SUBSYSTEM.to_string(),
        version: CI_TOOL_VERSION,
        // The agent-native triage hook reads the structured ci.run.failed payload (which step / which
        // test / log excerpt — the E2E-2 flagship input, CI-P34); the per-tool input schema is the
        // minimal id carrier here (the full schema is the emit follow-on).
        input_schema: r#"{"type":"object"}"#.to_string(),
        required_caps: ci_required_caps(tool),
        effect_kind: ci_effect_kind(tool),
        side_effecting: ci_side_effecting(tool),
        // SEEDED from the frozen X-6 table (deploy/secret/rollback/approve = yes; the rest = no).
        requires_approval: ci_requires_approval_default(tool),
        // Not MCP-exposed at v1 (the external MCP endpoint is the post-M5 follow-on, AG-P25).
        exposed_over_mcp: false,
    }
}

/// **The complete CI agent-tool def set (contract 8.1 / X-6), in catalogue order.** The single list
/// every registration + CDC consumes. Each carries its FROZEN X-6 `requires_approval` + 4.9 caps.
pub fn ci_tool_defs() -> Vec<ToolDef> {
    CI_TOOL_NAMES.iter().map(|t| ci_tool_def(t)).collect()
}

/// **Register the COMPLETE CI agent-tool set into the ONE [`ToolSurface`] (8.1 / arch 04 §3).** The
/// deliverable: every CI agent-facing action is a row in the ONE catalogue with its frozen X-6
/// gating. Returns the registered defs so a caller can assert the registered shape. `ToolHands::exec`
/// (the runner) is DELIBERATELY ABSENT — it is the runner itself, never a side-effecting tool (X-6).
pub fn register_ci_tools<S: ToolSurface>(surface: &mut S) -> Vec<ToolDef> {
    let defs = ci_tool_defs();
    for def in &defs {
        surface.register_tool(def.clone());
    }
    defs
}

#[cfg(test)]
#[path = "surfacing_tools_tests.rs"]
mod tests;
