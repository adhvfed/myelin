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

#[test]
fn check_attempt_counter_is_monotonic_per_context() {
    let mut counter = CheckAttemptCounter::new();

    assert_eq!(counter.bump("deadbeef", "build"), 1, "first dispatch = 1");
    assert_eq!(counter.bump("deadbeef", "build"), 2, "a re-run bumps");
    assert_eq!(counter.bump("deadbeef", "build"), 3, "strictly increasing");

    assert_eq!(counter.bump("deadbeef", "test"), 1, "per-context sequence");
    assert_eq!(counter.bump("deadbeef", "test"), 2);

    assert_eq!(counter.bump("cafef00d", "build"), 1, "per-commit sequence");

    assert_eq!(counter.current("deadbeef", "build"), 3);
    assert_eq!(counter.current("deadbeef", "test"), 2);
    assert_eq!(counter.current("cafef00d", "build"), 1);
    assert_eq!(
        counter.current("deadbeef", "lint"),
        0,
        "an un-issued context is 0"
    );
}

#[test]
fn a_lower_attempt_is_stale_the_higher_supersedes() {
    let mut counter = CheckAttemptCounter::new();
    let a1 = counter.bump("deadbeef", "build");
    let a2 = counter.bump("deadbeef", "build");
    assert_eq!((a1, a2), (1, 2));

    assert!(
        counter.is_stale("deadbeef", "build", 1),
        "attempt 1 < current 2 → stale"
    );
    assert!(
        !counter.is_stale("deadbeef", "build", 2),
        "attempt 2 == current → not stale (it IS the current)"
    );
    assert!(!counter.is_stale("deadbeef", "build", 3));
}

#[test]
fn bump_check_attempt_sql_is_the_monotonic_upsert() {
    let sql = BUMP_CHECK_ATTEMPT_SQL;
    assert!(
        sql.contains("INSERT INTO check_attempt"),
        "upserts the attempt counter table"
    );
    assert!(
        sql.contains("ELSE check_attempt.next_attempt + 1"),
        "a re-dispatch bumps monotonically (+1) - never wall-clock"
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
        "the (commit_oid, context) key + tenant scope - one attempt source per key"
    );
}

#[test]
fn assembled_payload_carries_every_frozen_5_9_field() {
    let ctx = emit_ctx(2, Some("2026-06-23T00:01:00Z"));
    let p = check_status_payload(
        &ctx,
        &CheckStatusUpdate::required(CheckProvider::Ci, "build", CheckState::Success).settled(),
    );

    assert_eq!(p["tenant"], "acme");
    assert_eq!(p["repo"], "myelin://acme/git/repo/core");
    assert_eq!(p["commit_oid"], "deadbeef");
    assert_eq!(
        p["context"],
        serde_json::json!({ "provider": "ci", "name": "build" }),
        "the CheckContext is {{provider, name}} - the key half"
    );
    assert_eq!(p["state"], "success");
    assert_eq!(p["required"], true, "CI reports required; Git decides");
    assert_eq!(p["run"], "myelin://acme/ci/run/run-7");
    assert_eq!(p["run_attempt"], 2, "the monotonic supersession key");
    assert_eq!(p["trust_tier"], "trusted");
    assert_eq!(
        p["details_ref"], "myelin://acme/ci/run/run-7",
        "a success anchors on the canonical run root"
    );
    assert_eq!(p["started_at"], "2026-06-23T00:00:00Z");
    assert_eq!(p["completed_at"], "2026-06-23T00:01:00Z");
    assert_eq!(
        p["cost_settled"], true,
        "settled → cost_settled true (the check is final)"
    );
}

#[test]
fn summary_is_a_humanised_ref_never_a_raw_string() {
    let ctx = emit_ctx(1, Some("2026-06-23T00:01:00Z"));
    let p = check_status_payload(
        &ctx,
        &CheckStatusUpdate::required(CheckProvider::Ci, "build", CheckState::Failure)
            .settled()
            .failed_at_step(3),
    );

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

#[test]
fn cost_settled_flips_only_on_settle() {
    let ctx = emit_ctx(1, Some("2026-06-23T00:01:00Z"));

    let unsettled = check_status_payload(
        &ctx,
        &CheckStatusUpdate::required(CheckProvider::Ci, "build", CheckState::Success),
    );
    assert_eq!(unsettled["state"], "success", "terminal verdict");
    assert_eq!(
        unsettled["cost_settled"], false,
        "terminal but NOT settled until the reserve/settle bookend closes (X-1)"
    );

    let settled = check_status_payload(
        &ctx,
        &CheckStatusUpdate::required(CheckProvider::Ci, "build", CheckState::Success).settled(),
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

#[test]
fn trust_tier_is_stamped_from_provenance_fork_never_endorsed() {
    let mut ctx = emit_ctx(1, Some("2026-06-23T00:01:00Z"));
    ctx.trust_tier = TrustTier::UntrustedFork;

    let p = check_status_payload(
        &ctx,
        &CheckStatusUpdate::required(CheckProvider::Ci, "build", CheckState::Success).settled(),
    );
    assert_eq!(p["state"], "success", "the fork's success is recorded");
    assert_eq!(
        p["trust_tier"], "untrusted_fork",
        "the tier is stamped from provenance - CI never recomputes or endorses (X-1)"
    );

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

#[test]
fn details_ref_anchors_on_the_failing_step() {
    let run = "myelin://acme/ci/run/run-7";
    assert_eq!(
        details_ref(run, CheckState::Failure, Some(4)),
        "myelin://acme/ci/run/run-7#step-4"
    );
    assert_eq!(
        details_ref(run, CheckState::Failure, None),
        "myelin://acme/ci/run/run-7"
    );
    assert_eq!(
        details_ref(run, CheckState::Success, None),
        "myelin://acme/ci/run/run-7"
    );
    assert_eq!(
        details_ref(run, CheckState::Error, Some(2)),
        "myelin://acme/ci/run/run-7#step-2"
    );
}

#[test]
fn assembled_draft_rides_the_frozen_envelope_grammar() {
    let ctx = emit_ctx(1, Some("2026-06-23T00:01:00Z"));
    let draft = assemble_check_status(
        &ctx,
        &CheckStatusUpdate::required(CheckProvider::Ci, "build", CheckState::Success).settled(),
    );
    assert_eq!(draft.type_.0, "ci.check.updated", "the X-1 token (2.9)");
    assert_eq!(
        draft.subject.0, "myelin://acme/git/repo/core#commit-deadbeef/check-build",
        "subject = repo#commit-<oid>/check-<context> (§4.12 - byte-identical to Git's consumer)"
    );
    assert_eq!(
        draft.aggregate,
        myelin_events::check_seam::check_aggregate("myelin://acme/git/repo/core", "deadbeef",),
        "aggregate = (repo, commit_oid) - all contexts for one commit share the ordering partition"
    );
    assert!(
        !draft.contains_personal_data,
        "references-not-payloads (no inline PII)"
    );
    assert_eq!(draft.payload["run_attempt"], 1);
    assert_eq!(draft.payload["summary"]["template_key"], "ci.check.success");
}

#[test]
fn a_pending_fact_carries_no_completed_at() {
    let ctx = emit_ctx(1, None);
    let p = check_status_payload(
        &ctx,
        &CheckStatusUpdate::required(CheckProvider::Ci, "build", CheckState::InProgress),
    );
    assert_eq!(p["state"], "in_progress");
    assert!(
        p["completed_at"].is_null(),
        "a pending check has no completed_at"
    );
    assert!(!CheckState::InProgress.is_terminal());
    assert!(CheckState::Success.is_terminal());
}
