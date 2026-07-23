//! # `check_emitter` — the X-1 `check_attempt` counter + the `ci.check.updated` PRODUCER (CI-P18 → P-361, M4)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §4 (the Git↔CI check seam, PRODUCED — bump the `(commit_oid, context)` attempt counter, assemble
//! `CheckStatus`, emit `ci.check.updated` via the outbox; what CI does NOT do);
//! `01-tech-and-data-model.md` §3.2 (the `check_attempt` counter — CI's SOURCE of `run_attempt`,
//! monotonic, NEVER wall-clock); `03-events-contracts-and-glue.md` §4 (the `CheckStatus` seam, the
//! full shape, what CI owns vs what Git owns); `05-hard-problems.md` HP-0 (the seam frozen — the
//! poisoned-pipeline-execution defence).
//! **Reconciliation:** `00-reconciliation-decisions.md` X-1 / OQ-A (the FROZEN `CheckStatus` struct,
//! the monotonic-`run_attempt` supersession, the fork-trust-tier gating).
//! **Contracts:** OWNED 5.9 (the Git↔CI `CheckStatus` seam — CI is the PRODUCER; `ci.check.updated` +
//! the `run_attempt` source). CONSUMED 2.9 (the token), 2.2 (outbox-only emit), 5.7 (the
//! `#step-<n>` `details_ref` sub-anchor), 7.3 (the `summary` as a HumanisedRef — NEVER a raw string).
//!
//! ## What CI-P18 ships — the MONOTONIC COUNTER + the FROZEN-SHAPE producer (the check-fact half)
//!
//! Two things the M4 producer needs, both proven monotonic / frozen-shape here:
//!
//! 1. **The `check_attempt` monotonic counter** ([`CheckAttemptCounter`] + [`BUMP_CHECK_ATTEMPT_SQL`]).
//!    On a new run / re-run for `(commit_oid, context)`, the counter bumps:
//!    `UPDATE check_attempt SET next_attempt = next_attempt + 1 RETURNING (next_attempt - 1)` — the
//!    returned attempt is stamped into the emitted `CheckStatus.run_attempt`. CI is the SOURCE of
//!    `run_attempt`: monotonic, NEVER wall-clock (clocks are not authority — X-1). Git's
//!    last-writer-wins is on the attempt; a LOWER attempt is the stale one (droppable). The in-memory
//!    [`CheckAttemptCounter`] is the deterministic model the unit tests prove monotonicity on; the
//!    LIVE bump rides the [`BUMP_CHECK_ATTEMPT_SQL`] UPSERT against the `check_attempt` table (arch 01
//!    §3.2), proven against the dev Postgres in the integration drill.
//!
//! 2. **The FROZEN 5.9 `CheckStatus` assembly** ([`assemble_check_status`] →
//!    [`check_status_payload`]). CI assembles the frozen struct EXACTLY (the shape Git's
//!    `myelin_git::check_status::CheckStatus` decodes off the OPAQUE `ci.check.updated` payload — CI
//!    never depends on Git, but it produces the byte-identical JSON Git's `serde_json::from_value`
//!    accepts):
//!    `{ tenant, repo, commit_oid, context: {provider, name}, state, required, run, run_attempt,
//!    trust_tier, details_ref: "#step-<n>", summary: (template_key, args), started_at, completed_at?,
//!    cost_settled }`. Key invariants:
//!    - the **`summary` is a HumanisedRef** `(template_key, args)`, NEVER a raw string (7.3 / NOTIF-1)
//!      — so the PR-checks panel renders a backend-humanised string, never a CI-supplied
//!      `"build failed"` ([`summary_for`]);
//!    - **`cost_settled` flips true ONLY when the reserve settles** — a check is NOT "final" until
//!      settled (X-1). A terminal-but-unsettled fact carries `cost_settled: false`; the
//!      terminal-SETTLED fact (after CI-P17's reserve/settle bookend closes) carries
//!      `cost_settled: true` ([`CostPosture`]);
//!    - the **`trust_tier` is STAMPED FROM PROVENANCE** at trigger time (CI-P10), read off the run,
//!      NEVER recomputed here — a fork run is recorded faithfully with `trust_tier =
//!      untrusted_fork`, but CI NEVER endorses it (the poisoned-pipeline defence; Git gates).
//!
//! ## What CI does NOT do (arch §4 / X-1)
//! CI never owns the `check_status` projection table, never decides which contexts are `required`
//! (`required` here is CI's REPORT/echo — Git's branch-protection policy is authoritative), never
//! recomputes trust, never endorses a fork, never merges. CI reports facts; Git gates.
//!
//! ## references-not-payloads (2.2 outbox-only)
//! Every fact rides the FROZEN [`myelin_events::check_seam::check_updated_draft`] (the canonical
//! envelope: `subject = repo#commit-<oid>/check-<context>`, `aggregate = (repo, commit_oid)`) so the
//! grammar is byte-identical to what Git's gate consumes (0 drift). `run` / `details_ref` are
//! `ArtifactRef`s (the producing run + the jump-to-failure sub-anchor), NEVER log bytes. Emitted via
//! the OUTBOX ONLY (the `no-raw-publish` lint green); the producer plumbing onto `ctx.emit` is in
//! [`crate::ci_pipeline`] (the `ci.pipeline` body), which now assembles its terminal facts THROUGH
//! this module (one producer shape, no divergence — EI-01 §7 reconcile-in-place).
//!
//! ## NAMED FLOORS (recorded here, filled later)
//! - **The `ci.result` rollup signal end-to-end + the GIT-D10 / CI-D8 seam GATE (0 double-merge)** is
//!   **CI-P19** (P-362). This module ships the per-context check-FACT half (the counter + the frozen
//!   assembly); the rollup-to-merge-queue end-to-end seam gate is the next prompt.
//! - **External-provider checks** via `CheckStatus { context.provider: external }` are a
//!   demand-driven follow-on — [`CheckProvider::External`] is in the closed shape (Git decodes it),
//!   but CI only PRODUCES `provider: ci` facts today (the external-CI-integration ingest is the
//!   demand follow-on the prompt names).

