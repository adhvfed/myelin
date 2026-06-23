//! # `events` — the complete `ci.*` event taxonomy REGISTERED into the Bus seed (CI-P7 / P-350, M4)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md`
//! §1 (**the complete `ci.*` event taxonomy CI OWNS** — the X-1 tokens `ci.check.updated` +
//! `ci.result`, the run/job/deployment lifecycle, the log/artifact/cost pointer & resource events,
//! the runner/pipeline/supply-chain fleet+config events, and the cross-cutting `*.erased` /
//! `*.snapshot` events), §1.1 (the §6.2 Δ1 rename note: `ci.status.updated` / `ci.run.passed` are
//! SUPERSEDED by `ci.check.updated` / `ci.result`).
//!
//! **Contract-index rows (registered here — against the FROZEN Bus grammar / §6.2 token table):**
//! - **2.9** Event taxonomy + token table — `<subsystem>.<artifact_type>.<event_name>`. The Bus owns
//!   the **grammar + the seed** (`myelin_events::taxonomy`, EB-02 / P-042); **each subsystem
//!   completes + REGISTERS its own list** (the contract-2.9 text). This module is the **CI Control
//!   Plane** — the outbox PRODUCER / check emitter (see crate doc, the home CI-P7 names) —
//!   REGISTERING CI's complete `ci.*` list into the Bus seed, validated against the ONE Bus grammar
//!   ([`myelin_events::validate_event_type`]) AND the §6.2 singular subsystem/type token table. CI
//!   **registers**; it does **not** author the grammar (EI-01 §7 — one grammar, no per-subsystem
//!   drift).
//! - **2.1 / 2.2** `EventEnvelope` + `OutboxTx::emit` — CONSUMED: every `ci.*` token below is the
//!   `type` field of the canonical envelope, emitted ONLY via the transactional outbox (the
//!   `no-raw-publish` lint). Referenced, not re-defined.
//!
//! ## The single source of truth — NOT a second copy (coherence, EI-01 §7)
//! The canonical `ci.*` token CONSTANTS + the durable/firehose tables were frozen early by
//! EB-27 / P-327 in [`myelin_ci_sandbox::events`] (the M4 names freeze the Bus harness validates).
//! CI-P7 does **not** re-define them — that would be a second, drift-prone token language. Instead
//! this module **re-exports the canonical tables** ([`ci_event_tokens`] / [`CI_DURABLE_TOKENS`] /
//! [`CI_FIREHOSE_TOKENS`]) from that one source and adds what CI-P7 genuinely contributes over the
//! early freeze:
//!   1. the **CI Control Plane registration entry** [`register_ci_taxonomy`] — the control plane (the
//!      outbox producer, arch 00 §4) is the proper OWNER of the `ci.*` taxonomy registration, the
//!      home CI-P7 names (`myelin-ci-controlplane`); and
//!   2. the **§6.2 singular subsystem/type token-table validation** [`validate_ci_type_tokens`] —
//!      the early sandbox freeze proved §6.1 *grammar* conformance; CI-P7 adds the §6.2 check that
//!      the subsystem token is the canonical `ci` and every artifact-type segment is a well-formed
//!      SINGULAR §6.2 token (`run`/`job`/`check`/`deployment`/`runner`/`pipeline`/`log`/`artifact`/
//!      `cost`/`supply_chain`), per the CI-P7 deliverable ("ci is the canonical token + run/
//!      deployment/pipeline/runner/artifact type tokens — CI registers, it does not author").
//!
//! The CDC (`tests/cdc_2_9_ci_tokens.rs`) pins that the control-plane registration is byte-identical
//! to the [`myelin_ci_sandbox::events`] source of truth — ONE list, proven, no divergent copy.
//!
//! ## FLOOR named (VISION §3 name-your-floors): the per-event EMIT bodies land later
//! CI-P7 is the **token-LIST registration** (the names freeze + the §6.2 validation). Each event's
//! actual EMISSION lands with its producing prompt:
//!   - the X-1 check tokens (`ci.check.updated` / `ci.result`) — CI-P18 / CI-P19;
//!   - the log / artifact pointer tokens — CI-P20;
//!   - the deploy / supply-chain tokens — CI-P23 / CI-P24;
//!   - the `*.snapshot` reindex tokens — CI-P22.
//!
//! No emit body lives here — this is the registration surface only.

use myelin_events::validate_event_type;

