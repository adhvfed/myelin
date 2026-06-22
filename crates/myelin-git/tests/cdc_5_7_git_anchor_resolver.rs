//! # The CDC pair for the GIT half of contract 5.7 — the content-anchored line-range RESOLVER
//! (GIT-P24 / P-286, M3-G4)
//!
//! **Contract:** `contract-index.md` row 5.7 — "git line-ranges are **content-anchored** (BLAKE3
//! fingerprint + 3-way context match → exact/rebased/partial/tombstone); git is the owner's
//! sub-anchor resolver the Refs ladder calls". **Reconciliation:** `00-reconciliation-decisions.md`
//! X-4 §"Git line-ranges" (the four states) + §"the one resolution ladder" (the resolver is step 3 —
//! the owner's sub-resolve — of the unified ladder Refs drives). Owning architecture: git
//! `02-internals-and-algorithms.md` §5.1 (the mint+resolve algorithm).
//!
//! ## The seam this pair pins (Refs owns the ladder; git OWNS the L-range sub-resolve)
//! Row 5.7's resolution ladder is: `permission → root → SUB-RESOLVE → erased`. The SUB-RESOLVE step
//! for an `L<a>-L<b>` line range is **delegated to git** (the owner of the kind). This pair pins the
//! contract between:
//! - the CONSUMER (the Refs ladder): it hands git a minted `#L<a>-L<b>` ref + the new-blob bytes and
//!   expects back exactly ONE of the four frozen states `{live/moved/outdated/gone}` — never "unknown",
//!   never a relocated range with no state (the never-silently-wrong invariant);
//! - the PROVIDER (git — [`myelin_git::anchor`]): it mints the content fingerprint and resolves
//!   through the 4-state ladder, mapping each state to the Refs ladder's `{live/moved/outdated/gone}`.
//!
//! The minted ref the consumer hands in is the GIT-P4 mint ([`myelin_git::subs::mint_blob_line_range`])
//! — so this pair also pins that the MINT and the RESOLVE agree on the `L<a>-L<b>` endpoints.

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

/// **PROVIDER side** — git resolves a minted `#L<a>-L<b>` against a new blob into one of the four
/// frozen states, threading the endpoints from the minted URN (the consumer-supplied ref) into the
/// resolver. This is the exact path the Refs ladder calls.
fn git_sub_resolve(
    minted: &ArtifactRef,
    anchor: &LineAnchor,
    new_blob: &[u8],
    new_oid: &str,
) -> AnchorState {
    // The consumer hands a minted #L<a>-L<b> ref — the resolver reads its endpoints (Refs owns the
    // grammar; git reads the parsed range) and they MUST equal the anchor's minted range.
    let range_from_urn = line_range_of(minted).expect("a minted #L<a>-L<b> carries a line range");
    assert_eq!(
        range_from_urn, anchor.range,
        "the minted URN and the anchor agree on the range"
    );
    resolve(anchor, new_blob, new_oid, &pr_root()).state
}

/// The 5.7 resolver pair, end-to-end: for each of the four frozen states, the CONSUMER mints a
/// content-anchored `#L<a>-L<b>` (the GIT-P4 mint) and the PROVIDER (git) resolves it to EXACTLY that
/// state — proving the seam returns one of `{live/moved/outdated/gone}`, never silently wrong.
#[test]
fn cdc_5_7_git_resolver_returns_exactly_one_of_the_four_frozen_states() {
    let pre = blob(&[
        "// module preamble", // 1
        "use std::io;",       // 2
        "use std::fmt;",      // 3
        "fn a() {",           // 4
        "    step_one();",    // 5
        "    step_two();",    // 6
        "}",                  // 7
        "",                   // 8
        "fn doomed() {",      // 9
        "    gone();",        // 10
        "}",                  // 11
    ]);
    let pre_oid = oid("pre");
    let commit = oid("commit");

    // ── LIVE: the blob is untouched. (anchored body = lines 5-6) ──
    {
        let minted = mint_blob_line_range("acme", "repo7", "main", "src/a.rs", 5, 6).unwrap();
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

    // ── MOVED: a block prepended above shifts the anchored body (+ its full context) intact. ──
    {
        let minted = mint_blob_line_range("acme", "repo7", "main", "src/a.rs", 5, 6).unwrap();
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
            "// header4", // prepend 4 — block + context travel intact
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

    // ── OUTDATED: one of two anchored lines survives, context perturbed. ──
    {
        let minted = mint_blob_line_range("acme", "repo7", "main", "src/a.rs", 5, 6).unwrap();
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
            "fn a_renamed() {", // context changed
            "    step_one();",  // survives (anchored line 5)
            "    NEW_LINE();",  // anchored line 6 (step_two) gone
            "    extra();",     // inserted
            "}",
        ]);
        assert_eq!(
            git_sub_resolve(&minted, &anchor, &outdated, &oid("outdated")),
            AnchorState::Outdated
        );
    }

    // ── GONE: the whole `doomed` function deleted. (anchored body = lines 9-10) ──
    {
        let minted = mint_blob_line_range("acme", "repo7", "main", "src/a.rs", 9, 10).unwrap();
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

/// The CONSUMER (Refs grammar) hands git ONLY a parseable `#L<a>-L<b>` root + sub. The negative half
/// of the seam: the minted ref strips cleanly to a git blob root, and a NON-line-range ref carries no
/// range for the resolver (git resolves L-ranges, not arbitrary subs).
#[test]
fn cdc_5_7_resolver_seam_is_line_range_only_and_strips_to_a_git_blob_root() {
    let minted = mint_blob_line_range("acme", "repo7", "main", "src/lib.rs", 42, 88).unwrap();
    // the consumer can strip the sub back to git's canonical blob root (what Refs stores alongside).
    let root = strip_sub(&minted);
    assert!(myelin_refs::format(&root).starts_with("myelin://acme/git/blob/repo7:main:"));
    assert!(!myelin_refs::format(&root).contains('#'));
    // a comment sub is NOT a line range → the resolver bridge yields None (git does not resolve it here).
    let comment = myelin_refs::mint(&pr_root(), Sub::Comment("c1".into())).unwrap();
    assert_eq!(line_range_of(&comment), None);
}
