//! # The Issues CDC pair for contract 5.1 — the `<PROJECTKEY>-<seqno>` canonical key (ISS-P08 / P-374)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 5.1 (the
//! Issues id grammar frozen `<PROJECTKEY>-<seqno>` as the stored canonical key; `#1421` is the
//! render-time display projection, REF-3). Owning architecture:
//! `planning/04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md`
//! §4 (the Hi/Lo allocator mints the canonical `<id>`). Reconciliation:
//! `00-reconciliation-decisions.md` REF-3 ("the human key IS the ArtifactRef id"; the `#<seqno>`
//! display form is render-time only, never stored as the link).
//!
//! ## The contract this pair pins (the Issues key is a real URN `<id>`, the `#form` is not)
//! Row 5.1 (Issues slice) is the seam between the side that **PRODUCES** an Issues canonical key (the
//! **PROVIDER** — the Hi/Lo allocator [`myelin_issues::HiLoKeyAllocator`] minting
//! `<PROJECTKEY>-<seqno>` as the stored `<id>` segment of the issue's `ArtifactRef`) and the side
//! that **CONSUMES** that key off the wire (the **CONSUMER** — any service that parses the issue's
//! `ArtifactRef` through the ONE `myelin-refs` codec, REF-3). The frozen behaviour both sides agree on:
//!
//! - the PROVIDER (the allocator) only ever mints a canonical `<PROJECTKEY>-<seqno>` and embeds it as
//!   the `<id>` of a fully-scoped `myelin://<tenant>/issue/issue/<key>` URN — never the short
//!   display projection `#<seqno>` as a stored scope;
//! - the CONSUMER (`myelin_refs::parse`) ADMITS the issue's stored canonical URN and round-trips it
//!   byte-identical (`format(parse(s)?) == s`), and REJECTS the render-time `#<seqno>` display form
//!   LOUDLY (a typed `ParseError`) — never guessing a scope (REF-3).
//!
//! This is the dedicated 5.1 Issues provider+consumer pair the ISS-P08 TESTS field names (the
//! `<PROJECTKEY>-<seqno>` key grammar); the allocator's gap/monotonic/isolation invariants are pinned
//! in `keys.rs::tests` + the create-storm drill `drill_iss_d4_create_storm.rs`.

use myelin_issues::{CanonicalKey, HiLoKeyAllocator, InMemoryPrefixCounter};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

/// **PROVIDER side of 5.1 (Issues)** — the Hi/Lo allocator mints a canonical key and embeds it as the
/// stored `<id>` of a fully-scoped issue `ArtifactRef`. The provider's promise: the string it puts on
/// the wire is `myelin://<tenant>/issue/issue/<PROJECTKEY>-<seqno>` (the canonical id), never the
/// `#<seqno>` display projection.
fn provider_mints_issue_ref(prefix: &str) -> (CanonicalKey, String) {
    let allocator = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let key = allocator.allocate(&tenant(), prefix).expect("allocate");
    let r = key.issue_artifact_ref(&tenant());
    (key, r.0)
}

/// **CONSUMER side of 5.1 (Issues)** — a service that links `myelin-refs` parses the issue's stored
/// `ArtifactRef` off the wire and round-trips it byte-identical. Returns the re-formatted canonical
/// string the consumer recovers.
fn consumer_round_trips(stored: &str) -> String {
    let r = myelin_refs::parse(stored).expect("the consumer admits the stored canonical issue id");
    myelin_refs::format(&r)
}

#[test]
fn provider_mints_projectkey_seqno_consumer_round_trips_byte_identical() {
    let (key, stored) = provider_mints_issue_ref("ENG");
    // the stored <id> segment is the canonical <PROJECTKEY>-<seqno> (5.1).
    assert_eq!(key.render(), "ENG-1");
    assert_eq!(stored, "myelin://acme/issue/issue/ENG-1");
    // the consumer admits it and round-trips byte-identical (one codec, no Issues-side drift).
    assert_eq!(
        consumer_round_trips(&stored),
        stored,
        "format(parse(s)) == s — the stored canonical key round-trips"
    );
}

#[test]
fn render_time_display_form_is_rejected_as_a_scope_ref_3() {
    let (key, _) = provider_mints_issue_ref("OPS");
    // the #<seqno> short form is render-time only, NOT a parseable scope (REF-3).
    let display = key.render_display_key();
    assert_eq!(display, "#1");
    assert!(
        myelin_refs::parse(&display).is_err(),
        "the #<seqno> display projection is render-time only — the consumer rejects it as a scope"
    );
    // nor is the bare canonical key (without the URN scope) a parseable scope — the stored link is
    // always the full URN, the key is only its <id> segment.
    assert!(
        myelin_refs::parse(&key.render()).is_err(),
        "the bare <PROJECTKEY>-<seqno> is an <id> segment, not a standalone scope"
    );
}

#[test]
fn distinct_prefixes_mint_distinct_canonical_urns() {
    // per-prefix isolation reflected in the canonical URN: two prefixes never collide on the <id>.
    let allocator = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let eng = allocator.allocate(&tenant(), "ENG").unwrap();
    let ops = allocator.allocate(&tenant(), "OPS").unwrap();
    assert_ne!(eng.render(), ops.render(), "ENG-1 != OPS-1");
    assert_eq!(
        eng.issue_artifact_ref(&tenant()).0,
        "myelin://acme/issue/issue/ENG-1"
    );
    assert_eq!(
        ops.issue_artifact_ref(&tenant()).0,
        "myelin://acme/issue/issue/OPS-1"
    );
}
