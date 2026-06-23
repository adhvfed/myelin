//! # The CDC pair for contract 2.9 — CI's `ci.*` taxonomy registered from the Control Plane
//! (CI-P7 / P-350, M4)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 2.9
//! (Event taxonomy + token table — `<subsystem>.<artifact_type>.<event_name>`; **each subsystem
//! completes + registers its list**). Owning architecture: CI
//! `04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md`
//! §1 (the complete `ci.*` taxonomy CI owns); Bus `event-bus.md` §6.1 (the grammar — the AUTHORITY),
//! §6.2 (the subsystem/type token table — `ci` canonical + the CI type tokens).
//!
//! ## The seam this pair pins (the CI Control Plane registers; the Bus owns the grammar)
//! Row 2.9 is the seam between the side that OWNS + registers a subsystem's dotted-name list (the
//! **PROVIDER** — here the CI Control Plane, [`myelin_ci_controlplane::events`], the outbox producer
//! / check emitter that is the proper home for CI's taxonomy registration) and the side that owns the
//! grammar + the §6.2 token table and validates every registered name (the **CONSUMER** — the one Bus
//! validator, [`myelin_events::validate_event_type`] + the §6.2 [`SUBSYSTEM_TOKENS`] table).
//!
//! The frozen behaviour both sides agree on:
//! - the PROVIDER (the CI Control Plane) registers CI's COMPLETE `ci.*` list (durable ∪ firehose) —
//!   every name §6.1-grammatical (lowercase, singular, past-tense, `[a-z][a-z0-9_]*` tokens, 2-or-3
//!   segments, leading `ci`) AND §6.2-conformant (the canonical `ci` subsystem token + a registered
//!   CI type token), and registers NO foreign-subsystem token (the acyclic-producer invariant);
//! - the CONSUMER (the Bus validator + the §6.2 token table) ADMITS every registered CI name (0
//!   ungrammatical, 0 §6.2-nonconforming) and would REJECT a malformed CI-shaped name LOUDLY — CI
//!   does not get to author the grammar.
//!
//! This is the dedicated 2.9 provider+consumer pair the CI-P7 TESTS field names. CI's canonical token
//! CONSTANTS are the one source of truth in [`myelin_ci_sandbox::events`] (the early M4 names freeze,
//! EB-27 / P-327); the control plane RE-EXPORTS + registers them (one list, no second token language).

use myelin_ci_controlplane::events::{
    ci_event_tokens, register_ci_taxonomy, validate_ci_type_token, CiTypeTokenError,
    CI_DURABLE_TOKENS, CI_FIREHOSE_TOKENS, CI_SUBSYSTEM_TOKEN,
};
use myelin_events::{validate_event_type, TaxonomyError, SUBSYSTEM_TOKENS};

/// **PROVIDER side of 2.9** — the CI Control Plane, the owner, registers CI's complete `ci.*` list
/// (durable ∪ firehose). The provider's promise: every `type` token it puts on the wire is one of
/// these, §6.1-grammatical AND §6.2-conformant by construction. Returns the registry the consumer
/// validates.
fn provider_registers_ci_tokens() -> Vec<&'static str> {
    ci_event_tokens().collect()
}

/// **CONSUMER side of 2.9** — the one Bus grammar validator (§6.1) + the §6.2 subsystem token table.
/// It ADMITS a canonical name (grammar + `ci` is a known subsystem token) and would REJECT a
/// malformed one. The consumer's promise: it never silently accepts a non-conformant `type`.
fn consumer_admits(type_name: &str) -> bool {
    validate_event_type(type_name).is_ok()
        && type_name
            .split('.')
            .next()
            .is_some_and(|head| SUBSYSTEM_TOKENS.contains(&head))
}

/// The 2.9 pair, end-to-end: the PROVIDER (the CI Control Plane) registers CI's complete list, and
/// the CONSUMER (the Bus validator + the §6.2 token table) admits **every** registered token — 0
/// ungrammatical, 0 §6.2-nonconforming. This is the dated green artifact the CI-P7 GATE names.
#[test]
fn cdc_2_9_ci_provider_registers_consumer_admits_every_token() {
    for tok in provider_registers_ci_tokens() {
        assert!(
            consumer_admits(tok),
            "consumer (Bus validator + §6.2 table) wrongly REJECTED registered ci token `{tok}`: {:?}",
            validate_event_type(tok)
        );
    }
    // The whole-list control-plane registration helper is the provider's one-call assertion: every
    // token passes BOTH the §6.1 grammar AND the §6.2 token table (0 ungrammatical, 0 nonconforming).
    assert_eq!(
        register_ci_taxonomy(),
        Ok(()),
        "the CI Control Plane register_ci_taxonomy() must be green: {:?}",
        register_ci_taxonomy()
    );
}

