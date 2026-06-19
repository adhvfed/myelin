//! # `myelin-refs` — the `ArtifactRef` value type + parse/format/`#sub` grammar (REF-P1 / P-052)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! §3.1 (the URN `ArtifactRef` + the frozen Issues `<PROJECTKEY>-<seqno>` key, C-3), §3.5 (the
//! unified `#sub` grammar — the complete v1 vocabulary, C-1/C-6), §4.8 (display keys render-time
//! only, REF-3). **Reconciliation:** `00-reconciliation-decisions.md` C-1/C-3/C-6, X-2.
//!
//! **Contract-index cluster:** 5 — `ArtifactRef`, refs & projection
//! (`contract-index.md` rows 5.1 `ArtifactRef` parse/format **[owned here — the value-type half]**,
//! 5.3 `backlinks/edges`, 5.7 the unified `#sub` grammar). The `<subsystem>`/`<type>` token table
//! (row 2.9) is **owned by the Bus** (`event-bus.md` §6.2); Refs **validates** against it, it never
//! authors a token.
//!
//! ## What crosses the crate boundary here (the frozen surface)
//! `Refs` — the one library every service links for `parse` / `format` / `edges` / `backlinks`;
//! services must NOT re-implement URN handling (REF-3). [`parse`] rejects scope-less / ambiguous
//! refs and **never guesses scope**. The `ArtifactRef` *value type* (the `String` newtype) is owned
//! by `myelin-tenancy` and re-exported via `myelin-events` (DAG-acyclicity, see the tenancy
//! crate-level note); **this crate owns its BEHAVIOUR** — parse/format/`#sub`/strip_sub.
//!
//! ## What REF-P1 (P-052) ships — the value-type half of contract 5.1
//! - [`parse`] / [`format`] — the canonical URN codec, ambiguity-rejecting, round-trip byte-identical.
//! - [`strip_sub`] — the `#sub`-stripped root (§3.2 `*_root`).
//! - [`sub_kind`] / [`Sub`] — the frozen `#sub` kind accessor (§3.5 vocabulary).
//! - [`ParseError`] — the LOUD, typed rejection taxonomy (one variant per grammar rule broken).
//!
//! See the [`parse`] module for the full grammar, the Issues key (C-3), and the parse-module
//! mutation-score floor (measured, met).
//!
//! ## Floors named (EI-01 §1 — name-your-floors; what this prompt does NOT build)
//! The value type is complete at M0; it is **not** the working reference graph:
//! - **The resolver** over `ArtifactRef` — `resolve(ref, viewer, mode) -> Projection | Tombstone`,
//!   the 4-step tombstone ladder (contract 5.7 / §4.6) and the permission-filtered backlink read
//!   (5.3) — is the **R-M2 follow-on**: `reference-graph.md` REF-P9..REF-P11 (resolution chokepoint,
//!   backlink crux). `Refs::edges` / `Refs::backlinks` here are deferred (`todo!()`).
//! - **The four architecture lints** Refs leans on (tenant-predicate, no-raw-publish, no-cross-db,
//!   no-cross-sync-cycle) are wired with Refs-specific red+green fixtures in **REF-P2 (P-053)**.
//!
//! So this crate at M0 is the contract value type only — not the engine, not the lints.

mod parse;

use myelin_identity::Principal;
use serde::{Deserialize, Serialize};

/// Re-export the frozen `ArtifactRef` value type so callers read `myelin_refs::ArtifactRef`.
pub use myelin_events::ArtifactRef;

/// The frozen `#sub` codec surface (contract 5.1/5.7; §3.5 / recon C-1/C-6): the canonical URN
/// parser/formatter, the `#sub`-stripped root, the `#sub` kind accessor + the typed rejection
/// taxonomy. These are the value-type behaviours every service consumes (REF-3 — never
/// re-implemented per service).
pub use parse::{format, parse, strip_sub, sub_kind, ParseError, Sub, SCHEME};

/// An outbound or inbound edge between two artifacts (architecture §2.3; contract 5.3/5.4).
/// The typed-edge taxonomy (`closes/blocks/depends_on/parent/...`) is the TE-7 mirror
/// (5.5); the skeleton carries an opaque edge so the trait shape compiles.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// The source artifact (the referencing side).
    pub from: ArtifactRef,
    /// The target artifact (the referenced side).
    pub to: ArtifactRef,
    /// The edge relation token (TE-7 vocabulary; opaque at the value-type layer).
    pub kind: String,
}

