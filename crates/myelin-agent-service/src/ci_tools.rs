//! # `ci_tools` — the per-consumer **CI** ToolDefs registered into the ONE ToolSurface
//! (AG-P20 → P-347, M4): `deploy` / `approve_deploy` / `write_secret` (the privileged gates) +
//! `run_pipeline` (non-prod — cheap, reversible, metered, NOT gated)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §6.1 (ONE catalogue — every
//! subsystem registers typed [`ToolDef`]s into the ONE shared [`ToolSurface`]), §6.3 (**the FROZEN
//! `requires_approval` defaults table** — CI `deploy(env)` to a protected env = **yes** (consequential),
//! `approve_deploy` / `write_secret` = **yes** (privileged), `run_pipeline` (non-prod) = **no** (cheap,
//! reversible, metered)), §5.0/§5.2 (a `mutate` effect routes through [`EffectApi::apply`] —
//! plan-then-apply; a `requires_approval` tool WITHHOLDS at step 6 → `Gated`, applies only after the
//! HITL resume).
//!
//! **VISION §3** (security by construction; consequential/irreversible/privileged actions
//! human-confirmed — a protected-env deploy, a deploy approval, and a secret write are the privileged
//! CI gates; a non-prod pipeline run is cheap and reversible). **EI-03 §4** (each new tool is a
//! PROJECTION of the existing plan-then-apply path — NO new engine). **EI-01 §7** (the compounding
//! payoff — the SAME registration shape as the Git/KN/Issues surfaces).
//!
//! **Contract-index:** OWNS the CI slice of **8.1** (`register_tool` — the four CI consumer ToolDefs).
//! CONSUMES **4.9** (the CI ReBAC fragment supplies the `required_caps` — the `environment.deploy` /
//! `ci_project.administer` / `run.trigger` permission names from
//! [`myelin_identity_service::ci_fragment`], the canonical §5 CI fragment). The `requires_approval`
//! column is SEEDED from the frozen §6.3 table via [`crate::defaults::seed_requires_approval`].
//!
//! ## What this prompt (AG-P20) ships — the CI consumer ToolDefs (NO new engine)
//! - [`deploy_tool_def`] — `ci.deploy`: `effect_kind = mutate`, `requires_approval = yes` (seeded from
//!   §6.3 — a protected-env deploy is consequential), `required_caps = [environment.deploy]` (4.9).
//!   WITHHOLDS at step 6 → `Gated` until the HITL resume (AG-P9). A deploy NEVER applies before
//!   approval.
//! - [`approve_deploy_tool_def`] — `ci.approve_deploy`: gated identically (`yes`), `required_caps =
//!   [environment.deploy]` (4.9 — approving a deploy is governed by the same environment-deploy gate).
//! - [`write_secret_tool_def`] — `ci.write_secret`: gated identically (`yes`), `required_caps =
//!   [ci_project.administer]` (4.9 — managing a secret is a CI-project-admin op; a secret's READ is the
//!   separate DIRECT NARROW relation, CI-1, never inherited).
//! - [`run_pipeline_tool_def`] — `ci.run_pipeline` (non-prod): `requires_approval = no` (seeded — cheap,
//!   reversible, metered), `required_caps = [run.trigger]` (4.9). Applies DIRECTLY (no HITL gate).
//! - [`register_ci_tools`] — registers ALL FOUR into a caller-supplied [`ToolSurface`] through the
//!   frozen seed + the no-silent-loosening guard ([`crate::defaults::assert_no_silent_loosening`]), so
//!   a registration that tried to silently un-gate `deploy`/`approve_deploy`/`write_secret` is REJECTED
//!   LOUD (VISION §3).
//!
//! ## Why this is NOT a new engine (the EI-03 §4 / EI-01 §7 compounding-payoff check)
//! The CI deploy/secret endpoints + the CI ReBAC fragment are CI's deliverables. The Fabric half is
//! PURELY the catalogue registration: a `ToolDef` is a row in the ONE registry; the routing (`mutate`
//! → [`EffectApi`]), the gating (`requires_approval` → step-6 withhold), and the HITL machinery already
//! exist (AG-P6/P9). NO `apply` path, NO gate machinery — data that lights up the existing pipeline.
//!
//! ## FLOORS named (cross-references; VISION §3, EI-01 §1)
//! - **NONE for the CI tools** — projections of the existing plan-then-apply path.
//! - **The AG-D4 / CI-T1 re-confirm on the PRODUCTION CI runner image is AG-P21 (→ P-348)** — the M4
//!   hard gate. The CI deploy tools run on that prod image; the ZERO-escapes re-confirm is the SEPARATE
//!   next prompt (AG-P21), not this one. Cross-reference stated.
//! - **The external MCP ENDPOINT** is the post-M5 follow-on (AG-P25); not MCP-exposed at v1.

