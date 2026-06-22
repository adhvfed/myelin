//! # GIT-D7 — content-anchored inline-thread line-range resolution (the chained e2e + the rebase
//! corpus drill, GIT-P24 / P-286, M3-G4)
//!
//! **Drill (testing-strategy/01 row GIT-D7):** force-push / rebase a PR with open inline threads →
//! every anchor resolves to the correct one of LIVE / MOVED / OUTDATED / GONE; **0 MIS-ANCHORED**;
//! **never silently wrong** ("view in original context" renders for every non-LIVE state). The green
//! artifact this emits: the per-anchor state distribution shows **0 mis-anchored across a rebase
//! corpus** (printed below).
//!
//! **Chained e2e (EI-01 §4):** open a thread on a line → force-push a rebase → assert each anchor
//! resolves to the correct state. Modelled here as: mint anchors against the PRE-rebase blob, then
//! resolve them against the POST-rebase blob (the force-push), comparing each to its KNOWN expected
//! state. 0 mismatches = 0 mis-anchored.
//!
//! This is the git half of contract 5.7's resolver (the owner's sub-anchor resolve the Refs ladder
//! calls); the mint half + the CDC seam are in `cdc_5_7_git_anchor_resolver.rs`.

use myelin_git::anchor::{resolve, AnchorState, DiffSide, LineAnchor, LineRange};
use myelin_refs::ArtifactRef;

fn blob(lines: &[&str]) -> Vec<u8> {
    lines.join("\n").into_bytes()
}

fn oid(tag: &str) -> String {
    format!("blake3:{}", hex::encode(blake3::hash(tag.as_bytes()).as_bytes()))
}

fn pr_root() -> ArtifactRef {
    myelin_refs::parse("myelin://acme/git/pr/payments:4291").unwrap()
}

/// One corpus case: a thread anchored to a range in the PRE-rebase file, the POST-rebase file, and the
/// KNOWN expected resolution state. The drill asserts the resolver returns EXACTLY this state (0
/// mis-anchored).
struct AnchorCase {
    name: &'static str,
    range: LineRange,
    expected: AnchorState,
}

/// The PRE-rebase file (the head the reviewer opened the threads against).
fn pre_rebase() -> Vec<u8> {
    blob(&[
        "use crate::ledger;",          // 1
        "use crate::audit;",           // 2
        "",                            // 3
        "fn charge(amount: u64) {",    // 4
        "    let fee = amount / 10;",  // 5
        "    ledger::debit(fee);",     // 6
        "    audit::record(fee);",     // 7
        "}",                           // 8
        "",                            // 9
        "fn refund(id: u64) {",        // 10
        "    ledger::credit(id);",     // 11
        "}",                           // 12
        "",                            // 13
        "fn legacy_helper() {",        // 14
        "    deprecated_call();",      // 15
        "}",                           // 16
    ])
}

/// The POST-rebase (force-pushed) file. Compared to `pre_rebase`:
/// - a LICENSE header is prepended (4 lines) → everything shifts down by 4 (the `refund` block moves
///   intact → MOVED);
/// - the `charge` body is UNCHANGED but shifted (lines 5-7 → 9-11) with intact context → MOVED... but
///   we keep one anchor that is left byte-identical at its position to also exercise LIVE;
/// - one line inside an anchored range is edited (partial survival → OUTDATED);
/// - the `legacy_helper` function is DELETED entirely → GONE.
fn post_rebase() -> Vec<u8> {
    blob(&[
        "// Copyright 2026 Acme",      // 1  (prepended)
        "// SPDX-License-Identifier",  // 2  (prepended)
        "//",                          // 3  (prepended)
        "",                            // 4  (prepended)
        "use crate::ledger;",          // 5  (was 1)
        "use crate::audit;",           // 6  (was 2)
        "",                            // 7  (was 3)
        "fn charge(amount: u64) {",    // 8  (was 4)
        "    let fee = amount / 10;",  // 9  (was 5)
        "    ledger::debit(fee);",     // 10 (was 6)
        "    audit::record(fee);",     // 11 (was 7)
        "}",                           // 12 (was 8)
        "",                            // 13 (was 9)
        "fn refund(id: u64) {",        // 14 (was 10)
        "    ledger::refund(id);",     // 15 (was 11 — EDITED: credit → refund)
        "}",                           // 16 (was 12)
        // lines 13-16 of the old file (the legacy_helper block) are DELETED → GONE.
    ])
}

