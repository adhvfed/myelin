use myelin_ci_sandbox::{validate_wire_repo_slug, validate_wire_segment};
use myelin_git::gix_backend::validate_path_segment;
use myelin_refs::git_coordinate::RepositorySlug;

const SEGMENT_CORPUS: &[&str] = &[
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
    "",
    ".",
    "..",
    "...",
    "a/b",
    "a\\b",
    "/etc/passwd",
    "abs\\windows",
    "a//b",
    "trailing/",
    "nul\0byte",
    "tab\tted",
    "new\nline",
    "space here",
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
    "café",
    "emoji😀",
];

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
    "release.git",
    "namespace.git/repo",
    "NAMESPACE.GIT/repo",
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
            "DRIFT: segment {seg:?} - git canon accept={canon_ok}, sandbox replica accept={replica_ok}. \
             The replica MUST mirror myelin_git::gix_backend::validate_path_segment exactly (the GT-001 \
             cross-tenant boundary). Re-sync the replica in gvisor.rs - do NOT weaken the test."
        );
    }
}

#[test]
fn wire_repo_slug_adapter_preserves_the_shared_coordinate() {
    for &slug in SLUG_CORPUS {
        let canon = RepositorySlug::parse(slug)
            .map(|slug| slug.segments().map(str::to_owned).collect::<Vec<String>>());
        let replica = validate_wire_repo_slug(slug);
        assert_eq!(
            canon.is_ok(),
            replica.is_ok(),
            "wire slug {slug:?} disagrees with the shared coordinate: canonical accept={}, wire accept={}",
            canon.is_ok(),
            replica.is_ok()
        );
        if let (Ok(c), Ok(r)) = (canon, replica) {
            assert_eq!(
                c, r,
                "wire slug {slug:?} changed the shared segments (canonical={c:?}, wire={r:?})"
            );
        }
    }

    let oversized = "x".repeat(myelin_refs::git_coordinate::MAX_REPOSITORY_SLUG_BYTES + 1);
    assert!(RepositorySlug::parse(&oversized).is_err());
    assert!(validate_wire_repo_slug(&oversized).is_err());
}

#[test]
fn corpus_exercises_both_accept_and_reject_on_each_validator() {
    let seg_accepts = SEGMENT_CORPUS
        .iter()
        .filter(|s| validate_wire_segment("repo", s).is_ok())
        .count();
    let seg_rejects = SEGMENT_CORPUS.len() - seg_accepts;
    assert!(
        seg_accepts > 0 && seg_rejects > 0,
        "segment corpus must exercise both arms"
    );

    let slug_accepts = SLUG_CORPUS
        .iter()
        .filter(|s| validate_wire_repo_slug(s).is_ok())
        .count();
    let slug_rejects = SLUG_CORPUS.len() - slug_accepts;
    assert!(
        slug_accepts > 0 && slug_rejects > 0,
        "slug corpus must exercise both arms"
    );
}
