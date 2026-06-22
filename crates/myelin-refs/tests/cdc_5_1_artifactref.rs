//! # The CDC pair for contract 5.1 — `ArtifactRef` parse/format (REF-P1 / P-052)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 5.1
//! (`ArtifactRef` — `myelin://<tenant>/<subsystem>/<type>/<id>[#sub]`; `parse`/`format` reject
//! ambiguity; the Issues `<PROJECTKEY>-<seqno>` key frozen as the stored canonical id; `#1421` is
//! the render-time display projection, REF-3). Owning architecture: `reference-graph.md` §3.1
//! (the URN + Issues key, C-3), §3.5 (the frozen `#sub` grammar, C-1/C-6), §4.8 (display keys
//! render-time only). Reconciliation: `00-reconciliation-decisions.md` C-1/C-3/C-6.
//!
//! ## The contract this pair pins (one URN codec, no per-service drift, REF-3)
//! Row 5.1 is the seam between the side that **PRODUCES** an `ArtifactRef` as a canonical URN string
//! (the **PROVIDER** — any subsystem that mints a ref into an `EventEnvelope.subject`, a content
//! `artifact_ref` node, a stored edge) and the side that **CONSUMES** an `ArtifactRef` string off
//! the wire by parsing it (the **CONSUMER** — every service that links `myelin-refs` instead of
//! re-implementing URN handling, REF-3). The frozen behaviour both sides agree on:
//!
//! - the PROVIDER only ever puts a **canonical**, totally-scoped URN on the wire
//!   (`format(ref)` of a parsed ref — four explicit segments, valid Bus §6.2 tokens, a frozen
//!   `#sub` kind if any); it never emits a scope-less / short-hash display projection as a stored
//!   scope;
//! - the CONSUMER (`parse`) ADMITS every canonical URN the provider emits and **round-trips it
//!   byte-identical** (`format(parse(s)?) == s`), and REJECTS an ambiguous / display-projection /
//!   unknown-`#sub`-kind input LOUDLY (a typed [`myelin_refs::ParseError`]) — **never guessing a
//!   scope** (REF-3).
//!
//! This is the dedicated 5.1 provider+consumer pair the REF-P1 TESTS field names; the focused
//! reject/admit + fuzz fixtures live in `parse.rs::tests`.

use myelin_refs::{format, parse, strip_sub, sub_kind, ArtifactRef, ParseError, Sub};

/// **PROVIDER side of 5.1** — a producing subsystem mints a canonical `ArtifactRef` and puts its
/// **formatted** string on the wire (the `EventEnvelope.subject`, an `artifact_ref` content node, a
/// stored edge `source`/`target`). The provider's promise: the string it emits is the canonical
/// rendering of a fully-scoped ref (it parses + re-formats, never hand-builds an ambiguous string).
fn provider_mints_canonical_urn(raw: &str) -> String {
    let r = parse(raw).expect("a provider only mints a well-formed, fully-scoped ref");
    format(&r)
}

/// **CONSUMER side of 5.1** — a service that links `myelin-refs` parses an `ArtifactRef` off the
/// wire. The consumer's promise: it admits a canonical URN (yielding a usable `ArtifactRef`) and it
/// never silently accepts an ambiguous one (it returns a typed error).
fn consumer_parses(on_the_wire: &str) -> Result<ArtifactRef, ParseError> {
    parse(on_the_wire)
}

