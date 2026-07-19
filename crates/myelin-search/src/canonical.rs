//! **Canonical vs legacy Git-blob document identity (the rebuild's discriminator).**
//!
//! Git blob projection identities changed shape. The LEGACY identity embedded the repo, ref and
//! path as raw slash-delimited text inside the artifact id:
//!
//! ```text
//! myelin://acme/git/blob/core/refs/heads/main/src/charge.rs      ← legacy (raw, ambiguous)
//! ```
//!
//! The CANONICAL identity composes the same three logical components as ONE id segment, each
//! reversibly percent-encoded by [`SubjectComponent`] so no component can smuggle a delimiter:
//!
//! ```text
//! myelin://acme/git/blob/core:refs%2Fheads%2Fmain:src%2Fcharge%2Ers   ← canonical
//! ```
//!
//! The legacy form is ambiguous by construction — `core/refs/heads/main/src/charge.rs` cannot be
//! split back into `(repo, ref, path)` without guessing, and a repo named `refs` or a ref containing
//! a slash collides outright. That ambiguity is why the identity moved.
//!
//! ## Why this module exists
//!
//! Documents, vectors and metadata written under the legacy identity SURVIVE the code change: the
//! index is keyed by the id string, so a legacy doc is simply a doc the new writer never addresses.
//! A canonical re-index therefore ADDS a second document for the same blob rather than replacing
//! the first, and a delete or restriction of the canonical id leaves the legacy twin queryable.
//! [`crate::rebuild`] fixes that by rebuilding the index from owner truth; this module is the
//! predicate that decides what "no legacy identity survived" MEANS, and the verification gate reads
//! it before reads reopen.
//!
//! ## The predicate is STRICT, and deliberately so
//!
//! [`is_canonical_blob_id`] admits an id only if every component round-trips
//! [`SubjectComponent::parse`] — which rejects lowercase escapes, over-escaped safe literals,
//! malformed escapes and invalid UTF-8 rather than normalizing them. Anything else is legacy. The
//! asymmetry is intentional: a false "canonical" verdict lets a stale doc survive the rebuild
//! silently, while a false "legacy" verdict is caught loudly by the verification gate. We take the
//! loud failure.
//!
//! ## PII posture
//!
//! Every function here takes an id and returns a BOOLEAN or a count. Nothing in this module
//! formats, logs, or returns an id, a path, a repo name or a tenant — the verification gate reports
//! *how many* legacy identities survived, never *which*. A legacy git blob id contains the raw
//! repository name and file path, so echoing one into an error or a `Debug` render would disclose
//! exactly what §12 of the migration forbids.

use myelin_events::SubjectComponent;

/// The URN scheme every artifact ref carries.
const SCHEME: &str = "myelin://";

/// The subsystem token Git's code projection indexes under.
pub const GIT_SUBSYSTEM: &str = "git";

/// The artifact type of a single indexed blob.
pub const GIT_BLOB_TYPE: &str = "blob";

/// The delimiter separating the three logical components of a canonical blob id
/// (`<repo>:<ref>:<path>`). It is safe as a delimiter precisely because
/// [`SubjectComponent::encode`] escapes it (`:` → `%3A`) inside any component.
pub const BLOB_COMPONENT_DELIMITER: char = ':';

/// The number of logical components a canonical blob id composes: repo, ref, path.
pub const BLOB_COMPONENT_COUNT: usize = 3;

/// Split an artifact ref into `(subsystem, type, id)`, ignoring any `#sub` anchor.
///
/// Returns `None` if the ref is not a `myelin://<tenant>/<subsystem>/<type>/<id>` URN. The tenant
/// segment is parsed but deliberately DISCARDED — nothing downstream of this module needs it, and
/// not returning it means no caller can accidentally log it.
fn split_ref(ref_: &str) -> Option<(&str, &str, &str)> {
    let rest = ref_.strip_prefix(SCHEME)?;
    // The id may itself contain no `/` in canonical form, but a LEGACY id contains several — so
    // split off exactly the first three segments and treat the whole remainder as the id.
    let mut segs = rest.splitn(4, '/');
    let _tenant = segs.next()?;
    let subsystem = segs.next()?;
    let type_ = segs.next()?;
    let id = segs.next()?;
    if subsystem.is_empty() || type_.is_empty() || id.is_empty() {
        return None;
    }
    // A sub-artifact anchor (`#L12-L40`) is not part of the identity grammar this module judges.
    let id = id.split('#').next().unwrap_or(id);
    if id.is_empty() {
        return None;
    }
    Some((subsystem, type_, id))
}

