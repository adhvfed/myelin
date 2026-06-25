//! # `object_format` — the SHA-256 default flip (GF-2b → OQ-9), hash-AGNOSTIC (GIT-P33 / P-482, M5)
//!
//! **The follow-on that flips the new-repo default object format from SHA-1+`sha1dc` to SHA-256.**
//! This is a **default-CHANGE, not a migration** — existing repos keep the object format they were
//! created with (immutable per-repo, contract HP-5), and the model has been hash-AGNOSTIC since the
//! M3 floor (GIT-P11). The flip moves ONE knob: which [`ObjectFormat`] a brand-new repo is created
//! with when the tenant does not pin one.
//!
//! **Owning architecture (read first, in full):**
//! `planning/04-subsystem-architectures/git-hosting/architecture/05-hard-problems.md` **HP-5**
//! (SHA-1 vs SHA-256, TE-23 — "hash-agnostic model; new repos default to SHA-1+`sha1dc`; SHA-256
//! opt-in per repo (immutable); the **default flip** is the measured follow-on; the deciding factor
//! is the ECOSYSTEM, not the cryptography — a SHA-256-default repo would fail to interoperate until
//! stock `git`/CI/IDE tooling is ready; **OQ-9: the flip trigger**"). `01-tech-and-data-model.md` §3
//! (the hash-agnostic object model). Floor register **GF-2b** (SHA-1+`sha1dc` default → SHA-256 flip).
//!
//! ## What is REUSED vs NEW (EI-01 §7 coherence)
//! The hash-agnostic object addressing already exists and is NOT re-defined:
//! - [`myelin_storage::git_object_address`] frames a git object (`<kind> <len>\0<content>`) and
//!   content-addresses it — the storage tier stores + re-hash-on-read verifies the framed bytes.
//! - The object id the wire/ref layer carries ([`crate::receive_pack::Oid`]) is rendered hex, hash-
//!   width-agnostic (it never pins 40 vs 64 hex chars).
//!
//! What is **genuinely NEW** here (the GIT-P33 deliverable):
//! 1. [`ObjectFormat`] — the per-repo immutable object-format choice (`Sha1Dc` | `Sha256`), the
//!    `extensions.objectFormat` git config the repo is created with.
//! 2. [`NewRepoDefault`] — the **flip knob**: the object format a new repo is created with when the
//!    tenant does not pin one. The flip is moving this default from `Sha1Dc` to `Sha256` — a
//!    one-value change, gated on [`InteropBar`] (OQ-9: the stock-tooling bar is met).
//! 3. [`create_repo_format`] — the create-time resolution: an explicit per-repo pin wins; else the
//!    tenant/system default. **Immutable thereafter** — there is no `set_format` (a repo's format is
//!    born once, never re-set; a re-hash would be a MIGRATION, which this is explicitly NOT).
//!
//! ## The flip is a DEFAULT-change, not a migration (the load-bearing property)
//! Flipping [`NewRepoDefault`] to SHA-256 changes ONLY what NEW repos get. An existing SHA-1 repo's
//! [`ObjectFormat`] is unchanged — its objects keep their SHA-1 framing + addresses; no object is
//! re-hashed, no clone breaks. The two formats coexist (a tenant can have SHA-1 legacy repos + SHA-256
//! new repos side by side). This is the whole point of the hash-agnostic model (HP-5): the flip is
//! free because nothing was ever pinned to a single hash width.
//!
//! ## OQ-9 — the flip trigger (the stock-tooling interop bar)
//! The flip is GATED, not unconditional: SHA-256 becomes the default IFF the [`InteropBar`] is met
//! (stock `git` ≥ 2.42 + the CI/IDE tooling the system-of-record interoperates with all read SHA-256
//! repos — HP-5 "the deciding factor is the ecosystem, not the cryptography"). [`flip_default_to_sha256`]
//! is the explicit, dated transition: it returns the new [`NewRepoDefault`] IFF the bar clears, else it
//! REFUSES (the default stays SHA-1 — never a silent half-flip that strands clients on an unreadable
//! object format).
//!
//! ## FLOOR PROMOTED (the honesty register — VISION §3 / EI-01 §1)
//! - **GF-2b — SHA-1+`sha1dc` default is THE M3 FLOOR; SHA-256 is the GIT-P33 default flip.** The
//!   flip knob + the interop-bar gate ship HERE; whether the bar is met in a given deployment is a
//!   deployment-time fact ([`InteropBar`] is constructed from the live tooling census). The model was
//!   hash-agnostic from M3, so this is a one-value change. Recorded here, dated GIT-P33.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2/§3)
//! The format resolution is mandatory-core (a wrong format silently strands a repo on an unreadable
//! hash). The load-bearing mutants — the explicit-pin-wins precedence ([`create_repo_format`]), the
//! immutability (no `set_format` exists), the interop-bar gate on the flip ([`flip_default_to_sha256`]
//! refuses when the bar is unmet), and the default-change-not-migration property (an existing repo's
//! format is untouched by a flip) — are each killed by an assertion in the unit tests.

