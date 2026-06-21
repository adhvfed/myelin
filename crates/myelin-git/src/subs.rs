//! # `subs` — git's `#sub` mints registered with Refs (GIT-P4 / P-230, contract 5.7)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md`
//! §2 (the `#sub` mints git owns + the canonical root grammar) and `00-overview.md` §0.1 Δ7 (the
//! `ArtifactRef` id grammar — `pr/<repo>:<n>`, `commit/<repo>:<sha>` are the stored canonical roots,
//! never a render-time display form, REF-3). **Reconciliation:**
//! `05-refined-shared-systems-architecture/00-reconciliation-decisions.md` X-4 (the unified `#sub`
//! grammar + the one resolution ladder, frozen). **Contract:** `contract-index.md` row 5.7 — the
//! unified `#sub` grammar; **git owns the `comment-` / `thread-` / `L<a>-L<b>` mints** (Refs owns the
//! grammar + the ladder).
//!
//! ## The seam this module pins (git mints; Refs owns the grammar)
//! Refs ([`myelin_refs`]) owns ONE `#sub` URN grammar + ONE resolution ladder (recon X-4). Each
//! subsystem owns the **stable opaque mint** of its declared kinds. This module is git's side:
//!
//! - [`register_git_sub_kinds`] — git's REGISTRATION with Refs: it DECLARES that git owns (mints +
//!   will resolve) the [`SubKind::Comment`] / [`SubKind::Thread`] / [`SubKind::LineRange`] kinds. The
//!   registration is validated by Refs ([`SubKindRegistration::validate`]) and ACCEPTED (the GATE).
//!   This is the deliverable of GIT-P4: the kinds are declared to Refs.
//! - [`mint_pr_comment`] / [`mint_pr_thread`] / [`mint_blob_line_range`] — git's typed `#sub` mints.
//!   They build git's **canonical root** (`git/pr/<repo>:<n>`, `git/blob/<repo>:<ref>:<path>`) and
//!   attach the stable opaque sub-id through the one Refs codec ([`myelin_refs::mint`]), so every
//!   minted ref is grammatical **by construction** (0 ungrammatical — `mint` re-parses through the
//!   frozen grammar and rejects a malformed opaque body LOUDLY). Refs stores both the full sub-URN
//!   AND the [`myelin_refs::strip_sub`] root.
//!
//! ## The canonical roots (architecture §2 / Δ7) — the stored mintable key, never a display form
//! The `<id>` segment git mints is the **stable canonical key git owns** — the PR number, the commit
//! sha, the repo id — composed `<repo>:<n>` / `<repo>:<sha>` / `<repo>:<ref>:<path>` (a `:`-joined
//! key, NOT a `/`-segment, so it is one URN `<id>` token). It is NEVER a render-time display form
//! (`#42`, a 7-char short sha) — those are projections (REF-3, §4.8), rejected by [`myelin_refs`].
//!
//! ## FLOORS named (EI-01 §1 — this is the REGISTRATION + the mints, NOT the resolvers)
//! Only the kind REGISTRATION + the grammatical mints ship here. The per-kind `project(ref, viewer)`
//! sub-anchor resolvers the Refs ladder calls are named follow-ons:
//! - **`comment-` / `thread-` resolvers** → **GIT-P18** (the inline + review-thread resolver: live /
//!   moved-by-edit / resolved-thread states).
//! - **the `L<a>-L<b>` 4-state resolver** → **GIT-P24** (the content-anchored
//!   exact/rebased/partial/tombstone resolver — the BLAKE3 fingerprint + 3-way context match, X-4).
//!
//! So this module is the contract-5.7 mint half (git-owned), not the working resolution ladder.

use myelin_refs::{mint, ArtifactRef, ParseError, Sub, SubKind, SubKindRegistration};

/// The canonical Bus §6.2 subsystem token git owns (§2 — `myelin://<tenant>/git/<type>/<id>`).
pub const GIT_SUBSYSTEM: &str = "git";

