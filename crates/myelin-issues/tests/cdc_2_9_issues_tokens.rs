//! # The CDC pair for contract 2.9 — Issues' `issue.*` token registration (ISS-P03 / P-242)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 2.9
//! (Event taxonomy + token table — `<subsystem>.<artifact_type>.<event_name>`; **each subsystem
//! completes its list**; **+ the `initiative` type token**). Owning architecture: Issues
//! `04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md` §1 (the
//! complete `issue.*` taxonomy Issues OWNS incl. `initiative`); Bus `event-bus.md` §6.1 (the grammar
//! — the AUTHORITY), §6.2 (the subsystem/type token table incl. `initiative`). Reconciliation:
//! `00-reconciliation-decisions.md` §2 (the `initiative` token registered).
//!
//! ## The seam this pair pins (Issues registers; the Bus owns the grammar)
//! Row 2.9 is the seam between the side that OWNS + registers a subsystem's dotted-name list (the
//! **PROVIDER** — here Issues, [`myelin_issues::events`]) and the side that owns the grammar +
//! validates every registered name (the **CONSUMER** — the one Bus validator,
//! [`myelin_events::validate_event_type`]). The frozen behaviour both sides agree on:
//!
//! - the PROVIDER (Issues) registers its COMPLETE v1 `issue.*` list ([`ISSUE_EVENT_TOKENS`]) — every
//!   name of the §6.1 shape (lowercase, singular, past-tense, `[a-z][a-z0-9_]*` tokens, 2-or-3
//!   segments, the leading token the canonical `issue` subsystem), **including the registered
//!   `initiative` type token** — and registers NO foreign-subsystem token (the cross-subsystem
//!   reflexes are CONSUMED, never originated);
//! - the CONSUMER (the Bus validator) ADMITS every registered issue name (**0 ungrammatical**) and
//!   would REJECT a malformed issue-shaped name LOUDLY — Issues does not get to author the grammar.
//!
//! Plus the **2.1 EventEnvelope unit anchor** half: an issue payload in the FROZEN units (durations
//! in seconds, timestamps RFC-3339 UTC) validates, and a **seconds-vs-millis** fixture is REJECTED.
//! This is the dedicated 2.9 (+ 2.1 units) provider+consumer pair the ISS-P03 TESTS field names; the
//! focused per-token round-trip + unit fixtures live in `myelin_issues::events::tests`.

use myelin_events::{validate_event_type, TaxonomyError};
use myelin_issues::events::unit_check::{validate_issue_payload_units, UnitError};
use myelin_issues::events::{
    register_issue_tokens, INITIATIVE_HEALTH_CHANGED, ISSUE_EVENT_TOKENS, ISSUE_TRANSITIONED,
    ISSUE_UPDATED, RELATION_CREATED, SLA_AT_RISK,
};

/// **PROVIDER side of 2.9** — Issues, the owner, registers its complete `issue.*` list. The
/// provider's promise: every `type` token it puts on the wire is one of these, grammar-conformant by
/// construction. This returns the registry the consumer validates.
fn provider_registers_issue_tokens() -> &'static [&'static str] {
    ISSUE_EVENT_TOKENS
}

/// **CONSUMER side of 2.9** — the one Bus grammar validator every consumer (and the Bus itself) runs
/// a `type` through. It ADMITS a canonical name and REJECTS a malformed one. The consumer's promise:
/// it never silently accepts a non-conformant `type`.
fn consumer_admits(type_name: &str) -> bool {
    validate_event_type(type_name).is_ok()
}

/// The 2.9 pair, end-to-end: the PROVIDER (Issues) registers its complete list — incl. the
/// `initiative` type token — and the CONSUMER (the Bus validator) admits **every** registered token
/// (**0 ungrammatical**). This is the dated green artifact the ISS-P03 GATE names.
#[test]
fn cdc_2_9_issues_provider_registers_consumer_admits_every_token() {
    for &tok in provider_registers_issue_tokens() {
        assert!(
            consumer_admits(tok),
            "consumer (Bus validator) wrongly REJECTED registered issue token `{tok}`: {:?}",
            validate_event_type(tok)
        );
    }
    // The whole-list registration helper is the provider's one-call assertion (0 ungrammatical).
    assert!(
        register_issue_tokens().is_ok(),
        "Issues' register_issue_tokens() must be green: {:?}",
        register_issue_tokens()
    );
}

