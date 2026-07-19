//! # `events` — the complete `ci.*` event taxonomy, split durable vs firehose (CI / EB-27 / P-327)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md`
//! §1 (**the complete `ci.*` event taxonomy CI owns** — the v1 token list incl. the **frozen X-1
//! tokens** `ci.check.updated` + `ci.result`, and the Δ1 rename note that supersedes the legacy
//! `ci.status.updated` / `ci.run.passed`).
//!
//! **Contract-index rows (registered here — against the FROZEN Bus grammar / envelope):**
//! - **2.9** Event taxonomy + token table — `<subsystem>.<artifact_type>.<event_name>`. The Bus owns
//!   the **grammar + the seed** (`myelin_events::taxonomy`, EB-02 / P-042); **each subsystem
//!   completes its own list** (the contract-2.9 text). This module is CI COMPLETING its `ci.*` list
//!   for **M4** (EB-27 / P-327) — the M4 counterpart to Git/KN's M3 lists (EB-26). CI **registers**;
//!   it does **not** author the grammar — every token below is validated against the ONE Bus
//!   validator ([`myelin_events::validate_event_type`]); there is no second token language (EI-01 §7).
//! - **5.9** The Git↔CI CheckStatus seam — CI is the PRODUCER (`ci.check.updated` per
//!   `(commit_oid, context)` + the `ci.result` rollup the merge-queue waits on). The producer-leg
//!   *carriage* is the Bus's narrow half ([`myelin_events::check_seam`]); the `CheckStatus` shape is
//!   CI's (carried OPAQUE by the Bus). This module names the two frozen tokens; the producer's emit
//!   path attaches to them via the outbox.
//!
//! ## The durable / firehose split is STRUCTURAL (arch 03 §1)
//! - **DURABLE (via the OUTBOX)** — [`CI_DURABLE_TOKENS`]. The state-changing facts (`ci.run.*`,
//!   `ci.job.*`, `ci.check.updated`, `ci.result`, `ci.deployment.*`, `ci.pipeline.*`, the
//!   `*.erased` tombstones, the `*.snapshot` reindex events, the audit-critical
//!   `ci.supply_chain.verification_failed`). The ONLY emit path is the outbox (BUS-2 / contract 2.2).
//! - **FIREHOSE-ONLY (never the durable bus)** — [`CI_FIREHOSE_TOKENS`]. The high-volume log frames
//!   (`ci.log.appended`) ride the firehose + the resume-cursor protocol (contract 3.5); the durable
//!   bus carries only the COALESCED `ci.log.available` POINTER (which is durable). The `no-raw-publish`
//!   lint + the firehose seam keep the log frames off the durable bus STRUCTURALLY.
//!
//! ## FLOOR named (VISION §3 name-your-floors): the emit bodies attach in later CI prompts
//! These tokens are **registered** here (the names freeze) but **actually EMITTED only from the
//! OUTBOX** in the CI write-path prompts (the run/job/check producer + the merge-queue durable
//! workflow). This module is the M4 **names freeze** the Bus harness validates (contract 2.9) and the
//! producer leg's `ci.check.updated` / `ci.result` attach to via [`myelin_events::check_seam`].

use myelin_events::validate_event_type;

// ===========================================================================
// §1 — the frozen X-1 check-seam tokens (the producer leg, contract 5.9)
// ===========================================================================

/// **The frozen X-1 per-context check fact** (aggregate `(repo, commit_oid)`, subject
/// canonical commit root plus `#check-<context>`). Carries the CI-owned `CheckStatus` (small, PII-free,
/// references-not-payloads). Last-writer-wins by `run_attempt` on Git's side; the Bus carries it
/// per-aggregate ordered ([`myelin_events::check_seam`]). Pinned to the Bus's named seam token so
/// CI and the Bus agree by construction.
pub const CI_CHECK_UPDATED: &str = myelin_events::taxonomy::new_tokens::CI_CHECK_UPDATED;

/// **The frozen X-1 rollup signal** the merge-queue durable workflow waits on
/// (`wait_for_signal("ci.result", idem_key=<merge_attempt_id>)`, contract 9.4). Distinct from the
/// per-context `ci.check.updated` facts. Pinned to the Bus's named token.
pub const CI_RESULT: &str = myelin_events::taxonomy::new_tokens::CI_RESULT;

// ===========================================================================
// §1 — the run / job lifecycle (aggregate: ci/run/<run_id>)
// ===========================================================================