/// The CONSUMER REJECTS a malformed CI-shaped `type` LOUDLY (the specific [`TaxonomyError`] for the
/// broken §6.1 rule), never silently coerced — CI does NOT get to author the grammar. The negative
/// half of the seam: the validator is a real gate, not a pass-through.
#[test]
fn cdc_2_9_consumer_rejects_a_malformed_ci_type_loudly() {
    // present-tense verb (ci.run.start, not started)
    assert!(matches!(
        validate_event_type("ci.run.start"),
        Err(TaxonomyError::PresentTenseVerb { .. })
    ));
    // uppercase token
    assert!(matches!(
        validate_event_type("ci.Run.started"),
        Err(TaxonomyError::BadToken { .. })
    ));
    // hyphen (not [a-z0-9_])
    assert!(matches!(
        validate_event_type("ci.run-step.started"),
        Err(TaxonomyError::BadToken { .. })
    ));
}

/// **The §6.2 token-table half of the seam** (CI-P7's net-new contribution over the early grammar
/// freeze): the consumer-side §6.2 check rejects a non-`ci` subject and an unregistered CI type token
/// LOUDLY. CI registers its type list; it does not author a new type token at emit time.
#[test]
fn cdc_2_9_consumer_rejects_nonconforming_6_2_tokens_loudly() {
    // A foreign subsystem name is not a `ci` token (§6.2).
    assert!(matches!(
        validate_ci_type_token("git.pr.opened"),
        Err(CiTypeTokenError::NotCiSubsystem { .. })
    ));
    // A fabricated `ci.<type>.<event>` with an UNREGISTERED type token is rejected with its name.
    assert!(matches!(
        validate_ci_type_token("ci.widget.created"),
        Err(CiTypeTokenError::UnregisteredTypeToken { .. })
    ));
}

/// The PROVIDER registers ONLY its own subsystem — CI does NOT register a foreign-subsystem token
/// (no `git.*` / `chat.*` / `refs.*` echoes). The acyclic-producer invariant (EI-02 §3), pinned at
/// the contract seam. `ci` is the canonical §6.2 subsystem token CI registers under.
#[test]
fn cdc_2_9_ci_registers_only_its_own_subsystem() {
    assert!(
        SUBSYSTEM_TOKENS.contains(&CI_SUBSYSTEM_TOKEN),
        "`ci` must be a canonical Bus subsystem token (§6.2)"
    );
    for tok in provider_registers_ci_tokens() {
        assert!(
            tok.starts_with("ci."),
            "CI registered the foreign-subsystem token `{tok}` (must own `ci.*` only)"
        );
    }
    // The frozen X-1 check-seam token is present under its named constant (X-5 names anchor).
    assert!(provider_registers_ci_tokens().contains(&"ci.check.updated"));
    assert!(provider_registers_ci_tokens().contains(&"ci.result"));
}

/// **The DURABLE / FIREHOSE split is part of the 2.9 contract** (arch §1): the durable class is the
/// ONLY class that may ride `OutboxTx::emit`; the firehose log frame never touches the durable bus.
/// The two classes partition the registry exactly (0 misclassified tokens) — the structural gate,
/// re-pinned at the control-plane seam over the one source of truth.
#[test]
fn cdc_2_9_ci_durable_firehose_split_partitions_the_registry() {
    // Disjoint: no firehose token is in the durable set.
    for f in CI_FIREHOSE_TOKENS {
        assert!(
            !CI_DURABLE_TOKENS.contains(f),
            "firehose token `{f}` must NOT be in the durable set"
        );
    }
    // The two classes partition the union exactly (no token lost / double-counted).
    assert_eq!(
        CI_DURABLE_TOKENS.len() + CI_FIREHOSE_TOKENS.len(),
        provider_registers_ci_tokens().len(),
        "the durable + firehose sizes must partition the registry exactly"
    );
    // The log frame is firehose-only; the log POINTER is durable (arch §1.3).
    assert!(CI_FIREHOSE_TOKENS.contains(&"ci.log.appended"));
    assert!(CI_DURABLE_TOKENS.contains(&"ci.log.available"));
}