/// The CONSUMER validator REJECTS a malformed issue-shaped `type` LOUDLY (the specific
/// [`TaxonomyError`] for the broken rule), never silently coerced — Issues does NOT get to author
/// the grammar. The negative half of the seam: the validator is a real gate, not a pass-through.
#[test]
fn cdc_2_9_consumer_rejects_a_malformed_issue_type_loudly() {
    // present-tense verb (issue.issue.transition, not transitioned)
    assert!(matches!(
        validate_event_type("issue.issue.transition"),
        Err(TaxonomyError::PresentTenseVerb { .. })
    ));
    // plural artifact-type token (issue.comments.created)
    assert!(matches!(
        validate_event_type("issue.comments.created"),
        Err(TaxonomyError::PluralToken { .. })
    ));
    // uppercase token
    assert!(matches!(
        validate_event_type("issue.Issue.created"),
        Err(TaxonomyError::BadToken { .. })
    ));
}

/// The PROVIDER registers NO foreign-subsystem token — the cross-subsystem reflexes
/// (`git.branch.created`, `git.pr.merged`, `ci.check.updated`, `chat.message.created`,
/// `identity.member.*` — arch §1.1) are CONSUMED, never originated by Issues (the dependency is
/// acyclic: those subsystems emit, Issues reads). The acyclic-producer invariant (EI-02 §3), pinned
/// at the contract seam.
#[test]
fn cdc_2_9_issues_registers_only_its_own_subsystem() {
    for &tok in provider_registers_issue_tokens() {
        assert!(
            tok.starts_with("issue."),
            "issue registered the foreign-subsystem token `{tok}` (must own `issue.*` only)"
        );
    }
    // The load-bearing cross-subsystem-consumed tokens are present under their named constants (the
    // names anchor X-5): the rollup/feeder input, the transition (category), the TE-7 typed edge,
    // the SLA feed, and the registered `initiative` type token.
    for tok in [
        ISSUE_UPDATED,
        ISSUE_TRANSITIONED,
        RELATION_CREATED,
        SLA_AT_RISK,
        INITIATIVE_HEALTH_CHANGED,
    ] {
        assert!(
            provider_registers_issue_tokens().contains(&tok),
            "`{tok}` must be registered (the names anchor X-5)"
        );
    }
}

/// **The 2.1 EventEnvelope unit-anchor half (the ISS-P03 TESTS "EventEnvelope units validate"
/// clause).** PROVIDER: an issue payload authored in the FROZEN units — durations in **seconds**,
/// timestamps RFC-3339 UTC — validates. CONSUMER (the unit check): a **seconds-vs-millis** fixture
/// is REJECTED loudly. The frozen units (durations in seconds; timestamps RFC-3339 UTC) are the
/// green artifact; the millis drift is the proof the anchor is a real gate.
#[test]
fn cdc_2_1_issue_payload_units_validate_and_seconds_vs_millis_is_rejected() {
    // GREEN: an issue.sla.started payload in the frozen units validates.
    let frozen = serde_json::json!({
        "issue": "myelin://acme/issue/issue/ENG-1421",
        "target_seconds": 86_400,            // SLA target in SECONDS (the frozen unit)
        "stale_after_seconds": 2_592_000,    // the trigger stale_after in SECONDS (§10)
        "started_at": "2026-06-21T10:00:00Z" // RFC-3339 UTC (the frozen timestamp unit)
    });
    assert_eq!(
        validate_issue_payload_units(&frozen),
        Ok(()),
        "an issue payload in the frozen units (seconds + RFC-3339 UTC) must validate"
    );

    // RED: the SAME payload with the duration in MILLIS — the seconds-vs-millis drift, rejected.
    let drifted = serde_json::json!({
        "issue": "myelin://acme/issue/issue/ENG-1421",
        "target_millis": 86_400_000,         // SECONDS-VS-MILLIS DRIFT — must be rejected
        "started_at": "2026-06-21T10:00:00Z"
    });
    assert_eq!(
        validate_issue_payload_units(&drifted),
        Err(UnitError::DurationNotSeconds {
            field: "target_millis".into()
        }),
        "a millis-expressed duration must be REJECTED (the frozen unit is seconds)"
    );
}