use myelin_agent::{EffectKind, ToolDef, ToolName, ToolSurface};
use myelin_identity_service::ci_fragment::object_types as ci_objects;
use myelin_identity_service::ci_fragment::{ADMINISTER, DEPLOY, TRIGGER};

use crate::defaults::{assert_no_silent_loosening, seed_requires_approval, LooseningViolation};

// ───────────────────────── the frozen CI consumer-tool identity (the §6.3 keys) ──────────────────

/// **The CI subsystem token** — the `subsystem` half of the catalogue key + the key the FROZEN §6.3
/// defaults table is looked up under (`("ci", "deploy")` → gated, `("ci", "run_pipeline")` → not). The
/// SINGLE source of truth so a typo can't drift the seed.
pub const CI_SUBSYSTEM: &str = "ci";

/// **The `ci.deploy` tool name** (§6.3 — a protected-env deploy is consequential, gated). The seed
/// keys on `("ci", "deploy")`.
pub const DEPLOY_TOOL: &str = "deploy";

/// **The `ci.approve_deploy` tool name** (§6.3 — approving a deploy is privileged, gated). The seed
/// keys on `("ci", "approve_deploy")`.
pub const APPROVE_DEPLOY_TOOL: &str = "approve_deploy";

/// **The `ci.write_secret` tool name** (§6.3 — a secret write is privileged, gated). The seed keys on
/// `("ci", "write_secret")`.
pub const WRITE_SECRET_TOOL: &str = "write_secret";

/// **The `ci.run_pipeline` tool name** (§6.3 — non-prod: cheap, reversible, metered, NOT gated). The
/// seed keys on `("ci", "run_pipeline")`.
pub const RUN_PIPELINE_TOOL: &str = "run_pipeline";

/// **The ToolDef version** the CI consumer tools register at (forward-only; the catalogue key is
/// `(subsystem, name, version)`, §4.2). v1 is the first frozen shape.
pub const CI_TOOL_VERSION: u32 = 1;

// ───────────────────────── the required_caps from the CI ReBAC fragment (4.9) ────────────────────

/// **The `required_caps` for `ci.deploy` / `ci.approve_deploy` (CONSUMED from 4.9).** Deploying to (and
/// approving a deploy to) a protected environment is governed by the `environment.deploy` permission
/// the canonical CI fragment declares
/// ([`environment_fragment`](myelin_identity_service::ci_fragment::environment_fragment): `deploy =
/// deployer ∪ parent_ci_project->administer`). Built from the canonical `myelin-identity-service`
/// constants so a rename in the fragment is a compile-or-test break here, never a silent drift.
pub fn deploy_required_caps() -> Vec<String> {
    vec![format!("{}.{}", ci_objects::ENVIRONMENT, DEPLOY)]
}

/// **The `required_caps` for `ci.write_secret` (CONSUMED from 4.9).** Managing a secret is a CI-project
/// administration op governed by the `ci_project.administer` permission
/// ([`ci_project_fragment`](myelin_identity_service::ci_fragment::ci_project_fragment): `administer =
/// admin`). A secret's READ is the separate DIRECT NARROW `secret.read` relation (CI-1) — never
/// inherited; the WRITE is the project-admin gate.
pub fn write_secret_required_caps() -> Vec<String> {
    vec![format!("{}.{}", ci_objects::CI_PROJECT, ADMINISTER)]
}

/// **The `required_caps` for `ci.run_pipeline` (CONSUMED from 4.9).** Triggering a (non-prod) pipeline
/// run is governed by the `run.trigger` permission ([`run_fragment`](myelin_identity_service::ci_fragment::run_fragment):
/// `trigger = parent_repo->push`). Cheap, reversible, metered → NOT gated.
pub fn run_pipeline_required_caps() -> Vec<String> {
    vec![format!("{}.{}", ci_objects::RUN, TRIGGER)]
}