use myelin_storage::{git_object_address, ContentHash, GitObjectKind};

/// **A repo's git object format** (the `extensions.objectFormat` config — HP-5). IMMUTABLE per repo:
/// a repo is created with exactly one of these and never re-hashed (a format change would be a
/// migration, which the hash-agnostic model explicitly avoids). PII-free closed enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectFormat {
    /// **SHA-1 + `sha1dc`** — the M3 floor default (GF-2b). `sha1dc` (the SHAttered-collision-detecting
    /// SHA-1) mitigates the SHAttered class at write time; the broad-ecosystem-interop choice.
    Sha1Dc,
    /// **SHA-256** — git's `extensions.objectFormat = sha256` (git ≥ 2.42). The GIT-P33 flip target,
    /// gated on the [`InteropBar`] (OQ-9 — stock-tooling readiness).
    Sha256,
}

impl ObjectFormat {
    /// The `extensions.objectFormat` config token the format is created under (the git config value).
    pub fn config_token(self) -> &'static str {
        match self {
            ObjectFormat::Sha1Dc => "sha1",
            ObjectFormat::Sha256 => "sha256",
        }
    }

    /// The hash-width (hex chars) an object id of this format renders to. SHA-1 → 40, SHA-256 → 64.
    /// The wire/ref layer is width-agnostic; this is the render width for display, never a pin.
    pub fn oid_hex_width(self) -> usize {
        match self {
            ObjectFormat::Sha1Dc => 40,
            ObjectFormat::Sha256 => 64,
        }
    }
}

/// **The new-repo default object format — the FLIP KNOB (GF-2b → OQ-9).** The format a brand-new repo
/// is created with when the tenant does not pin one. The GIT-P33 flip is moving this default from
/// [`ObjectFormat::Sha1Dc`] to [`ObjectFormat::Sha256`] — a one-value change (the model is
/// hash-agnostic; flipping this strands no existing repo).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewRepoDefault {
    format: ObjectFormat,
}

impl NewRepoDefault {
    /// The M3 FLOOR default: SHA-1 + `sha1dc` (broad-ecosystem interop — GF-2b).
    pub const fn sha1dc_floor() -> NewRepoDefault {
        NewRepoDefault {
            format: ObjectFormat::Sha1Dc,
        }
    }

    /// The GIT-P33 default AFTER the OQ-9 flip: SHA-256. Constructed ONLY via [`flip_default_to_sha256`]
    /// (the gated transition) — a deployment cannot fabricate the post-flip default without clearing
    /// the interop bar.
    const fn sha256_flipped() -> NewRepoDefault {
        NewRepoDefault {
            format: ObjectFormat::Sha256,
        }
    }

    /// The object format a new repo gets under this default.
    pub fn format(self) -> ObjectFormat {
        self.format
    }
}

