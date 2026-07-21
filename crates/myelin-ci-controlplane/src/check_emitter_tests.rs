//! Unit tests for the X-1 `check_attempt` counter + the `ci.check.updated` producer (CI-P18 → P-361).
//!
//! Two GATE properties (the prompt's quantified must-be-green):
//! 1. **the `check_attempt` counter is monotonic** — a re-run bumps the attempt; a lower attempt is
//!    the STALE one (CI never uses wall-clock for supersession; the `run_attempt` is the only key);
//! 2. **`ci.check.updated` is well-formed** — the `summary` is a HumanisedRef (NEVER a raw string);
//!    `cost_settled` flips ONLY on settle; the envelope rides the frozen `check_updated_draft` (the
//!    no-raw-publish path — emitted via the outbox only).
//!
//! Plus the CDC provider assertion (the check-fact half of row 5.9): the assembled payload carries
//! EVERY frozen 5.9 field so Git's `serde_json::from_value::<CheckStatus>` decodes it (CI never
//! depends on Git — it produces the byte-identical frozen shape; the field-by-field assertion is the
//! provider stub the Git consumer half pairs with in CI-P19's end-to-end gate).

use super::*;

fn emit_ctx(attempt: u32, completed: Option<&str>) -> CheckEmitContext {
    CheckEmitContext {
        tenant: "acme".into(),
        repo: "myelin://acme/git/repo/core".into(),
        commit_oid: "deadbeef".into(),
        run_ref: "myelin://acme/ci/run/run-7".into(),
        run_attempt: attempt,
        trust_tier: TrustTier::Trusted,
        started_at: "2026-06-23T00:00:00Z".into(),
        completed_at: completed.map(|s| s.to_string()),
    }
}

// ── 1. The check_attempt monotonic counter ────────────────────────────────

/// **The counter is MONOTONIC: the first dispatch is attempt 1, each re-run bumps it.** CI is the
/// SOURCE of `run_attempt` — the sequence is strictly increasing, never a clock.
#[test]
fn check_attempt_counter_is_monotonic_per_context() {
    let mut counter = CheckAttemptCounter::new();

    // The FIRST dispatch of `build` for this commit → attempt 1.
    assert_eq!(counter.bump("deadbeef", "build"), 1, "first dispatch = 1");
    // A RE-RUN of `build` bumps → attempt 2, then 3 …
    assert_eq!(counter.bump("deadbeef", "build"), 2, "a re-run bumps");
    assert_eq!(counter.bump("deadbeef", "build"), 3, "strictly increasing");

    // A DIFFERENT context (`test`) has its OWN sequence — independent of `build`.
    assert_eq!(counter.bump("deadbeef", "test"), 1, "per-context sequence");
    assert_eq!(counter.bump("deadbeef", "test"), 2);

    // A DIFFERENT commit has its OWN sequence too (the key is `(commit_oid, context)`).
    assert_eq!(counter.bump("cafef00d", "build"), 1, "per-commit sequence");

    // The high-water marks reflect the highest issued per key.
    assert_eq!(counter.current("deadbeef", "build"), 3);
    assert_eq!(counter.current("deadbeef", "test"), 2);
    assert_eq!(counter.current("cafef00d", "build"), 1);
    assert_eq!(
        counter.current("deadbeef", "lint"),
        0,
        "an un-issued context is 0"
    );
}

/// **A LOWER attempt is the STALE one (the supersession key is `run_attempt`, never wall-clock).** A
/// re-delivered lower attempt is droppable; the current (highest) attempt supersedes — the rule CI's
/// monotonic counter makes well-defined (clocks are not authority — X-1).
#[test]
fn a_lower_attempt_is_stale_the_higher_supersedes() {
    let mut counter = CheckAttemptCounter::new();
    let a1 = counter.bump("deadbeef", "build"); // attempt 1
    let a2 = counter.bump("deadbeef", "build"); // attempt 2 (a re-run)
    assert_eq!((a1, a2), (1, 2));

    // The CURRENT (highest) attempt is 2 — attempt 1 is STALE (a re-delivery the gate drops).
    assert!(
        counter.is_stale("deadbeef", "build", 1),
        "attempt 1 < current 2 → stale"
    );
    assert!(
        !counter.is_stale("deadbeef", "build", 2),
        "attempt 2 == current → not stale (it IS the current)"
    );
    // A hypothetical FUTURE attempt is never stale (a higher attempt always supersedes).
    assert!(!counter.is_stale("deadbeef", "build", 3));
}