// ───────────────────────── the four CI consumer ToolDefs (8.1 — the OWNED registration) ───────────

/// **The `ci.deploy` ToolDef (8.1) — the consequential, HITL-GATED protected-env deploy (§6.3).**
///
/// - `effect_kind = Mutate` ⇒ it routes through [`EffectApi::apply`](myelin_agent::EffectApi) —
///   plan-then-apply, NEVER a direct mutation (§5.0).
/// - `requires_approval` is SEEDED from the frozen §6.3 default (`("ci", "deploy")` → `true`), so step
///   6 WITHHOLDS (`Gated`) until the HITL resume (AG-P9). A deploy NEVER applies before approval.
/// - `required_caps = [environment.deploy]` (4.9). `exposed_over_mcp = false` (AG-P25).
pub fn deploy_tool_def() -> ToolDef {
    seed_requires_approval(ToolDef {
        name: ToolName(DEPLOY_TOOL.to_string()),
        subsystem: CI_SUBSYSTEM.to_string(),
        version: CI_TOOL_VERSION,
        input_schema: r#"{"type":"object","required":["environment","artifact"],"properties":{"environment":{"type":"string"},"artifact":{"type":"string"}}}"#.to_string(),
        required_caps: deploy_required_caps(),
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        // SEEDED below from §6.3 (a protected-env deploy is gated → true).
        requires_approval: true,
        exposed_over_mcp: false,
    })
}

/// **The `ci.approve_deploy` ToolDef (8.1) — the privileged, HITL-GATED deploy approval (§6.3).** Gated
/// identically to `deploy` (`yes`), `required_caps = [environment.deploy]` (4.9).
pub fn approve_deploy_tool_def() -> ToolDef {
    seed_requires_approval(ToolDef {
        name: ToolName(APPROVE_DEPLOY_TOOL.to_string()),
        subsystem: CI_SUBSYSTEM.to_string(),
        version: CI_TOOL_VERSION,
        input_schema: r#"{"type":"object","required":["deployment"],"properties":{"deployment":{"type":"string"}}}"#.to_string(),
        required_caps: deploy_required_caps(),
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        // SEEDED below from §6.3 (a deploy approval is gated → true).
        requires_approval: true,
        exposed_over_mcp: false,
    })
}

/// **The `ci.write_secret` ToolDef (8.1) — the privileged, HITL-GATED secret write (§6.3).** Gated
/// identically (`yes`), `required_caps = [ci_project.administer]` (4.9 — a secret WRITE is a
/// project-admin op; the secret READ is the separate DIRECT NARROW relation, CI-1).
pub fn write_secret_tool_def() -> ToolDef {
    seed_requires_approval(ToolDef {
        name: ToolName(WRITE_SECRET_TOOL.to_string()),
        subsystem: CI_SUBSYSTEM.to_string(),
        version: CI_TOOL_VERSION,
        input_schema: r#"{"type":"object","required":["ci_project","name"],"properties":{"ci_project":{"type":"string"},"name":{"type":"string"},"value_ref":{"type":"string"}}}"#.to_string(),
        required_caps: write_secret_required_caps(),
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        // SEEDED below from §6.3 (a secret write is privileged → gated → true).
        requires_approval: true,
        exposed_over_mcp: false,
    })
}

/// **The `ci.run_pipeline` ToolDef (8.1) — the non-prod, NON-gated pipeline trigger (§6.3).**
///
/// - `effect_kind = Mutate` ⇒ it routes through [`EffectApi::apply`] (plan-then-apply) — still
///   governed (cap-checked, metered), just NOT HITL-gated.
/// - `requires_approval` is SEEDED from the frozen §6.3 default (`("ci", "run_pipeline")` → `false`)
///   — non-prod runs are cheap, reversible, metered. Applies DIRECTLY (no HITL gate).
/// - `required_caps = [run.trigger]` (4.9). `exposed_over_mcp = false`.
pub fn run_pipeline_tool_def() -> ToolDef {
    seed_requires_approval(ToolDef {
        name: ToolName(RUN_PIPELINE_TOOL.to_string()),
        subsystem: CI_SUBSYSTEM.to_string(),
        version: CI_TOOL_VERSION,
        input_schema: r#"{"type":"object","required":["ci_project","ref"],"properties":{"ci_project":{"type":"string"},"ref":{"type":"string"}}}"#.to_string(),
        required_caps: run_pipeline_required_caps(),
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        // SEEDED below from §6.3 (a non-prod pipeline run is NOT gated).
        requires_approval: false,
        exposed_over_mcp: false,
    })
}