/// A run began (carries trust_tier, trigger_kind, the CAS snapshot ref).
pub const CI_RUN_STARTED: &str = "ci.run.started";
/// A run terminated successfully.
pub const CI_RUN_SUCCEEDED: &str = "ci.run.succeeded";
/// A run failed — carries STRUCTURED failure (which step/test, log excerpt) for agent triage.
pub const CI_RUN_FAILED: &str = "ci.run.failed";
/// A run was cancelled.
pub const CI_RUN_CANCELLED: &str = "ci.run.cancelled";
/// A run timed out.
pub const CI_RUN_TIMED_OUT: &str = "ci.run.timed_out";
/// A runner died mid-job and the run was re-queued/failed — honest, never silent.
pub const CI_RUN_REAPED: &str = "ci.run.reaped";

/// A job began (per-job; job ordering is within the run aggregate).
pub const CI_JOB_STARTED: &str = "ci.job.started";
/// A job succeeded.
pub const CI_JOB_SUCCEEDED: &str = "ci.job.succeeded";
/// A job failed.
pub const CI_JOB_FAILED: &str = "ci.job.failed";
/// A job was cancelled.
pub const CI_JOB_CANCELLED: &str = "ci.job.cancelled";

// ===========================================================================
// §1 — logs + artifacts + cost (aggregate: ci/run/<run_id>)
// ===========================================================================

/// **The ONLY log-related DURABLE event** — a coalesced POINTER ("lines N..M ready at `<ArtifactRef>`").
/// The log BYTES ride the firehose ([`CI_LOG_APPENDED`]); this pointer is durable.
pub const CI_LOG_AVAILABLE: &str = "ci.log.available";
/// A retained artifact (binary/SBOM/report/SCIP-LSIF) is available at an `ArtifactRef`.
pub const CI_ARTIFACT_PUBLISHED: &str = "ci.artifact.published";
/// One metered unit (resource-seconds) — Commercial/OLAP consume for usage rollups.
pub const CI_COST_METERED: &str = "ci.cost.metered";

// ===========================================================================
// §1 — deployment lifecycle (aggregate: ci/deployment/<dep_id>)
// ===========================================================================

/// A deployment was requested (the protected-env HITL flow opens).
pub const CI_DEPLOYMENT_REQUESTED: &str = "ci.deployment.requested";
/// A deployment needs approval (the HITL gate opens; OQ-F per-effect idem_key).
pub const CI_DEPLOYMENT_APPROVAL_REQUIRED: &str = "ci.deployment.approval_required";
/// A deployment was approved (the durable signal landing).
pub const CI_DEPLOYMENT_APPROVED: &str = "ci.deployment.approved";
/// A deployment was rejected.
pub const CI_DEPLOYMENT_REJECTED: &str = "ci.deployment.rejected";
/// A deployment started.
pub const CI_DEPLOYMENT_STARTED: &str = "ci.deployment.started";
/// A deployment succeeded.
pub const CI_DEPLOYMENT_SUCCEEDED: &str = "ci.deployment.succeeded";
/// A deployment failed.
pub const CI_DEPLOYMENT_FAILED: &str = "ci.deployment.failed";
/// A deployment was rolled back (first-class reversibility).
pub const CI_DEPLOYMENT_ROLLED_BACK: &str = "ci.deployment.rolled_back";

// ===========================================================================
// §1 — runner fleet health (aggregate: ci/runner/<runner_id>)
// ===========================================================================

/// A runner registered into the fleet.
pub const CI_RUNNER_REGISTERED: &str = "ci.runner.registered";
/// A runner attested (the self-hosted attestation surface).
pub const CI_RUNNER_ATTESTED: &str = "ci.runner.attested";
/// A runner degraded.
pub const CI_RUNNER_DEGRADED: &str = "ci.runner.degraded";
/// A runner went offline.
pub const CI_RUNNER_OFFLINE: &str = "ci.runner.offline";

// ===========================================================================
// §1 — pipeline config-as-code lifecycle (aggregate: ci/pipeline/<pipeline_id>)
// ===========================================================================

/// A pipeline config was created.
pub const CI_PIPELINE_CREATED: &str = "ci.pipeline.created";
/// A pipeline config was updated.
pub const CI_PIPELINE_UPDATED: &str = "ci.pipeline.updated";
/// A pipeline config was validated (a `plan` succeeded).
pub const CI_PIPELINE_VALIDATED: &str = "ci.pipeline.validated";

// ===========================================================================
// §1 — supply-chain audit (aggregate: ci/run/<run_id>)
// ===========================================================================

/// A floating-tag / unsigned-component / failed-signature was REFUSED (audit-critical fail-closed).
pub const CI_SUPPLY_CHAIN_VERIFICATION_FAILED: &str = "ci.supply_chain.verification_failed";