use myelin_events::check_seam::check_updated_draft;
use myelin_events::EventDraft;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// 1. The check_attempt monotonic counter (arch 01 §3.2 — CI's source of run_attempt)
// ---------------------------------------------------------------------------

/// **The live `check_attempt` bump (arch 01 §3.2).** On a new run / re-run for
/// `(tenant, repo, commit_oid, context)`: UPSERT the row, bump `next_attempt`, and RETURN the attempt
/// to STAMP into the emitted `CheckStatus.run_attempt`. The returned `run_attempt` is
/// `next_attempt - 1` AFTER the bump — i.e. the FIRST dispatch returns `1` (the row inserts with
/// `next_attempt = 2` and returns `2 - 1 = 1`), the first distinct RE-dispatch returns `2`, and so
/// on. An exact retry for the same `current_run` returns its already-issued attempt without
/// incrementing, so a starter transaction can be retried without superseding itself. CI is the
/// SOURCE of `run_attempt`: monotonic, never wall-clock.
///
/// The `current_run` is set to the run that most recently produced this context's status (the
/// supersession provenance). The bump is one atomic statement so two concurrent re-dispatches of the
/// same context can never collide on the same attempt (the `(tenant, repo_ref, commit_oid, context)`
/// PK + the `RETURNING` make it serialisable).
pub const BUMP_CHECK_ATTEMPT_SQL: &str = "\
INSERT INTO check_attempt (tenant_id, region, repo_ref, commit_oid, context, next_attempt, current_run)
VALUES ($1, $2, $3, $4, $5, 2, $6)
ON CONFLICT (tenant_id, repo_ref, commit_oid, context)
DO UPDATE SET
  next_attempt = CASE
    WHEN check_attempt.current_run IS NOT DISTINCT FROM EXCLUDED.current_run
      THEN check_attempt.next_attempt
    ELSE check_attempt.next_attempt + 1
  END,
  current_run = EXCLUDED.current_run
RETURNING next_attempt - 1 AS run_attempt";

