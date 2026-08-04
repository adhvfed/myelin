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
        summary: HumanisedRef {
            template_key: "ci.check.ok".into(),
            args: BTreeMap::new(),
        },
        started_at: crate::check_status::Timestamp("2026-06-22T00:00:00Z".into()),
        completed_at: None,
        cost_settled: true,
    };
    CheckStatusRow::from_fact(&fact)
}

#[test]
fn status_is_never_colour_alone_every_check_state_has_glyph_and_label() {
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
        assert!(
            !cue.glyph.is_empty(),
            "state {state:?} has an empty glyph (colour-only)"
        );
        assert!(
            !cue.label.is_empty(),
            "state {state:?} has an empty label (colour-only)"
        );
    }
}

#[test]
fn error_and_cancelled_are_distinct_from_failure() {
    let failure = StatusCue::for_check_state(CheckState::Failure);
    let error = StatusCue::for_check_state(CheckState::Error);
    let cancelled = StatusCue::for_check_state(CheckState::Cancelled);
    assert_eq!(failure.token, StatusToken::Danger);
    assert_eq!(
        error.token,
        StatusToken::Warning,
        "error must be distinct (warning) from failure"
    );
    assert_ne!(error.glyph, failure.glyph);
    assert_ne!(cancelled.glyph, failure.glyph);
    assert_eq!(cancelled.token, StatusToken::Warning);
}

#[test]
fn fork_badge_appears_only_for_unendorsed_fork_success() {
    let trusted = row(CheckState::Success, TrustTier::Trusted, "build");
    assert!(
        ForkTrustBadge::for_row(&trusted, true, false).is_none(),
        "trusted success: no badge"
    );

    let fork = row(CheckState::Success, TrustTier::UntrustedFork, "build");
    assert!(
        ForkTrustBadge::for_row(&fork, true, false).is_some(),
        "un-endorsed fork success MUST show the neutral-until-trusted badge"
    );
    assert!(
        ForkTrustBadge::for_row(&fork, true, true).is_none(),
        "endorsed fork success: badge clears (counts toward the gate)"
    );
}

#[test]
fn fork_trust_action_is_absent_for_a_viewer_without_the_capability() {
    let fork = row(CheckState::Success, TrustTier::UntrustedFork, "build");

    let with_cap = ForkTrustBadge::for_row(&fork, true, false)
        .unwrap()
        .render();
    assert!(
        with_cap.contains("Trust this run"),
        "a maintainer sees the trust action"
    );

    let without_cap = ForkTrustBadge::for_row(&fork, false, false)
        .unwrap()
        .render();
    assert!(
        !without_cap.contains("Trust this run"),
        "a viewer without approve_untrusted_ci must NOT see the trust action (no leaked affordance)"
    );
    assert!(without_cap.contains("untrusted fork") || without_cap.contains("FORK run"));
}

#[test]
fn checks_panel_renders_humanised_summary_not_a_raw_ci_string() {
    let r = row(CheckState::Failure, TrustTier::Trusted, "test");
    let view = CheckRowView::from_row(&r, "3 tests failed", true, false, false);
    let html = view.render();
    assert!(
        html.contains("3 tests failed"),
        "the humanised summary renders"
    );
    assert!(html.contains("ci/test"), "the context renders");
    assert!(html.contains("required"));
    assert!(
        html.contains("failed"),
        "the failure label renders (never colour alone)"
    );
}

#[test]
fn checks_panel_covers_empty_loading_error_states() {
    let empty = ChecksPanel::Empty.render();
    assert!(empty.contains("No checks configured"));

    let loading = ChecksPanel::Loading { skeleton_rows: 3 }.render();
    assert_eq!(
        loading.matches("skeleton-row").count(),
        3,
        "skeleton matches the final layout"
    );
    assert!(
        loading.contains("aria-busy=\"true\""),
        "skeleton sets aria-busy (DESIGN-MANUAL §6)"
    );
    assert!(
        loading.contains("aria-live=\"polite\""),
        "one polite live region announces loading"
    );
    assert!(!loading.to_lowercase().contains("spinner"));

    let error = ChecksPanel::Error.render();
    assert!(error.contains("Checks unavailable"));
    assert!(
        error.contains("Retry"),
        "error offers a scoped retry path, never a dead end"
    );
}