// Re-export the canonical `ci.*` token tables from the ONE source of truth (EB-27 / P-327). CI-P7
// REGISTERS this list from the control plane; it does NOT re-define the constants (no second token
// language — the coherence rule). A rename/drop in the source is a contract change every consumer
// reconciles; the CDC proves this registration equals the source byte-for-byte.
pub use myelin_ci_sandbox::events::{
    ci_event_tokens, is_durable, register_ci_tokens, CI_DURABLE_TOKENS, CI_FIREHOSE_TOKENS,
};

/// The canonical **subsystem token** for CI (Bus §6.2 — the names anchor). Every `ci.*` event's
/// leading segment is exactly this. CLI aliases are render-time only and never the stored token.
pub const CI_SUBSYSTEM_TOKEN: &str = "ci";

/// The CI **artifact-type tokens** that appear in CI's complete taxonomy (Bus §6.2 — the `<type>`
/// segment of `<subsystem>.<artifact_type>.<event_name>`). `ci` is the canonical subsystem token;
/// these are the type tokens CI registers (a sanctioned §6.2 extension of the seed table — CI owns
/// its own type list, validated SINGULAR + well-formed against the §6.1 grammar, EB-24 / §6.4 "each
/// subsystem completes its full list"). Two-segment names (`ci.result`) carry NO type segment.
///
/// The CI-P7 deliverable names `run` / `deployment` / `pipeline` / `runner` / `artifact` explicitly;
/// the complete CI list also carries `job` / `check` / `log` / `cost` / `supply_chain`.
pub const CI_TYPE_TOKENS: &[&str] = &[
    "run",
    "job",
    "check",
    "deployment",
    "runner",
    "pipeline",
    "log",
    "artifact",
    "cost",
    "supply_chain",
];

/// Why a CI token fails the §6.2 subsystem/type token-table check (distinct from the §6.1 grammar
/// failures — those are [`TaxonomyError`]). LOUD, never silently coerced (EI-01 §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiTypeTokenError {
    /// The leading segment is not the canonical `ci` subsystem token (§6.2).
    NotCiSubsystem { token: String, head: String },
    /// The artifact-type segment of a three-segment name is not one CI registered in
    /// [`CI_TYPE_TOKENS`] — an un-declared type token (CI registers its type list, it does not
    /// silently mint a new type at emit time).
    UnregisteredTypeToken { token: String, type_seg: String },
}

impl std::fmt::Display for CiTypeTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CiTypeTokenError::NotCiSubsystem { token, head } => write!(
                f,
                "`{token}`: leading segment `{head}` is not the canonical `ci` subsystem token (§6.2)"
            ),
            CiTypeTokenError::UnregisteredTypeToken { token, type_seg } => write!(
                f,
                "`{token}`: artifact-type segment `{type_seg}` is not a registered CI type token \
                 (§6.2 — CI registers its type list, it does not author a new type at emit time)"
            ),
        }
    }
}

/// **The §6.2 subsystem/type token-table check for ONE `ci.*` name** (CI-P7's net-new validation
/// over the §6.1 grammar EB-27 already proved). Asserts the leading segment is the canonical `ci`
/// subsystem token AND — for a three-segment `<ci>.<type>.<event>` name — the `<type>` segment is a
/// registered CI type token ([`CI_TYPE_TOKENS`]). Two-segment names (`ci.result`) carry no type
/// segment and pass the type half vacuously. Returns `Ok(())` or the LOUD [`CiTypeTokenError`].
///
/// This is layered ON TOP of [`validate_event_type`] (the §6.1 grammar) — call both: grammar first,
/// then this. `ci` itself must be a canonical Bus subsystem token (asserted in the tests below).
pub fn validate_ci_type_token(name: &str) -> Result<(), CiTypeTokenError> {
    let segments: Vec<&str> = name.split('.').collect();
    let head = segments.first().copied().unwrap_or("");
    if head != CI_SUBSYSTEM_TOKEN {
        return Err(CiTypeTokenError::NotCiSubsystem {
            token: name.to_string(),
            head: head.to_string(),
        });
    }
    // Three-segment form carries the artifact-type token in the middle; it must be a registered CI
    // type token. The two-segment form (`ci.result`) has no type segment — vacuously fine (§6.1).
    if segments.len() == 3 {
        let type_seg = segments[1];
        if !CI_TYPE_TOKENS.contains(&type_seg) {
            return Err(CiTypeTokenError::UnregisteredTypeToken {
                token: name.to_string(),
                type_seg: type_seg.to_string(),
            });
        }
    }
    Ok(())
}