/// The `#sub` kinds git is the mint + resolver owner of (contract 5.7 / architecture §2): a PR review
/// comment (`comment-`), a review-thread root (`thread-`, shared kind with Chat, OQ-L), and a
/// content-anchored file line-range (`L<a>-L<b>`). The `check-`/`step-` kinds belong to CI, NOT git
/// (architecture §2 — git only RENDERS a CI `details_ref`, it never mints those).
pub const GIT_OWNED_SUB_KINDS: &[SubKind] =
    &[SubKind::Comment, SubKind::Thread, SubKind::LineRange];

/// Git's registration of its owned `#sub` kinds WITH Refs (contract 5.7, the deliverable of GIT-P4 /
/// P-230). Returns the [`SubKindRegistration`] that Refs **accepts** (validated against the frozen
/// grammar + the Bus token table). This DECLARES the kinds git mints; it does NOT install a resolver
/// (the resolvers are the named follow-ons GIT-P18 / GIT-P24).
///
/// # Errors
/// Returns a [`myelin_refs::RegistrationError`] if the registration is not accepted — by construction
/// it always is (the subsystem token is canonical, the kinds are a non-empty, duplicate-free subset
/// of the frozen vocabulary); the fallible signature is the honest contract surface (Refs is the
/// authority that accepts, git does not get to assert acceptance).
pub fn register_git_sub_kinds() -> Result<SubKindRegistration, myelin_refs::RegistrationError> {
    SubKindRegistration {
        subsystem: GIT_SUBSYSTEM.to_string(),
        kinds: GIT_OWNED_SUB_KINDS.to_vec(),
    }
    .validate()
}

/// Build git's canonical **PR root** `myelin://<tenant>/git/pr/<repo>:<n>` (architecture §2 / Δ7 —
/// the `<repo>:<n>` stable mintable key, NOT the display `#n`). The PR number is git's stable
/// canonical key; this is the root a `comment-`/`thread-` sub attaches to.
fn pr_root(tenant: &str, repo: &str, pr_number: u64) -> Result<ArtifactRef, ParseError> {
    myelin_refs::parse(&format!("myelin://{tenant}/git/pr/{repo}:{pr_number}"))
}

/// Build git's canonical **blob root** `myelin://<tenant>/git/blob/<repo>:<ref>:<path>` (architecture
/// §2 — the content-addressed file root a `L<a>-L<b>` line-range sub attaches to). `<ref>` is the
/// branch/commit the range was anchored against at mint time (the fingerprint's blob-oid pin is the
/// GIT-P24 resolver's, not the URN's).
///
/// **DOCUMENTED DEVIATION (EI-01 §1 — the frozen contract shape vs reality).** The architecture §2
/// writes the blob id `<repo>:<ref>:<path>` with a RAW `<path>` (e.g. `src/lib.rs`), but the frozen
/// Refs URN grammar (REF-3, `parse.rs`) admits **no `/` inside any segment** — a `/` is a scope
/// delimiter. The two cannot both hold literally. We reconcile WITHOUT weakening either: the `<path>`
/// is **percent-encoded** (`/` → `%2F`) so the composed `<id>` is a single grammatical segment. This
/// is a pure id-encoding decision local to git's blob mint (git owns the `<id>` minting); the parent
/// PR/commit/repo roots are unaffected (they carry no path). The GIT-P24 resolver decodes it back to
/// the on-disk path. Escalation note: if a later prompt prefers a different blob-id encoding, it is a
/// git-local change (Refs' grammar is untouched). Logged in the GIT-P4 report.
fn blob_root(tenant: &str, repo: &str, git_ref: &str, path: &str) -> Result<ArtifactRef, ParseError> {
    let encoded_path = encode_path_segment(path);
    myelin_refs::parse(&format!(
        "myelin://{tenant}/git/blob/{repo}:{git_ref}:{encoded_path}"
    ))
}

