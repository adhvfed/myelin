//! # The CDC pair for contract 2.9 — the event taxonomy grammar + token table (EB-02 / P-042)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 2.9
//! (Event taxonomy + token table — `<subsystem>.<artifact_type>.<event_name>`; **+ new tokens**
//! `ci.check.updated`, `ci.result` (X-1), the `initiative` type token). Owning architecture:
//! `event-bus.md` §6.1 (the dotted-name grammar — the AUTHORITY), §6.2 (subsystem/type tokens +
//! `initiative`), §6.3 (the check-seam tokens), §6.4 (the seed names). Reconciliation:
//! `00-reconciliation-decisions.md` §2.
//!
//! ## The contract this pair pins (one grammar, no per-subsystem drift)
//! Row 2.9 is the seam between the side that EMITS an event with a canonical dotted `type` name
//! (the **PROVIDER** — every producing subsystem) and the side that VALIDATES / consumes a `type`
//! against the one frozen grammar (the **CONSUMER** — the Bus grammar validator + every
//! permission-aware reader). The frozen behaviour both sides agree on:
//!
//! - the PROVIDER only ever emits `type` names of the §6.1 shape (lowercase, singular,
//!   past-tense, `[a-z][a-z0-9_]*` tokens, 2 segments min / 3 when an artifact type clarifies,
//!   the leading token a canonical §6.2 subsystem) — including the three NEW tokens
//!   `ci.check.updated`, `ci.result`, `issue.initiative.created`;
//! - the CONSUMER (the validator) ADMITS every such canonical name and REJECTS a malformed one
//!   LOUDLY (with the specific rule broken) — never silently coercing a bad name.
//!
//! This is the dedicated 2.9 provider+consumer pair the EB-02 TESTS field names; the focused
//! reject/admit fixture pair (the §6.1 ratchet) lives in `taxonomy.rs::tests`.

use myelin_events::taxonomy::{self, new_tokens};
use myelin_events::{
    validate_event_type, ArtifactRef, EventDraft, EventType, SEED_EVENT_NAMES, SUBSYSTEM_TOKENS,
};
use myelin_events::{AggregateKey, DataRole, Visibility};

/// **PROVIDER side of 2.9** — a producing subsystem authors an [`EventDraft`] whose `type_` is a
/// canonical dotted name. This models the emit side: every producer that builds a draft sets a
/// `type` of the §6.1 shape (here a representative seed name + each of the three new tokens). The
/// provider's promise is that the `type` it puts on the wire is grammar-conformant.
fn provider_emits_typed_draft(type_name: &str) -> EventDraft {
    EventDraft {
        type_: EventType(type_name.to_string()),
        subject: ArtifactRef("myelin://acme/ci/run/01J".into()),
        aggregate: AggregateKey("ci:01J".into()),
        payload: serde_json::json!({ "ref": "myelin://acme/ci/run/01J" }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// **CONSUMER side of 2.9** — the Bus grammar validator is what every consumer (and the Bus
/// itself) runs a `type` through. It ADMITS a canonical name and REJECTS a malformed one. This is
/// the consumer's promise: it never silently accepts a non-conformant `type`.
fn consumer_admits(draft: &EventDraft) -> bool {
    validate_event_type(&draft.type_.0).is_ok()
}

/// The 2.9 pair, end-to-end: a PROVIDER emits each of the three NEW tokens (the EB-02 headline)
/// as a canonical-shape draft, and the CONSUMER (the validator) admits every one of them.
#[test]
fn cdc_2_9_provider_emits_new_tokens_consumer_admits_them() {
    for token in [
        new_tokens::CI_CHECK_UPDATED,
        new_tokens::CI_RESULT,
        new_tokens::ISSUE_INITIATIVE_CREATED,
    ] {
        let draft = provider_emits_typed_draft(token);
        assert!(
            consumer_admits(&draft),
            "consumer (validator) wrongly rejected the new token `{token}`"
        );
    }
}

/// The 2.9 pair across the WHOLE seed: every PROVIDER-authored seed name is ADMITTED by the
/// CONSUMER validator (the §6.4 seed is grammar-conformant by construction — 0 false rejects).
#[test]
fn cdc_2_9_provider_seed_names_all_admitted_by_consumer() {
    for name in SEED_EVENT_NAMES {
        let draft = provider_emits_typed_draft(name);
        assert!(
            consumer_admits(&draft),
            "consumer (validator) wrongly rejected seed name `{name}`"
        );
    }
}

/// The CONSUMER validator REJECTS a malformed `type` a (mis)behaving provider might emit — LOUDLY
/// (the specific [`taxonomy::TaxonomyError`] for the broken rule), never silently coerced. This is
/// the negative half of the seam: the validator is a real gate, not a pass-through.
#[test]
fn cdc_2_9_consumer_rejects_a_malformed_type_loudly() {
    // An uppercase / present-tense / unknown-subsystem name — three distinct providers' mistakes.
    let bad = provider_emits_typed_draft("CI.Run.Started");
    assert!(!consumer_admits(&bad), "validator must reject `CI.Run.Started`");

    let present = provider_emits_typed_draft("ci.run.start");
    assert!(matches!(
        validate_event_type(&present.type_.0),
        Err(taxonomy::TaxonomyError::PresentTenseVerb { .. })
    ));

    let unknown = provider_emits_typed_draft("billing.invoice.created");
    assert!(matches!(
        validate_event_type(&unknown.type_.0),
        Err(taxonomy::TaxonomyError::UnknownSubsystem { .. })
    ));
}

/// The subsystem token set the provider+consumer agree on is the §6.2 canonical singular set —
/// the names anchor (X-5) both sides bind to.
#[test]
fn cdc_2_9_subsystem_token_set_is_the_shared_anchor() {
    assert_eq!(
        SUBSYSTEM_TOKENS,
        &["git", "ci", "issue", "knowledge", "chat", "identity", "refs"]
    );
}