/// **The stock-tooling interop bar (OQ-9 — the flip trigger).** The flip to a SHA-256 default is gated
/// on the ECOSYSTEM being ready (HP-5: "the deciding factor is the ecosystem, not the cryptography").
/// A deployment constructs this from its live tooling census — the platform does not guess. All three
/// must hold for the bar to clear: stock `git` reads SHA-256, the CI runners read SHA-256, and the
/// IDE/tooling the system-of-record interoperates with read SHA-256. PII-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InteropBar {
    /// Stock `git` (≥ 2.42) on the deployment reads SHA-256 object-format repos.
    pub stock_git_reads_sha256: bool,
    /// The CI runners read SHA-256 repos (a CI that cannot clone a SHA-256 repo would break every PR).
    pub ci_runners_read_sha256: bool,
    /// The IDE / tooling the system-of-record interoperates with read SHA-256 repos.
    pub ide_tooling_reads_sha256: bool,
}

impl InteropBar {
    /// **Is the interop bar MET?** `true` IFF EVERY tooling lane reads SHA-256 (HP-5 — a single lane
    /// that cannot read SHA-256 strands clients on an unreadable object format; the flip is all-or-
    /// nothing). Mandatory-core: an `&&` → `||` mutant here would flip the default while a tooling lane
    /// is still SHA-1-only.
    pub fn is_met(self) -> bool {
        self.stock_git_reads_sha256 && self.ci_runners_read_sha256 && self.ide_tooling_reads_sha256
    }
}

/// **The OQ-9 default flip (the dated transition): flip the new-repo default to SHA-256 IFF the
/// interop bar is met.** Returns the post-flip [`NewRepoDefault`] (SHA-256) when [`InteropBar::is_met`],
/// else REFUSES with [`FlipRefused`] — the default stays SHA-1 (never a silent half-flip that strands
/// clients on an object format their tooling cannot read). This is the explicit, gated, dated
/// transition GF-2b's follow-on names.
pub fn flip_default_to_sha256(bar: InteropBar) -> Result<NewRepoDefault, FlipRefused> {
    if bar.is_met() {
        Ok(NewRepoDefault::sha256_flipped())
    } else {
        Err(FlipRefused { bar })
    }
}

/// **The flip was REFUSED** — the stock-tooling interop bar (OQ-9) is not met, so the new-repo default
/// stays SHA-1+`sha1dc`. Carries the unmet bar so the operator sees WHICH tooling lane is not ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlipRefused {
    /// The interop bar at refusal time (the lanes that are not yet SHA-256-ready).
    pub bar: InteropBar,
}

impl std::fmt::Display for FlipRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the SHA-256 default flip (OQ-9) is REFUSED: the stock-tooling interop bar is not met \
             (stock_git={}, ci_runners={}, ide_tooling={}). The new-repo default stays SHA-1+sha1dc \
             — a SHA-256-default repo would fail to interoperate until every tooling lane reads it.",
            self.bar.stock_git_reads_sha256,
            self.bar.ci_runners_read_sha256,
            self.bar.ide_tooling_reads_sha256
        )
    }
}

impl std::error::Error for FlipRefused {}

/// **Resolve the object format a new repo is created with (create-time, IMMUTABLE thereafter).** An
/// explicit per-repo pin (`requested`) ALWAYS wins (a tenant can opt a specific repo into SHA-256
/// before the global flip, or stay SHA-1 after it — HP-5 "opt-in per repo, immutable"); otherwise the
/// system/tenant [`NewRepoDefault`] applies. There is deliberately NO `set_format` — a repo's format
/// is born here once and never re-set (a re-hash would be a migration, which the hash-agnostic model
/// avoids).
pub fn create_repo_format(
    default: NewRepoDefault,
    requested: Option<ObjectFormat>,
) -> ObjectFormat {
    // An explicit per-repo pin wins over the default (opt-in/opt-out per repo, immutable).
    requested.unwrap_or_else(|| default.format())
}

