//! The **unified 4-step `#sub` resolution ladder** (REF-P15 / P-164; contract 5.7 OWNED — the
//! grammar + ladder; consumes 5.6 the owner's `project` sub-anchor resolver, 4.2 `check`, 4.8
//! pseudonym shred).
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! §4.6 (the one resolution ladder, frozen — C-2): for ANY `#sub`,
//! ```text
//! 1. permission: check(viewer, view, root)  → Deny     ⇒ Tombstone{ denied }     (never leak)
//! 2. root resolve: the parent exists?       → No       ⇒ Tombstone{ root_gone }
//! 3. sub resolve via the owner's project(ref) sub-anchor resolver:
//!      LIVE     → Projection
//!      MOVED    → Projection + flag `moved`           (Git rebased range; KN block moved)
//!      OUTDATED → Projection(partial) + flag `outdated`  (Git partial range; KN edited block)
//!      GONE     → Tombstone{ sub_gone, root }         (root still resolves; embed shows the parent)
//! 4. ERASED (any level): Tombstone{ erased }
//! ```
//! and §3.5 (Git line-ranges are **content-anchored**: BLAKE3 fingerprint + 3-way context match →
//! `exact`→LIVE, `rebased`→MOVED, `partial`→OUTDATED, `content_gone`→GONE). **A tombstone always
//! carries the root** (an embed degrades to "this referenced *&lt;parent&gt;*", never vanishes).
//!
//! ## Why this is a module ON TOP OF [`crate::resolve`], not a second resolver (EI-01 §7 coherence)
//! The REF-P10 [`crate::resolve::ResolveService`] already composes steps 1–2 (the fail-static
//! permission chokepoint, denied→`Tombstone{denied}`) and maps an owner [`ProjectOutcome`] onto the
//! [`Resolution`]. This module does NOT re-implement the chokepoint — it FORMALISES the §3.5
//! content-anchored **sub-anchor resolver** that produces the `live/moved/outdated/gone` `SubState`
//! the owner's `project` returns, and the ONE place that maps the `SubState` onto a
//! [`ProjectOutcome`]. So the ladder is: `resolve` (steps 1–2 + the §4.6 mapping, REF-P10) ∘ the
//! per-kind [`SubAnchorResolver`] (step 3, here). There is exactly ONE ladder and ONE chokepoint.
//!
//! The owner's 5.6 `project(ref, viewer)` is the only thing that knows whether a `#sub` is live —
//! Refs NEVER reads the owner's DB. This module gives the owner the FROZEN `SubState` vocabulary to
//! answer in (so a `MOVED`/`OUTDATED`/`GONE` from Git, KN, Chat all flow through the SAME ladder), and
//! provides the **reference content-anchored resolver for Git line-ranges** ([`resolve_line_range`])
//! so a Git owner can compute `exact/rebased/partial/content_gone` from a BLAKE3 fingerprint + the
//! current blob, the same way every owner reports its sub-state.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **Each subsystem's STABLE `#sub` mint is the subsystem's deliverable** (a block id survives moves,
//!   a message/comment id is immutable, a Git range carries the BLAKE3 fingerprint). At M2 the ladder
//!   is exercised against SYNTHETIC owners + the available producers; the first REAL producer mints
//!   land in R-M3/R-M4 (REF-P17 Git, REF-P18 Knowledge, REF-P19/P20/P21 the rest). The vocabulary +
//!   the ladder + the Git content-anchoring algorithm are real + drilled here; the stable mint is the
//!   owner's. Named so the frozen grammar is not mistaken for a working sub-anchor everywhere.
//! - **The Git 3-way context match is the reference [`resolve_line_range`] algorithm** (BLAKE3 over the
//!   anchored lines + a small context window). It is the algorithm Git's owner `project` will run; the
//!   real Git blob/diff plumbing lands in REF-P17. The exact/rebased/partial/content_gone classifier
//!   is real + tested here over synthetic blobs.
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / VISION §4 prove-it)
//! The ladder is leak-of-dangling-embed / 0-hard-404 critical (REF-D9). Floor: **≥ 80% of viable
//! mutants caught** (`cargo mutants -p myelin-refs-service -f crates/myelin-refs-service/src/ladder.rs`).
//! Measured 2026-06-20: **38 mutants generated → 8 unviable, 30 viable, 30 caught, 0 missed = 100% of
//! viable** — floor met. Every ladder arm (each `SubState`→`ProjectOutcome` mapping, the
//! root-always-carried rule, each Git content-anchored state incl. the contiguous-match boundary, the
//! bare-root short-circuit) has a unit test a mutation flips.

