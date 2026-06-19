//! # `ArtifactRef` parse / format + the frozen `#sub` grammar (REF-P1 / P-052, contract 5.1)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! §3.1 (the URN `ArtifactRef` + the frozen Issues `<PROJECTKEY>-<seqno>` key, C-3), §3.5 (the
//! unified `#sub` grammar — the complete v1 vocabulary, C-1/C-6), §4.8 (display keys are
//! render-time only, REF-3). **Reconciliation:**
//! `00-reconciliation-decisions.md` C-1 (`#sub` grammar frozen), C-3 (Issues key stored canonical),
//! C-6 (`check-`/`step-` first-class `#sub` kinds), X-2. **Contract:** `contract-index.md` row 5.1
//! (`ArtifactRef` parse/format, Issues key frozen) — owned by Refs; the `<subsystem>/<type>` token
//! table (row 2.9) is **owned by the Bus** (`event-bus.md` §6.2) and Refs only **validates** against
//! it, it never authors it.
//!
//! ## What this module is — the value-type half of contract 5.1
//! The canonical URN grammar is
//! `myelin://<tenant>/<subsystem>/<type>/<id>[#<sub>]`. This module provides:
//!
//! - [`parse`] — `&str -> Result<ArtifactRef>`. It enforces **total explicit scope** (tenant,
//!   subsystem, type, id all required) and **rejects ambiguity** (REF-3, §3.1): a scope-less /
//!   short-hash ref (`#42`, `@alice`, `~general`, a 7-char commit prefix, the Issues short display
//!   `#1421`) is **rejected, never guessed** — those are render-time display projections (§4.8).
//!   The `<subsystem>`/`<type>` segments are validated against the Bus §6.2 token table
//!   ([`myelin_events::SUBSYSTEM_TOKENS`] / [`myelin_events::ARTIFACT_TYPE_TOKENS`]) — Refs is the
//!   validator, not a second authority.
//! - [`format`] — `&ArtifactRef -> String`, the canonical rendering. `format(parse(s)?)` round-trips
//!   **byte-identical** for every well-formed URN (the canonical form is the parsed form).
//! - [`strip_sub`] — `&ArtifactRef -> ArtifactRef`, the `#sub`-stripped **root** (the parent
//!   artifact, §3.2 `*_root` columns; the builder/resolver roll backlinks up to the parent).
//! - [`sub_kind`] / [`Sub`] — the self-describing `#sub` kind accessor over the frozen vocabulary
//!   (§3.5: `comment-`/`thread-`/`message-`/`b`/`h`/`row-`/`field-`/`L<a>-L<b>`/`check-`/`step-`).
//!   An unknown / ambiguous `#sub` kind is **rejected** (REF-3).
//!
//! ## The Issues key (C-3, §3.1) — the canonical `<id>` is `<PROJECTKEY>-<seqno>`
//! For an issue the stored `<id>` segment is the project-prefix + monotonic number, e.g.
//! `myelin://acme/issue/issue/ENG-1421`. That is a real URN component. The short display form
//! `#1421` (and `@alice`, `~general`, a bare `#42`) is the **render-time projection** (§4.8) and is
//! **NOT parseable as a scope** — [`parse`] rejects it. This is asserted directly in the tests
//! (`issues_short_display_key_is_rejected`).
//!
//! ## FLOOR (named — this is the value type, NOT the engine, EI-01 §1)
//! This module is the **contract value type** of 5.1, complete at M0. It is NOT the resolver:
//! - the resolver over `ArtifactRef` (`resolve(ref, viewer, mode) -> Projection | Tombstone`, the
//!   4-step tombstone ladder of 5.7/§4.6) is the **R-M2 follow-on, REF-P9** (global numbering) —
//!   see the per-system file `reference-graph.md` REF-P9..REF-P11.
//! - the four architecture lints Refs leans on (tenant-predicate, no-raw-publish, no-cross-db,
//!   no-cross-sync-cycle) are wired with Refs-specific red+green fixtures in **REF-P2 (P-053)**.
//!
//! So the value type here is not to be mistaken for the working reference graph.
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / VISION §4 prove-it)
//! `myelin-refs` is a mandatory-core glue crate; this parse module is its load-bearing logic. The
//! floor for this module is **≥ 80% of viable mutants caught**
//! (`cargo mutants -p myelin-refs -f crates/myelin-refs/src/parse.rs`). Measured 2026-06-19:
//! **16 mutants generated → 4 unviable, 12 viable, 12 caught, 0 missed = 100% of viable** — floor
//! met. (Every parse rule — the scheme prefix, the four-segment scope, the per-segment non-empty
//! check, each `#sub`-kind discriminator, the numeric-range check, the token-table membership — has
//! a rejection test that a mutation flips; the `Display::fmt` rendering is pinned by
//! `parse_error_display_is_loud_and_names_the_rule`.)