/// The 5.1 pair, end-to-end: a PROVIDER mints each representative canonical URN (one per Bus §6.2
/// subsystem, the Issues `<PROJECTKEY>-<seqno>` key, and a representative `#sub` of each frozen kind)
/// and the CONSUMER parses + round-trips every one byte-identical (0 drift across the seam).
#[test]
fn cdc_5_1_provider_mints_consumer_parses_round_trip() {
    let canonical = [
        // one per canonical Bus §6.2 subsystem token
        "myelin://acme/git/pr/4291",
        "myelin://acme/ci/run/01J0RUN",
        "myelin://acme/issue/issue/ENG-1421", // the frozen Issues key (C-3)
        "myelin://acme/issue/initiative/PLAT-9", // the new §6.2 type token
        "myelin://acme/knowledge/page/7c2",
        "myelin://acme/chat/message/01J0MSG",
        "myelin://acme/identity/member/p-7",
        "myelin://acme/refs/edge/abc123",
        // a representative `#sub` of each frozen §3.5 kind
        "myelin://acme/git/pr/4291#comment-c9",
        "myelin://acme/chat/message/1#thread-t7",
        "myelin://acme/chat/message/1#message-m3",
        "myelin://acme/knowledge/page/7#b9",
        "myelin://acme/knowledge/page/7#hIntro",
        "myelin://acme/knowledge/row/7#row-r2",
        "myelin://acme/issue/issue/ENG-1#field-status",
        "myelin://acme/git/ref/main#L42-L88",
        "myelin://acme/ci/check/x#check-build",
        "myelin://acme/ci/run/01J#step-3",
    ];
    for raw in canonical {
        // PROVIDER → wire
        let on_the_wire = provider_mints_canonical_urn(raw);
        assert_eq!(
            on_the_wire, raw,
            "provider canonical form drifted for `{raw}`"
        );
        // wire → CONSUMER, round-trip byte-identical
        let parsed = consumer_parses(&on_the_wire)
            .unwrap_or_else(|e| panic!("consumer rejected provider URN `{on_the_wire}`: {e}"));
        assert_eq!(
            format(&parsed),
            raw,
            "round-trip not byte-identical for `{raw}`"
        );
    }
}

/// The 5.1 pair's NEGATIVE half: the CONSUMER REJECTS a display projection / ambiguous input a
/// (mis)behaving caller might pass — LOUDLY (the specific [`ParseError`]), never guessing a scope
/// (REF-3, §4.8). This is the cardinal Refs invariant: a short-hash `#1421` is render-time, never a
/// stored scope.
#[test]
fn cdc_5_1_consumer_rejects_display_projections_loudly() {
    // The Issues short display `#1421` (and the bare `#42`, `@alice`, `~general`) — never a scope.
    for display in ["#1421", "#42", "@alice", "~general", "ENG-1421"] {
        assert_eq!(
            consumer_parses(display),
            Err(ParseError::MissingScheme {
                input: display.to_string()
            }),
            "consumer must reject the display projection `{display}` (REF-3)"
        );
    }
    // A scope-less / short URN is rejected as incomplete — never guessed into a full scope.
    assert_eq!(
        consumer_parses("myelin://acme/issue"),
        Err(ParseError::IncompleteScope { got_segments: 2 })
    );
    // An unknown subsystem token is rejected (Refs validates the Bus §6.2 table; never authors).
    assert!(matches!(
        consumer_parses("myelin://acme/billing/invoice/1"),
        Err(ParseError::UnknownSubsystem { .. })
    ));
    // An unknown `#sub` kind is rejected (the kind prefix is self-describing; no resolver → reject).
    assert!(matches!(
        consumer_parses("myelin://acme/git/pr/42#widget-9"),
        Err(ParseError::UnknownSubKind { .. })
    ));
}

/// The 5.1 pair's ROOT-rollup agreement: the CONSUMER derives the same `*_root` (the `#sub`-stripped
/// parent) the PROVIDER's edge builder will write into `source_root`/`target_root` (§3.2) — so a
/// sub-anchored ref rolls up to its parent identically on both sides of the seam.
#[test]
fn cdc_5_1_strip_sub_root_agreement() {
    let sub_ref = parse("myelin://acme/git/pr/42#comment-c9").unwrap();
    let root = strip_sub(&sub_ref);
    assert_eq!(format(&root), "myelin://acme/git/pr/42");
    // the sub kind the resolver dispatches on is the self-describing frozen kind.
    assert_eq!(sub_kind(&sub_ref), Some(Sub::Comment("c9".into())));
    // a bare root has no sub anchor.
    assert_eq!(sub_kind(&root), None);
}