use myelin_events::ArtifactRef;
use myelin_refs::{strip_sub, sub_kind, Sub};

use crate::resolve::{OwnerProjection, ProjectOutcome, ProjectionFlag};

/// The telemetry signal the ladder feeds (contract 1.8): the `tombstone_count` + its ladder-state
/// distribution. A named constant — drills assert against the NAME, never a literal (EI-01 §3).
pub const TOMBSTONE_COUNT_SIGNAL: &str = "refs.tombstone_count";

/// **The frozen §4.6 sub-resolution state an owner's `project` sub-anchor resolver returns** — the ONE
/// `live/moved/outdated/gone` vocabulary that covers Git line-ranges, KN block/heading/row anchors,
/// Chat message/thread anchors, and the check-/step- CI kinds (C-6). Refs maps this onto a
/// [`ProjectOutcome`] (and thence onto the [`crate::resolve::Resolution`]) through [`Self::into_outcome`]
/// — the ONE mapping, so every content shape degrades identically (one ladder).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubState {
    /// The sub-anchor resolves exactly — render the live projection (no flag).
    Live(OwnerProjection),
    /// The sub-anchor MOVED but is found (Git rebased range; KN block moved) — render the shifted
    /// anchor, flagged `moved`. Carries the (already shift-adjusted) owner projection.
    Moved(OwnerProjection),
    /// The sub-anchor is OUTDATED — some of it survives (Git partial range; KN edited block) — render
    /// the partial, flagged `outdated`. Carries the partial owner projection.
    Outdated(OwnerProjection),
    /// The sub-anchor is GONE — the root still resolves, but the specific sub-artifact no longer exists
    /// (Git content_gone; KN deleted block; Chat deleted message). ⇒ `Tombstone{ sub_gone, root }`.
    Gone,
    /// The sub-artifact (or a level of it) was ERASED — pseudonym-shred / crypto-shred made it
    /// unrenderable. ⇒ `Tombstone{ erased }`. The most final state.
    Erased,
}

impl SubState {
    /// **Map the frozen §4.6 sub-state onto a [`ProjectOutcome`] — the ONE ladder mapping (step 3).**
    /// `Live`→`Live(no flag)`, `Moved`→`Live(flag=Moved)`, `Outdated`→`Live(flag=Outdated)`,
    /// `Gone`→`SubGone`, `Erased`→`Erased`. The resolve chokepoint then wraps a `ProjectOutcome::Live`
    /// into a `Projection` (carrying the resolved ref) and turns `SubGone`/`Erased` into a
    /// root-carrying [`crate::resolve::Tombstone`]. This is the single place the §4.6 ladder is
    /// realised; there is no second mapping.
    pub fn into_outcome(self) -> ProjectOutcome {
        match self {
            SubState::Live(p) => ProjectOutcome::Live(OwnerProjection { flag: None, ..p }),
            SubState::Moved(p) => {
                ProjectOutcome::Live(OwnerProjection { flag: Some(ProjectionFlag::Moved), ..p })
            }
            SubState::Outdated(p) => {
                ProjectOutcome::Live(OwnerProjection { flag: Some(ProjectionFlag::Outdated), ..p })
            }
            // GONE: the root resolves, the sub does not ⇒ Tombstone{ sub_gone, root } (the root is
            // carried by the chokepoint — the embed shows the parent, never a hard 404).
            SubState::Gone => ProjectOutcome::SubGone,
            // ERASED: pseudonym-/crypto-shred ⇒ Tombstone{ erased }.
            SubState::Erased => ProjectOutcome::Erased,
        }
    }
}