/// Percent-encode the two characters that would break the single-segment URN `<id>` grammar (REF-3):
/// `/` (a scope delimiter) → `%2F`, and a literal `%` → `%25` (so the encoding is reversible). All
/// other path bytes pass through (a git path is otherwise URN-segment-safe). Minimal by design — git
/// owns this id encoding; the resolver ([`decode_path_segment`], GIT-P24) reverses it.
pub fn encode_path_segment(path: &str) -> String {
    path.replace('%', "%25").replace('/', "%2F")
}

/// Reverse [`encode_path_segment`] — decode the `<path>` portion of a git blob id back to the on-disk
/// path. `%2F` → `/`, `%25` → `%` (decoded in that order so a literal `%2F` in the source round-trips
/// — `%25` is decoded LAST). The GIT-P24 line-range resolver decodes the path before opening the blob.
pub fn decode_path_segment(encoded: &str) -> String {
    encoded.replace("%2F", "/").replace("%25", "%")
}

/// Mint a **PR review-comment** sub-URN `…/git/pr/<repo>:<n>#comment-<comment_id>` (contract 5.7,
/// `comment-` kind). `comment_id` is git's **stable opaque** comment id (the stability obligation is
/// git's, §3.5 — the id does not change when the comment is edited). The result is grammatical by
/// construction (it round-trips the frozen grammar); an empty `comment_id` is rejected LOUDLY.
///
/// **FLOOR:** this MINTS the stable ref; the `project(ref, viewer)` resolver (live / moved-by-edit /
/// resolved) is the GIT-P18 follow-on.
pub fn mint_pr_comment(
    tenant: &str,
    repo: &str,
    pr_number: u64,
    comment_id: &str,
) -> Result<ArtifactRef, ParseError> {
    let root = pr_root(tenant, repo, pr_number)?;
    mint(&root, Sub::Comment(comment_id.to_string()))
}

/// Mint a **review-thread root** sub-URN `…/git/pr/<repo>:<n>#thread-<thread_id>` (contract 5.7,
/// `thread-` kind — the kind shared with Chat, OQ-L). `thread_id` is git's stable opaque thread id.
///
/// **FLOOR:** the `project(ref, viewer)` resolver (live / resolved-thread) is the GIT-P18 follow-on.
pub fn mint_pr_thread(
    tenant: &str,
    repo: &str,
    pr_number: u64,
    thread_id: &str,
) -> Result<ArtifactRef, ParseError> {
    let root = pr_root(tenant, repo, pr_number)?;
    mint(&root, Sub::Thread(thread_id.to_string()))
}