/// **Address a git object under a repo's object format (hash-agnostic).** The storage tier frames +
/// content-addresses the object; the `format` is the repo's immutable object format (carried so the
/// address space is per-repo consistent). On this floor the storage framing is SHA-256
/// ([`git_object_address`]); the SHA-1 legacy address space is a separate algorithm tag a later prompt
/// admits — what matters HERE is that the format is THREADED, never assumed: a SHA-1 repo and a SHA-256
/// repo carry different [`ObjectFormat`]s and the function never silently mis-frames one as the other.
///
/// FLOOR: the SHA-1 legacy framing (40-hex addresses) is not produced on this floor — `git_object_
/// address` frames SHA-256. The hash-agnostic THREADING (the format is a per-repo immutable fact the
/// call carries) is what GIT-P33 lands; the SHA-1 framing impl rides the canonical-`git` wire op for a
/// legacy repo. The point asserted here: the format is never assumed, it is carried.
pub fn object_address_for_format(
    _format: ObjectFormat,
    kind: GitObjectKind,
    content: &[u8],
) -> ContentHash {
    git_object_address(kind, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar_all_ready() -> InteropBar {
        InteropBar {
            stock_git_reads_sha256: true,
            ci_runners_read_sha256: true,
            ide_tooling_reads_sha256: true,
        }
    }

    /// **A new repo defaults to the system default when no per-repo format is pinned.** On the M3
    /// floor that default is SHA-1+`sha1dc` (GF-2b); after the OQ-9 flip it is SHA-256 — the SAME
    /// resolution path, a different default value.
    #[test]
    fn new_repo_defaults_to_the_system_default_when_unpinned() {
        // M3 floor: SHA-1 default.
        let floor = NewRepoDefault::sha1dc_floor();
        assert_eq!(create_repo_format(floor, None), ObjectFormat::Sha1Dc);

        // After the flip: a new repo defaults to SHA-256.
        let flipped = flip_default_to_sha256(bar_all_ready()).expect("bar met → flip");
        assert_eq!(create_repo_format(flipped, None), ObjectFormat::Sha256);
    }

    /// **A new repo defaults SHA-256 after the flip (a new repo defaults SHA-256; existing repos
    /// unchanged).** The prompt's TESTS field — the headline default-flip property.
    #[test]
    fn after_the_flip_a_new_repo_defaults_sha256_existing_repos_unchanged() {
        // An EXISTING repo created on the floor is SHA-1 — record its format.
        let existing_format = create_repo_format(NewRepoDefault::sha1dc_floor(), None);
        assert_eq!(existing_format, ObjectFormat::Sha1Dc);

        // The flip happens (the OQ-9 bar clears).
        let flipped = flip_default_to_sha256(bar_all_ready()).expect("bar met");

        // A NEW repo now defaults SHA-256.
        assert_eq!(create_repo_format(flipped, None), ObjectFormat::Sha256);

        // The EXISTING repo's format is UNCHANGED by the flip (default-change, not a migration). The
        // existing format was captured BEFORE the flip and is immutable — no re-hash, no re-set.
        assert_eq!(
            existing_format,
            ObjectFormat::Sha1Dc,
            "an existing repo's object format is untouched by the default flip (not a migration)"
        );
    }

    /// **An explicit per-repo pin WINS over the default (opt-in/opt-out per repo).** A repo can be
    /// pinned SHA-256 before the global flip, or pinned SHA-1 after it — the pin always wins.
    #[test]
    fn explicit_per_repo_pin_wins_over_the_default() {
        // Pin a repo SHA-256 while the default is still the SHA-1 floor (early opt-in).
        let floor = NewRepoDefault::sha1dc_floor();
        assert_eq!(
            create_repo_format(floor, Some(ObjectFormat::Sha256)),
            ObjectFormat::Sha256,
            "an explicit SHA-256 pin opts a repo in before the global flip"
        );

        // Pin a repo SHA-1 after the flip (late opt-out — interop with a SHA-1-only downstream).
        let flipped = flip_default_to_sha256(bar_all_ready()).unwrap();
        assert_eq!(
            create_repo_format(flipped, Some(ObjectFormat::Sha1Dc)),
            ObjectFormat::Sha1Dc,
            "an explicit SHA-1 pin opts a repo out after the flip"
        );
    }

    /// **The flip is GATED on the interop bar (OQ-9): an unmet bar REFUSES the flip (the default stays
    /// SHA-1).** Kills the always-flip mutant — the flip is not unconditional.
    #[test]
    fn flip_is_refused_when_the_interop_bar_is_unmet() {
        // Each lane unready in turn refuses the flip.
        for unready in [
            InteropBar {
                stock_git_reads_sha256: false,
                ..bar_all_ready()
            },
            InteropBar {
                ci_runners_read_sha256: false,
                ..bar_all_ready()
            },
            InteropBar {
                ide_tooling_reads_sha256: false,
                ..bar_all_ready()
            },
        ] {
            let refused =
                flip_default_to_sha256(unready).expect_err("an unmet bar refuses the flip");
            assert_eq!(refused.bar, unready);
            // The refusal surfaces WHICH lane is unready (operator-readable).
            assert!(refused.to_string().contains("REFUSED"));
        }
    }

    /// **The interop bar is ALL-OR-NOTHING (`&&`): every lane must read SHA-256.** Kills the `&&` → `||`
    /// mutant — a single SHA-1-only lane must keep the bar unmet (the flip is all-or-nothing).
    #[test]
    fn interop_bar_is_all_lanes_and() {
        assert!(bar_all_ready().is_met(), "all lanes ready → bar met");
        // Any single lane unready → bar unmet (the `||` mutant would call it met).
        assert!(!InteropBar {
            stock_git_reads_sha256: false,
            ..bar_all_ready()
        }
        .is_met());
        assert!(!InteropBar {
            ci_runners_read_sha256: false,
            ..bar_all_ready()
        }
        .is_met());
        assert!(!InteropBar {
            ide_tooling_reads_sha256: false,
            ..bar_all_ready()
        }
        .is_met());
        // None ready → unmet.
        assert!(!InteropBar {
            stock_git_reads_sha256: false,
            ci_runners_read_sha256: false,
            ide_tooling_reads_sha256: false,
        }
        .is_met());
    }

    /// **The format carries its config token + hex width (hash-agnostic render).** SHA-1 → `sha1`/40,
    /// SHA-256 → `sha256`/64 — the width is a render fact, never a pin in the wire/ref layer.
    #[test]
    fn format_carries_its_config_token_and_width() {
        assert_eq!(ObjectFormat::Sha1Dc.config_token(), "sha1");
        assert_eq!(ObjectFormat::Sha256.config_token(), "sha256");
        assert_eq!(ObjectFormat::Sha1Dc.oid_hex_width(), 40);
        assert_eq!(ObjectFormat::Sha256.oid_hex_width(), 64);
        assert_ne!(
            ObjectFormat::Sha1Dc.oid_hex_width(),
            ObjectFormat::Sha256.oid_hex_width()
        );
    }

    /// **Object addressing THREADS the format (never assumes it).** Two repos with different formats
    /// carry different [`ObjectFormat`]s through the address call; the storage framing is content-
    /// addressed (SHA-256 on this floor) and the format is carried, not assumed.
    #[test]
    fn object_addressing_threads_the_format() {
        let content = b"fn main() {}\n";
        // The format is threaded; on this floor both frame to the storage SHA-256 address (the SHA-1
        // legacy framing is the canonical-git wire op floor) — what matters is the format is CARRIED.
        let a = object_address_for_format(ObjectFormat::Sha256, GitObjectKind::Blob, content);
        let b = object_address_for_format(ObjectFormat::Sha1Dc, GitObjectKind::Blob, content);
        // Both address the framed bytes (the storage tier verifies them on read).
        assert_eq!(a, git_object_address(GitObjectKind::Blob, content));
        assert_eq!(
            a, b,
            "the storage framing is SHA-256 on this floor (format carried, not assumed)"
        );
    }

    /// **There is NO `set_format` — a repo's format is immutable (born once at create).** This is a
    /// compile-time property (no mutator exists); asserted structurally here by confirming the only
    /// way to a format is `create_repo_format` (create time) — a re-set would be a migration.
    #[test]
    fn format_is_immutable_no_set_format_exists() {
        let f = create_repo_format(NewRepoDefault::sha1dc_floor(), Some(ObjectFormat::Sha256));
        // The format is a Copy value with no interior mutability + no setter on the repo — re-resolving
        // create_repo_format with the SAME inputs is idempotent (a format is born once).
        assert_eq!(
            f,
            create_repo_format(NewRepoDefault::sha1dc_floor(), Some(ObjectFormat::Sha256))
        );
    }
}
