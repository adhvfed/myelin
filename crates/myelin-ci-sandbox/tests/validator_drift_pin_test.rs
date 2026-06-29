//! # Validator drift-pinning (CT-006b 4b) — the sandbox replica MUST mirror the git canon
//!
//! `myelin-ci-sandbox` REPLICATES (does not import) git's path-confinement validators
//! [`myelin_git::gix_backend::validate_path_segment`] / [`validate_repo_slug`] as
//! [`myelin_ci_sandbox::validate_wire_segment`] / [`validate_wire_repo_slug`] — the replication is
//! deliberate (the CI sandbox must carry NO production edge to the git crate; X-1 acyclic: CI emits,
//! Git reads — `myelin-git` is a DEV-dep here only). The replica is the GT-001 cross-tenant isolation
//! boundary; if it ever DRIFTED from the canon (accepted a hostile locator the canon rejects, or vice
//! versa) a repo path could resolve differently on the wire path than on the read/durable path — a
//! cross-tenant breakout. This COMMITTED test pins the two byte-for-byte: over a shared hostile corpus
//! the replica accepts/rejects IDENTICALLY to the canon. A future edit that drifts either side trips
//! this RED.

use myelin_ci_sandbox::{validate_wire_repo_slug, validate_wire_segment};
use myelin_git::gix_backend::{validate_path_segment, validate_repo_slug};

/// The shared hostile + benign corpus the two validators are pinned over. Every traversal / separator /
/// NUL / control / absolute / namespacing vector the GT-001 boundary must agree on, plus benign names.
const SEGMENT_CORPUS: &[&str] = &[
    // benign
    "acme",
    "fr-par",
    "widgets",
    "Team-App_1.0",
    "ULID01ABCDEF",
    "a",
    ".hidden",
    "v1.2.3",
    "trailing-dash-",
    "_leading",
    // traversal / empty / dot
    "",
    ".",
    "..",
    "...",
    // separators
    "a/b",
    "a\\b",
    "/etc/passwd",
    "abs\\windows",
    "a//b",
    "trailing/",
    // NUL / control / whitespace
    "nul\0byte",
    "tab\tted",
    "new\nline",
    "space here",
    // non-allowlist punctuation
    "a:b",
    "a;b",
    "a*b",
    "a$b",
    "a%b",
    "a@b",
    "a#b",
    "a~b",
    "a'b",
    "a\"b",
    "café",      // non-ascii
    "emoji😀",   // non-ascii
];

/// A separate slug corpus (slugs MAY be `/`-namespaced; the per-piece rules then apply).
const SLUG_CORPUS: &[&str] = &[
    "widgets",
    "team/app",
    "a/b/c",
    "../../victim/fr-par/secret",
    "team/../escape",
    "/leading/slash",
    "trailing/slash/",
    "double//slash",
    "back\\slash",
    "nul\0slug",
    "團隊/app",
    "ok-1/ok_2/v1.0",
    ".",
    "..",
    "",
];

#[test]
fn wire_segment_validator_mirrors_the_git_canon_byte_for_byte() {
    for &seg in SEGMENT_CORPUS {
        let canon_ok = validate_path_segment("repo", seg).is_ok();
        let replica_ok = validate_wire_segment("repo", seg).is_ok();
        assert_eq!(
            canon_ok, replica_ok,
            "DRIFT: segment {seg:?} — git canon accept={canon_ok}, sandbox replica accept={replica_ok}. \
             The replica MUST mirror myelin_git::gix_backend::validate_path_segment exactly (the GT-001 \
             cross-tenant boundary). Re-sync the replica in gvisor.rs — do NOT weaken the test."
        );
    }
}

#[test]
fn wire_repo_slug_validator_mirrors_the_git_canon_byte_for_byte() {
    for &slug in SLUG_CORPUS {
        let canon = validate_repo_slug(slug);
        let replica = validate_wire_repo_slug(slug);
        assert_eq!(
            canon.is_ok(),
            replica.is_ok(),
            "DRIFT: slug {slug:?} — git canon accept={}, sandbox replica accept={}. The replica MUST \
             mirror myelin_git::gix_backend::validate_repo_slug exactly. Re-sync gvisor.rs.",
            canon.is_ok(),
            replica.is_ok()
        );
        // When both accept, the decomposed pieces MUST be identical (same on-disk path resolution).
        if let (Ok(c), Ok(r)) = (canon, replica) {
            assert_eq!(
                c, r,
                "DRIFT: slug {slug:?} accepted by both but decomposed to different pieces \
                 (canon={c:?}, replica={r:?}) — the resolved on-disk path would differ across the \
                 wire vs read/durable paths."
            );
        }
    }
}

/// A meta-guard: the corpus actually contains BOTH accepted and rejected inputs (so a vacuously-green
/// "both always reject" bug can't hide). Pins that the validators are really exercised on each side.
#[test]
fn corpus_exercises_both_accept_and_reject_on_each_validator() {
    let seg_accepts = SEGMENT_CORPUS
        .iter()
        .filter(|s| validate_wire_segment("repo", s).is_ok())
        .count();
    let seg_rejects = SEGMENT_CORPUS.len() - seg_accepts;
    assert!(seg_accepts > 0 && seg_rejects > 0, "segment corpus must exercise both arms");

    let slug_accepts = SLUG_CORPUS
        .iter()
        .filter(|s| validate_wire_repo_slug(s).is_ok())
        .count();
    let slug_rejects = SLUG_CORPUS.len() - slug_accepts;
    assert!(slug_accepts > 0 && slug_rejects > 0, "slug corpus must exercise both arms");
}