/// Mint a **content-anchored line-range** sub-URN `…/git/blob/<repo>:<ref>:<path>#L<a>-L<b>`
/// (contract 5.7, `L<a>-L<b>` kind — the OQ-D content-anchored range). `start`/`end` are 1-based,
/// `end >= start` (an inverted range is rejected LOUDLY by the grammar). The mint pins the *position*
/// at mint time; the content fingerprint (BLAKE3 + context window + blob oid) and the
/// exact/rebased/partial/tombstone resolution is the GIT-P24 resolver's, not the URN's.
///
/// **FLOOR:** the `L<a>-L<b>` 4-state content-anchored resolver is the GIT-P24 follow-on.
pub fn mint_blob_line_range(
    tenant: &str,
    repo: &str,
    git_ref: &str,
    path: &str,
    start: u64,
    end: u64,
) -> Result<ArtifactRef, ParseError> {
    let root = blob_root(tenant, repo, git_ref, path)?;
    mint(&root, Sub::LineRange { start, end })
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_refs::{strip_sub, sub_kind};

    /// The registration is ACCEPTED by Refs (the GIT-P4 GATE) and declares exactly the three kinds
    /// git owns — comment-/thread-/L<a>-L<b> — and NO foreign (CI-owned) kind.
    #[test]
    fn git_sub_kind_registration_is_accepted_and_declares_only_git_owned_kinds() {
        let reg = register_git_sub_kinds().expect("Refs must accept git's #sub registration");
        assert_eq!(reg.subsystem, "git");
        assert_eq!(
            reg.kinds,
            vec![SubKind::Comment, SubKind::Thread, SubKind::LineRange]
        );
        // git does NOT register the CI-owned check-/step- kinds (architecture §2).
        assert!(!reg.kinds.contains(&SubKind::Check));
        assert!(!reg.kinds.contains(&SubKind::Step));
    }

    /// Every git mint produces a GRAMMATICAL sub-URN (0 ungrammatical): it round-trips the frozen
    /// grammar, classifies to the right [`SubKind`], and its `strip_sub` root is git's canonical root.
    #[test]
    fn git_mints_produce_grammatical_round_tripping_sub_urns() {
        // comment- on a PR root
        let c = mint_pr_comment("acme-eu", "repo7", 4291, "cAbc123").unwrap();
        assert_eq!(
            myelin_refs::format(&c),
            "myelin://acme-eu/git/pr/repo7:4291#comment-cAbc123"
        );
        assert_eq!(sub_kind(&c).map(|s| s.kind()), Some(SubKind::Comment));
        assert_eq!(
            myelin_refs::format(&strip_sub(&c)),
            "myelin://acme-eu/git/pr/repo7:4291"
        );

        // thread- on a PR root
        let t = mint_pr_thread("acme-eu", "repo7", 4291, "tXyz").unwrap();
        assert_eq!(
            myelin_refs::format(&t),
            "myelin://acme-eu/git/pr/repo7:4291#thread-tXyz"
        );
        assert_eq!(sub_kind(&t).map(|s| s.kind()), Some(SubKind::Thread));

        // L<a>-L<b> on a blob root — the `<path>` is percent-encoded (the documented deviation: a
        // raw `/` would break the single-segment URN grammar, REF-3).
        let l = mint_blob_line_range("acme-eu", "repo7", "main", "src/lib.rs", 42, 88).unwrap();
        assert_eq!(
            myelin_refs::format(&l),
            "myelin://acme-eu/git/blob/repo7:main:src%2Flib.rs#L42-L88"
        );
        assert_eq!(sub_kind(&l).map(|s| s.kind()), Some(SubKind::LineRange));
        assert_eq!(
            myelin_refs::format(&strip_sub(&l)),
            "myelin://acme-eu/git/blob/repo7:main:src%2Flib.rs"
        );
        // The encoding is reversible: `%2F` decodes back to `/` (the on-disk path GIT-P24 resolves).
        assert_eq!(decode_path_segment("src%2Flib.rs"), "src/lib.rs");
        assert_eq!(decode_path_segment("a%25b%2Fc"), "a%b/c");
    }

    /// A single-line range (`start == end`) is admitted; an INVERTED range is rejected LOUDLY by the
    /// grammar (the mint cannot emit an ungrammatical ref).
    #[test]
    fn line_range_endpoints_are_grammar_checked_at_mint_time() {
        assert!(mint_blob_line_range("acme", "r", "main", "f.rs", 7, 7).is_ok());
        assert!(matches!(
            mint_blob_line_range("acme", "r", "main", "f.rs", 88, 42),
            Err(ParseError::UnknownSubKind { .. })
        ));
    }

    /// An empty opaque comment / thread id is rejected LOUDLY (the stable-id obligation is git's, but
    /// the GRAMMAR refuses an empty body — a malformed mint never reaches Refs as a sub-URN).
    #[test]
    fn empty_opaque_id_is_rejected_at_mint_time() {
        assert!(matches!(
            mint_pr_comment("acme", "r", 1, ""),
            Err(ParseError::UnknownSubKind { .. })
        ));
        assert!(matches!(
            mint_pr_thread("acme", "r", 1, ""),
            Err(ParseError::UnknownSubKind { .. })
        ));
    }
}