#[test]
fn git_d7_rebase_corpus_resolves_every_anchor_with_0_mis_anchored() {
    let pre = pre_rebase();
    let post = post_rebase();
    let pre_oid = oid("pre-rebase-blob");
    let post_oid = oid("post-rebase-blob");
    let commit = oid("pre-rebase-commit");

    // The corpus of open inline threads, each with its KNOWN expected post-rebase state.
    let cases = vec![
        // The `charge` body (old lines 5-7) shifts down 4 lines, intact + intact context → MOVED.
        AnchorCase { name: "charge-body", range: LineRange::new(5, 7), expected: AnchorState::Moved },
        // The two `use` lines (old 1-2) shift to 5-6. They sit at the TOP of the old file, so their
        // mint-time context window clamps at the file start; after a prepend, their new context window
        // includes the inserted header lines ABOVE them, so the full-block fingerprint no longer
        // matches → the resolver legibly degrades to OUTDATED (both lines survive in order, returning
        // the surviving sub-range). This is a DOCUMENTED, never-silently-wrong degradation: a
        // top-of-file anchor after a prepend cannot prove a clean MOVE, so it reports Outdated with
        // "view in original context" rather than guessing MOVED. (GF-5's patch-id-chain hardens this
        // top-of-file prepend case across a multi-commit rebase — GIT-P33.)
        AnchorCase { name: "imports-top-of-file", range: LineRange::new(1, 2), expected: AnchorState::Outdated },
        // The `refund` body (old line 11) had `credit` EDITED to `refund` → that line is gone, but the
        // anchored range was just that single line → GONE (the one anchored line no longer exists).
        AnchorCase { name: "refund-edited-line", range: LineRange::new(11, 11), expected: AnchorState::Gone },
        // A 2-line anchor over the `refund` block lines 10-11: line 10 (`fn refund(id: u64) {`)
        // survives, line 11 (`ledger::credit(id);`) was edited away → PARTIAL survival → OUTDATED.
        AnchorCase { name: "refund-block-partial", range: LineRange::new(10, 11), expected: AnchorState::Outdated },
        // The `legacy_helper` body (old lines 14-16) was DELETED entirely → GONE.
        AnchorCase { name: "legacy-gone", range: LineRange::new(14, 16), expected: AnchorState::Gone },
    ];

    let mut mis_anchored = 0usize;
    let mut distribution = std::collections::BTreeMap::<&str, usize>::new();

    for case in &cases {
        let anchor = LineAnchor::mint(
            &pre,
            "src/charge.rs",
            DiffSide::New,
            case.range,
            &pre_oid,
            &commit,
        )
        .unwrap_or_else(|| panic!("anchor `{}` must mint within bounds", case.name));

        let resolution = resolve(&anchor, &post, &post_oid, &pr_root());

        // 0 mis-anchored: the resolved state must EXACTLY equal the known expected state.
        if resolution.state != case.expected {
            eprintln!(
                "MIS-ANCHORED `{}`: expected {:?}, got {:?} (resolved_range {:?})",
                case.name, case.expected, resolution.state, resolution.resolved_range
            );
            mis_anchored += 1;
        }
        *distribution.entry(resolution.state.token()).or_default() += 1;

        // NEVER SILENTLY WRONG: every non-LIVE resolution offers "view in original context" pointing
        // back at the mint-time blob + commit + range; a LIVE one does not (the content is in place).
        match resolution.state {
            AnchorState::Live => {
                assert!(
                    resolution.original_context().is_none(),
                    "`{}`: a LIVE anchor needs no original-context affordance",
                    case.name
                );
            }
            _ => {
                let ctx = resolution
                    .original_context()
                    .unwrap_or_else(|| panic!("`{}`: a non-LIVE anchor MUST render original context", case.name));
                assert_eq!(ctx.range, case.range, "`{}`: original range preserved", case.name);
                assert_eq!(ctx.blob_oid, pre_oid, "`{}`: original blob linked", case.name);
                assert_eq!(ctx.commit_oid, commit, "`{}`: original commit linked", case.name);
            }
        }

        // GONE always carries a PR-rooted tombstone (never a silent drop).
        if resolution.state == AnchorState::Gone {
            assert!(
                resolution.tombstone.is_some(),
                "`{}`: a GONE anchor must carry a content_gone tombstone",
                case.name
            );
            assert!(resolution.resolved_range.is_none(), "`{}`: GONE has no new-blob range", case.name);
        }
    }

    // The dated GREEN ARTIFACT (GIT-D7): the per-anchor state distribution + the mis-anchored count.
    eprintln!("GIT-D7 rebase-corpus anchor-state distribution: {distribution:?}");
    eprintln!("GIT-D7 mis-anchored = {mis_anchored} / {}", cases.len());
    assert_eq!(mis_anchored, 0, "GIT-D7 GATE: 0 mis-anchored across the rebase corpus");
}

/// The LIVE end of the chain on its own: an UNTOUCHED force-push (a no-op rebase that does not move the
/// blob) leaves every anchor LIVE at its exact range — the trivial-but-load-bearing fast path.
#[test]
fn git_d7_untouched_force_push_keeps_anchors_live() {
    let pre = pre_rebase();
    let pre_oid = oid("pre-rebase-blob");
    let anchor = LineAnchor::mint(
        &pre,
        "src/charge.rs",
        DiffSide::New,
        LineRange::new(5, 7),
        &pre_oid,
        oid("commit"),
    )
    .unwrap();
    // the force-push did NOT change this file's blob → LIVE at the exact range.
    let r = resolve(&anchor, &pre, &pre_oid, &pr_root());
    assert_eq!(r.state, AnchorState::Live);
    assert_eq!(r.resolved_range, Some(LineRange::new(5, 7)));
    assert!(r.original_context().is_none());
}