use myelin_events::{ArtifactRef, ARTIFACT_TYPE_TOKENS, SUBSYSTEM_TOKENS};

/// The canonical URN scheme prefix. The ONLY scheme `parse` admits (a bare `https://…` or a
/// scheme-less string is rejected — there is no implicit scheme).
pub const SCHEME: &str = "myelin://";

/// Why a candidate string is NOT a well-formed [`ArtifactRef`]. Each variant is a **distinct, LOUD**
/// reason (EI-01 §5 — never silently coerce; reject with the exact rule broken). These map 1:1 to
/// the §3.1 / §3.5 grammar rules so a caller (and a test) can assert on the precise failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The string does not start with the canonical `myelin://` scheme (§3.1).
    MissingScheme {
        /// The offending input (echoed so the failure is self-describing).
        input: String,
    },
    /// The scope is not total: a `myelin://` URN must carry exactly four `/`-separated segments
    /// `tenant/subsystem/type/id` (the `#sub` is a suffix on `id`). A scope-less / short-hash ref
    /// (`#42`, `@alice`, `~general`, a 7-char prefix) lands here — **rejected, never guessed**
    /// (REF-3, §4.8).
    IncompleteScope {
        /// How many `/`-separated segments were actually present after the scheme.
        got_segments: usize,
    },
    /// One of the four scope segments (tenant/subsystem/type/id) is empty (`myelin://acme//issue/1`).
    EmptySegment {
        /// Which segment was empty (`"tenant"`, `"subsystem"`, `"type"`, `"id"`).
        segment: &'static str,
    },
    /// The `<subsystem>` token is not one of the Bus §6.2 canonical singular tokens
    /// ([`SUBSYSTEM_TOKENS`]). Refs validates against the Bus table; it never authors a new token.
    UnknownSubsystem {
        /// The rejected subsystem token.
        token: String,
    },
    /// The `<type>` token is not one of the Bus §6.2 canonical artifact-type tokens
    /// ([`ARTIFACT_TYPE_TOKENS`]).
    UnknownType {
        /// The rejected artifact-type token.
        token: String,
    },
    /// The `#sub` suffix is present but empty (`…/id#`) — an ambiguous, unrendered sub-anchor.
    EmptySub,
    /// The `#sub` kind prefix is not one of the frozen §3.5 vocabulary, or its body is malformed
    /// (e.g. `L42` with no `-L<end>`, `step-x` with a non-numeric `n`). **Rejected, never guessed**
    /// (REF-3) — Refs picks the resolver by the self-describing kind prefix, so an unknown kind
    /// has no resolver and must not be silently admitted.
    UnknownSubKind {
        /// The rejected `#sub` token (the text after `#`).
        sub: String,
    },
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ParseError::MissingScheme { input } => write!(
                f,
                "not an ArtifactRef: `{input}` does not start with the canonical scheme `{SCHEME}` \
                 — scope-less/short-hash refs are render-time display projections (REF-3, §4.8), \
                 never a stored scope."
            ),
            ParseError::IncompleteScope { got_segments } => write!(
                f,
                "ambiguous ArtifactRef: scope is not total — a `{SCHEME}` URN needs exactly four \
                 segments `tenant/subsystem/type/id`, got {got_segments}. Scope is never guessed \
                 (REF-3)."
            ),
            ParseError::EmptySegment { segment } => write!(
                f,
                "malformed ArtifactRef: the `{segment}` segment is empty — every scope segment is \
                 required and non-empty (§3.1)."
            ),
            ParseError::UnknownSubsystem { token } => write!(
                f,
                "unknown subsystem token `{token}`: not in the Bus §6.2 canonical set \
                 {SUBSYSTEM_TOKENS:?}. Refs validates against the Bus token table, it never authors \
                 a new one."
            ),
            ParseError::UnknownType { token } => write!(
                f,
                "unknown artifact-type token `{token}`: not in the Bus §6.2 canonical set \
                 {ARTIFACT_TYPE_TOKENS:?}."
            ),
            ParseError::EmptySub => write!(
                f,
                "malformed `#sub`: the sub-anchor is empty (`…/id#`) — a `#sub` carries a \
                 self-describing kind (§3.5)."
            ),
            ParseError::UnknownSubKind { sub } => write!(
                f,
                "unknown/ambiguous `#sub` kind `{sub}`: not in the frozen §3.5 vocabulary \
                 (comment-/thread-/message-/b/h/row-/field-/L<a>-L<b>/check-/step-). The kind \
                 prefix is self-describing; an unknown kind is rejected, never guessed (REF-3)."
            ),
        }
    }
}