/// Is `ref_` a Git blob projection identity at all (canonical or legacy)?
///
/// Used to scope the rebuild's legacy sweep: a `knowledge.page` or `chat.message` doc is neither
/// canonical-blob nor legacy-blob, and must not be counted as either.
pub fn is_git_blob_id(ref_: &str) -> bool {
    matches!(
        split_ref(ref_),
        Some((GIT_SUBSYSTEM, GIT_BLOB_TYPE, _))
    )
}

/// **Is `ref_` a CANONICAL Git blob identity?**
///
/// True iff the id segment is exactly [`BLOB_COMPONENT_COUNT`] delimiter-separated components and
/// each one is a strictly canonical [`SubjectComponent`]. Strict by design: an id that merely
/// *looks* encoded (a lowercase `%2f`, an over-escaped `-`) is NOT canonical, because the producer
/// that writes canonical ids would never emit it — so it is a legacy or hand-forged identity.
///
/// A non-blob ref returns `false`; ask [`is_git_blob_id`] first if you need to tell "not canonical"
/// from "not a blob".
pub fn is_canonical_blob_id(ref_: &str) -> bool {
    let Some((GIT_SUBSYSTEM, GIT_BLOB_TYPE, id)) = split_ref(ref_) else {
        return false;
    };
    let components: Vec<&str> = id.split(BLOB_COMPONENT_DELIMITER).collect();
    if components.len() != BLOB_COMPONENT_COUNT {
        return false;
    }
    components
        .iter()
        .all(|c| SubjectComponent::parse(c).is_ok())
}

/// **Is `ref_` a LEGACY Git blob identity — one the canonical writer can never produce?**
///
/// A Git blob id that is not canonical. This is the predicate the rebuild's verification gate
/// counts: after a successful rebuild the answer must be `false` for every document, vector and
/// metadata id in the `(tenant, region)` index.
///
/// Note the deliberate framing — legacy is defined as the COMPLEMENT of canonical within the blob
/// namespace, not as a positive pattern match on slashes. Defining it positively ("contains a raw
/// `/`") would miss the identities that motivated the cutover in the first place: an id whose
/// components were never encoded but happen to contain no slash, e.g. a repo/ref/path triple joined
/// with raw colons where the path itself contains a colon.
pub fn is_legacy_blob_id(ref_: &str) -> bool {
    is_git_blob_id(ref_) && !is_canonical_blob_id(ref_)
}

