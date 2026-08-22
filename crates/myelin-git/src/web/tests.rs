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
fn fork_trust_capability_is_explicit_in_the_view_model() {
    let fork = row(CheckState::Success, TrustTier::UntrustedFork, "build");
    assert_eq!(
        ForkTrustBadge::for_row(&fork, true, false)
            .unwrap()
            .to_json(),
        serde_json::json!({ "viewer_may_endorse": true })
    );
    assert_eq!(
        ForkTrustBadge::for_row(&fork, false, false)
            .unwrap()
            .to_json(),
        serde_json::json!({ "viewer_may_endorse": false })
    );
}

#[test]
fn check_rows_expose_humanised_status_and_context() {
    let r = row(CheckState::Failure, TrustTier::Trusted, "test");
    assert_eq!(
        CheckRowView::from_row(&r, "3 tests failed", true, false, false).to_json(),
        serde_json::json!({
            "context": "ci/test",
            "cue": { "token": "danger", "glyph": "✗", "label": "failed" },
            "required": true,
            "summary": "3 tests failed",
            "fork_badge": null,
        })
    );
}

#[test]
fn checks_panel_states_have_exact_transport_shapes() {
    assert_eq!(
        ChecksPanel::Empty.to_json(),
        serde_json::json!({ "state": "empty" })
    );
    assert_eq!(
        ChecksPanel::Loading { skeleton_rows: 3 }.to_json(),
        serde_json::json!({ "state": "loading", "skeleton_rows": 3 })
    );
    assert_eq!(
        ChecksPanel::Error.to_json(),
        serde_json::json!({ "state": "error" })
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
    assert_eq!(
        MergeReadiness::from_gate(&outcome, (0, 2)).to_json(),
        serde_json::json!({
            "state": "blocked",
            "unmet": [
                { "context": "test", "reason": "test failed" },
                { "context": "e2e", "reason": "e2e awaiting fork trust" },
            ],
        })
    );
}

#[test]
fn merge_readiness_ready_carries_the_approval_count() {
    assert_eq!(
        MergeReadiness::from_gate(&MergeGateOutcome::Admitted, (2, 2)).to_json(),
        serde_json::json!({
            "state": "ready",
            "approvals": { "current": 2, "required": 2 },
        })
    );
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
    assert_eq!(
        page.to_json(),
        serde_json::json!({ "visible": false, "restricted": true })
    );
}

#[test]
fn pr_overview_visible_carries_title_state_checks_and_merge() {
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
    let json = page.to_json();
    assert_eq!(json["visible"], true);
    assert_eq!(json["title"], "Fix the receive-pack CAS");
    assert_eq!(json["pr_state"], "open");
    assert_eq!(json["render_hint"]["approvals"]["current"], 1);
    assert_eq!(json["checks"]["rows"][0]["summary"], "3 tests failed");
    assert_eq!(json["merge"]["unmet"][0]["reason"], "test failed");
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
}

#[test]
fn repository_list_rows_reject_unsafe_or_oversized_fields() {
    for slug in [
        "",
        "/repo",
        "acme/",
        "acme/../repo",
        "acme.git/repo",
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
    let cursor =
        RepoListCursor::catalogued([7; 32], "01J00000000000000000000000", "platform/alpha")
            .unwrap();
    let encoded = cursor.encode();
    assert!(encoded.starts_with(REPO_LIST_CURSOR_PREFIX));
    assert!(!encoded.contains('='), "the base64url token is unpadded");
    assert_eq!(RepoListCursor::parse(&encoded).unwrap(), cursor);
    let legacy = RepoListCursor::legacy([7; 32], "platform/legacy").unwrap();
    assert_eq!(RepoListCursor::parse(&legacy.encode()).unwrap(), legacy);

    for malformed in [
        "rl2_".to_string(),
        "rl2_not-base64!".to_string(),
        format!("{encoded}="),
        RepoListCursor::legacy([7; 32], "alpha").unwrap().encode() + "%",
        format!("rl2_{}", "a".repeat(REPO_LIST_CURSOR_MAX_BYTES)),
    ] {
        assert_eq!(RepoListCursor::parse(&malformed), Err(RepoListCursorError));
    }
    for slug in ["", ".", "..", "platform.git/api", "white space"] {
        assert_eq!(
            RepoListCursor::legacy([0; 32], slug),
            Err(RepoListCursorError)
        );
    }
    for key in ["", "white space", "punctuation!"] {
        assert_eq!(
            RepoListCursor::catalogued([0; 32], key, "alpha"),
            Err(RepoListCursorError)
        );
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
                DiffLineView {
                    origin: ' ',
                    content: "context".into(),
                },
                DiffLineView {
                    origin: '+',
                    content: "added".into(),
                },
                DiffLineView {
                    origin: '-',
                    content: "removed".into(),
                },
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
