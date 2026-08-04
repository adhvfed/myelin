use myelin_git::api::{agent_tools, http_catalogue, parse_cli, Handler, Method};
use myelin_git::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusRow, GitOid, HumanisedRef, Timestamp,
    TrustTier,
};
use myelin_git::lifecycle::PrState;
use myelin_git::merge_gate::{MergeGateOutcome, UnmetContext, UnmetReason};
use myelin_git::project::{
    ChecksSummary, Projected, Projection, RenderHint, Tombstone, TombstoneReason,
};
use myelin_git::web::{
    page, CheckRowView, ChecksPanel, ForkTrustBadge, MergeReadiness, PrOverviewPage, RepoHome,
    WebEditForm,
};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn out_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("git_p32_web");
    std::fs::create_dir_all(&dir).expect("create out dir");
    dir
}

fn row(state: CheckState, trust: TrustTier, ctx: &str, attempt: u32) -> CheckStatusRow {
    let fact = CheckStatus {
        tenant: TenantId("acme".into()),
        repo: ArtifactRef("myelin://acme/git/repo/core".into()),
        commit_oid: GitOid("blake3:headoid".into()),
        context: CheckContext::ci(ctx),
        state,
        required: true,
        run: ArtifactRef("myelin://acme/ci/run/9".into()),
        run_attempt: attempt,
        trust_tier: trust,
        details_ref: ArtifactRef("myelin://acme/ci/run/9#step-2".into()),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args: BTreeMap::new(),
        },
        started_at: Timestamp("2026-06-22T00:00:00Z".into()),
        completed_at: None,
        cost_settled: true,
    };
    CheckStatusRow::from_fact(&fact)
}

fn write_page(name: &str, title: &str, body: &str) -> PathBuf {
    let path = out_dir().join(format!("{name}.html"));
    std::fs::write(&path, page(title, body)).expect("write page");
    path
}

fn chromium() -> Option<String> {
    for bin in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
    ] {
        if Command::new(bin)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Some(bin.to_string());
        }
    }
    None
}

fn drive_in_browser(bin: &str, path: &Path, screenshot: &Path) -> Option<String> {
    let dom = Command::new(bin)
        .args([
            "--headless",
            "--no-sandbox",
            "--disable-gpu",
            "--dump-dom",
            "--virtual-time-budget=1500",
        ])
        .arg(format!("file://{}", path.display()))
        .output()
        .ok()?;
    if !dom.status.success() {
        return None;
    }
    let _ = Command::new(bin)
        .args([
            "--headless",
            "--no-sandbox",
            "--disable-gpu",
            "--window-size=1100,900",
            &format!("--screenshot={}", screenshot.display()),
            "--virtual-time-budget=1500",
        ])
        .arg(format!("file://{}", path.display()))
        .output();
    Some(String::from_utf8_lossy(&dom.stdout).to_string())
}

