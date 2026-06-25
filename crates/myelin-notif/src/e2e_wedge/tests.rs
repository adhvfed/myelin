//! Whole-system E2E wedge tests for Notif's E2E-1 leg (NOTIF-P28 / P-470) and E2E-2 leg
//! (NOTIF-P29 / P-471 — the HITL flagship).
//!
//! Each test drives a CHAINED mutation end-to-end (EI-01 §4) — NOT a single-handler test. E2E-1: a
//! ref change → firehose live frame → shared per-ref cache bust → live pane re-humanise → per-viewer
//! re-resolve. E2E-2: CI-fail → triage agent → HITL card withheld → human approves → apply, with a
//! kill mid-escalation asserting exactly-once. The leak-invariant mutation floors (NOTIF-D4
//! humanise, NOTIF-P15 firehose) and the escalation exactly-once / explicit-first floors (NOTIF-P14,
//! NOTIF-P22) are UNCHANGED and re-asserted at E2E scale.

use super::*;

/// **The E2E-1 leg is GREEN end-to-end (the named green artifact's `is_green()`).** The whole pane
/// flow — per-viewer humanise, the mid-flight live firehose update, the shared per-ref cache bust,
/// the per-viewer tombstone — holds with 0 leak.
#[test]
fn e2e_1_pr_pane_is_green_end_to_end() {
    let artifact = run_e2e_1_pr_pane();
    assert_eq!(artifact.scenario, "E2E-1");
    assert!(
        artifact.is_green(),
        "the E2E-1 PR-pane leg must be green end-to-end: {}",
        artifact.evidence
    );
    assert_eq!(artifact.leaks, 0, "0 title leak to the unauthorized viewer");
}

/// **The wedge driver returns the E2E-1 leg's green artifact (the master M5 exit-gate row).** A red
/// E2E-1 must NOT let M6 start; here it is green.
#[test]
fn the_wedge_driver_returns_a_green_e2e_1_artifact() {
    let artifact = run_notif_e2e_wedge();
    assert_eq!(artifact.scenario, E2E_SCENARIO);
    assert!(artifact.is_green(), "{}", artifact.evidence);
}

