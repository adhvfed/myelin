//! Unit tests for the Git Web UI view-model (GIT-P32). These assert the design-pass invariants at the
//! view-model level (the browser-driven e2e is `tests/e2e_git_p32_web_browser.rs`):
//! - the fork-trust badge NEVER lets a fork's own green read as gating-green (the signed-off security
//!   invariant, design pass §4.1);
//! - status is NEVER colour alone (glyph + label always present, WCAG 1.4.1 / design pass §3);
//! - a tombstone NEVER leaks a title (the 0-leak boundary, design pass §5);
//! - the inline-colour ban holds in the rendered markup (no `style="color:` on interactive elements);
//! - the GF-6 single-file web-edit refuses a stale base (no silent overwrite, no 3-way editor).

use super::*;
use crate::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusRow, GitOid, HumanisedRef, TrustTier,
};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::collections::BTreeMap;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn aref(s: &str) -> ArtifactRef {
    ArtifactRef(s.into())
}

fn row(state: CheckState, trust: TrustTier, ctx: &str) -> CheckStatusRow {
    let fact = CheckStatus {
        tenant: tenant(),
        repo: aref("myelin://acme/git/repo/1"),
        commit_oid: GitOid("blake3:abc".into()),
        context: CheckContext::ci(ctx),
        state,
        required: true,
        run: aref("myelin://acme/ci/run/1"),
        run_attempt: 1,
        trust_tier: trust,
        details_ref: aref("myelin://acme/ci/run/1#step-2"),
        summary: HumanisedRef { template_key: "ci.check.ok".into(), args: BTreeMap::new() },
        started_at: crate::check_status::Timestamp("2026-06-22T00:00:00Z".into()),
        completed_at: None,
        cost_settled: true,
    };
    CheckStatusRow::from_fact(&fact)
}

#[test]
fn status_is_never_colour_alone_every_check_state_has_glyph_and_label() {
    // WCAG 1.4.1 / design pass §3: glyph + label always present (the colour-blind / monochrome floor).
    for state in [
        CheckState::Success,
        CheckState::Failure,
        CheckState::Error,
        CheckState::Cancelled,
        CheckState::Queued,
        CheckState::InProgress,
        CheckState::Neutral,
    ] {
        let cue = StatusCue::for_check_state(state);
        assert!(!cue.glyph.is_empty(), "state {state:?} has an empty glyph (colour-only)");
        assert!(!cue.label.is_empty(), "state {state:?} has an empty label (colour-only)");
    }
}

#[test]
fn error_and_cancelled_are_distinct_from_failure() {
    // design pass §4.2: an infra error is NOT a test failure and must not read as one.
    let failure = StatusCue::for_check_state(CheckState::Failure);
    let error = StatusCue::for_check_state(CheckState::Error);
    let cancelled = StatusCue::for_check_state(CheckState::Cancelled);
    assert_eq!(failure.token, StatusToken::Danger);
    assert_eq!(error.token, StatusToken::Warning, "error must be distinct (warning) from failure");
    assert_ne!(error.glyph, failure.glyph);
    assert_ne!(cancelled.glyph, failure.glyph);
    assert_eq!(cancelled.token, StatusToken::Warning);
}

#[test]
fn fork_badge_appears_only_for_unendorsed_fork_success() {
    // The signed-off security invariant (design pass §4.1): a fork's own green NEVER reads as
    // gating-green. The badge (the "neutral until trusted" warning) appears EXACTLY for an
    // un-endorsed untrusted_fork row.
    let trusted = row(CheckState::Success, TrustTier::Trusted, "build");
    assert!(ForkTrustBadge::for_row(&trusted, true, false).is_none(), "trusted success: no badge");

    let fork = row(CheckState::Success, TrustTier::UntrustedFork, "build");
    assert!(
        ForkTrustBadge::for_row(&fork, true, false).is_some(),
        "un-endorsed fork success MUST show the neutral-until-trusted badge"
    );
    // Once endorsed it flips to the success state — no warning badge.
    assert!(
        ForkTrustBadge::for_row(&fork, true, true).is_none(),
        "endorsed fork success: badge clears (counts toward the gate)"
    );
}