#[test]
fn merge_readiness_names_which_gate_is_unmet_never_a_bare_blocked() {
    let outcome = MergeGateOutcome::Blocked {
        unmet: vec![
            UnmetContext {
                context: CheckContext::ci("test"),
                reason: UnmetReason::NotGreen {
                    state: CheckState::Failure,
                },
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
    assert!(
        html.contains("awaiting fork trust"),
        "humanises the fork-neutral reason"
    );
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
    use crate::project::{Tombstone, TombstoneReason};
    let page = PrOverviewPage {
        projected: Projected::Tombstoned(Tombstone {
            reason: TombstoneReason::Unauthorized,
        }),
        pr_state: PrState::Open,
        checks: ChecksPanel::Empty,
        merge: MergeReadiness::Queued { position: 1 },
    };
    let html = page.render();
    assert!(
        html.contains("not available to you"),
        "dignified permission state"
    );
    assert!(
        !html.contains("pr-title"),
        "no title element for a tombstone"
    );
    assert!(
        !html.contains("checks-panel"),
        "no checks surface leaked for a tombstone"
    );
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
                reason: UnmetReason::NotGreen {
                    state: CheckState::Failure,
                },
            }],
        },
    };
    let html = page.render();
    assert!(
        html.contains("Fix the receive-pack CAS"),
        "title renders for a visible projection"
    );
    assert!(html.contains("pr-state-pill"));
    assert!(html.contains("checks-panel"));
    assert!(html.contains("merge-readiness"));
    assert!(html.contains("1/2 approvals"));
}

#[test]
fn web_edit_refuses_a_stale_base_no_silent_overwrite() {
    let committed = WebEditOutcome::evaluate("base-oid", "base-oid", "new-oid", true);
    assert_eq!(
        committed,
        WebEditOutcome::Committed {
            new_oid: "new-oid".into()
        }
    );

    let stale = WebEditOutcome::evaluate("base-oid", "moved-oid", "new-oid", true);
    assert_eq!(
        stale,
        WebEditOutcome::StaleBase {
            current_oid: "moved-oid".into()
        },
        "stale base refuses"
    );

    let denied = WebEditOutcome::evaluate("base-oid", "base-oid", "new-oid", false);
    assert_eq!(
        denied,
        WebEditOutcome::Denied,
        "no write permission is denied"
    );
}

#[test]
fn web_edit_form_omits_composer_for_a_read_only_viewer() {
    let editable = WebEditForm {
        path: "src/lib.rs".into(),
        contents: "fn main() {}".into(),
        base_oid: "base".into(),
        viewer_may_edit: true,
    };
    let ro = WebEditForm {
        viewer_may_edit: false,
        ..editable.clone()
    };
    assert!(
        editable.render().contains("Commit change"),
        "an editor sees the composer"
    );
    assert!(
        !ro.render().contains("Commit change"),
        "a read-only viewer: composer ABSENT, not greyed"
    );
    assert!(
        ro.render().contains("fn main()"),
        "the file still renders read-only"
    );
}

#[test]
fn rendered_markup_carries_no_inline_interactive_colour() {
    let r = row(CheckState::Success, TrustTier::UntrustedFork, "build");
    let badge = ForkTrustBadge::for_row(&r, true, false).unwrap().render();
    let checks = ChecksPanel::Live {
        rows: vec![CheckRowView::from_row(&r, "ok", true, true, false)],
    }
    .render();
    for html in [badge, checks] {
        assert!(
            !html.contains("style=\"color"),
            "no inline colour on rendered elements"
        );
        assert!(
            !html.contains("style='color"),
            "no inline colour on rendered elements"
        );
    }
}