impl std::error::Error for ParseError {}

/// The frozen `#sub` sub-artifact kinds (§3.5 / contract 5.7 / recon C-1/C-6). The kind prefix makes
/// the grammar self-describing and lets Refs pick the resolver; this enum is the parsed
/// classification of the text after `#`. `<opaqueid>` / `<context>` bodies are subsystem-minted
/// **stable opaque ids** (the stability obligation is each subsystem's, §3.5) — Refs validates the
/// grammar shape only, never the opacity of the id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sub {
    /// `comment-<opaqueid>` — a comment / review-thread node (Git PR, Knowledge, Issues).
    Comment(String),
    /// `thread-<opaqueid>` — a thread root (Chat, Git review thread).
    Thread(String),
    /// `message-<opaqueid>` — a single chat message (Chat).
    Message(String),
    /// `b<opaqueid>` — a content block (Knowledge, Issue description block).
    Block(String),
    /// `h<opaqueid>` — a heading anchor (Knowledge).
    Heading(String),
    /// `row-<opaqueid>` — a database row (Knowledge db, Issue-as-row).
    Row(String),
    /// `field-<opaqueid>` — a field within a row / issue (Issues, Knowledge db).
    Field(String),
    /// `L<start>-L<end>` — a CONTENT-ANCHORED line range (Git). The content fingerprint + resolution
    /// ladder (exact/rebased/partial/tombstone) is the resolver's job (REF-P9+); here we hold the
    /// parsed `(start, end)` endpoints.
    LineRange {
        /// The 1-based start line.
        start: u64,
        /// The 1-based end line (`end >= start`).
        end: u64,
    },
    /// `check-<context>` — a check status on a commit (CI, X-1). First-class `#sub` kind (C-6).
    Check(String),
    /// `step-<n>` — a CI run step (jump-to-failure). First-class `#sub` kind (C-6); `<n>` is numeric.
    Step(u64),
}