#[test]
fn fork_trust_action_is_absent_for_a_viewer_without_the_capability() {
    // design pass §4.1: the [ Trust this run ] action is gated on approve_untrusted_ci; absent
    // (read-only) for a viewer without it — no leaked affordance.
    let fork = row(CheckState::Success, TrustTier::UntrustedFork, "build");

    let with_cap = ForkTrustBadge::for_row(&fork, true, false).unwrap().render();
    assert!(with_cap.contains("Trust this run"), "a maintainer sees the trust action");

    let without_cap = ForkTrustBadge::for_row(&fork, false, false).unwrap().render();
    assert!(
        !without_cap.contains("Trust this run"),
        "a viewer without approve_untrusted_ci must NOT see the trust action (no leaked affordance)"
    );
    // The honest copy is present regardless (the badge is never silently hidden).
    assert!(without_cap.contains("untrusted fork") || without_cap.contains("FORK run"));
}

#[test]
fn checks_panel_renders_humanised_summary_not_a_raw_ci_string() {
    // design pass §4.2 / §5: the panel renders the Notif-humanised summary (the frontend owns no
    // humanisation map). The view-model takes the already-humanised text.
    let r = row(CheckState::Failure, TrustTier::Trusted, "test");
    let view = CheckRowView::from_row(&r, "3 tests failed", true, false, false);
    let html = view.render();
    assert!(html.contains("3 tests failed"), "the humanised summary renders");
    assert!(html.contains("ci/test"), "the context renders");
    assert!(html.contains("required"));
    assert!(html.contains("failed"), "the failure label renders (never colour alone)");
}

#[test]
fn checks_panel_covers_empty_loading_error_states() {
    // design pass §4.2: empty / loading-skeleton / error (fail-static for this surface only).
    let empty = ChecksPanel::Empty.render();
    assert!(empty.contains("No checks configured"));

    let loading = ChecksPanel::Loading { skeleton_rows: 3 }.render();
    assert_eq!(loading.matches("skeleton-row").count(), 3, "skeleton matches the final layout");
    assert!(loading.contains("aria-busy=\"true\""), "skeleton sets aria-busy (DESIGN-MANUAL §6)");
    assert!(loading.contains("aria-live=\"polite\""), "one polite live region announces loading");
    // No blank spinner — there is no spinner token in the system.
    assert!(!loading.to_lowercase().contains("spinner"));

    let error = ChecksPanel::Error.render();
    assert!(error.contains("Checks unavailable"));
    assert!(error.contains("Retry"), "error offers a scoped retry path, never a dead end");
}

#[test]
fn merge_readiness_names_which_gate_is_unmet_never_a_bare_blocked() {
    // design pass §4.3: the readiness names WHICH context is unmet, humanised, with the next action —
    // never a bare "blocked".
    let outcome = MergeGateOutcome::Blocked {
        unmet: vec![
            UnmetContext {
                context: CheckContext::ci("test"),
                reason: UnmetReason::NotGreen { state: CheckState::Failure },
            },
            UnmetContext {
                context: CheckContext::ci("e2e"),
                reason: UnmetReason::UntrustedForkNeutral,
            },
        ],
    };
    let html = MergeReadiness::from_gate(&outcome, (0, 2)).render();
    assert!(html.contains("test"), "names the failing context");
    assert!(html.contains("e2e"), "names the fork-neutral context");
    assert!(html.contains("awaiting fork trust"), "humanises the fork-neutral reason");
    // The bare word "Blocked:" is a prefix to the named list — never alone.
    assert!(html.contains("Blocked:"));
    assert!(html.len() > "Blocked".len() + 20);
}

#[test]
fn merge_readiness_ready_shows_merge_and_auto_merge() {
    let html = MergeReadiness::from_gate(&MergeGateOutcome::Admitted, (2, 2)).render();
    assert!(html.contains("All required checks green"));
    assert!(html.contains("2/2 approvals"));
    assert!(html.contains("Merge"));
    assert!(html.contains("auto-merge"));
}

#[test]
fn pr_overview_tombstone_never_leaks_a_title() {
    // The 0-leak boundary (design pass §5): a tombstoned projection renders the dignified, content-free
    // permission/erased state — the title NEVER reaches the render.
    use crate::project::{Tombstone, TombstoneReason};
    let page = PrOverviewPage {
        projected: Projected::Tombstoned(Tombstone { reason: TombstoneReason::Unauthorized }),
        pr_state: PrState::Open,
        checks: ChecksPanel::Empty,
        merge: MergeReadiness::Queued { position: 1 },
    };
    let html = page.render();
    assert!(html.contains("not available to you"), "dignified permission state");
    // No title, no state pill, no checks panel leaked for a tombstone.
    assert!(!html.contains("pr-title"), "no title element for a tombstone");
    assert!(!html.contains("checks-panel"), "no checks surface leaked for a tombstone");
}