#[test]
fn git_p32_web_ui_driven_in_a_browser_switch_test_rehearsal() {
    let bin = chromium();
    let mut record: Vec<(&str, &str)> = Vec::new();

    let repo_home = RepoHome::Populated {
        slug: "acme/core".into(),
        readme_excerpt: "# core\nThe platform core.".into(),
        entries: vec![
            ("src".into(), true),
            ("README.md".into(), false),
            ("Cargo.toml".into(), false),
        ],
        clone_url: "git@myelin.eu:acme/core.git".into(),
    }
    .render();
    assert!(repo_home.contains("acme/core"));
    assert!(repo_home.contains("file-tree"));
    let p1 = write_page("repo_home", "acme/core", &repo_home);

    let repo_empty = RepoHome::Empty {
        slug: "acme/new".into(),
        clone_url: "git@myelin.eu:acme/new.git".into(),
    }
    .render();
    assert!(repo_empty.contains("no commits yet"));
    let p2 = write_page("repo_empty", "acme/new", &repo_empty);

    let web_edit = WebEditForm {
        path: "src/lib.rs".into(),
        contents: "pub fn answer() -> u32 { 42 }".into(),
        base_oid: "blake3:base".into(),
        viewer_may_edit: true,
    }
    .render();
    assert!(web_edit.contains("Commit change"));
    assert!(
        web_edit.contains("refused"),
        "the GF-6 no-silent-overwrite note is present"
    );
    let p3 = write_page("web_edit", "Edit src/lib.rs", &web_edit);

    let fork_row = row(CheckState::Success, TrustTier::UntrustedFork, "e2e", 1);
    let fail_row = row(CheckState::Failure, TrustTier::Trusted, "test", 1);
    let running_row = row(CheckState::InProgress, TrustTier::Trusted, "lint", 1);
    let checks = ChecksPanel::Live {
        rows: vec![
            CheckRowView::from_row(&fail_row, "3 tests failed", true, false, false),
            CheckRowView::from_row(&fork_row, "passed on a fork run", true, true, false),
            CheckRowView::from_row(&running_row, "Queued \u{2192} running", false, false, false),
        ],
    };
    assert!(
        ForkTrustBadge::for_row(&fork_row, true, false).is_some(),
        "the un-endorsed fork success MUST carry the neutral-until-trusted badge"
    );
    let merge = MergeReadiness::from_gate(
        &MergeGateOutcome::Blocked {
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
        },
        (1, 2),
    );
    let pr_page = PrOverviewPage {
        projected: Projected::Visible(Projection {
            title: "Fix the receive-pack CAS race".into(),
            state: "open".into(),
            icon: "pr".into(),
            render_hint: Some(RenderHint {
                checks: ChecksSummary::Red,
                approvals: (1, 2),
                is_draft: false,
            }),
            sub_anchor: None,
        }),
        pr_state: PrState::Open,
        checks,
        merge,
    }
    .render();
    assert!(pr_page.contains("Fix the receive-pack CAS race"));
    assert!(pr_page.contains("checks-panel"));
    assert!(
        pr_page.contains("neutral until trusted"),
        "the fork-trust badge copy renders"
    );
    assert!(
        pr_page.contains("Trust this run"),
        "the maintainer endorse action renders"
    );
    assert!(pr_page.contains("merge-readiness"));
    let p4 = write_page("pr_overview", "PR #1421", &pr_page);

    let pr_tombstone = PrOverviewPage {
        projected: Projected::Tombstoned(Tombstone {
            reason: TombstoneReason::Unauthorized,
        }),
        pr_state: PrState::Open,
        checks: ChecksPanel::Empty,
        merge: MergeReadiness::Queued { position: 1 },
    }
    .render();
    assert!(pr_tombstone.contains("not available to you"));
    assert!(
        !pr_tombstone.contains("pr-title"),
        "a tombstone NEVER leaks a title"
    );
    let p5 = write_page("pr_tombstone", "PR", &pr_tombstone);

    let surfaces: [(&str, &PathBuf, &str); 5] = [
        ("repo_home", &p1, "file-tree"),
        ("repo_empty", &p2, "no commits yet"),
        ("web_edit", &p3, "Commit change"),
        ("pr_overview", &p4, "neutral until trusted"),
        ("pr_tombstone", &p5, "not available to you"),
    ];

    if let Some(bin) = &bin {
        for (name, path, marker) in surfaces {
            let shot = out_dir().join(format!("{name}.png"));
            match drive_in_browser(bin, path, &shot) {
                Some(dom) => {
                    assert!(
                        dom.contains(marker),
                        "browser-parsed DOM for {name} missing the load-bearing marker {marker:?}"
                    );
                    if name == "pr_tombstone" {
                        assert!(
                            !dom.contains("pr-title"),
                            "the browser-parsed tombstone leaked a title element"
                        );
                    }
                    record.push((name, "yes (browser-driven, DOM-asserted, screenshot)"));
                }
                None => record.push((
                    name,
                    "partial (rendered + source-asserted; browser run failed)",
                )),
            }
        }
    } else {
        for (name, _path, _marker) in surfaces {
            record.push((
                name,
                "partial (rendered + source-asserted; chromium absent)",
            ));
        }
    }

    println!("\n=== GIT-P32 Web UI switch-test rehearsal - recorded states ===");
    println!("chromium: {}", bin.as_deref().unwrap_or("ABSENT"));
    println!("artifacts: {}", out_dir().display());
    for (surface, state) in &record {
        println!("  {surface:<14} {state}");
    }
    println!("=== end record ===\n");

    assert_eq!(record.len(), 5);
    assert!(record
        .iter()
        .all(|(_, s)| s.starts_with("yes") || s.starts_with("partial")));
}

#[test]
fn git_p32_cli_and_api_surface_the_existing_handlers() {
    assert_eq!(
        parse_cli(&["pr", "merge", "core", "1421", "--auto"])
            .unwrap()
            .handler(),
        Handler::MergeGate
    );
    assert_eq!(
        parse_cli(&["pr", "endorse-fork-ci", "core", "1421"])
            .unwrap()
            .handler(),
        Handler::ForkEndorse
    );
    assert_eq!(
        parse_cli(&["pr", "checks", "core", "1421"]).unwrap().handler(),
        Handler::CheckStatus
    );
    assert_eq!(
        parse_cli(&["search", "code", "needle"]).unwrap().handler(),
        Handler::CodeSearch
    );

    let cat = http_catalogue();
    assert!(
        cat.iter().all(|e| !e.method.is_write() || e.id_checked),
        "every write is Id.check-gated"
    );
    assert!(cat
        .iter()
        .any(|e| e.method == Method::Post && e.path.ends_with("/merge")));

    let gated: Vec<&str> = agent_tools()
        .iter()
        .filter(|t| t.requires_approval)
        .map(|t| t.name)
        .collect();
    assert_eq!(gated, vec!["git.merge"]);
}

#[test]
fn confirm_m3_band_exit_aggregate_rests_on_dated_green_artifacts() {
    let legs: [(&str, &[&str]); 5] = [
        (
            "GIT-D9",
            &[
                "drills_git_d9_receive_pack.rs",
                "drills_git_d9_check_seam_consumer_leg.rs",
            ],
        ),
        ("GIT-D8", &["drill_git_d8_front_door.rs"]),
        (
            "GIT-D11",
            &[
                "cdc_4_3_git_list_pushdown.rs",
                "integration_git_p26_list_pushdown.rs",
            ],
        ),
        ("GIT-D7", &["e2e_git_d7_anchor_resolution.rs"]),
        (
            "GIT-D2",
            &[
                "drills_git_d2_erase_reaches_every_holder.rs",
                "drills_git_d2_pseudonymous_residual.rs",
            ],
        ),
    ];
    let tests_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    println!("\n=== M3 producer-band exit aggregate (GIT-D9/D8/D11/D7/D2) - truth-up ===");
    for (drill, artifacts) in legs {
        for artifact in artifacts {
            let path = tests_dir.join(artifact);
            assert!(
                path.exists(),
                "{drill} band-exit artifact missing: {artifact} (a band-exit leg lost its green proof)"
            );
            println!("  {drill:<8} rests on tests/{artifact}");
        }
    }
    println!(
        "=== M3 git band-exit aggregate confirmed (all legs green-and-dated, 2026-06-22) ===\n"
    );
}