/// **The in-memory `check_attempt` monotonic counter model (arch 01 §3.2).** CI's deterministic
/// SOURCE of `run_attempt` per `(commit_oid, context)` — the model the unit tests use to prove the
/// cross-run monotonicity of [`BUMP_CHECK_ATTEMPT_SQL`]: a new context's FIRST dispatch returns
/// attempt `1` and each subsequent run returns the next integer. The SQL layer additionally makes
/// an exact retry of one durable run idempotent. The sequence is strictly increasing across runs and
/// NEVER reads a clock.
///
/// This is NOT the live table (that is [`crate::migrations::CREATE_CHECK_ATTEMPT_DDL`], bumped via the
/// SQL above against the dev Postgres). It is the in-process attempt-source the producer reads to
/// STAMP a fact, and the model the supersession-correctness drill exercises: a higher attempt
/// SUPERSEDES; a lower attempt is STALE (the at-least-once transport makes the drop mandatory; Git
/// drops it because CI's `run_attempt` order is well-defined).
#[derive(Debug, Default, Clone)]
pub struct CheckAttemptCounter {
    /// `(commit_oid, context) → the highest attempt issued so far`. A `BTreeMap` so the state is
    /// deterministic/ordered (no clock, no RNG — the CI-D9 determinism posture).
    issued: BTreeMap<(String, String), u32>,
}

impl CheckAttemptCounter {
    /// A fresh counter with no contexts issued.
    pub fn new() -> CheckAttemptCounter {
        CheckAttemptCounter::default()
    }

    /// **Bump the attempt for `(commit_oid, context)` and RETURN the stamped `run_attempt`** — the
    /// in-memory model of [`BUMP_CHECK_ATTEMPT_SQL`]'s cross-run behavior. The FIRST dispatch of a
    /// context returns `1`; each re-dispatch returns the next integer (strictly increasing). The SQL
    /// layer binds an attempt to `current_run` to make an exact transaction retry idempotent. The
    /// returned value is the `CheckStatus.run_attempt` the producer stamps — the ONLY supersession
    /// key (never wall-clock).
    pub fn bump(&mut self, commit_oid: &str, context: &str) -> u32 {
        let key = (commit_oid.to_string(), context.to_string());
        let slot = self.issued.entry(key).or_insert(0);
        *slot += 1;
        *slot
    }

    /// The highest attempt issued for `(commit_oid, context)` so far (`0` if never issued). The
    /// supersession high-water mark CI has produced — a fact carrying a LOWER attempt than this is
    /// STALE (a re-delivery the gate drops).
    pub fn current(&self, commit_oid: &str, context: &str) -> u32 {
        self.issued
            .get(&(commit_oid.to_string(), context.to_string()))
            .copied()
            .unwrap_or(0)
    }

    /// **Is `incoming` a STALE attempt for `(commit_oid, context)`?** A `true` means the incoming
    /// attempt is LOWER than the highest issued — the supersession rule (Git's) drops it. This is the
    /// CI-side statement of the rule CI's counter MAKES well-defined: monotonic `run_attempt` is the
    /// only supersession key, so a lower attempt is ALWAYS the stale one (clocks are not authority).
    pub fn is_stale(&self, commit_oid: &str, context: &str, incoming: u32) -> bool {
        incoming < self.current(commit_oid, context)
    }
}

// ---------------------------------------------------------------------------
// 2. The frozen 5.9 CheckStatus assembly (the producer side — arch §4 / X-1)
// ---------------------------------------------------------------------------

/// The check producer class (frozen 5.9 `CheckContext.provider`): `ci` (a Myelin CI run) or
/// `external` (a third-party status surfaced as a check). CI PRODUCES `Ci` facts today; `External` is
/// in the closed shape (Git decodes it) for the demand-driven external-CI follow-on the prompt names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckProvider {
    /// A check produced by a Myelin CI run (the only kind CI emits today).
    Ci,
    /// An external/third-party status (e.g. sonarcloud) — the named demand follow-on.
    External,
}

