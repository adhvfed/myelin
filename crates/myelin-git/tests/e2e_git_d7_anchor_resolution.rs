use myelin_git::anchor::{resolve, AnchorState, DiffSide, LineAnchor, LineRange};
use myelin_refs::ArtifactRef;

fn blob(lines: &[&str]) -> Vec<u8> {
    lines.join("\n").into_bytes()
}

fn oid(tag: &str) -> String {
    format!(
        "blake3:{}",
        hex::encode(blake3::hash(tag.as_bytes()).as_bytes())
    )
}

fn pr_root() -> ArtifactRef {
    myelin_refs::parse("myelin://acme/git/pr/payments:4291").unwrap()
}

struct AnchorCase {
    name: &'static str,
    range: LineRange,
    expected: AnchorState,
}

fn pre_rebase() -> Vec<u8> {
    blob(&[
        "use crate::ledger;",
        "use crate::audit;",
        "",
        "fn charge(amount: u64) {",
        "    let fee = amount / 10;",
        "    ledger::debit(fee);",
        "    audit::record(fee);",
        "}",
        "",
        "fn refund(id: u64) {",
        "    ledger::credit(id);",
        "}",
        "",
        "fn legacy_helper() {",
        "    deprecated_call();",
        "}",
    ])
}

fn post_rebase() -> Vec<u8> {
    blob(&[
        "// Copyright 2026 Acme",
        "// SPDX-License-Identifier",
        "//",
        "",
        "use crate::ledger;",
        "use crate::audit;",
        "",
        "fn charge(amount: u64) {",
        "    let fee = amount / 10;",
        "    ledger::debit(fee);",
        "    audit::record(fee);",
        "}",
        "",
        "fn refund(id: u64) {",
        "    ledger::refund(id);",
        "}",
    ])
}

#[test]
fn git_d7_rebase_corpus_resolves_every_anchor_with_0_mis_anchored() {
    let pre = pre_rebase();
    let post = post_rebase();
    let pre_oid = oid("pre-rebase-blob");
    let post_oid = oid("post-rebase-blob");
    let commit = oid("pre-rebase-commit");

    let cases = vec![
        AnchorCase {
            name: "charge-body",
            range: LineRange::new(5, 7),
            expected: AnchorState::Moved,
        },
        AnchorCase {
            name: "imports-top-of-file",
            range: LineRange::new(1, 2),
            expected: AnchorState::Outdated,
        },
        AnchorCase {
            name: "refund-edited-line",
            range: LineRange::new(11, 11),
            expected: AnchorState::Gone,
        },
        AnchorCase {
            name: "refund-block-partial",
            range: LineRange::new(10, 11),
            expected: AnchorState::Outdated,
        },
        AnchorCase {
            name: "legacy-gone",
            range: LineRange::new(14, 16),
            expected: AnchorState::Gone,
        },
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

        if resolution.state != case.expected {
            eprintln!(
                "MIS-ANCHORED `{}`: expected {:?}, got {:?} (resolved_range {:?})",
                case.name, case.expected, resolution.state, resolution.resolved_range
            );
            mis_anchored += 1;
        }
        *distribution.entry(resolution.state.token()).or_default() += 1;

        match resolution.state {
            AnchorState::Live => {
                assert!(
                    resolution.original_context().is_none(),
                    "`{}`: a LIVE anchor needs no original-context affordance",
                    case.name
                );
            }
            _ => {
                let ctx = resolution.original_context().unwrap_or_else(|| {
                    panic!(
                        "`{}`: a non-LIVE anchor MUST render original context",
                        case.name
                    )
                });
                assert_eq!(
                    ctx.range, case.range,
                    "`{}`: original range preserved",
                    case.name
                );
                assert_eq!(
                    ctx.blob_oid, pre_oid,
                    "`{}`: original blob linked",
                    case.name
                );
                assert_eq!(
                    ctx.commit_oid, commit,
                    "`{}`: original commit linked",
                    case.name
                );
            }
        }

        if resolution.state == AnchorState::Gone {
            assert!(
                resolution.tombstone.is_some(),
                "`{}`: a GONE anchor must carry a content_gone tombstone",
                case.name
            );
            assert!(
                resolution.resolved_range.is_none(),
                "`{}`: GONE has no new-blob range",
                case.name
            );
        }
    }

    eprintln!("GIT-D7 rebase-corpus anchor-state distribution: {distribution:?}");
    eprintln!("GIT-D7 mis-anchored = {mis_anchored} / {}", cases.len());
    assert_eq!(
        mis_anchored, 0,
        "GIT-D7 GATE: 0 mis-anchored across the rebase corpus"
    );
}

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
    let r = resolve(&anchor, &pre, &pre_oid, &pr_root());
    assert_eq!(r.state, AnchorState::Live);
    assert_eq!(r.resolved_range, Some(LineRange::new(5, 7)));
    assert!(r.original_context().is_none());
}
