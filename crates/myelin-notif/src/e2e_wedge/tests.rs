//! Whole-system E2E wedge tests for Notif's E2E-1 leg (NOTIF-P28 / P-470).
//!
//! Each test drives a CHAINED mutation end-to-end across the full pane flow (EI-01 §4) — NOT a
//! single-handler test: a ref change → firehose live frame → shared per-ref cache bust → live pane
//! re-humanise → per-viewer re-resolve. The leak-invariant mutation floors (NOTIF-D4 humanise,
//! NOTIF-P15 firehose) are UNCHANGED and re-asserted at E2E scale.

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