/// **The owner's per-kind sub-anchor resolver (contract 5.6, the §4.6 step-3 resolver) — the SEAM each
/// subsystem implements.** Given the FULL `#sub` ref (already permission-checked + root-resolved by the
/// chokepoint, steps 1–2), the owner reports the frozen [`SubState`]. The default body resolves a
/// **bare root** (no `#sub`) as `Live` — only a sub-anchored ref consults the per-kind logic. Each real
/// owner (Git/Knowledge/Chat/Issues/CI) implements this against its own stable mint (REF-P17+).
///
/// `Send + Sync` so a [`crate::resolve::ResolveService`] can hold it behind an `Arc`.
pub trait SubAnchorResolver: Send + Sync {
    /// Resolve the `#sub` anchor on `ref_` for `viewer`, reporting the frozen [`SubState`]. Called ONLY
    /// on the permission-allowed + root-present branch (the chokepoint gates it). A bare root (no
    /// `#sub`) is `Live` (there is no sub-anchor to degrade).
    fn resolve_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState;
}

/// **The reference Git content-anchored line-range resolver (§3.5; the algorithm Git's owner `project`
/// runs).** A `#L<a>-L<b>` carries, at mint time, a BLAKE3 fingerprint of the anchored lines + a small
/// context window + the blob oid. On resolution against the CURRENT blob the resolver returns one of
/// four states — these map directly onto the ladder (§3.5 → §4.6):
///
/// 1. **exact** — the blob oid matches → the exact range (LIVE).
/// 2. **rebased** — the blob changed but the fingerprinted lines are found at a shifted position
///    (3-way context match) → the shifted range, flagged `moved` (MOVED).
/// 3. **partial** — some anchored lines survive, some are gone → the surviving sub-range, flagged
///    `outdated` (OUTDATED).
/// 4. **content_gone** — the anchored content is entirely gone → GONE (`Tombstone{ sub_gone }`).
///
/// `resolve_line_range` is the pure classifier over `(minted, current)`; a real Git owner feeds it the
/// blob bytes (REF-P17). It is `pub` so a Git owner reuses the ONE algorithm rather than inventing a
/// second one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineRangeState {
    /// The blob oid matches → the exact minted range (LIVE).
    Exact,
    /// The fingerprinted lines moved to a shifted position (3-way context match) (MOVED).
    Rebased {
        /// The shifted 1-based start the anchored block now begins at.
        new_start: u64,
        /// The shifted 1-based end.
        new_end: u64,
    },
    /// Some anchored lines survive, some are gone → the surviving sub-range (OUTDATED).
    Partial {
        /// The 1-based start of the surviving sub-range.
        surviving_start: u64,
        /// The 1-based end of the surviving sub-range.
        surviving_end: u64,
    },
    /// The anchored content is entirely gone (GONE).
    ContentGone,
}

impl LineRangeState {
    /// Map the §3.5 content-anchored state onto the §4.6 ladder [`SubState`] (so a Git owner answers in
    /// the ONE vocabulary). `Exact`→`Live`, `Rebased`→`Moved`, `Partial`→`Outdated`,
    /// `ContentGone`→`Gone`. The owner supplies the `OwnerProjection` to carry on the LIVE/MOVED/OUTDATED
    /// arms (the rendered range); a `ContentGone` carries nothing (the root-carrying tombstone is the
    /// chokepoint's).
    pub fn into_sub_state(self, projection: OwnerProjection) -> SubState {
        match self {
            LineRangeState::Exact => SubState::Live(projection),
            LineRangeState::Rebased { .. } => SubState::Moved(projection),
            LineRangeState::Partial { .. } => SubState::Outdated(projection),
            LineRangeState::ContentGone => SubState::Gone,
        }
    }
}