/// **The live bump SQL is the monotonic UPSERT (arch 01 §3.2).** The `RETURNING next_attempt - 1`
/// stamps the attempt; the `ON CONFLICT` branch makes a distinct run monotonic while an exact retry
/// reuses its issued value. Pin the shape so either guarantee cannot disappear silently.
#[test]
fn bump_check_attempt_sql_is_the_monotonic_upsert() {
    let sql = BUMP_CHECK_ATTEMPT_SQL;
    assert!(
        sql.contains("INSERT INTO check_attempt"),
        "upserts the attempt counter table"
    );
    assert!(
        sql.contains("ELSE check_attempt.next_attempt + 1"),
        "a re-dispatch bumps monotonically (+1) — never wall-clock"
    );
    assert!(
        sql.contains("check_attempt.current_run IS NOT DISTINCT FROM EXCLUDED.current_run"),
        "an exact retry of one durable run must not supersede itself"
    );
    assert!(
        sql.contains("RETURNING next_attempt - 1"),
        "returns the attempt to STAMP into CheckStatus.run_attempt"
    );
    assert!(
        sql.contains("ON CONFLICT (tenant_id, repo_ref, commit_oid, context)"),
        "the (commit_oid, context) key + tenant scope — one attempt source per key"
    );
}

// ── 2. The frozen 5.9 CheckStatus assembly (the producer side) ─────────────

/// **The assembled payload carries EVERY frozen 5.9 field (the CDC provider half of row 5.9).** Git's
/// `serde_json::from_value::<CheckStatus>` requires `tenant`/`repo`/`commit_oid`/`context`/`state`/
/// `required`/`run`/`run_attempt`/`trust_tier`/`details_ref`/`summary`/`started_at`/`completed_at`/
/// `cost_settled`. CI produces the byte-identical shape (it never depends on Git).
#[test]
fn assembled_payload_carries_every_frozen_5_9_field() {
    let ctx = emit_ctx(2, Some("2026-06-23T00:01:00Z"));
    let p = check_status_payload(
        &ctx,
        CheckProvider::Ci,
        "build",
        CheckState::Success,
        true,
        CostPosture::Settled,
        None,
    );

    assert_eq!(p["tenant"], "acme");
    assert_eq!(p["repo"], "myelin://acme/git/repo/core");
    assert_eq!(p["commit_oid"], "deadbeef");
    assert_eq!(
        p["context"],
        serde_json::json!({ "provider": "ci", "name": "build" }),
        "the CheckContext is {{provider, name}} — the key half"
    );
    assert_eq!(p["state"], "success");
    assert_eq!(p["required"], true, "CI reports required; Git decides");
    assert_eq!(p["run"], "myelin://acme/ci/run/run-7");
    assert_eq!(p["run_attempt"], 2, "the monotonic supersession key");
    assert_eq!(p["trust_tier"], "trusted");
    assert_eq!(
        p["details_ref"], "myelin://acme/ci/run/run-7#summary",
        "a success anchors on the run summary"
    );
    assert_eq!(p["started_at"], "2026-06-23T00:00:00Z");
    assert_eq!(p["completed_at"], "2026-06-23T00:01:00Z");
    assert_eq!(
        p["cost_settled"], true,
        "settled → cost_settled true (the check is final)"
    );
}