#[test]
fn page_shell_is_well_formed_and_uses_semantic_tokens() {
    let html = page("PR #1", &ChecksPanel::Empty.render());
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("lang=\"en\""));
    assert!(html.contains("data-theme=\"dark\""));
    assert!(STYLE.contains("var(--success)"));
    assert!(STYLE.contains("var(--focus-ring)"));
    assert!(STYLE.contains("--focus-ring:") && STYLE.contains("--accent:"));
    assert!(
        STYLE.contains("prefers-reduced-motion"),
        "reduced-motion is a first-class path"
    );
}

#[test]
fn escape_neutralises_html_metacharacters() {
    assert_eq!(escape("<script>&\"'"), "&lt;script&gt;&amp;&quot;&#39;");
}

#[test]
fn repository_list_rows_have_exact_lightweight_json_shapes() {
    assert_eq!(
        RepoListRow::populated("acme/myelin", "/acme/eu-north/myelin.git")
            .unwrap()
            .to_json(),
        serde_json::json!({
            "state": "populated",
            "slug": "acme/myelin",
            "clone_url": "/acme/eu-north/myelin.git",
        })
    );
    assert_eq!(
        RepoListRow::empty("acme/empty").unwrap().to_json(),
        serde_json::json!({ "state": "empty", "slug": "acme/empty" })
    );
    assert_eq!(
        RepoListRow::restricted().to_json(),
        serde_json::json!({ "state": "restricted" })
    );
}

#[test]
fn repository_list_rows_reject_unsafe_or_oversized_fields() {
    for slug in [
        "",
        "/repo",
        "acme/",
        "acme/../repo",
        "acme/re po",
        "acme/repo\\escape",
    ] {
        assert_eq!(RepoListRow::empty(slug), Err(RepoListRowError::InvalidSlug));
    }
    assert!(RepoListRow::empty("a".repeat(REPO_LIST_ROW_MAX_SLUG_BYTES)).is_ok());
    assert_eq!(
        RepoListRow::empty("a".repeat(REPO_LIST_ROW_MAX_SLUG_BYTES + 1)),
        Err(RepoListRowError::InvalidSlug)
    );

    let exact_url = format!("/{}", "a".repeat(REPO_LIST_ROW_MAX_CLONE_URL_BYTES - 1));
    assert!(RepoListRow::populated("acme/repo", exact_url).is_ok());
    for clone_url in [
        String::new(),
        "https://git.example/repo with space.git".into(),
        "https://git.example/repo.git\nsecret".into(),
        "x".repeat(REPO_LIST_ROW_MAX_CLONE_URL_BYTES + 1),
    ] {
        assert_eq!(
            RepoListRow::populated("acme/repo", clone_url),
            Err(RepoListRowError::InvalidCloneUrl)
        );
    }
}

#[test]
fn repository_list_cursor_codec_is_canonical_bounded_and_round_trips() {
    let cursor = RepoListCursor::new([7; 32], "alpha").unwrap();
    let encoded = cursor.encode();
    assert!(encoded.starts_with(REPO_LIST_CURSOR_PREFIX));
    assert!(!encoded.contains('='), "the base64url token is unpadded");
    assert_eq!(RepoListCursor::parse(&encoded).unwrap(), cursor);

    for malformed in [
        "rl1_".to_string(),
        "rl1_not-base64!".to_string(),
        format!("{encoded}="),
        RepoListCursor::new([7; 32], "alpha").unwrap().encode() + "%",
        format!("rl1_{}", "a".repeat(REPO_LIST_CURSOR_MAX_BYTES)),
    ] {
        assert_eq!(RepoListCursor::parse(&malformed), Err(RepoListCursorError));
    }
    for slug in ["", ".", "..", "a/b", "white space"] {
        assert_eq!(RepoListCursor::new([0; 32], slug), Err(RepoListCursorError));
    }
}