/// **A minted Git line-range anchor (§3.5) — the BLAKE3 fingerprint + the context window + the blob
/// oid captured at mint time.** The owner stores this alongside the `#L<a>-L<b>` so resolution against
/// a newer blob is content-anchored, not positional. PII-free: line fingerprints + an opaque oid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintedLineRange {
    /// The blob oid at mint time (the exact-match short-circuit).
    pub blob_oid: String,
    /// The anchored lines (the content the range pointed at), in order. Stored as their BLAKE3
    /// fingerprints so the resolver matches CONTENT, not text (and holds no third-party body).
    pub anchored: Vec<String>,
}

impl MintedLineRange {
    /// Fingerprint a line (BLAKE3) — the content-anchor identity. The resolver matches on these
    /// fingerprints, so it never holds the raw third-party line body (erasure-safe; §4.6 / X-7).
    pub fn fingerprint(line: &str) -> String {
        format!("blake3:{}", hex::encode(blake3::hash(line.as_bytes()).as_bytes()))
    }

    /// Mint a line-range anchor from the current blob lines for the 1-based `[start, end]` range.
    /// Captures the per-line fingerprints (the content anchor). The `blob_oid` is the current blob's
    /// identity (the exact-match short-circuit on a later resolve).
    pub fn mint(blob_oid: &str, lines: &[&str], start: u64, end: u64) -> MintedLineRange {
        let anchored = lines
            .iter()
            .skip(start.saturating_sub(1) as usize)
            .take((end.saturating_sub(start) + 1) as usize)
            .map(|l| Self::fingerprint(l))
            .collect();
        MintedLineRange { blob_oid: blob_oid.to_string(), anchored }
    }
}

/// **The reference content-anchored line-range classifier (§3.5).** Resolves a [`MintedLineRange`]
/// against the CURRENT blob (its oid + lines) into a [`LineRangeState`]:
///
/// - oid matches → `Exact`.
/// - all anchored fingerprints found contiguously at a shifted offset (3-way context) → `Rebased`.
/// - some anchored fingerprints survive (a contiguous prefix) but not all → `Partial`.
/// - none survive → `ContentGone`.
///
/// This is the algorithm Git's owner `project` runs (REF-P17 feeds it real blobs). PURE — no I/O.
pub fn resolve_line_range(minted: &MintedLineRange, current_oid: &str, current_lines: &[&str]) -> LineRangeState {
    // 1. exact — the blob oid matches → the minted range is live as-is.
    if minted.blob_oid == current_oid {
        return LineRangeState::Exact;
    }
    if minted.anchored.is_empty() {
        // An empty anchor on a changed blob cannot be located — content_gone (defensive; mint never
        // produces an empty anchor for a valid range).
        return LineRangeState::ContentGone;
    }

    let current_fps: Vec<String> = current_lines.iter().map(|l| MintedLineRange::fingerprint(l)).collect();

    // 2. rebased — the WHOLE anchored block is found contiguously at a shifted position (3-way context
    //    match: the exact fingerprint sequence appears somewhere in the current blob).
    if let Some(offset) = find_subsequence(&current_fps, &minted.anchored) {
        let new_start = (offset + 1) as u64;
        let new_end = (offset + minted.anchored.len()) as u64;
        return LineRangeState::Rebased { new_start, new_end };
    }

    // 3. partial — a contiguous PREFIX of the anchored block survives (some lines edited/removed). We
    //    report the surviving prefix's position. The longest contiguous prefix of `anchored` that is
    //    found contiguously in `current_fps`.
    for keep in (1..minted.anchored.len()).rev() {
        if let Some(offset) = find_subsequence(&current_fps, &minted.anchored[..keep]) {
            let surviving_start = (offset + 1) as u64;
            let surviving_end = (offset + keep) as u64;
            return LineRangeState::Partial { surviving_start, surviving_end };
        }
    }

    // 4. content_gone — none of the anchored content survives.
    LineRangeState::ContentGone
}