/// Count the legacy Git blob identities in `ids` (the verification gate's zero-legacy leg).
///
/// Returns a COUNT, never the offending ids — a legacy blob id embeds the raw repository name and
/// file path, which the migration's disclosure rule (§12) forbids surfacing.
pub fn count_legacy_blob_ids<'a, I>(ids: I) -> usize
where
    I: IntoIterator<Item = &'a str>,
{
    ids.into_iter().filter(|id| is_legacy_blob_id(id)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canonical id as the emitter actually builds it (`myelin-git`'s `blob_ref`): three
    /// `SubjectComponent`-encoded components joined by `:`.
    fn canonical(repo: &str, ref_name: &str, path: &str) -> String {
        format!(
            "myelin://acme/git/blob/{}:{}:{}",
            SubjectComponent::encode(repo).unwrap().as_str(),
            SubjectComponent::encode(ref_name).unwrap().as_str(),
            SubjectComponent::encode(path).unwrap().as_str(),
        )
    }

    #[test]
    fn the_emitters_canonical_form_is_recognised_as_canonical() {
        let id = canonical("core", "refs/heads/main", "src/charge.rs");
        assert_eq!(
            id, "myelin://acme/git/blob/core:refs%2Fheads%2Fmain:src%2Fcharge%2Ers",
            "the canonical shape is pinned — a change here is an identity cutover, not a refactor"
        );
        assert!(is_canonical_blob_id(&id));
        assert!(!is_legacy_blob_id(&id));
        assert!(is_git_blob_id(&id));
    }

    /// **The legacy shape — the raw slash-delimited identity this whole migration exists to
    /// retire.** It is a git blob id, and it is legacy.
    #[test]
    fn the_raw_slash_delimited_legacy_form_is_legacy() {
        let id = "myelin://acme/git/blob/core/refs/heads/main/src/charge.rs";
        assert!(is_git_blob_id(id), "it IS a blob identity");
        assert!(!is_canonical_blob_id(id));
        assert!(is_legacy_blob_id(id));
    }

    /// **A legacy id with no slash is still legacy.** The predicate is the complement of canonical,
    /// not a slash hunt — a raw `repo:ref:path` triple whose components were never encoded (here the
    /// path contains a `.`, which canonical encoding escapes to `%2E`) does not round-trip.
    #[test]
    fn an_unencoded_colon_triple_is_legacy_even_without_a_slash() {
        let id = "myelin://acme/git/blob/core:main:charge.rs";
        assert!(is_legacy_blob_id(id), "`charge.rs` is not canonically encoded");
    }

    /// **Near-miss encodings are legacy, not canonical.** Lowercase escapes and over-escaping are
    /// rejected by `SubjectComponent::parse` rather than normalized — a producer emitting canonical
    /// ids never writes them, so admitting them would let a stale doc survive the rebuild silently.
    #[test]
    fn near_miss_encodings_are_legacy() {
        // lowercase hex escape.
        assert!(is_legacy_blob_id(
            "myelin://acme/git/blob/core:refs%2fheads%2fmain:a"
        ));
        // over-escaped safe literal (`-` is `-`, never `%2D`).
        assert!(is_legacy_blob_id("myelin://acme/git/blob/my%2Drepo:main:a"));
        // truncated escape.
        assert!(is_legacy_blob_id("myelin://acme/git/blob/core:main:a%2"));
    }

    /// **The component COUNT is load-bearing.** Two components (a pre-cutover two-part id) or four
    /// (a path whose raw colon was never escaped, splitting the id) are both legacy.
    #[test]
    fn wrong_component_count_is_legacy() {
        assert!(is_legacy_blob_id("myelin://acme/git/blob/core:main"));
        assert!(is_legacy_blob_id("myelin://acme/git/blob/core:main:a:b"));
    }

    /// **Non-blob corpora are neither canonical nor legacy blobs.** The rebuild replays issues,
    /// knowledge and chat too; miscounting one of their docs as a surviving legacy blob would fail a
    /// correct rebuild's verification gate.
    #[test]
    fn unrelated_corpora_are_not_blob_identities() {
        for id in [
            "myelin://acme/knowledge/page/home",
            "myelin://acme/issues/issue/1421",
            "myelin://acme/chat/message/m-7",
            "myelin://acme/git/pr/42",
        ] {
            assert!(!is_git_blob_id(id), "{id} is not a blob identity");
            assert!(!is_legacy_blob_id(id), "{id} must not count as legacy");
            assert!(!is_canonical_blob_id(id));
        }
    }

    /// **A `#sub` anchor does not change the identity's verdict.** Sub-artifact blob docs
    /// (`#L12-L40`) are keyed sub-precisely but carry the same underlying identity grammar.
    #[test]
    fn a_sub_anchor_does_not_change_the_verdict() {
        let base = canonical("core", "main", "src/a.rs");
        assert!(is_canonical_blob_id(&format!("{base}#L12-L40")));
        assert!(is_legacy_blob_id(
            "myelin://acme/git/blob/core/main/src/a.rs#L12-L40"
        ));
    }

    /// Malformed / non-URN strings are not blob identities (no panic, no false positive).
    #[test]
    fn malformed_refs_are_not_blob_identities() {
        for id in [
            "",
            "myelin://",
            "myelin://acme",
            "myelin://acme/git",
            "myelin://acme/git/blob",
            "myelin://acme/git/blob/",
            "https://example.test/git/blob/a:b:c",
            "acme/git/blob/a:b:c",
        ] {
            assert!(!is_git_blob_id(id), "{id:?}");
            assert!(!is_legacy_blob_id(id), "{id:?}");
            assert!(!is_canonical_blob_id(id), "{id:?}");
        }
    }

    /// **The gate's counting leg reports a NUMBER, over a mixed corpus.** One legacy blob among
    /// canonical blobs and unrelated corpora counts exactly once.
    #[test]
    fn the_legacy_count_is_exact_over_a_mixed_corpus() {
        let canon = canonical("core", "main", "src/a.rs");
        let ids = vec![
            canon.as_str(),
            "myelin://acme/git/blob/core/main/src/a.rs",
            "myelin://acme/knowledge/page/home",
            "myelin://acme/chat/message/m-1",
        ];
        assert_eq!(count_legacy_blob_ids(ids), 1);
        assert_eq!(count_legacy_blob_ids(Vec::<&str>::new()), 0);
    }
}