/// **The §6.2 token-table check over the WHOLE registered `ci.*` list** (CI-P7). Returns `Ok(())`
/// iff every registered token (durable ∪ firehose) carries the canonical `ci` subsystem token and a
/// registered CI type token; otherwise the first offending token + its [`CiTypeTokenError`].
pub fn validate_ci_type_tokens() -> Result<(), (&'static str, CiTypeTokenError)> {
    for tok in ci_event_tokens() {
        validate_ci_type_token(tok).map_err(|e| (tok, e))?;
    }
    Ok(())
}

/// **THE CI CONTROL PLANE TAXONOMY REGISTRATION (contract 2.9, CI-P7 — the headline).** The control
/// plane (the outbox producer / check emitter, arch 00 §4) registers CI's complete `ci.*` list into
/// the Bus seed. Returns `Ok(())` iff **every** registered token passes BOTH:
///   1. the §6.1 Bus grammar (the ONE [`validate_event_type`] — `[a-z][a-z0-9_]*` singular tokens,
///      past-tense verbs, 2..=3 segments, known subsystem prefix); AND
///   2. the §6.2 subsystem/type token-table ([`validate_ci_type_token`] — `ci` is canonical, every
///      type segment is a registered CI type token).
///
/// Otherwise the first offending token + a LOUD reason (the grammar error or the §6.2 error,
/// rendered as a `String`). This is the GATE the CI-P7 DEFINITION OF DONE asserts: **0 ungrammatical
/// tokens, 0 §6.2-nonconforming tokens.** CI registers; it does not author the grammar.
pub fn register_ci_taxonomy() -> Result<(), (&'static str, String)> {
    for tok in ci_event_tokens() {
        // §6.1 grammar (the one Bus validator).
        if let Err(e) = validate_event_type(tok) {
            return Err((tok, format!("§6.1 grammar: {e}")));
        }
        // §6.2 subsystem/type token table.
        if let Err(e) = validate_ci_type_token(tok) {
            return Err((tok, format!("§6.2 token table: {e}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::SUBSYSTEM_TOKENS;
    use std::collections::BTreeSet;

    /// **THE CI-P7 GATE (contract 2.9): 0 ungrammatical tokens + 0 §6.2-nonconforming tokens.** The
    /// control-plane registration succeeds — every registered `ci.*` token parses the §6.1 Bus
    /// grammar AND conforms to the §6.2 subsystem/type token table. The successful registration is
    /// the dated GREEN artifact.
    #[test]
    fn register_ci_taxonomy_passes_grammar_and_token_table() {
        assert_eq!(
            register_ci_taxonomy(),
            Ok(()),
            "the CI control-plane taxonomy registration must be GREEN: {:?}",
            register_ci_taxonomy()
        );
        // Spelled out: every token parses the §6.1 grammar (the EB-02 validator).
        for tok in ci_event_tokens() {
            assert!(
                validate_event_type(tok).is_ok(),
                "registered ci token `{tok}` is UNGRAMMATICAL: {:?}",
                validate_event_type(tok)
            );
        }
        // ...and conforms to the §6.2 subsystem/type token table.
        assert_eq!(
            validate_ci_type_tokens(),
            Ok(()),
            "every ci token must conform to the §6.2 token table: {:?}",
            validate_ci_type_tokens()
        );
    }

    /// **§6.2: `ci` is the canonical Bus subsystem token** (the names anchor) and every registered
    /// token carries it as the leading segment. CLI aliases are render-time only and never stored.
    #[test]
    fn ci_is_the_canonical_subsystem_token_and_every_token_carries_it() {
        assert!(
            SUBSYSTEM_TOKENS.contains(&CI_SUBSYSTEM_TOKEN),
            "`ci` must be a canonical Bus subsystem token (§6.2)"
        );
        for tok in ci_event_tokens() {
            let head = tok.split('.').next().expect("non-empty token");
            assert_eq!(
                head, "ci",
                "token `{tok}` must carry the `ci` subsystem prefix"
            );
        }
    }

    /// **§6.2: every artifact-type segment is a registered SINGULAR CI type token.** For each
    /// three-segment `ci.<type>.<event>` name, the `<type>` is one of [`CI_TYPE_TOKENS`]; the
    /// CI-P7-named types (`run`/`deployment`/`pipeline`/`runner`/`artifact`) are present, and each
    /// type token is itself a well-formed singular §6.1 token (no plural, no uppercase).
    #[test]
    fn every_artifact_type_segment_is_a_registered_singular_type_token() {
        // The CI-P7 deliverable names these explicitly.
        for named in ["run", "deployment", "pipeline", "runner", "artifact"] {
            assert!(
                CI_TYPE_TOKENS.contains(&named),
                "the CI-P7-named type token `{named}` must be registered"
            );
        }
        // Each registered three-segment token resolves to a registered type token.
        for tok in ci_event_tokens() {
            let segs: Vec<&str> = tok.split('.').collect();
            if segs.len() == 3 {
                assert!(
                    CI_TYPE_TOKENS.contains(&segs[1]),
                    "token `{tok}`: type segment `{}` is not a registered CI type token",
                    segs[1]
                );
            }
        }
        // Each type token is a well-formed SINGULAR §6.1 token — a fabricated `ci.<type>.created`
        // name parses the grammar (so the type token is itself grammatical, not a plural/uppercase
        // smell). `cost`/`log`/`run` etc. all admit; this is the §6.2-singular proof.
        for ty in CI_TYPE_TOKENS {
            let probe = format!("ci.{ty}.created");
            assert!(
                validate_event_type(&probe).is_ok(),
                "CI type token `{ty}` is not a well-formed singular §6.2 token (probe `{probe}`): {:?}",
                validate_event_type(&probe)
            );
        }
    }

    /// **The control-plane registration is the §6.2 token-table check that REJECTS a non-CI subject
    /// or an unregistered type token LOUDLY** (the RED half — the gate has teeth). A `git.*` name is
    /// not a CI subsystem token; a fabricated `ci.widget.created` carries an unregistered type token.
    #[test]
    fn the_token_table_check_rejects_foreign_and_unregistered_loudly() {
        // A foreign subsystem name is not a `ci` token.
        assert!(matches!(
            validate_ci_type_token("git.pr.opened"),
            Err(CiTypeTokenError::NotCiSubsystem { .. })
        ));
        // A fabricated `ci.<type>.<event>` with an UNREGISTERED type token is rejected with its name.
        assert!(matches!(
            validate_ci_type_token("ci.widget.created"),
            Err(CiTypeTokenError::UnregisteredTypeToken { type_seg, .. }) if type_seg == "widget"
        ));
        // The two-segment `ci.result` carries NO type segment — it passes the §6.2 type half.
        assert_eq!(validate_ci_type_token("ci.result"), Ok(()));
    }

    /// **Coherence (EI-01 §7): the control-plane registration is the ONE source of truth, not a
    /// second copy.** The re-exported tables are identical to the [`myelin_ci_sandbox::events`]
    /// canonical constants (same pointers / same contents) — there is no divergent re-definition.
    #[test]
    fn the_registration_reuses_the_one_canonical_source() {
        // Re-exported, not re-defined: the control-plane tables ARE the sandbox source tables.
        assert_eq!(
            CI_DURABLE_TOKENS,
            myelin_ci_sandbox::events::CI_DURABLE_TOKENS
        );
        assert_eq!(
            CI_FIREHOSE_TOKENS,
            myelin_ci_sandbox::events::CI_FIREHOSE_TOKENS
        );
        // The early grammar-only registration (sandbox) AND the new grammar+§6.2 registration
        // (control plane) agree — both GREEN over the same list.
        assert!(
            register_ci_tokens().is_ok(),
            "the §6.1 source registration is green"
        );
        assert!(
            register_ci_taxonomy().is_ok(),
            "the §6.1+§6.2 control-plane registration is green"
        );
    }

    /// The registry has no duplicates across the durable ∪ firehose union (each name minted once) —
    /// re-asserted at the control-plane registration surface (defence in depth over the source).
    #[test]
    fn the_registered_list_has_no_duplicates() {
        let mut seen = BTreeSet::new();
        for tok in ci_event_tokens() {
            assert!(
                seen.insert(tok),
                "ci token `{tok}` is registered more than once"
            );
        }
    }

    /// **The Δ1-superseded legacy tokens are DELIBERATELY ABSENT** from the registered list (arch 03
    /// §1 rename note — the code emits `ci.check.updated` / `ci.result`, never `ci.status.updated` /
    /// `ci.run.passed`). The §6.2 grammar would ADMIT them (they are well-formed); the LIST curation
    /// supersedes them, not the grammar.
    #[test]
    fn the_superseded_legacy_tokens_are_absent_from_the_registration() {
        for tok in ci_event_tokens() {
            assert_ne!(
                tok, "ci.status.updated",
                "superseded by ci.check.updated (Δ1)"
            );
            assert_ne!(tok, "ci.run.passed", "superseded by ci.check.updated (Δ1)");
        }
        // The grammar itself does not reject them — the curation does.
        assert!(validate_event_type("ci.status.updated").is_ok());
        assert!(validate_event_type("ci.run.passed").is_ok());
    }
}