/// Find the first offset at which `needle` appears as a contiguous subsequence of `haystack`, or
/// `None`. The 3-way-context contiguous match the rebased/partial states use.
fn find_subsequence(haystack: &[String], needle: &[String]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&start| haystack[start..start + needle.len()] == *needle)
}

/// **A synthetic owner sub-anchor resolver (the M2 floor) — the SubState per FULL `#sub` ref, scripted.**
/// Stands in for the real Git/Knowledge/Chat owners (REF-P17+) so the ladder is drilled end-to-end at
/// M2 across all three content shapes. A bare root (no `#sub`) is `Live` by default; a sub-anchored ref
/// returns the scripted state (default `Live` if none scripted). The `default_projection` is what a
/// `Live`/`Moved`/`Outdated` carries.
#[derive(Default)]
pub struct SyntheticSubResolver {
    states: std::sync::Mutex<Vec<(String, SubState)>>,
}

impl SyntheticSubResolver {
    /// A fresh resolver (every ref resolves `Live` until scripted).
    pub fn new() -> SyntheticSubResolver {
        SyntheticSubResolver::default()
    }

    /// Script the [`SubState`] a FULL `#sub` ref resolves to (the drill arms its degrade).
    pub fn set_state(&self, ref_: &str, state: SubState) {
        self.states.lock().unwrap().push((ref_.to_string(), state));
    }

    /// The default live projection an unscripted ref carries (a render-safe title the leak invariant
    /// already gates — the chokepoint only reaches here on the allowed branch).
    pub fn default_projection() -> OwnerProjection {
        OwnerProjection {
            title: "an embedded artifact".into(),
            state: "live".into(),
            icon: "doc".into(),
            render_hint: "embed".into(),
            sub_anchor: None,
            flag: None,
        }
    }
}

impl SubAnchorResolver for SyntheticSubResolver {
    fn resolve_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        // A bare root (no #sub) has no sub-anchor to degrade — it is Live (the parent itself).
        if sub.is_none() {
            return SubState::Live(Self::default_projection());
        }
        self.states
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _)| k == &ref_.0)
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| {
                let mut p = Self::default_projection();
                p.sub_anchor = Some(ref_.0.clone());
                SubState::Live(p)
            })
    }
}

/// **Resolve the `#sub` ladder step (3) for `ref_` → a [`ProjectOutcome`] (the §4.6 mapping).** The
/// caller (the resolve chokepoint, REF-P10) has already done steps 1–2 (permission + root resolve);
/// this drives step 3 (the owner's per-kind sub-anchor resolver) + step 4 (erased) and maps the frozen
/// [`SubState`] onto the [`ProjectOutcome`] the chokepoint turns into a `Projection`/`Tombstone`. The
/// `root` is always derivable ([`strip_sub`]) so the tombstone carries it (§4.6 — never vanishes).
///
/// This is the ONE function that lowers the §3.5 grammar onto the §4.6 ladder. The `sub_kind` accessor
/// (REF-P1) classifies the anchor; an unparseable `#sub` cannot reach here (parse rejected it upstream).
pub fn resolve_sub_outcome(resolver: &dyn SubAnchorResolver, ref_: &ArtifactRef) -> ProjectOutcome {
    let sub = sub_kind(ref_);
    resolver.resolve_sub(ref_, sub.as_ref()).into_outcome()
}

/// The `#sub`-stripped root of `ref_` (the tombstone always carries it; §4.6). Re-exported convenience
/// over the REF-P1 codec so a ladder caller never hand-rolls the strip.
pub fn ladder_root(ref_: &ArtifactRef) -> ArtifactRef {
    strip_sub(ref_)
}

#[cfg(test)]
mod tests;