/// **The four CI consumer ToolDefs, in catalogue order (deploy → approve_deploy → write_secret →
/// run_pipeline).** The single list every registration + CDC consumes (one source of truth). All four
/// are SEEDED from the frozen §6.3 defaults; only `run_pipeline` is NOT gated.
pub fn ci_tool_defs() -> Vec<ToolDef> {
    vec![
        deploy_tool_def(),
        approve_deploy_tool_def(),
        write_secret_tool_def(),
        run_pipeline_tool_def(),
    ]
}

// ───────────────────────── the registration seam (8.1 — into the ONE ToolSurface) ────────────────

/// **Register the CI consumer ToolDefs into the ONE [`ToolSurface`] (8.1 / §6.1) — the OWNED
/// deliverable.** Every def is passed through the VISION §3 no-silent-loosening guard FIRST: a
/// registration that tried to flip the frozen `deploy`/`approve_deploy`/`write_secret` `yes → no`
/// WITHOUT a written deviation is REJECTED LOUD. The seeded defs always admit; the guard is the
/// structural ratchet. Identical in shape to the Git/KN/Issues registrations — the compounding-payoff
/// reuse.
pub fn register_ci_tools<S: ToolSurface>(
    surface: &mut S,
) -> Result<Vec<ToolDef>, LooseningViolation> {
    let defs = ci_tool_defs();
    for def in &defs {
        assert_no_silent_loosening(def, &[])?;
    }
    for def in &defs {
        surface.register_tool(def.clone());
    }
    Ok(defs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::requires_approval_default;

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

    /// **`deploy` + `approve_deploy` + `write_secret` carry the FROZEN §6.3 `requires_approval = yes`
    /// default (consequential/privileged) — seeded, not hand-set.** Each WITHHOLDS until the HITL
    /// resume.
    #[test]
    fn deploy_approve_and_write_secret_are_gated_by_the_frozen_default() {
        for (def, tool) in [
            (deploy_tool_def(), DEPLOY_TOOL),
            (approve_deploy_tool_def(), APPROVE_DEPLOY_TOOL),
            (write_secret_tool_def(), WRITE_SECRET_TOOL),
        ] {
            assert!(
                def.requires_approval,
                "ci.{tool} is HITL-gated (§6.3 — consequential/privileged)"
            );
            assert_eq!(
                def.requires_approval,
                requires_approval_default(CI_SUBSYSTEM, tool),
                "ci.{tool}'s gating IS the frozen §6.3 default (seeded, not hand-set)"
            );
            assert_eq!(def.effect_kind, EffectKind::Mutate);
            assert!(def.side_effecting);
        }
    }

    /// **`run_pipeline` (non-prod) carries the FROZEN §6.3 `requires_approval = no` default — seeded,
    /// not hand-set.** Cheap, reversible, metered → applies DIRECTLY (no HITL gate).
    #[test]
    fn run_pipeline_non_prod_is_not_gated_by_the_frozen_default() {
        let def = run_pipeline_tool_def();
        assert!(
            !def.requires_approval,
            "ci.run_pipeline (non-prod) is cheap/reversible → NOT gated (§6.3)"
        );
        assert_eq!(
            def.requires_approval,
            requires_approval_default(CI_SUBSYSTEM, RUN_PIPELINE_TOOL),
            "ci.run_pipeline's (non-)gating IS the frozen §6.3 default (seeded, not hand-set)"
        );
        assert_eq!(def.effect_kind, EffectKind::Mutate);
    }

    /// **The `required_caps` come from the FROZEN CI ReBAC fragment (4.9), not invented here.**
    /// `deploy`/`approve_deploy` → `environment.deploy`; `write_secret` → `ci_project.administer`;
    /// `run_pipeline` → `run.trigger`. Built from the canonical `myelin-identity-service` constants, so
    /// a fragment rename breaks this test (no silent drift — the CI parallel to the Git/KN CDCs).
    #[test]
    fn required_caps_are_the_ci_rebac_fragment_permissions() {
        assert_eq!(
            deploy_tool_def().required_caps,
            vec!["environment.deploy".to_string()]
        );
        assert_eq!(
            approve_deploy_tool_def().required_caps,
            vec!["environment.deploy".to_string()]
        );
        assert_eq!(
            write_secret_tool_def().required_caps,
            vec!["ci_project.administer".to_string()]
        );
        assert_eq!(
            run_pipeline_tool_def().required_caps,
            vec!["run.trigger".to_string()]
        );
        // the canonical CI fragment names (4.9), not local strings.
        assert_eq!(ci_objects::ENVIRONMENT, "environment");
        assert_eq!(ci_objects::CI_PROJECT, "ci_project");
        assert_eq!(ci_objects::RUN, "run");
        assert_eq!(DEPLOY, "deploy");
        assert_eq!(ADMINISTER, "administer");
        assert_eq!(TRIGGER, "trigger");
    }

    /// **`register_ci_tools` registers ALL FOUR consumer ToolDefs into the ONE catalogue (8.1 / §6.1)
    /// and they resolve by name with their frozen shapes.**
    #[test]
    fn register_ci_tools_registers_all_four_into_the_one_surface() {
        let mut cat = Catalogue { defs: vec![] };
        let registered = register_ci_tools(&mut cat).expect("seeded defs always admit");
        assert_eq!(
            registered.len(),
            4,
            "deploy + approve_deploy + write_secret + run_pipeline"
        );

        let deploy = cat
            .resolve(&ToolName(DEPLOY_TOOL.into()))
            .expect("deploy registered");
        assert!(deploy.requires_approval, "the registered deploy is gated");
        assert_eq!(deploy.required_caps, vec!["environment.deploy".to_string()]);

        let pipeline = cat
            .resolve(&ToolName(RUN_PIPELINE_TOOL.into()))
            .expect("run_pipeline registered");
        assert!(
            !pipeline.requires_approval,
            "the registered run_pipeline is NOT gated"
        );

        assert!(cat.resolve(&ToolName("ci.delete_project".into())).is_none());
    }

    /// **The no-silent-loosening guard (VISION §3) protects the registration path.** A `ci.deploy` def
    /// hand-loosened to `requires_approval = false` WITHOUT a written deviation is REJECTED LOUD —
    /// proving the registration seam can't silently un-gate the protected-env deploy.
    #[test]
    fn a_hand_loosened_deploy_registration_is_rejected_loud() {
        let mut loosened = deploy_tool_def();
        loosened.requires_approval = false;
        let err = assert_no_silent_loosening(&loosened, &[]).unwrap_err();
        assert_eq!(err.subsystem, "ci");
        assert_eq!(err.tool, "deploy");
        assert!(
            err.to_string().contains("WITHOUT a written deviation"),
            "the loosening is surfaced LOUD: {err}"
        );
    }

    /// **The compounding-payoff / no-new-engine check (EI-03 §4 / EI-01 §7).** Every CI consumer tool
    /// is PURE data: a `mutate` `ToolDef` whose gating is the frozen §6.3 seed and whose caps are the
    /// frozen 4.9 fragment. The consequential split: exactly three CI tools are gated (deploy,
    /// approve_deploy, write_secret); only run_pipeline is not.
    #[test]
    fn the_ci_tools_are_a_projection_not_a_new_engine() {
        let defs = ci_tool_defs();
        assert_eq!(defs.len(), 4);
        for d in &defs {
            assert_eq!(d.effect_kind, EffectKind::Mutate);
            assert!(d.side_effecting);
            assert_eq!(
                d.requires_approval,
                requires_approval_default(&d.subsystem, &d.name.0),
                "{}.{} gating is the frozen §6.3 seed",
                d.subsystem,
                d.name.0
            );
        }
        let gated: Vec<&str> = defs
            .iter()
            .filter(|d| d.requires_approval)
            .map(|d| d.name.0.as_str())
            .collect();
        assert_eq!(
            gated,
            vec!["deploy", "approve_deploy", "write_secret"],
            "the three privileged CI gates; run_pipeline (non-prod) is not gated"
        );
    }
}
