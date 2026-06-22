//! Unit tests for the Git CLI + HTTP/RPC + agent-tool API catalogue (GIT-P32). These assert the
//! structural surface invariants:
//! - every WRITE endpoint is `Id.check`-gated (BUS-2 — the catalogue refuses an un-gated write);
//! - every CLI verb / HTTP route maps to an ALREADY-BUILT handler (no new handler);
//! - the agent-tool `requires_approval` defaults are FROZEN (`git.merge = yes`, the rest `no`);
//! - the CLI parser is total + loud (an unknown verb / missing arg is a typed error, never coerced).

use super::*;

#[test]
fn every_write_endpoint_is_id_checked_bus2() {
    // BUS-2 / the X-1 floor: a write endpoint that skips Id.check is a leak. The catalogue must carry
    // no such route.
    for ep in http_catalogue() {
        if ep.method.is_write() {
            assert!(ep.id_checked, "write endpoint {} is not Id.check-gated (BUS-2 violation)", ep.path);
        }
    }
}

#[test]
fn endpoint_constructor_refuses_an_ungated_write() {
    // The structural guard: a mutant that registers an un-gated write fails at `new`, not silently.
    assert!(
        Endpoint::new(Method::Post, "/api/git/repos/{repo}/prs/{n}/merge", Handler::MergeGate, false)
            .is_none(),
        "an un-gated write endpoint must be refused"
    );
    // A gated write + any read are admitted.
    assert!(Endpoint::new(Method::Post, "/x", Handler::Lifecycle, true).is_some());
    assert!(Endpoint::new(Method::Get, "/x", Handler::Project, false).is_some());
}

#[test]
fn the_catalogue_covers_the_arch_section_4_endpoints() {
    let cat = http_catalogue();
    let paths: Vec<&str> = cat.iter().map(|e| e.path).collect();
    // The representative arch §4 endpoints are all present.
    for expected in [
        "/api/git/repos",
        "/api/git/repos/{repo}/prs/{n}",
        "/api/git/repos/{repo}/prs/{n}/checks",
        "/api/git/repos/{repo}/prs/{n}/endorse-fork-ci",
        "/api/git/repos/{repo}/prs/{n}/merge",
        "/api/git/repos/{repo}/blob/{ref}/{path}",
        "/api/git/search/code",
    ] {
        assert!(paths.contains(&expected), "the catalogue is missing arch §4 endpoint {expected}");
    }
    // The merge route lowers to the merge gate; the checks route to the X-1 projection.
    let merge = cat.iter().find(|e| e.path.ends_with("/merge")).unwrap();
    assert_eq!(merge.handler, Handler::MergeGate);
    let checks = cat.iter().find(|e| e.path.ends_with("/checks")).unwrap();
    assert_eq!(checks.handler, Handler::CheckStatus);
    let endorse = cat.iter().find(|e| e.path.ends_with("/endorse-fork-ci")).unwrap();
    assert_eq!(endorse.handler, Handler::ForkEndorse);
}

#[test]
fn cli_parses_the_arch_section_3_2_verbs() {
    assert_eq!(parse_cli(&["repo", "list"]).unwrap(), CliCommand::RepoList);
    assert_eq!(
        parse_cli(&["repo", "view", "core"]).unwrap(),
        CliCommand::RepoView { repo: "core".into() }
    );
    assert_eq!(parse_cli(&["pr", "list"]).unwrap(), CliCommand::PrList { repo: None });
    assert_eq!(
        parse_cli(&["pr", "list", "--repo", "core"]).unwrap(),
        CliCommand::PrList { repo: Some("core".into()) }
    );
    assert_eq!(parse_cli(&["pr", "view", "42"]).unwrap(), CliCommand::PrView { number: 42 });
    assert_eq!(parse_cli(&["pr", "checks", "42"]).unwrap(), CliCommand::PrChecks { number: 42 });
    assert_eq!(
        parse_cli(&["pr", "review", "42", "--approve"]).unwrap(),
        CliCommand::PrReview { number: 42, verdict: "approve".into() }
    );
    assert_eq!(
        parse_cli(&["pr", "merge", "42", "--auto"]).unwrap(),
        CliCommand::PrMerge { number: 42, auto: true }
    );
    assert_eq!(
        parse_cli(&["pr", "endorse-fork-ci", "42"]).unwrap(),
        CliCommand::PrEndorseForkCi { number: 42 }
    );
    assert_eq!(
        parse_cli(&["search", "code", "needle", "--repo", "core"]).unwrap(),
        CliCommand::SearchCode { query: "needle".into(), repo: Some("core".into()) }
    );
}

#[test]
fn cli_each_verb_lowers_to_an_existing_handler() {
    // No new handler — every CLI verb maps to an already-built module.
    assert_eq!(CliCommand::RepoList.handler(), Handler::ListFilter);
    assert_eq!(CliCommand::PrChecks { number: 1 }.handler(), Handler::CheckStatus);
    assert_eq!(CliCommand::PrMerge { number: 1, auto: false }.handler(), Handler::MergeGate);
    assert_eq!(CliCommand::PrEndorseForkCi { number: 1 }.handler(), Handler::ForkEndorse);
    assert_eq!(
        CliCommand::SearchCode { query: "x".into(), repo: None }.handler(),
        Handler::CodeSearch
    );
}

#[test]
fn cli_write_commands_are_classified_for_the_bus2_gate() {
    assert!(CliCommand::PrMerge { number: 1, auto: false }.is_write());
    assert!(CliCommand::PrReview { number: 1, verdict: "approve".into() }.is_write());
    assert!(CliCommand::PrEndorseForkCi { number: 1 }.is_write());
    // Reads are not writes.
    assert!(!CliCommand::RepoList.is_write());
    assert!(!CliCommand::PrChecks { number: 1 }.is_write());
}

#[test]
fn cli_is_loud_on_unknown_and_missing() {
    assert_eq!(parse_cli(&[]), Err(CliParseError::Empty));
    assert!(matches!(parse_cli(&["nope"]), Err(CliParseError::Unknown { .. })));
    assert!(matches!(parse_cli(&["pr", "frobnicate"]), Err(CliParseError::Unknown { .. })));
    assert!(matches!(parse_cli(&["repo", "view"]), Err(CliParseError::MissingArg { .. })));
    assert!(matches!(parse_cli(&["pr", "view", "notanum"]), Err(CliParseError::BadArg { .. })));
    assert!(matches!(parse_cli(&["pr", "review", "1"]), Err(CliParseError::MissingArg { .. })));
}

#[test]
fn agent_tool_requires_approval_defaults_are_frozen() {
    // recon X-1 / ADR-08 frozen defaults: git.merge = yes (the ONLY consequential git gate), rest = no.
    let tools = agent_tools();
    let merge = tools.iter().find(|t| t.name == "git.merge").unwrap();
    assert!(merge.requires_approval, "git.merge MUST be HITL-gated (the only consequential git gate)");
    assert_eq!(merge.handler, Handler::MergeGate);

    let open_pr = tools.iter().find(|t| t.name == "git.open_pr").unwrap();
    assert!(!open_pr.requires_approval, "open_pr is reversible → not HITL-gated");

    // Exactly ONE consequential git gate.
    let gated: Vec<&str> = tools.iter().filter(|t| t.requires_approval).map(|t| t.name).collect();
    assert_eq!(gated, vec!["git.merge"], "git.merge is the ONLY HITL-gated git tool (§6.3)");
}
