use super::*;

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

#[test]
fn the_wedge_driver_returns_a_green_e2e_1_artifact() {
    let artifact = run_notif_e2e_wedge();
    assert_eq!(artifact.scenario, E2E_SCENARIO);
    assert!(artifact.is_green(), "{}", artifact.evidence);
}

#[test]
fn e2e_1_outsider_zero_title_leak_across_every_channel() {
    let tenant = e2e_tenant();
    let (_pr, check_ref, confidential) = pr_pane_subjects(&tenant.0);
    let owner = Arc::new(PrPaneOwner::new("insider", confidential.clone(), check_ref));
    let cache = Arc::new(SharedRefCache::new(owner));
    let templates = TemplateStore::with_platform_defaults();
    let outsider = e2e_viewer("outsider");
    let at = bounded_stale();

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
        "the insider (allowed) DOES see the title - the render is per-viewer, not vacuous"
    );
}

#[test]
fn e2e_1_check_panel_live_updates_over_the_firehose_with_cache_bust() {
    let tenant = e2e_tenant();
    let (_pr, check_ref, confidential) = pr_pane_subjects(&tenant.0);
    let owner = Arc::new(PrPaneOwner::new("insider", confidential, check_ref.clone()));
    let cache = Arc::new(SharedRefCache::new(owner.clone()));
    let templates = TemplateStore::with_platform_defaults();
    let insider = e2e_viewer("insider");
    let at = bounded_stale();

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

#[test]
fn e2e_1_live_update_rides_the_bounded_inbox_key() {
    let insider = e2e_viewer("insider");
    assert_eq!(inbox_stream(&insider), "fan.acme.inbox");
    let scope = inbox_scope(&insider).expect("a real principal makes a bounded inbox scope");
    assert_eq!(scope.selector(), "inbox:insider");
}

#[test]
fn e2e_1_live_frame_is_a_pointer_not_a_rendered_string() {
    let draft = e2e_live_frame_draft("notif-item-7");
    let mut firehose = Firehose::new();
    let insider = e2e_viewer("insider");
    let watch = watch_open(&mut firehose, &insider)
        .unwrap()
        .into_live()
        .unwrap();
    let _ = draft;
    publish_inbox_frame(&mut firehose, &insider, "notif-item-7").unwrap();
    let frames = watch.drain();
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames[0].item_id, "notif-item-7",
        "the frame carries the opaque item_id pointer, never a rendered pane string"
    );
}

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

#[test]
fn e2e_2_hitl_card_is_critical_and_shows_action_risk_cost() {
    let (prio, class) = reason_base_class(Reason::ApprovalRequested);
    assert_eq!(
        class,
        Class::Critical,
        "the approval card is critical-banded"
    );
    assert_eq!(prio, 90, "the approval card sits at the top priority band");

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
        "the approver (allowed) sees the fix-PR title - the card is per-viewer, not vacuous"
    );
}

#[test]
fn e2e_2_escalation_is_exactly_once_across_a_kill() {
    assert!(
        escalation_exactly_once_across_a_kill(),
        "the escalation/notify legs must be exactly-once across a kill (no missed step, no double page)"
    );
}

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

#[test]
fn e2e_4_inbox_items_show_erased_user() {
    let artifact = run_e2e_4_dsar_and_stor_d2();
    assert!(
        artifact.evidence.contains("inbox_shows_[erased_user]=true"),
        "every inbox appearance of the erased subject shows [erased user]: {}",
        artifact.evidence
    );
}

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

#[test]
fn stor_d2_thresholds_are_load_bearing() {
    let green = run_stor_d2_at_cell_scale(&e2e_tenant());
    assert!(green.is_green());

    let lost = StorD2Verdict {
        cold_equals_live: false,
        ..green.clone()
    };
    assert!(
        !lost.is_green(),
        "a cold != live restore (data loss) MUST be RED"
    );

    let resurrected = StorD2Verdict {
        erasure_held: false,
        ..green.clone()
    };
    assert!(
        !resurrected.is_green(),
        "an erased subject resurrected by the restore MUST be RED"
    );

    let stale = StorD2Verdict {
        rpo_seconds: 301,
        ..green.clone()
    };
    assert!(
        !stale.is_green(),
        "an RPO over the 5-min budget MUST be RED"
    );

    let slow_tenant = StorD2Verdict {
        rto_tenant_seconds: 60 * 60 + 1,
        ..green.clone()
    };
    assert!(
        !slow_tenant.is_green(),
        "an RTO over the 1h-per-tenant budget MUST be RED"
    );

    let slow_cell = StorD2Verdict {
        rto_cell_seconds: 4 * 60 * 60 + 1,
        ..green
    };
    assert!(
        !slow_cell.is_green(),
        "an RTO over the 4h-per-cell budget MUST be RED"
    );
}

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