#[test]
fn pr_overview_visible_renders_title_state_checks_merge() {
    let projection = crate::project::Projection {
        title: "Fix the receive-pack CAS".into(),
        state: "open".into(),
        icon: "pr".into(),
        render_hint: Some(RenderHint {
            checks: ChecksSummary::Red,
            approvals: (1, 2),
            is_draft: false,
        }),
        sub_anchor: None,
    };
    let page = PrOverviewPage {
        projected: Projected::Visible(projection),
        pr_state: PrState::Open,
        checks: ChecksPanel::Live {
            rows: vec![CheckRowView::from_row(
                &row(CheckState::Failure, TrustTier::Trusted, "test"),
                "3 tests failed",
                true,
                false,
                false,
            )],
        },
        merge: MergeReadiness::Blocked {
            unmet: vec![UnmetContext {
                context: CheckContext::ci("test"),
                reason: UnmetReason::NotGreen { state: CheckState::Failure },
            }],
        },
    };
    let html = page.render();
    assert!(html.contains("Fix the receive-pack CAS"), "title renders for a visible projection");
    assert!(html.contains("pr-state-pill"));
    assert!(html.contains("checks-panel"));
    assert!(html.contains("merge-readiness"));
    assert!(html.contains("1/2 approvals"));
}

#[test]
fn web_edit_refuses_a_stale_base_no_silent_overwrite() {
    // GF-6: single-file web edit refuses a stale base honestly (no 3-way editor, no silent overwrite).
    let committed = WebEditOutcome::evaluate("base-oid", "base-oid", "new-oid", true);
    assert_eq!(committed, WebEditOutcome::Committed { new_oid: "new-oid".into() });

    let stale = WebEditOutcome::evaluate("base-oid", "moved-oid", "new-oid", true);
    assert_eq!(stale, WebEditOutcome::StaleBase { current_oid: "moved-oid".into() }, "stale base refuses");

    let denied = WebEditOutcome::evaluate("base-oid", "base-oid", "new-oid", false);
    assert_eq!(denied, WebEditOutcome::Denied, "no write permission is denied");
}

#[test]
fn web_edit_form_omits_composer_for_a_read_only_viewer() {
    // DESIGN-MANUAL §4.2: an unpickable affordance is ABSENT, not greyed.
    let editable = WebEditForm {
        path: "src/lib.rs".into(),
        contents: "fn main() {}".into(),
        base_oid: "base".into(),
        viewer_may_edit: true,
    };
    let ro = WebEditForm { viewer_may_edit: false, ..editable.clone() };
    assert!(editable.render().contains("Commit change"), "an editor sees the composer");
    assert!(!ro.render().contains("Commit change"), "a read-only viewer: composer ABSENT, not greyed");
    assert!(ro.render().contains("fn main()"), "the file still renders read-only");
}

#[test]
fn rendered_markup_carries_no_inline_interactive_colour() {
    // The inline-colour ban (design pass §1, PROVEN): no screen sets colour via inline style on an
    // interactive element (inline beats :hover/:focus specificity). All colour is token/class driven.
    let r = row(CheckState::Success, TrustTier::UntrustedFork, "build");
    let badge = ForkTrustBadge::for_row(&r, true, false).unwrap().render();
    let checks = ChecksPanel::Live {
        rows: vec![CheckRowView::from_row(&r, "ok", true, true, false)],
    }
    .render();
    for html in [badge, checks] {
        assert!(!html.contains("style=\"color"), "no inline colour on rendered elements");
        assert!(!html.contains("style='color"), "no inline colour on rendered elements");
    }
}

#[test]
fn page_shell_is_well_formed_and_uses_semantic_tokens() {
    let html = page("PR #1", &ChecksPanel::Empty.render());
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("lang=\"en\""));
    assert!(html.contains("data-theme=\"dark\""));
    // The stylesheet binds classes to var(--…) semantic tokens (never a primitive in markup).
    assert!(STYLE.contains("var(--success)"));
    assert!(STYLE.contains("var(--focus-ring)"));
    // focus-ring is distinct from accent (the carve-out rule, design pass §1).
    assert!(STYLE.contains("--focus-ring:") && STYLE.contains("--accent:"));
    assert!(STYLE.contains("prefers-reduced-motion"), "reduced-motion is a first-class path");
}

#[test]
fn escape_neutralises_html_metacharacters() {
    assert_eq!(escape("<script>&\"'"), "&lt;script&gt;&amp;&quot;&#39;");
}