// ===========================================================================
// §1 — *.erased tombstones (Bus §6.3, contract 2.7)
// ===========================================================================

/// The run `*.erased` tombstone.
pub const CI_RUN_ERASED: &str = "ci.run.erased";
/// The deployment `*.erased` tombstone.
pub const CI_DEPLOYMENT_ERASED: &str = "ci.deployment.erased";
/// The runner `*.erased` tombstone.
pub const CI_RUNNER_ERASED: &str = "ci.runner.erased";

// ===========================================================================
// §1 — *.snapshot reindex-from-source events (contract 2.6; sub-artifact-granular)
// ===========================================================================

/// The run `*.snapshot` reindex event (`replay(scope=ci:run:<id>, since)`).
pub const CI_RUN_SNAPSHOT: &str = "ci.run.snapshot";
/// The deployment `*.snapshot` reindex event.
pub const CI_DEPLOYMENT_SNAPSHOT: &str = "ci.deployment.snapshot";
/// The pipeline `*.snapshot` reindex event.
pub const CI_PIPELINE_SNAPSHOT: &str = "ci.pipeline.snapshot";

// ===========================================================================
// §1.2 — the FIREHOSE-only ci.* tokens (NEVER the durable bus — contract 3.5)
// ===========================================================================

/// A log frame (lines appended). Rides the FIREHOSE + the resume-cursor protocol (contract 3.5),
/// NEVER the durable bus — the durable bus carries only the coalesced [`CI_LOG_AVAILABLE`] pointer.
pub const CI_LOG_APPENDED: &str = "ci.log.appended";

// ===========================================================================
// The token tables
// ===========================================================================

/// The complete DURABLE `ci.*` token set — the ONLY set that may ride the durable bus (via the
/// outbox, contract 2.2). Includes the two frozen X-1 seam tokens. (The Δ1-superseded
/// `ci.status.updated` / `ci.run.passed` are DELIBERATELY ABSENT — arch 03 §1 rename note.)
pub const CI_DURABLE_TOKENS: &[&str] = &[
    // the frozen X-1 check seam
    CI_CHECK_UPDATED,
    CI_RESULT,
    // run / job lifecycle
    CI_RUN_STARTED,
    CI_RUN_SUCCEEDED,
    CI_RUN_FAILED,
    CI_RUN_CANCELLED,
    CI_RUN_TIMED_OUT,
    CI_RUN_REAPED,
    CI_JOB_STARTED,
    CI_JOB_SUCCEEDED,
    CI_JOB_FAILED,
    CI_JOB_CANCELLED,
    // logs (pointer) / artifacts / cost
    CI_LOG_AVAILABLE,
    CI_ARTIFACT_PUBLISHED,
    CI_COST_METERED,
    // deployment
    CI_DEPLOYMENT_REQUESTED,
    CI_DEPLOYMENT_APPROVAL_REQUIRED,
    CI_DEPLOYMENT_APPROVED,
    CI_DEPLOYMENT_REJECTED,
    CI_DEPLOYMENT_STARTED,
    CI_DEPLOYMENT_SUCCEEDED,
    CI_DEPLOYMENT_FAILED,
    CI_DEPLOYMENT_ROLLED_BACK,
    // runner fleet
    CI_RUNNER_REGISTERED,
    CI_RUNNER_ATTESTED,
    CI_RUNNER_DEGRADED,
    CI_RUNNER_OFFLINE,
    // pipeline config-as-code
    CI_PIPELINE_CREATED,
    CI_PIPELINE_UPDATED,
    CI_PIPELINE_VALIDATED,
    // supply-chain audit
    CI_SUPPLY_CHAIN_VERIFICATION_FAILED,
    // *.erased tombstones (contract 2.7)
    CI_RUN_ERASED,
    CI_DEPLOYMENT_ERASED,
    CI_RUNNER_ERASED,
    // *.snapshot reindex (contract 2.6)
    CI_RUN_SNAPSHOT,
    CI_DEPLOYMENT_SNAPSHOT,
    CI_PIPELINE_SNAPSHOT,
];

/// The FIREHOSE-only `ci.*` token set — NEVER the durable bus (contract 3.5; over the frozen
/// resume-cursor protocol). The durable bus carries only the coalesced `ci.log.available` pointer.
pub const CI_FIREHOSE_TOKENS: &[&str] = &[CI_LOG_APPENDED];

/// The complete `ci.*` token registry = the DURABLE set ∪ the FIREHOSE set. The union the Bus
/// harness (contract 2.9) validates as CI's M4 completed list.
pub fn ci_event_tokens() -> impl Iterator<Item = &'static str> {
    CI_DURABLE_TOKENS
        .iter()
        .chain(CI_FIREHOSE_TOKENS.iter())
        .copied()
}