impl CheckProvider {
    /// The `snake_case` token Git's `CheckProvider` decodes (`ci` / `external`).
    pub fn token(self) -> &'static str {
        match self {
            CheckProvider::Ci => "ci",
            CheckProvider::External => "external",
        }
    }
}

/// The check lifecycle state (frozen 5.9): the closed set
/// `queued | in_progress | success | failure | error | neutral | cancelled`. Serialised as the
/// `snake_case` token Git's `CheckState` decodes. Only `success` (with an acceptable trust posture)
/// can satisfy a `required` context — but THAT is Git's gate decision, not CI's; CI only reports the
/// state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckState {
    /// Queued (not yet running) — the gate treats it as pending.
    Queued,
    /// Running — pending.
    InProgress,
    /// Passed.
    Success,
    /// Failed (a test/build failure).
    Failure,
    /// Errored (infra/runner fault, distinct from a clean failure).
    Error,
    /// Explicitly neutral — recorded, never gating.
    Neutral,
    /// Cancelled.
    Cancelled,
}

impl CheckState {
    /// The `snake_case` token Git's `CheckState` decodes.
    pub fn token(self) -> &'static str {
        match self {
            CheckState::Queued => "queued",
            CheckState::InProgress => "in_progress",
            CheckState::Success => "success",
            CheckState::Failure => "failure",
            CheckState::Error => "error",
            CheckState::Neutral => "neutral",
            CheckState::Cancelled => "cancelled",
        }
    }

    /// Is this a terminal state (the check reached a verdict)? `queued`/`in_progress` are not. A
    /// terminal fact carries a `completed_at`; a pending one does not.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            CheckState::Success
                | CheckState::Failure
                | CheckState::Error
                | CheckState::Neutral
                | CheckState::Cancelled
        )
    }
}

/// The trust tier (frozen 5.9): `trusted | untrusted_fork`. **Stamped by CI from the run's
/// PROVENANCE at trigger time (CI-P10), read off the run, NEVER recomputed here** (X-1). CI records a
/// fork run faithfully with `UntrustedFork` but NEVER endorses it — Git treats an `untrusted_fork`
/// success as neutral-for-gating until a maintainer endorses or the context is re-run trusted (the
/// poisoned-pipeline-execution defence). CI stamps the tier from provenance only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustTier {
    /// The run executed trusted code (a non-fork PR, or an endorsed/re-run-trusted fork run).
    Trusted,
    /// The run executed untrusted contributor code (a fork PR) — CI records it faithfully but never
    /// endorses it; Git gates it neutral until endorsed/re-run-trusted.
    UntrustedFork,
}

impl TrustTier {
    /// The `snake_case` token Git's `TrustTier` decodes.
    pub fn token(self) -> &'static str {
        match self {
            TrustTier::Trusted => "trusted",
            TrustTier::UntrustedFork => "untrusted_fork",
        }
    }

    /// Parse the trust-tier token STAMPED at trigger time (read off the run's CI-P10 stamp). An
    /// unrecognised tier is conservatively treated as `UntrustedFork` (fail-closed — CI never
    /// upgrades an unknown provenance to trusted; the poisoned-pipeline defence).
    pub fn from_stamp(stamp: &str) -> TrustTier {
        match stamp {
            "trusted" => TrustTier::Trusted,
            // Anything that is not the explicit `trusted` stamp is treated as untrusted (fail-closed).
            _ => TrustTier::UntrustedFork,
        }
    }
}

/// **The reserve/settle cost posture of a check (X-1 — `cost_settled`).** A check is NOT "final"
/// until its run's reserve/settle bookend CLOSES (CI-P17). A terminal-but-unsettled fact carries
/// `Unsettled` (`cost_settled: false`); the terminal-SETTLED fact (emitted after `settle_budget()`
/// closes on `job.done`) carries `Settled` (`cost_settled: true`). The merge gate may treat a
/// not-yet-settled check as still-in-progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CostPosture {
    /// The reserve has NOT yet settled — `cost_settled: false`. A terminal verdict that is not yet
    /// "final" (the X-1 cost gate).
    Unsettled,
    /// The reserve has settled — `cost_settled: true`. The check is "final".
    Settled,
}