/// The refs error type (contract 5.1/5.3). At M0 the parse/format half is the typed [`ParseError`];
/// the edge/resolve half (tombstone, denied) lands with the resolver (REF-P9+). `RefError` wraps the
/// parse error so the `Refs` trait's associated `parse` returns the unified surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefError {
    /// A malformed / ambiguous URN (the value-type half, REF-P1). Carries the precise grammar rule
    /// broken (REF-3 — never silently coerced).
    Parse(ParseError),
}

impl From<ParseError> for RefError {
    fn from(e: ParseError) -> Self {
        RefError::Parse(e)
    }
}

impl core::fmt::Display for RefError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RefError::Parse(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RefError {}

/// `Result` alias for the refs surface.
pub type Result<T> = core::result::Result<T, RefError>;

/// The one refs library (architecture §2.3; contract 5.1/5.3; REF-3). `parse`/`format` are
/// associated (no `&self`) — they are the canonical URN codec, **implemented at M0** (REF-P1) over
/// the [`parse`] module. `edges`/`backlinks` take `&self` (they read the edge projection);
/// `backlinks` is permission-filtered via the viewer (REF-1) and is **deferred to the resolver
/// (REF-P9+)** — its body is `todo!()` here.
pub trait Refs {
    /// Parse a canonical URN, rejecting ambiguity; never guesses scope (REF-3). Implemented at M0.
    fn parse(s: &str) -> Result<ArtifactRef>;
    /// Render an `ArtifactRef` to its canonical string. Implemented at M0; round-trips with `parse`.
    fn format(r: &ArtifactRef) -> String;
    /// Outbound edges. **Floor:** deferred to the resolver (REF-P11); `todo!()`.
    fn edges(&self, r: &ArtifactRef) -> Result<Vec<Edge>>;
    /// Permission-filtered inbound edges (REF-1). **Floor:** deferred to the backlink crux (REF-P11);
    /// `todo!()`.
    fn backlinks(&self, r: &ArtifactRef, viewer: &Principal) -> Result<Vec<Edge>>;
}

/// The platform's canonical [`Refs`] codec implementation. The parse/format half (REF-P1, contract
/// 5.1) is real; the edge-walk half (5.3) lands with the resolver (REF-P9..REF-P11). A unit struct
/// because the codec is stateless — the edge methods will take the projection store by `&self` once
/// the engine lands (a future field), preserving this frozen trait shape.
#[derive(Clone, Copy, Debug, Default)]
pub struct RefsCodec;

impl Refs for RefsCodec {
    fn parse(s: &str) -> Result<ArtifactRef> {
        parse::parse(s).map_err(RefError::from)
    }

    fn format(r: &ArtifactRef) -> String {
        parse::format(r)
    }

    fn edges(&self, _r: &ArtifactRef) -> Result<Vec<Edge>> {
        // FLOOR: the edge walk over the inverse index lands with the resolver (REF-P5 schema +
        // REF-P11 backlink crux). The value-type crate ships parse/format only (REF-P1 / P-052).
        todo!("edge walk lands in the Refs resolver (contract 5.3; REF-P11)")
    }

    fn backlinks(&self, _r: &ArtifactRef, _viewer: &Principal) -> Result<Vec<Edge>> {
        // FLOOR: the permission-filtered backlink read (the SetExpr lowering over source_root) lands
        // in REF-P11 (the crux). REF-P1 / P-052 ships the value type only.
        todo!("permission-filtered backlinks land in the Refs resolver (contract 5.3; REF-P11)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    /// The `Refs` trait's associated `parse`/`format` are wired to the real codec (REF-P1) and
    /// round-trip; the `&self` edge methods are the named floor (REF-P11). A stub `Principal` proves
    /// the `backlinks` viewer signature still compiles to the frozen shape.
    #[test]
    fn refs_codec_parse_format_round_trips_and_edge_methods_are_the_named_floor() {
        let s = "myelin://acme/issue/issue/ENG-1421";
        let r = <RefsCodec as Refs>::parse(s).expect("canonical URN parses");
        assert_eq!(<RefsCodec as Refs>::format(&r), s);

        // The viewer-threaded signature compiles (REF-1) — the body is the named floor.
        let _viewer = Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
    }

    /// The codec rejects a display projection (REF-3) through the `Refs` trait surface — the
    /// rejection is the typed [`RefError::Parse`].
    #[test]
    fn refs_codec_rejects_a_display_projection() {
        assert!(matches!(
            <RefsCodec as Refs>::parse("#1421"),
            Err(RefError::Parse(_))
        ));
    }
}