/// Register the complete `ci.*` list against the Bus grammar (contract 2.9). Returns `Ok(())` iff
/// **every** registered token parses the §6.1 grammar via the one Bus validator
/// ([`myelin_events::validate_event_type`]); otherwise the first offending token + its
/// [`myelin_events::TaxonomyError`] (LOUD, never silently coerced). CI REGISTERS its list against the
/// grammar it does not own.
pub fn register_ci_tokens() -> Result<(), (&'static str, myelin_events::TaxonomyError)> {
    for tok in ci_event_tokens() {
        validate_event_type(tok).map_err(|e| (tok, e))?;
    }
    Ok(())
}

/// Is `token` a DURABLE `ci.*` token (may ride the durable bus)? FIREHOSE tokens return `false`
/// (they ride the firehose only) — the structural durable/firehose split.
pub fn is_durable(token: &str) -> bool {
    CI_DURABLE_TOKENS.contains(&token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// **THE GATE (contract 2.9): 0 ungrammatical tokens.** Every registered `ci.*` token parses the
    /// Bus §6.1/§6.2 grammar via the one Bus validator — CI registers against the grammar it does not
    /// author. The parse is the green artifact.
    #[test]
    fn every_ci_token_parses_the_bus_grammar() {
        for tok in ci_event_tokens() {
            assert!(
                validate_event_type(tok).is_ok(),
                "registered ci token `{tok}` is UNGRAMMATICAL: {:?}",
                validate_event_type(tok)
            );
        }
        assert!(
            register_ci_tokens().is_ok(),
            "register_ci_tokens() must succeed: {:?}",
            register_ci_tokens()
        );
    }

    /// Every registered token carries the canonical `ci` subsystem prefix (§6.2).
    #[test]
    fn every_ci_token_carries_the_ci_subsystem_prefix() {
        for tok in ci_event_tokens() {
            let head = tok.split('.').next().expect("non-empty token");
            assert_eq!(
                head, "ci",
                "token `{tok}` must carry the `ci` subsystem prefix"
            );
        }
    }

    /// The durable + firehose sets are DISJOINT — a token is either durable or firehose, never both
    /// (the structural split that keeps log frames off the durable bus).
    #[test]
    fn durable_and_firehose_are_disjoint() {
        for f in CI_FIREHOSE_TOKENS {
            assert!(
                !CI_DURABLE_TOKENS.contains(f),
                "firehose token `{f}` must NOT be in the durable set"
            );
        }
    }

    /// No token appears twice in the union (each name is minted once).
    #[test]
    fn no_duplicate_tokens() {
        let mut seen = HashSet::new();
        for tok in ci_event_tokens() {
            assert!(seen.insert(tok), "token `{tok}` appears more than once");
        }
        assert_eq!(
            seen.len(),
            CI_DURABLE_TOKENS.len() + CI_FIREHOSE_TOKENS.len()
        );
    }

    /// The frozen X-1 seam tokens are present + are the Bus's named tokens (CI and the Bus agree by
    /// construction).
    #[test]
    fn the_frozen_x1_seam_tokens_are_registered() {
        assert!(CI_DURABLE_TOKENS.contains(&CI_CHECK_UPDATED));
        assert!(CI_DURABLE_TOKENS.contains(&CI_RESULT));
        assert_eq!(CI_CHECK_UPDATED, "ci.check.updated");
        assert_eq!(CI_RESULT, "ci.result");
    }

    /// The Δ1-superseded legacy tokens are DELIBERATELY ABSENT (arch 03 §1 rename note — the code
    /// emits `ci.check.updated` / `ci.result`, never the legacy `ci.status.updated` / `ci.run.passed`).
    #[test]
    fn the_superseded_legacy_tokens_are_absent() {
        for tok in ci_event_tokens() {
            assert_ne!(
                tok, "ci.status.updated",
                "ci.status.updated is superseded by ci.check.updated (Δ1)"
            );
            assert_ne!(
                tok, "ci.run.passed",
                "ci.run.passed is superseded by ci.check.updated (Δ1)"
            );
        }
    }

    /// The DURABLE classifier distinguishes the durable set from the firehose set.
    #[test]
    fn is_durable_distinguishes_the_split() {
        assert!(is_durable(CI_RUN_STARTED));
        assert!(is_durable(CI_LOG_AVAILABLE), "the log pointer is durable");
        assert!(
            !is_durable(CI_LOG_APPENDED),
            "the log frame is firehose-only, NOT durable"
        );
    }
}