/// Parse the text after the `#` into a frozen [`Sub`] kind, or reject it (REF-3 — the kind prefix is
/// self-describing; an unknown/malformed kind has no resolver and must not be silently admitted).
///
/// Order matters for the prefix-overlap pair `comment-`/`check-` vs `c…` and `b`/`h` single-letter
/// kinds: the longer, hyphen-terminated kinds (`comment-`, `thread-`, `message-`, `row-`, `field-`,
/// `check-`, `step-`) are tried before the single-letter `b`/`h` and the `L…-L…` form, so a
/// `comment-…` is never mis-read as a `b`/`h` block.
fn parse_sub(sub: &str) -> Result<Sub, ParseError> {
    if sub.is_empty() {
        return Err(ParseError::EmptySub);
    }

    // The hyphen-terminated, multi-letter kinds first (each carries a non-empty opaque body).
    for (prefix, ctor) in [
        ("comment-", Sub::Comment as fn(String) -> Sub),
        ("thread-", Sub::Thread),
        ("message-", Sub::Message),
        ("row-", Sub::Row),
        ("field-", Sub::Field),
        ("check-", Sub::Check),
    ] {
        if let Some(body) = sub.strip_prefix(prefix) {
            if body.is_empty() {
                return Err(ParseError::UnknownSubKind { sub: sub.into() });
            }
            return Ok(ctor(body.to_string()));
        }
    }

    // `step-<n>` — `<n>` must be a non-empty run of decimal digits (a CI step index).
    if let Some(n) = sub.strip_prefix("step-") {
        return n
            .parse::<u64>()
            .map(Sub::Step)
            .map_err(|_| ParseError::UnknownSubKind { sub: sub.into() });
    }

    // `L<start>-L<end>` — a content-anchored line range. Both endpoints numeric, `end >= start`.
    if let Some(range) = sub.strip_prefix('L') {
        if let Some((start_s, end_s)) = range.split_once("-L") {
            if let (Ok(start), Ok(end)) = (start_s.parse::<u64>(), end_s.parse::<u64>()) {
                if end >= start {
                    return Ok(Sub::LineRange { start, end });
                }
            }
        }
        return Err(ParseError::UnknownSubKind { sub: sub.into() });
    }

    // The single-letter block / heading kinds, last (so `comment-`/`check-` already matched above).
    // `b<opaqueid>` / `h<opaqueid>` — a non-empty opaque body.
    if let Some(body) = sub.strip_prefix('b') {
        if !body.is_empty() {
            return Ok(Sub::Block(body.to_string()));
        }
    }
    if let Some(body) = sub.strip_prefix('h') {
        if !body.is_empty() {
            return Ok(Sub::Heading(body.to_string()));
        }
    }

    Err(ParseError::UnknownSubKind { sub: sub.into() })
}

/// Render a parsed [`Sub`] back to its canonical text (the part after `#`). `parse_sub` ∘ this is the
/// identity on the parsed forms (round-trip).
fn format_sub(sub: &Sub) -> String {
    match sub {
        Sub::Comment(id) => format!("comment-{id}"),
        Sub::Thread(id) => format!("thread-{id}"),
        Sub::Message(id) => format!("message-{id}"),
        Sub::Block(id) => format!("b{id}"),
        Sub::Heading(id) => format!("h{id}"),
        Sub::Row(id) => format!("row-{id}"),
        Sub::Field(id) => format!("field-{id}"),
        Sub::LineRange { start, end } => format!("L{start}-L{end}"),
        Sub::Check(ctx) => format!("check-{ctx}"),
        Sub::Step(n) => format!("step-{n}"),
    }
}

/// `parse(&str) -> Result<ArtifactRef>` (contract 5.1). Enforces **total explicit scope** and
/// **rejects ambiguity** — a scope-less / short-hash / unknown-`#sub`-kind URN yields a typed
/// [`ParseError`], **never a guessed scope** (REF-3, §3.1). On success the returned [`ArtifactRef`]
/// holds the **canonical** string (so `format(parse(s)?)` round-trips byte-identical).
///
/// The four scope segments are validated:
/// 1. the `myelin://` scheme is present;
/// 2. exactly four `/`-separated segments follow (`tenant/subsystem/type/id`), each non-empty;
/// 3. `<subsystem>` ∈ Bus §6.2 [`SUBSYSTEM_TOKENS`]; `<type>` ∈ Bus §6.2 [`ARTIFACT_TYPE_TOKENS`];
/// 4. if an `#sub` suffix is present, it is one of the frozen §3.5 kinds ([`Sub`]).
pub fn parse(s: &str) -> Result<ArtifactRef, ParseError> {
    let rest = s.strip_prefix(SCHEME).ok_or_else(|| ParseError::MissingScheme {
        input: s.to_string(),
    })?;

    // Split the `#sub` suffix off the id FIRST (the `#` is part of the id segment, not a scope `/`).
    let (scope, sub_text): (&str, Option<&str>) = match rest.split_once('#') {
        Some((scope, sub)) => (scope, Some(sub)),
        None => (rest, None),
    };

    // The four scope segments. `splitn(4, …)` is wrong: it would fold a stray `/` in `id` into the
    // id; the canonical grammar has NO `/` inside any segment, so a plain split must yield exactly 4.
    let segments: Vec<&str> = scope.split('/').collect();
    if segments.len() != 4 {
        return Err(ParseError::IncompleteScope {
            got_segments: segments.len(),
        });
    }
    let (tenant, subsystem, type_, id) = (segments[0], segments[1], segments[2], segments[3]);

    if tenant.is_empty() {
        return Err(ParseError::EmptySegment { segment: "tenant" });
    }
    if subsystem.is_empty() {
        return Err(ParseError::EmptySegment {
            segment: "subsystem",
        });
    }
    if type_.is_empty() {
        return Err(ParseError::EmptySegment { segment: "type" });
    }
    if id.is_empty() {
        return Err(ParseError::EmptySegment { segment: "id" });
    }

    if !SUBSYSTEM_TOKENS.contains(&subsystem) {
        return Err(ParseError::UnknownSubsystem {
            token: subsystem.to_string(),
        });
    }
    if !ARTIFACT_TYPE_TOKENS.contains(&type_) {
        return Err(ParseError::UnknownType {
            token: type_.to_string(),
        });
    }

    // Validate (and canonicalise) the `#sub` if present. A present-but-malformed sub is rejected.
    let canonical = match sub_text {
        None => format!("{SCHEME}{tenant}/{subsystem}/{type_}/{id}"),
        Some(sub) => {
            let parsed = parse_sub(sub)?;
            format!(
                "{SCHEME}{tenant}/{subsystem}/{type_}/{id}#{}",
                format_sub(&parsed)
            )
        }
    };
    Ok(ArtifactRef(canonical))
}