/// **The `summary` is a HumanisedRef `(template_key, args)`, NEVER a raw string (7.3 / NOTIF-1).** The
/// PR-checks panel renders a backend-humanised string — CI never supplies a raw `"build failed"`.
#[test]
fn summary_is_a_humanised_ref_never_a_raw_string() {
    let ctx = emit_ctx(1, Some("2026-06-23T00:01:00Z"));
    let p = check_status_payload(
        &ctx,
        CheckProvider::Ci,
        "build",
        CheckState::Failure,
        true,
        CostPosture::Settled,
        Some(3),
    );

    // The summary is an OBJECT `{template_key, args}`, never a bare string.
    assert!(
        p["summary"].is_object(),
        "summary is a (template_key, args) ref, not a raw string"
    );
    assert_eq!(
        p["summary"]["template_key"], "ci.check.failure",
        "the template key is keyed on the state"
    );
    assert_eq!(
        p["summary"]["args"]["context"], "build",
        "the args carry the PII-free context name the template fills"
    );
    // The state→template-key map is total (every state has a key).
    for (state, key) in [
        (CheckState::Queued, "ci.check.queued"),
        (CheckState::InProgress, "ci.check.in_progress"),
        (CheckState::Success, "ci.check.success"),
        (CheckState::Failure, "ci.check.failure"),
        (CheckState::Error, "ci.check.error"),
        (CheckState::Neutral, "ci.check.neutral"),
        (CheckState::Cancelled, "ci.check.cancelled"),
    ] {
        let (k, _) = summary_for(state, "build");
        assert_eq!(k, key, "{state:?} → {key}");
    }
}

/// **`cost_settled` flips true ONLY on settle — a check is NOT "final" until settled (X-1).** A
/// terminal-but-unsettled fact carries `cost_settled: false`; the terminal-SETTLED fact carries
/// `cost_settled: true`. The state can be terminal `success` while the cost is still unsettled.
#[test]
fn cost_settled_flips_only_on_settle() {
    let ctx = emit_ctx(1, Some("2026-06-23T00:01:00Z"));

    // Terminal success but the reserve has NOT settled → cost_settled false (not yet "final").
    let unsettled = check_status_payload(
        &ctx,
        CheckProvider::Ci,
        "build",
        CheckState::Success,
        true,
        CostPosture::Unsettled,
        None,
    );
    assert_eq!(unsettled["state"], "success", "terminal verdict");
    assert_eq!(
        unsettled["cost_settled"], false,
        "terminal but NOT settled until the reserve/settle bookend closes (X-1)"
    );

    // The SAME context, after the reserve settles → cost_settled true (the check is final).
    let settled = check_status_payload(
        &ctx,
        CheckProvider::Ci,
        "build",
        CheckState::Success,
        true,
        CostPosture::Settled,
        None,
    );
    assert_eq!(
        settled["cost_settled"], true,
        "settled → cost_settled true (final)"
    );
    assert!(
        CostPosture::Settled.is_settled() && !CostPosture::Unsettled.is_settled(),
        "the cost posture maps to the bool"
    );
}

/// **CI stamps `trust_tier` FROM PROVENANCE, never recomputes — a fork run is recorded faithfully but
/// NEVER endorsed (the poisoned-pipeline defence).** A success WITH `untrusted_fork` carries the tier
/// faithfully; CI never upgrades it to trusted. The `from_stamp` is fail-closed (unknown → untrusted).
#[test]
fn trust_tier_is_stamped_from_provenance_fork_never_endorsed() {
    let mut ctx = emit_ctx(1, Some("2026-06-23T00:01:00Z"));
    ctx.trust_tier = TrustTier::UntrustedFork;

    // A FORK success is recorded faithfully with trust_tier = untrusted_fork — CI never endorses it.
    let p = check_status_payload(
        &ctx,
        CheckProvider::Ci,
        "build",
        CheckState::Success,
        true,
        CostPosture::Settled,
        None,
    );
    assert_eq!(p["state"], "success", "the fork's success is recorded");
    assert_eq!(
        p["trust_tier"], "untrusted_fork",
        "the tier is stamped from provenance — CI never recomputes or endorses (X-1)"
    );

    // The stamp parse is fail-closed: a non-`trusted` stamp is ALWAYS untrusted (CI never upgrades).
    assert_eq!(TrustTier::from_stamp("trusted"), TrustTier::Trusted);
    assert_eq!(
        TrustTier::from_stamp("untrusted_fork"),
        TrustTier::UntrustedFork
    );
    assert_eq!(
        TrustTier::from_stamp("self_hosted"),
        TrustTier::UntrustedFork,
        "an unknown provenance stamp is fail-closed to untrusted (never an upgrade)"
    );
}