impl CostPosture {
    /// `true` iff settled (the `cost_settled` field value).
    pub fn is_settled(self) -> bool {
        matches!(self, CostPosture::Settled)
    }
}

/// **The X-1 producer emit context for ONE check fact (contract 5.9 / arch §4).** The provenance the
/// assembled `CheckStatus` carries: the tenant + repo + commit the check keys on, the producing run
/// ref, the monotonic `run_attempt` (from [`CheckAttemptCounter::bump`]), the `trust_tier` STAMPED at
/// trigger time (read off the run, NEVER recomputed), and the wall-clock window (NOT the supersession
/// authority). All PII-free (references-not-payloads).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckEmitContext {
    /// The tenant the projection row is scoped to (the partition key, EI-02 §1).
    pub tenant: String,
    /// The repo the check ran in (`myelin://<tenant>/git/repo/<id>`) — the seam key half.
    pub repo: String,
    /// The content-addressed commit the check ran against — the other key half of `(commit_oid,
    /// context)`.
    pub commit_oid: String,
    /// The producing CI run ref (`myelin://<tenant>/ci/run/<id>`) — the supersession provenance +
    /// the `details_ref` anchor.
    pub run_ref: String,
    /// The monotonic `run_attempt` from [`CheckAttemptCounter::bump`] — Git's last-writer-wins key
    /// (NEVER wall-clock). The SAME value is stamped onto every context's fact for one run.
    pub run_attempt: u32,
    /// The `trust_tier` STAMPED at trigger time (read off the run's CI-P10 stamp, NEVER recomputed).
    pub trust_tier: TrustTier,
    /// When the run started (RFC3339). NOT the supersession authority — a display column only.
    pub started_at: String,
    /// When the run completed (RFC3339), if terminal. NOT the supersession authority. A pending
    /// (queued/in_progress) fact has `None`.
    pub completed_at: Option<String>,
}

/// **Build the `summary` HumanisedRef `(template_key, args)` for a check state (contract 7.3 /
/// NOTIF-1).** The `CheckStatus.summary` is a HumanisedRef, NEVER a raw string — so the PR-checks
/// panel renders a backend-humanised string, never a CI-supplied `"build failed"`. The template key
/// is keyed on the state (`ci.check.success` / `ci.check.failure` / …); the args carry the PII-free
/// context name the template fills. Notif resolves the key at render (the producer never renders).
pub fn summary_for(state: CheckState, context: &str) -> (String, BTreeMap<String, String>) {
    let template_key = match state {
        CheckState::Queued => "ci.check.queued",
        CheckState::InProgress => "ci.check.in_progress",
        CheckState::Success => "ci.check.success",
        CheckState::Failure => "ci.check.failure",
        CheckState::Error => "ci.check.error",
        CheckState::Neutral => "ci.check.neutral",
        CheckState::Cancelled => "ci.check.cancelled",
    };
    let mut args = BTreeMap::new();
    args.insert("context".to_string(), context.to_string());
    (template_key.to_string(), args)
}

/// **The jump-to-failure `details_ref` sub-anchor (`#step-<n>`, OQ-D / contract 5.7).** Git renders
/// it as a link into CI's run view — it NEVER reads CI's DB to resolve it. A failure anchors on the
/// failing step; otherwise it anchors on the run root. References-not-payloads (a ref, never bytes).
pub fn details_ref(run_ref: &str, state: CheckState, fail_step: Option<u32>) -> String {
    match (state, fail_step) {
        // A failure anchors on the failing step (the firehose-log `#step-<n>` jump-to-failure, the
        // CI-P21 log-index resolution target).
        (CheckState::Failure, Some(n)) | (CheckState::Error, Some(n)) => {
            format!("{run_ref}#step-{n}")
        }
        // The frozen ArtifactRef grammar has a numeric `#step-<n>` CI sub-anchor. When no exact
        // failing step exists, the canonical run root is the only honest target; `#step-failure`
        // and `#summary` are not ArtifactRefs and must never enter Git's durable projection.
        _ => run_ref.to_string(),
    }
}