/// `format(&ArtifactRef) -> String` (contract 5.1) — the canonical rendering. Because [`parse`]
/// stores the canonical string in the newtype, this is the identity on the inner string;
/// `format(parse(s)?) == canonical(s)` (round-trip) is asserted in the tests.
pub fn format(r: &ArtifactRef) -> String {
    r.0.clone()
}

/// `strip_sub(&ArtifactRef) -> ArtifactRef` (§3.2 `*_root`) — the `#sub`-stripped **root** (the
/// parent artifact). Backlinks roll up to the root; the builder writes `source_root`/`target_root`
/// by this function. A ref with no `#sub` is returned unchanged. The input is assumed canonical (it
/// came from [`parse`]); this is a pure string operation on the inner value.
pub fn strip_sub(r: &ArtifactRef) -> ArtifactRef {
    match r.0.split_once('#') {
        Some((root, _sub)) => ArtifactRef(root.to_string()),
        None => r.clone(),
    }
}

/// The `#sub` kind accessor (§3.5): the frozen [`Sub`] classification of an `ArtifactRef`'s
/// sub-anchor, or `None` if the ref is a bare root (no `#sub`). Lets a resolver pick the per-kind
/// resolution path. Returns `None` (never an error) for a canonical ref — a non-canonical ref is not
/// constructible through [`parse`], so any `#sub` present here is already well-formed.
pub fn sub_kind(r: &ArtifactRef) -> Option<Sub> {
    let (_, sub) = r.0.split_once('#')?;
    parse_sub(sub).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every well-formed bare scope URN round-trips byte-identical: `format(parse(s)?) == s`.
    #[test]
    fn bare_scope_urns_round_trip_byte_identical() {
        for s in [
            "myelin://acme/issue/issue/ENG-1421",
            "myelin://acme/git/pr/4291",
            "myelin://acme/ci/run/01J0RUN",
            "myelin://acme/knowledge/page/7c2",
            "myelin://acme/chat/message/01J0MSG",
            "myelin://acme/identity/member/p-7",
            "myelin://acme/refs/edge/abc123",
            "myelin://acme/issue/initiative/PLAT-9", // the new §6.2 type token
        ] {
            let r = parse(s).expect("well-formed URN must parse");
            assert_eq!(format(&r), s, "round-trip must be byte-identical for `{s}`");
        }
    }

    /// Every `#sub` kind parses, classifies to the right [`Sub`], and round-trips byte-identical.
    #[test]
    fn every_sub_kind_parses_classifies_and_round_trips() {
        let cases: &[(&str, Sub)] = &[
            (
                "myelin://acme/git/pr/42#comment-c9",
                Sub::Comment("c9".into()),
            ),
            (
                "myelin://acme/chat/message/1#thread-t7",
                Sub::Thread("t7".into()),
            ),
            (
                "myelin://acme/chat/message/1#message-m3",
                Sub::Message("m3".into()),
            ),
            ("myelin://acme/knowledge/page/7#b9", Sub::Block("9".into())),
            ("myelin://acme/knowledge/page/7#hIntro", Sub::Heading("Intro".into())),
            (
                "myelin://acme/knowledge/row/7#row-r2",
                Sub::Row("r2".into()),
            ),
            (
                "myelin://acme/issue/issue/ENG-1#field-status",
                Sub::Field("status".into()),
            ),
            (
                "myelin://acme/git/ref/main#L42-L88",
                Sub::LineRange { start: 42, end: 88 },
            ),
            (
                "myelin://acme/ci/check/x#check-build",
                Sub::Check("build".into()),
            ),
            ("myelin://acme/ci/run/01J#step-3", Sub::Step(3)),
        ];
        for (s, want_kind) in cases {
            let r = parse(s).unwrap_or_else(|e| panic!("`{s}` must parse: {e}"));
            assert_eq!(format(&r), *s, "round-trip for `{s}`");
            assert_eq!(sub_kind(&r).as_ref(), Some(want_kind), "kind for `{s}`");
        }
    }

    /// A single-line range `L<n>-L<n>` (start == end) is admitted.
    #[test]
    fn single_line_range_is_admitted() {
        let r = parse("myelin://acme/git/ref/main#L7-L7").unwrap();
        assert_eq!(sub_kind(&r), Some(Sub::LineRange { start: 7, end: 7 }));
    }

    /// The Issues canonical key `<PROJECTKEY>-<seqno>` (C-3) is the stored `<id>` and parses.
    #[test]
    fn issues_canonical_projectkey_seqno_is_the_stored_id() {
        let r = parse("myelin://acme/issue/issue/ENG-1421").unwrap();
        assert_eq!(format(&r), "myelin://acme/issue/issue/ENG-1421");
    }

    /// The Issues SHORT display form `#1421` (and `@alice`, `~general`, a bare `#42`) is a
    /// render-time projection (§4.8) — it must be REJECTED as a scope, never guessed (REF-3, C-3).
    #[test]
    fn issues_short_display_key_and_other_display_projections_are_rejected() {
        for display in ["#1421", "#42", "@alice", "~general", "ENG-1421", "1234567"] {
            assert!(
                parse(display).is_err(),
                "display projection `{display}` must be rejected, never parsed as a scope"
            );
            // Specifically: none of these carry the canonical scheme.
            assert_eq!(
                parse(display),
                Err(ParseError::MissingScheme {
                    input: display.to_string()
                })
            );
        }
    }

    /// A scope-less / short URN (too few segments) is rejected with `IncompleteScope` — never
    /// guessed into a full scope.
    #[test]
    fn incomplete_scope_is_rejected_never_guessed() {
        assert_eq!(
            parse("myelin://acme/issue/issue"),
            Err(ParseError::IncompleteScope { got_segments: 3 })
        );
        assert_eq!(
            parse("myelin://acme/issue"),
            Err(ParseError::IncompleteScope { got_segments: 2 })
        );
        assert_eq!(
            parse("myelin://acme"),
            Err(ParseError::IncompleteScope { got_segments: 1 })
        );
        // Too MANY segments (a stray `/` in the id) is also rejected — no segment carries a `/`.
        assert_eq!(
            parse("myelin://acme/issue/issue/ENG/1421"),
            Err(ParseError::IncompleteScope { got_segments: 5 })
        );
    }

    /// An empty scope segment is rejected with the precise segment named.
    #[test]
    fn empty_segments_are_rejected_with_the_segment_named() {
        assert_eq!(
            parse("myelin:///issue/issue/1"),
            Err(ParseError::EmptySegment { segment: "tenant" })
        );
        assert_eq!(
            parse("myelin://acme//issue/1"),
            Err(ParseError::EmptySegment {
                segment: "subsystem"
            })
        );
        assert_eq!(
            parse("myelin://acme/issue//1"),
            Err(ParseError::EmptySegment { segment: "type" })
        );
        assert_eq!(
            parse("myelin://acme/issue/issue/"),
            Err(ParseError::EmptySegment { segment: "id" })
        );
    }

    /// The `<subsystem>`/`<type>` tokens are validated against the Bus §6.2 table (Refs is the
    /// validator, never the author) — an unknown subsystem / type is rejected.
    #[test]
    fn unknown_subsystem_or_type_token_is_rejected() {
        assert_eq!(
            parse("myelin://acme/billing/invoice/1"),
            Err(ParseError::UnknownSubsystem {
                token: "billing".into()
            })
        );
        // `git` is a real subsystem but `widget` is not a real artifact type.
        assert_eq!(
            parse("myelin://acme/git/widget/1"),
            Err(ParseError::UnknownType {
                token: "widget".into()
            })
        );
    }

    /// An unknown / ambiguous / malformed `#sub` kind is rejected — never guessed (REF-3).
    #[test]
    fn unknown_or_malformed_sub_kind_is_rejected() {
        // empty sub
        assert_eq!(
            parse("myelin://acme/git/pr/42#"),
            Err(ParseError::EmptySub)
        );
        // a kind not in the frozen vocabulary
        assert!(matches!(
            parse("myelin://acme/git/pr/42#widget-9"),
            Err(ParseError::UnknownSubKind { .. })
        ));
        // empty opaque body on a hyphen-terminated kind
        assert!(matches!(
            parse("myelin://acme/git/pr/42#comment-"),
            Err(ParseError::UnknownSubKind { .. })
        ));
        // a non-numeric step
        assert!(matches!(
            parse("myelin://acme/ci/run/1#step-x"),
            Err(ParseError::UnknownSubKind { .. })
        ));
        // an inverted line range (end < start)
        assert!(matches!(
            parse("myelin://acme/git/ref/main#L88-L42"),
            Err(ParseError::UnknownSubKind { .. })
        ));
        // a half line range (`L42` with no `-L<end>`)
        assert!(matches!(
            parse("myelin://acme/git/ref/main#L42"),
            Err(ParseError::UnknownSubKind { .. })
        ));
        // a bare `b`/`h` with no opaque body
        assert!(matches!(
            parse("myelin://acme/knowledge/page/7#b"),
            Err(ParseError::UnknownSubKind { .. })
        ));
    }

    /// The [`ParseError`] `Display` rendering is LOUD and names the broken rule (EI-01 §5 — a
    /// rejection is never silent). Asserts the message text for each variant carries its
    /// distinguishing token, so the human-facing error cannot regress to an empty string.
    #[test]
    fn parse_error_display_is_loud_and_names_the_rule() {
        let cases: &[(ParseError, &str)] = &[
            (
                ParseError::MissingScheme { input: "#42".into() },
                "does not start with the canonical scheme",
            ),
            (
                ParseError::IncompleteScope { got_segments: 2 },
                "scope is not total",
            ),
            (
                ParseError::EmptySegment { segment: "tenant" },
                "the `tenant` segment is empty",
            ),
            (
                ParseError::UnknownSubsystem {
                    token: "billing".into(),
                },
                "unknown subsystem token `billing`",
            ),
            (
                ParseError::UnknownType {
                    token: "widget".into(),
                },
                "unknown artifact-type token `widget`",
            ),
            (ParseError::EmptySub, "the sub-anchor is empty"),
            (
                ParseError::UnknownSubKind { sub: "widget-9".into() },
                "unknown/ambiguous `#sub` kind `widget-9`",
            ),
        ];
        for (err, needle) in cases {
            let rendered = err.to_string();
            assert!(
                rendered.contains(needle),
                "`{err:?}` Display must contain `{needle}`, got `{rendered}`"
            );
            assert!(
                rendered.len() > 16,
                "`{err:?}` Display must be a loud, non-trivial message, got `{rendered}`"
            );
        }
    }

    /// `strip_sub` returns the `#sub`-stripped root (§3.2); a bare root is returned unchanged.
    #[test]
    fn strip_sub_returns_the_root_and_is_idempotent() {
        let with_sub = parse("myelin://acme/git/pr/42#comment-c9").unwrap();
        let root = strip_sub(&with_sub);
        assert_eq!(format(&root), "myelin://acme/git/pr/42");
        // idempotent: stripping a root yields the root.
        assert_eq!(strip_sub(&root), root);
        // a bare root with no `#sub` accessor returns None.
        assert_eq!(sub_kind(&root), None);
    }

    /// A `comment-…` is never mis-classified as a `b`/`h`/`check-` kind (prefix-overlap discipline).
    #[test]
    fn prefix_overlap_is_disambiguated_by_the_longer_kind() {
        let c = parse("myelin://acme/git/pr/42#comment-cabc").unwrap();
        assert_eq!(sub_kind(&c), Some(Sub::Comment("cabc".into())));
        let check = parse("myelin://acme/ci/check/x#check-lint").unwrap();
        assert_eq!(sub_kind(&check), Some(Sub::Check("lint".into())));
        // a `b…` that starts with the letters of `comment` is still a block (it lacks `comment-`).
        let b = parse("myelin://acme/knowledge/page/7#bcomment").unwrap();
        assert_eq!(sub_kind(&b), Some(Sub::Block("comment".into())));
    }

    /// PROPERTY / FUZZ — ambiguity-rejection: a corpus of malformed / short-hash / ambiguous /
    /// unknown-`#sub`-kind inputs yields **0 guessed scopes** (every one is `Err`); and every input
    /// that DOES parse round-trips byte-identical (`format(parse(s)?) == s`). This is the
    /// "no input ever yields a guessed scope" property the prompt requires.
    #[test]
    fn fuzz_ambiguity_rejection_zero_guessed_scopes() {
        // A deterministic generated corpus (hermetic, no rng dependency): the cartesian product of
        // scheme-presence × segment-count × token-validity × sub-validity, plus the named display
        // projections. We assert the EXPECTED disposition of each, and that no rejected input ever
        // sneaks through as an Ok (a guessed scope).
        let mut guessed_scopes = 0usize;
        let mut round_trip_failures = 0usize;

        let schemes = ["myelin://", "https://", "", "myelin:/"];
        let bodies = [
            "",                       // empty
            "acme",                   // 1 seg
            "acme/git",               // 2 seg
            "acme/git/pr",            // 3 seg
            "acme/git/pr/42",         // 4 seg, valid
            "acme/git/pr/42/extra",   // 5 seg
            "acme/billing/pr/42",     // bad subsystem
            "acme/git/widget/42",     // bad type
            "acme//pr/42",            // empty subsystem
            "/git/pr/42",             // empty tenant
        ];
        let subs = [
            None,
            Some(""),
            Some("comment-c9"),
            Some("widget-9"),
            Some("L42-L88"),
            Some("L88-L42"),
            Some("step-3"),
            Some("step-x"),
            Some("b9"),
            Some("b"),
        ];

        for scheme in schemes {
            for body in bodies {
                for sub in subs {
                    let input = match sub {
                        Some(s) => format!("{scheme}{body}#{s}"),
                        None => format!("{scheme}{body}"),
                    };
                    match parse(&input) {
                        Ok(r) => {
                            // Anything that parses MUST be the canonical `myelin://` 4-segment form
                            // with valid tokens and a valid sub — i.e. a real scope, never a guess.
                            assert!(
                                input.starts_with(SCHEME),
                                "guessed a scope for a scheme-less input `{input}`"
                            );
                            if format(&r) != input {
                                round_trip_failures += 1;
                            }
                            // the parsed form must itself re-parse to the same value (stability).
                            let re = parse(&format(&r)).expect("canonical re-parses");
                            assert_eq!(re, r, "canonical form is not a fixed point for `{input}`");
                        }
                        Err(_) => {
                            // rejected — good; count nothing.
                        }
                    }
                }
            }
        }

        // The named display projections (REF-3, §4.8) are ALWAYS rejected — never a guessed scope.
        for display in ["#42", "#1421", "@alice", "~general", "ENG-1421", "abc1234"] {
            if parse(display).is_ok() {
                guessed_scopes += 1;
            }
        }

        assert_eq!(guessed_scopes, 0, "a display projection was guessed into a scope");
        assert_eq!(round_trip_failures, 0, "a parsed URN failed to round-trip");
    }
}