/// **The `details_ref` is the `#step-<n>` jump-to-failure on failure, the run summary on success
/// (OQ-D / 5.7).** Git renders it as a link; CI never resolves it. References-not-payloads.
#[test]
fn details_ref_anchors_on_the_failing_step() {
    let run = "myelin://acme/ci/run/run-7";
    // A failure with a known step → #step-<n>.
    assert_eq!(
        details_ref(run, CheckState::Failure, Some(4)),
        "myelin://acme/ci/run/run-7#step-4"
    );
    // A failure without a known step → #step-failure (the log index resolves the step).
    assert_eq!(
        details_ref(run, CheckState::Failure, None),
        "myelin://acme/ci/run/run-7#step-failure"
    );
    // A success → the run summary.
    assert_eq!(
        details_ref(run, CheckState::Success, None),
        "myelin://acme/ci/run/run-7#summary"
    );
    // An error with a step also anchors on the step.
    assert_eq!(
        details_ref(run, CheckState::Error, Some(2)),
        "myelin://acme/ci/run/run-7#step-2"
    );
}

/// **`ci.check.updated` is well-formed: the envelope rides the FROZEN `check_updated_draft` (the
/// no-raw-publish path).** The `assemble_check_status` draft carries the §4.12 subject/aggregate
/// grammar byte-identical to what Git's gate consumes (0 drift) + the frozen-shape payload — emitted
/// via the outbox only.
#[test]
fn assembled_draft_rides_the_frozen_envelope_grammar() {
    let ctx = emit_ctx(1, Some("2026-06-23T00:01:00Z"));
    let draft = assemble_check_status(
        &ctx,
        CheckProvider::Ci,
        "build",
        CheckState::Success,
        true,
        CostPosture::Settled,
        None,
    );
    assert_eq!(draft.type_.0, "ci.check.updated", "the X-1 token (2.9)");
    assert_eq!(
        draft.subject.0, "myelin://acme/git/repo/core#commit-deadbeef/check-build",
        "subject = repo#commit-<oid>/check-<context> (§4.12 — byte-identical to Git's consumer)"
    );
    assert_eq!(
        draft.aggregate.0, "myelin://acme/git/repo/core#commit-deadbeef",
        "aggregate = (repo, commit_oid) — all contexts for one commit share the ordering partition"
    );
    assert!(
        !draft.contains_personal_data,
        "references-not-payloads (no inline PII)"
    );
    // The payload IS the frozen 5.9 shape (the producer's CheckStatus).
    assert_eq!(draft.payload["run_attempt"], 1);
    assert_eq!(draft.payload["summary"]["template_key"], "ci.check.success");
}

/// **A non-terminal (queued/in_progress) fact carries NO `completed_at` (a pending check).** The
/// terminal predicate gates the completion column — a pending fact's `completed_at` is `null`.
#[test]
fn a_pending_fact_carries_no_completed_at() {
    let ctx = emit_ctx(1, None); // no completion yet
    let p = check_status_payload(
        &ctx,
        CheckProvider::Ci,
        "build",
        CheckState::InProgress,
        true,
        CostPosture::Unsettled,
        None,
    );
    assert_eq!(p["state"], "in_progress");
    assert!(
        p["completed_at"].is_null(),
        "a pending check has no completed_at"
    );
    assert!(!CheckState::InProgress.is_terminal());
    assert!(CheckState::Success.is_terminal());
}