/// **Assemble the FROZEN 5.9 `CheckStatus` payload value (arch §4 / X-1 — the producer side).** Build
/// the byte-identical JSON Git's `myelin_git::check_status::CheckStatus` decodes off the OPAQUE
/// `ci.check.updated` payload. CI never depends on Git — it PRODUCES the frozen shape exactly; the
/// CDC pair (CI provider here + Git consumer) is the seam's contract test. A PURE function of its
/// inputs (no clock/RNG/IO), so a replay re-builds a BYTE-IDENTICAL payload (the CI-D9 property).
///
/// `required` is CI's REPORT/echo — Git's branch-protection policy is the authority on which contexts
/// gate (CI reports, Git decides). `cost` is the [`CostPosture`] — `cost_settled` flips true ONLY on
/// settle. `provider` is `Ci` for a Myelin run (the only kind CI emits today). `fail_step` is the
/// failing step index for the `#step-<n>` jump-to-failure (resolved through CI-P21's log index).
#[allow(clippy::too_many_arguments)]
pub fn check_status_payload(
    ctx: &CheckEmitContext,
    provider: CheckProvider,
    context: &str,
    state: CheckState,
    required: bool,
    cost: CostPosture,
    fail_step: Option<u32>,
) -> serde_json::Value {
    let (template_key, args) = summary_for(state, context);
    serde_json::json!({
        // The partition key — every projection row is tenant-scoped (EI-02 §1).
        "tenant": ctx.tenant,
        // The seam key half + the producing run (references-not-payloads).
        "repo": ctx.repo,
        "commit_oid": ctx.commit_oid,
        // The CheckContext — `{provider, name}` (the other key half).
        "context": { "provider": provider.token(), "name": context },
        "state": state.token(),
        // CI's REPORT/echo of required — Git's policy is authoritative (CI reports, Git decides).
        "required": required,
        "run": ctx.run_ref,
        // The monotonic supersession key (NEVER wall-clock).
        "run_attempt": ctx.run_attempt,
        // STAMPED at trigger time, read off the run, NEVER recomputed here (X-1).
        "trust_tier": ctx.trust_tier.token(),
        // The `#step-<n>` jump-to-failure (resolved through CI-P21's log index).
        "details_ref": details_ref(&ctx.run_ref, state, fail_step),
        // The HumanisedRef summary — `(template_key, args)`, NEVER a raw string (7.3 / NOTIF-1).
        "summary": { "template_key": template_key, "args": args },
        // The display window (NOT the supersession authority).
        "started_at": ctx.started_at,
        "completed_at": ctx.completed_at,
        // The X-1 cost gate: `cost_settled` flips true ONLY when the reserve settles (CI-P17). A
        // check is NOT "final" until settled.
        "cost_settled": cost.is_settled(),
    })
}

/// **Assemble the canonical `ci.check.updated` [`EventDraft`] for ONE context (arch §4 / 2.2).**
/// REUSES the FROZEN [`check_updated_draft`] so the `subject = repo#commit-<oid>/check-<context>` +
/// `aggregate = (repo, commit_oid)` grammar is byte-identical to what Git's gate consumes (0 drift,
/// EI-01 §7 reconcile-in-place). The payload is the frozen 5.9 `CheckStatus` shape from
/// [`check_status_payload`]. The producer emits this via the OUTBOX ONLY (`ctx.emit`, in
/// [`crate::ci_pipeline`]); references-not-payloads (never log bytes).
#[allow(clippy::too_many_arguments)]
pub fn assemble_check_status(
    ctx: &CheckEmitContext,
    provider: CheckProvider,
    context: &str,
    state: CheckState,
    required: bool,
    cost: CostPosture,
    fail_step: Option<u32>,
) -> EventDraft {
    let payload = check_status_payload(ctx, provider, context, state, required, cost, fail_step);
    check_updated_draft(&ctx.repo, &ctx.commit_oid, context, payload)
}

#[cfg(test)]
#[path = "check_emitter_tests.rs"]
mod tests;
