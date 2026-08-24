use myelin_git::anchor::{line_range_of, resolve, AnchorState, DiffSide, LineAnchor, LineRange};
use myelin_git::subs::mint_blob_line_range;
use myelin_refs::{strip_sub, ArtifactRef, Sub};

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
    myelin_refs::parse("myelin://acme/git/pr/repo7:42").unwrap()
}

fn git_sub_resolve(
    minted: &ArtifactRef,
    anchor: &LineAnchor,
    new_blob: &[u8],
    new_oid: &str,
) -> AnchorState {
    let range_from_urn = line_range_of(minted).expect("a minted #L<a>-L<b> carries a line range");
    assert_eq!(
        range_from_urn, anchor.range,
        "the minted URN and the anchor agree on the range"
    );
    resolve(anchor, new_blob, new_oid, &pr_root()).state
}

#[test]
fn cdc_5_7_git_resolver_returns_exactly_one_of_the_four_frozen_states() {
    let pre = blob(&[
        "// module preamble",
        "use std::io;",
        "use std::fmt;",
        "fn a() {",
        "    step_one();",
        "    step_two();",
        "}",
        "",
        "fn doomed() {",
        "    gone();",
        "}",
    ]);
    let pre_oid = oid("pre");
    let commit = oid("commit");

    {
        let minted =
            mint_blob_line_range("acme", "repo7", "refs/heads/main", "src/a.rs", 5, 6).unwrap();
        let anchor = LineAnchor::mint(
            &pre,
            "src/a.rs",
            DiffSide::New,
            LineRange::new(5, 6),
            &pre_oid,
            &commit,
        )
        .unwrap();
        assert_eq!(
            git_sub_resolve(&minted, &anchor, &pre, &pre_oid),
            AnchorState::Live
        );
    }

    {
        let minted =
            mint_blob_line_range("acme", "repo7", "refs/heads/main", "src/a.rs", 5, 6).unwrap();
        let anchor = LineAnchor::mint(
            &pre,
            "src/a.rs",
            DiffSide::New,
            LineRange::new(5, 6),
            &pre_oid,
            &commit,
        )
        .unwrap();
        let moved = blob(&[
            "// header",
            "// header2",
            "// header3",
            "// header4",
            "// module preamble",
            "use std::io;",
            "use std::fmt;",
            "fn a() {",
            "    step_one();",
            "    step_two();",
            "}",
            "",
            "fn doomed() {",
            "    gone();",
            "}",
        ]);
        assert_eq!(
            git_sub_resolve(&minted, &anchor, &moved, &oid("moved")),
            AnchorState::Moved
        );
    }

    {
        let minted =
            mint_blob_line_range("acme", "repo7", "refs/heads/main", "src/a.rs", 5, 6).unwrap();
        let anchor = LineAnchor::mint(
            &pre,
            "src/a.rs",
            DiffSide::New,
            LineRange::new(5, 6),
            &pre_oid,
            &commit,
        )
        .unwrap();
        let outdated = blob(&[
            "// module preamble",
            "use std::io;",
            "use std::fmt;",
            "fn a_renamed() {",
            "    step_one();",
            "    NEW_LINE();",
            "    extra();",
            "}",
        ]);
        assert_eq!(
            git_sub_resolve(&minted, &anchor, &outdated, &oid("outdated")),
            AnchorState::Outdated
        );
    }

    {
        let minted =
            mint_blob_line_range("acme", "repo7", "refs/heads/main", "src/a.rs", 9, 10).unwrap();
        let anchor = LineAnchor::mint(
            &pre,
            "src/a.rs",
            DiffSide::New,
            LineRange::new(9, 10),
            &pre_oid,
            &commit,
        )
        .unwrap();
        let gone = blob(&[
            "// module preamble",
            "use std::io;",
            "use std::fmt;",
            "fn a() {",
            "    step_one();",
            "    step_two();",
            "}",
        ]);
        assert_eq!(
            git_sub_resolve(&minted, &anchor, &gone, &oid("gone")),
            AnchorState::Gone
        );
    }
}

#[test]
fn cdc_5_7_resolver_seam_is_line_range_only_and_strips_to_a_git_blob_root() {
    let minted =
        mint_blob_line_range("acme", "repo7", "refs/heads/main", "src/lib.rs", 42, 88).unwrap();
    let root = strip_sub(&minted);
    assert_eq!(
        myelin_refs::format(&root),
        "myelin://acme/git/blob/repo7:refs%2Fheads%2Fmain:src%2Flib%2Ers"
    );
    assert!(!myelin_refs::format(&root).contains('#'));
    let comment = myelin_refs::mint(&pr_root(), Sub::Comment("c1".into())).unwrap();
    assert_eq!(line_range_of(&comment), None);
}