/// **0 leak to the unauthorized viewer (the F1 / NOTIF-D4 spine, at E2E scale).** A SECOND viewer
/// without access to the confidential issue opens the same pane — the issue humanises to a tombstone
/// ("a restricted issue"), the SECRET title NEVER present across ANY channel projection.
#[test]
fn e2e_1_outsider_zero_title_leak_across_every_channel() {
    let tenant = e2e_tenant();
    let (_pr, check_ref, confidential) = pr_pane_subjects(&tenant.0);
    let owner = Arc::new(PrPaneOwner::new("insider", confidential.clone(), check_ref));
    let cache = Arc::new(SharedRefCache::new(owner));
    let templates = TemplateStore::with_platform_defaults();
    let outsider = e2e_viewer("outsider");
    let at = bounded_stale();

    // Across CLI / Email / Markdown the secret title is absent for the denied viewer.
    let leaked = pane_humanise_leaks_title(
        cache.as_ref(),
        &templates,
        "review_requested",
        &confidential,
        &outsider,
        &at,
    );
    assert!(
        !leaked,
        "0 title leak to the unauthorized viewer across every channel projection"
    );

    // The insider (allowed) DOES see the title — the render is genuinely per-viewer (a vacuous
    // "nobody sees anything" green is caught: the insider's title is present).
    let insider = e2e_viewer("insider");
    let h = humanise(
        cache.as_ref(),
        &tenant,
        &e2e_region(),
        &templates,
        "review_requested",
        std::slice::from_ref(&confidential),
        &insider,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    assert!(
        h.text.contains("TOP SECRET acquisition plan"),
        "the insider (allowed) DOES see the title — the render is per-viewer, not vacuous"
    );
}

/// **The mid-flight `ci.check.updated` live-updates the pane over the firehose (NOTIF-P15).** The
/// checks panel serves the NEW state ONLY after the live frame arrives over the resume-cursor path
/// AND the shared per-ref cache busts — proving the live update is real, not a stale cache hit.
#[test]
fn e2e_1_check_panel_live_updates_over_the_firehose_with_cache_bust() {
    let tenant = e2e_tenant();
    let (_pr, check_ref, confidential) = pr_pane_subjects(&tenant.0);
    let owner = Arc::new(PrPaneOwner::new("insider", confidential, check_ref.clone()));
    let cache = Arc::new(SharedRefCache::new(owner.clone()));
    let templates = TemplateStore::with_platform_defaults();
    let insider = e2e_viewer("insider");
    let at = bounded_stale();

    // Render once — the checks panel shows the PENDING state, and the shared cache holds it.
    let first = humanise(
        cache.as_ref(),
        &tenant,
        &e2e_region(),
        &templates,
        "state_changed",
        std::slice::from_ref(&check_ref),
        &insider,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    assert!(
        first.text.contains("pending"),
        "first render shows the pending check state"
    );

    // CI flips the state, but WITHOUT a cache bust the shared cache would still serve the stale state.
    owner.update_check("success");
    let stale = humanise(
        cache.as_ref(),
        &tenant,
        &e2e_region(),
        &templates,
        "state_changed",
        std::slice::from_ref(&check_ref),
        &insider,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    assert!(
        stale.text.contains("pending"),
        "without a bust the shared per-ref cache serves the stale state (the cache is real)"
    );

    // The live firehose frame arrives over the bounded inbox stream/scope, then the cache busts.
    let mut firehose = Firehose::new();
    let watch = watch_open(&mut firehose, &insider)
        .unwrap()
        .into_live()
        .unwrap();
    assert!(publish_inbox_frame(&mut firehose, &insider, &check_ref.0).is_ok());
    let frames = watch.drain();
    assert!(
        frames.iter().any(|f| f.item_id == check_ref.0),
        "the live ci.check.updated frame arrives over the resume-cursor firehose path (0 items lost)"
    );
    cache.bust(&check_ref);

    // Now the pane re-humanises and serves the NEW state (the live update landed).
    let fresh = humanise(
        cache.as_ref(),
        &tenant,
        &e2e_region(),
        &templates,
        "state_changed",
        std::slice::from_ref(&check_ref),
        &insider,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    assert!(
        fresh.text.contains("success"),
        "after the firehose update + cache bust the checks panel live-updates to the new state"
    );
}

/// **The live update rides the BOUNDED inbox firehose key, never `*` (the §7 residency/isolation
/// shape).** The (stream, scope) is `(fan.<tenant>.inbox, inbox:<principal>)` — a client gets ONLY
/// its own inbox's frames.
#[test]
fn e2e_1_live_update_rides_the_bounded_inbox_key() {
    let insider = e2e_viewer("insider");
    assert_eq!(inbox_stream(&insider), "fan.acme.inbox");
    let scope = inbox_scope(&insider).expect("a real principal makes a bounded inbox scope");
    assert_eq!(scope.selector(), "inbox:insider");
}

/// **The live frame is the `item_id` POINTER, never a rendered string (references-not-payloads,
/// NOTIF-1).** The firehose never carries the humanised pane string — the watcher re-humanises on a
/// per-viewer READ.
#[test]
fn e2e_1_live_frame_is_a_pointer_not_a_rendered_string() {
    let draft = e2e_live_frame_draft("notif-item-7");
    // The draft body is the opaque item-id pointer (no rendered title can ride the firehose).
    let mut firehose = Firehose::new();
    let insider = e2e_viewer("insider");
    let watch = watch_open(&mut firehose, &insider)
        .unwrap()
        .into_live()
        .unwrap();
    let _ = draft; // the draft constructor is the convenience; publish through the frozen path.
    publish_inbox_frame(&mut firehose, &insider, "notif-item-7").unwrap();
    let frames = watch.drain();
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames[0].item_id, "notif-item-7",
        "the frame carries the opaque item_id pointer, never a rendered pane string"
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
//  E2E-2 — the HITL FLAGSHIP leg (NOTIF-P29 / P-471): the approval card + explicit-first +
//  exactly-once across a kill. Each test drives a CHAINED mutation end-to-end (EI-01 §4), not a
//  single handler: CI-fail → triage agent → HITL card withheld → human approves → apply, with a kill
//  mid-escalation asserting exactly-once. The NOTIF-P7/P9/P14/P22 mutation floors are UNCHANGED and
//  re-asserted at E2E scale.
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The E2E-2 HITL-flagship leg is GREEN end-to-end (the named green artifact's `is_green()`).** The
/// whole flow — the critical approval card humanised with action+risk+cost, the explicit-first
/// boundary (0 auto-spawn, 0 mutation pre-approval, exactly 1 apply), the escalation exactly-once
/// across a kill — holds with 0 leak.
#[test]
fn e2e_2_hitl_flagship_is_green_end_to_end() {
    let artifact = run_e2e_2_hitl_flagship();
    assert_eq!(artifact.scenario, "E2E-2");
    assert!(
        artifact.is_green(),
        "the E2E-2 HITL-flagship leg must be green end-to-end: {}",
        artifact.evidence
    );
    assert_eq!(artifact.leaks, 0, "0 title leak to the unauthorized viewer");
}

/// **The HITL approval card is a Notif item `reason=approval_requested`, ranked CRITICAL (NOTIF-P5/
/// P7), humanised with action+risk+cost per-viewer (NOTIF-P9).** The approver's card shows the
/// proposed action, the risk, and the cost (the §1.4 affordance — approve on full information).
#[test]
fn e2e_2_hitl_card_is_critical_and_shows_action_risk_cost() {
    // The card's reason ranks at the critical band (the §3.1 ranking — approval_requested pierces).
    let (prio, class) = reason_base_class(Reason::ApprovalRequested);
    assert_eq!(
        class,
        Class::Critical,
        "the approval card is critical-banded"
    );
    assert_eq!(prio, 90, "the approval card sits at the top priority band");

    // The card humanises action+risk+cost for the approver through the SAME contract-7.3 surface.
    let tenant = e2e_tenant();
    let templates = hitl_card_templates();
    let fix_pr = fix_pr_subject(&tenant.0);
    let owner = Arc::new(HitlCardOwner::new("maintainer", fix_pr.clone()));
    let approver = e2e_viewer("maintainer");
    let card = humanise(
        owner.as_ref(),
        &tenant,
        &e2e_region(),
        &templates,
        HITL_CARD_TEMPLATE_KEY,
        &[
            fix_pr,
            ArtifactRef("git.merge".into()),
            ArtifactRef("irreversible".into()),
            ArtifactRef("$0.00".into()),
        ],
        &approver,
        DEFAULT_LOCALE,
        &bounded_stale(),
        Channel::Cli,
    );
    assert!(
        card.text.contains("git.merge")
            && card.text.contains("irreversible")
            && card.text.contains("$0.00"),
        "the card renders action+risk+cost on full information: {}",
        card.text
    );
}

/// **0 leak on the HITL card to the unauthorized viewer (the F1 / NOTIF-D4 spine, at E2E scale).** A
/// viewer DENIED the confidential fix-PR sees a tombstone ("a restricted …"), the SECRET title NEVER
/// present across ANY channel projection.
#[test]
fn e2e_2_hitl_card_zero_title_leak_across_every_channel() {
    let tenant = e2e_tenant();
    let templates = hitl_card_templates();
    let fix_pr = fix_pr_subject(&tenant.0);
    let owner = Arc::new(HitlCardOwner::new("maintainer", fix_pr.clone()));
    let outsider = e2e_viewer("outsider");
    let at = bounded_stale();

    for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
        let card = humanise(
            owner.as_ref(),
            &tenant,
            &e2e_region(),
            &templates,
            HITL_CARD_TEMPLATE_KEY,
            std::slice::from_ref(&fix_pr),
            &outsider,
            DEFAULT_LOCALE,
            &at,
            channel,
        );
        let rendered = format!("{} {} {}", card.text, card.links.join(" "), card.icon);
        assert!(
            !rendered.contains("TOP SECRET acquisition fix") && !rendered.contains("acquisition"),
            "0 title leak on the HITL card to the unauthorized viewer (channel {channel:?})"
        );
    }

    // The approver (allowed) DOES see the title — the card render is genuinely per-viewer (not vacuous).
    let approver = e2e_viewer("maintainer");
    let allowed = humanise(
        owner.as_ref(),
        &tenant,
        &e2e_region(),
        &templates,
        HITL_CARD_TEMPLATE_KEY,
        std::slice::from_ref(&fix_pr),
        &approver,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    assert!(
        allowed.text.contains("TOP SECRET acquisition fix"),
        "the approver (allowed) sees the fix-PR title — the card is per-viewer, not vacuous"
    );
}

/// **The escalation/notify legs are exactly-once ACROSS A KILL (NOTIF-P14 / NOTIF-D7, at E2E scale).**
/// The unacked card escalates on the durable wheel; the engine is killed mid-`ack_window`, resumes
/// from the persisted handle, and the next step pages EXACTLY ONCE (never zero, never two); a
/// replayed fire is a no-op; the ack halts the chain idempotently; the ack event rode the outbox once.
#[test]
fn e2e_2_escalation_is_exactly_once_across_a_kill() {
    assert!(
        escalation_exactly_once_across_a_kill(),
        "the escalation/notify legs must be exactly-once across a kill (no missed step, no double page)"
    );
}

/// **Explicit-first (NOTIF-P22 / 8.6): a casual @agent mention is a NOTIFY, never a dispatch — 0
/// auto-spawn; the EXPLICIT human approval is the ONLY thing that drives the apply.** The leg proves
/// 0 mutation pre-approval and exactly 1 apply after the explicit approve. (Asserted through the
/// green artifact's evidence — the predicate is load-bearing in `run_e2e_2_hitl_flagship`.)
#[test]
fn e2e_2_explicit_first_zero_auto_spawn_and_exactly_one_apply() {
    let artifact = run_e2e_2_hitl_flagship();
    assert!(
        artifact.evidence.contains("auto_spawn=false"),
        "0 auto-spawn from the casual mention: {}",
        artifact.evidence
    );
    assert!(
        artifact.evidence.contains("applies_pre_approval=0"),
        "0 mutation pre-approval: {}",
        artifact.evidence
    );
    assert!(
        artifact.evidence.contains("applies_post_approval=1"),
        "exactly 1 apply after the explicit approval: {}",
        artifact.evidence
    );
    assert!(
        artifact.evidence.contains("exactly_once_apply=true"),
        "the apply is exactly-once (a replay does not double-mutate): {}",
        artifact.evidence
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
//  E2E-4 — the DSAR fan-out leg + STOR-D2 at cell scale (NOTIF-P30 / P-472 — the LAST Notif prompt).
//
//  Each test drives the CHAINED DSAR end-to-end (EI-01 §4) — NOT a single-handler test: holder locate
//  → residual erase (NOTIF-P27) → 0 recoverable PII → [erased user] at read time → multi-cell
//  member_cells iteration → STOR-D2 restore-verify of the system-of-record tables. The erase /
//  tombstone / cross-cell mutation floors (NOTIF-P4/P27/P24, NOTIF-D6) are UNCHANGED and re-asserted at
//  E2E scale.
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The E2E-4 DSAR leg + STOR-D2 is GREEN end-to-end (the named green artifact's `is_green()`).** The
/// whole DSAR fan-out — Notif located as a holder, the residual erased to 0 recoverable PII, the inbox
/// showing `[erased user]`, every member cell erased — plus the STOR-D2 permanent gate, all hold.
#[test]
fn e2e_4_dsar_and_stor_d2_is_green_end_to_end() {
    let artifact = run_e2e_4_dsar_and_stor_d2();
    assert_eq!(artifact.scenario, "E2E-4");
    assert!(
        artifact.is_green(),
        "the E2E-4 DSAR leg + STOR-D2 must be green end-to-end: {}",
        artifact.evidence
    );
}

/// **The DSAR threshold: 0 recoverable PII (NOTIF-D6 / E2E-4).** After the residual erase, the
/// inline-PII delivery column is unrecoverable — the count of recoverable columns is EXACTLY 0. Never
/// softened.
#[test]
fn e2e_4_post_erase_zero_recoverable_pii() {
    let artifact = run_e2e_4_dsar_and_stor_d2();
    assert_eq!(
        artifact.recoverable_pii, 0,
        "post-erase locate = 0 recoverable PII: {}",
        artifact.evidence
    );
    assert!(
        artifact.evidence.contains("recoverable_pii=0"),
        "the artifact records 0 recoverable PII: {}",
        artifact.evidence
    );
}

/// **Inbox items show `[erased user]` (the tombstone-for-free, at E2E scale).** Notif is one of the
/// H1–H18 holders; the erased subject's appearance humanises to `[erased user]` across every channel
/// projection, with the opaque id NEVER leaked.
#[test]
fn e2e_4_inbox_items_show_erased_user() {
    let artifact = run_e2e_4_dsar_and_stor_d2();
    assert!(
        artifact.evidence.contains("inbox_shows_[erased_user]=true"),
        "every inbox appearance of the erased subject shows [erased user]: {}",
        artifact.evidence
    );
}

/// **The multi-cell DSAR leg: 0 holders missed across the union (10.4 / NOTIF-P24 / GA-D8).** The DSR
/// orchestrator iterates `member_cells` over the cross-cell bridge; every member cell mints an erase
/// receipt (`erased = true`).
#[test]
fn e2e_4_multi_cell_member_cells_all_erased() {
    let artifact = run_e2e_4_dsar_and_stor_d2();
    assert_eq!(
        artifact.member_cells_erased, 2,
        "every member cell erased its inbox pointers (0 holders missed): {}",
        artifact.evidence
    );
    assert!(
        artifact.evidence.contains("all_erased=true"),
        "the member_cells iteration ran across the union: {}",
        artifact.evidence
    );
}

/// **STOR-D2 at cell scale is GREEN — the permanent gate (11.5 / §5.5).** The restore-verify of
/// Notif's system-of-record tables holds: cold == live (0 loss), the erasure held across the restore,
/// and the RPO/RTO are within the unweakened master §2 budgets.
#[test]
fn stor_d2_at_cell_scale_is_green() {
    let verdict = run_stor_d2_at_cell_scale(&e2e_tenant());
    assert!(
        verdict.is_green(),
        "STOR-D2 at cell scale must be green (the permanent gate): {}",
        verdict.summary()
    );
    assert!(verdict.cold_equals_live, "cold == live (0 loss)");
    assert!(
        verdict.erasure_held,
        "a pre-backup shred stayed dead across the restore"
    );
}

/// **MANDATORY-CORE: the STOR-D2 thresholds are NEVER weakened (EI-01 §3).** A measured RPO over the
/// 5-min budget — or an RTO over the 1h-tenant / 4h-cell budget — is RED. We assert the gate refuses a
/// loss (cold != live) and an over-budget RPO/RTO; a mutant that softens any threshold or inverts a
/// comparison is caught.
#[test]
fn stor_d2_thresholds_are_load_bearing() {
    // The honest measured verdict is green and well within budget.
    let green = run_stor_d2_at_cell_scale(&e2e_tenant());
    assert!(green.is_green());

    // A LOSS (cold != live) is RED — the restored copy is not whole.
    let lost = StorD2Verdict {
        cold_equals_live: false,
        ..green.clone()
    };
    assert!(
        !lost.is_green(),
        "a cold != live restore (data loss) MUST be RED"
    );

    // A RESURRECTED subject (erasure NOT held) is RED — the gravest failure.
    let resurrected = StorD2Verdict {
        erasure_held: false,
        ..green.clone()
    };
    assert!(
        !resurrected.is_green(),
        "an erased subject resurrected by the restore MUST be RED"
    );

    // An RPO over the 5-min (300s) budget is RED — never softened.
    let stale = StorD2Verdict {
        rpo_seconds: 301,
        ..green.clone()
    };
    assert!(
        !stale.is_green(),
        "an RPO over the 5-min budget MUST be RED"
    );

    // An RTO over the per-tenant 1h budget is RED.
    let slow_tenant = StorD2Verdict {
        rto_tenant_seconds: 60 * 60 + 1,
        ..green.clone()
    };
    assert!(
        !slow_tenant.is_green(),
        "an RTO over the 1h-per-tenant budget MUST be RED"
    );

    // An RTO over the per-cell 4h budget is RED.
    let slow_cell = StorD2Verdict {
        rto_cell_seconds: 4 * 60 * 60 + 1,
        ..green
    };
    assert!(
        !slow_cell.is_green(),
        "an RTO over the 4h-per-cell budget MUST be RED"
    );
}

/// **The E2E-4 driver returns the named green artifact (the master M5 → M6 GDPR exit row).** A red
/// E2E-4 must NOT let M6 start; here it is green. THIS IS THE LAST NOTIF PROMPT.
#[test]
fn the_e2e_4_driver_returns_a_green_artifact() {
    let artifact = run_notif_e2e_4_dsar();
    assert_eq!(artifact.scenario, E2E_4_SCENARIO);
    assert!(artifact.is_green(), "{}", artifact.evidence);
    assert!(
        artifact.stor_d2_green,
        "the STOR-D2 permanent gate is folded into the E2E-4 artifact: {}",
        artifact.evidence
    );
}