#[test]
fn pr_commit_cursor_codec_is_canonical_bounded_and_round_trips() {
    fn mutate_cursor(cursor: &str, mutation: impl FnOnce(&mut Vec<u8>)) -> String {
        let encoded = cursor.strip_prefix(PR_COMMIT_CURSOR_PREFIX).unwrap();
        let mut frame = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();
        mutation(&mut frame);
        format!(
            "{PR_COMMIT_CURSOR_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
        )
    }

    let base = "11".repeat(20);
    let head = "ab".repeat(20);
    for cursor in [
        PrCommitCursor::new([5; 32], Some(&base), &head, 37).unwrap(),
        PrCommitCursor::new([6; 32], None, &head, 1).unwrap(),
    ] {
        let encoded = cursor.encode();
        assert!(encoded.starts_with(PR_COMMIT_CURSOR_PREFIX));
        assert!(encoded.len() <= PR_COMMIT_CURSOR_MAX_BYTES);
        assert!(!encoded.contains('='), "the base64url token is unpadded");
        assert_eq!(PrCommitCursor::parse(&encoded).unwrap(), cursor);
    }

    assert_eq!(
        PrCommitCursor::new([0; 32], Some(&base), &head.to_uppercase(), 1),
        Err(PrCommitCursorError)
    );
    for position in [0, PR_COMMIT_CURSOR_MAX_POSITION + 1] {
        assert_eq!(
            PrCommitCursor::new([0; 32], None, &head, position),
            Err(PrCommitCursorError)
        );
    }

    let encoded = PrCommitCursor::new([7; 32], Some(&base), &head, 2)
        .unwrap()
        .encode();
    let absent_base = PrCommitCursor::new([7; 32], None, &head, 2)
        .unwrap()
        .encode();
    let wrong_version = mutate_cursor(&encoded, |frame| frame[0] = 2);
    let wrong_length = mutate_cursor(&encoded, |frame| {
        frame.pop();
    });
    let invalid_absent_base_sentinel = mutate_cursor(&absent_base, |frame| frame[34] = 1);
    let overflow = mutate_cursor(&encoded, |frame| {
        frame[74..78].copy_from_slice(&u32::MAX.to_be_bytes());
    });
    for malformed in [
        "pc1_".to_string(),
        "pc1_not-base64!".to_string(),
        format!("{encoded}="),
        format!("pc1_{}", "a".repeat(PR_COMMIT_CURSOR_MAX_BYTES)),
        encoded.replacen("pc1_", "pc2_", 1),
        wrong_version,
        wrong_length,
        invalid_absent_base_sentinel,
        overflow,
    ] {
        assert_eq!(PrCommitCursor::parse(&malformed), Err(PrCommitCursorError));
    }
}

#[test]
fn commit_row_and_diff_json_carry_the_browse_contract() {
    let row = CommitRow {
        oid: "0123456789abcdef0123".into(),
        summary: "feat: land the browse surface".into(),
        author: "u_dev@acme.noreply".into(),
        committed_at: 1_700_000_000,
        parents: vec!["aaaa".into()],
    };
    let j = row.to_json();
    assert_eq!(j["oid"], "0123456789abcdef0123");
    assert_eq!(j["short_oid"], "0123456789ab");
    assert_eq!(j["author"], "u_dev@acme.noreply");
    assert_eq!(j["committed_at"], 1_700_000_000i64);
    assert_eq!(j["parents"][0], "aaaa");

    let diff = CommitDiff {
        commit: row,
        message: "feat: land the browse surface\n\nbody".into(),
        files: vec![DiffFile {
            path: "README.md".into(),
            old_path: None,
            status: 'M',
            lines: vec![
                DiffLineView { origin: ' ', content: "context".into() },
                DiffLineView { origin: '+', content: "added".into() },
                DiffLineView { origin: '-', content: "removed".into() },
            ],
        }],
    };
    let dj = diff.to_json();
    assert_eq!(dj["short_oid"], "0123456789ab");
    assert_eq!(dj["files"][0]["path"], "README.md");
    assert_eq!(dj["files"][0]["status"], "M");
    assert_eq!(dj["files"][0]["lines"][1]["origin"], "+");
    assert_eq!(dj["files"][0]["lines"][1]["content"], "added");
}
