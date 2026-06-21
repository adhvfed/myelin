//! # `events` — the complete `issue.*` event taxonomy + the `initiative` token (ISS-P03 / P-242)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md`
//! §1 (**the complete `issue.*` event taxonomy this subsystem owns** — the v1 token list incl. the
//! registered `initiative` type token) + §1 "Units" (the frozen names/units anchor: timestamps
//! RFC-3339 UTC; SLA targets / `stale_after` / durations in **seconds**; estimates/story-points
//! numeric; actor/subject as `ArtifactRef`s; `contains_personal_data`/`data_role`/`pii_key_ref` on
//! any PII-bearing event).
//!
//! **Contract-index rows (registered here — against the FROZEN Bus grammar / envelope):**
//! - **2.9** Event taxonomy + token table — `<subsystem>.<artifact_type>.<event_name>`. The Bus owns
//!   the **grammar + the seed** (`myelin_events::taxonomy`, EB-02 / P-042); **each subsystem
//!   completes its own list** (the contract-2.9 text). This module is Issues COMPLETING its
//!   `issue.*` list — incl. the `initiative` type token (the sanctioned §6.2 extension, recon §2)
//!   carried by the `issue.initiative.*` event names. Issues **registers**; it does **not** author
//!   the grammar — every token below is validated against the ONE Bus validator
//!   ([`myelin_events::validate_event_type`]); there is no second token language (EI-01 §7).
//! - **2.1** `EventEnvelope` — the canonical envelope every `issue.*` event aligns to. The frozen
//!   units are the anchor (X-5): this module pins the Issues-side unit convention
//!   ([`unit_check`]) — durations/SLA targets in **seconds** (NOT millis), timestamps RFC-3339 UTC —
//!   so a seconds-vs-millis drift in an issue payload is REJECTED loudly, not silently coerced. The
//!   envelope itself is referenced, never re-defined (it lives in `myelin-events`).
//!
//! ## What this prompt (ISS-P03 / P-242) ships — and what it deliberately does NOT
//! **Ships:** the complete v1 `issue.*` event-token registration (arch §1) as named `&'static str`
//! constants + the [`ISSUE_EVENT_TOKENS`] table, each PROVEN grammatical against the Bus §6.1/§6.2
//! grammar by [`myelin_events::validate_event_type`] (**0 ungrammatical tokens** — the gate); the
//! `initiative` type token, carried by `issue.initiative.health_changed` (recon §2 / §6.2); and the
//! Issues-side [`unit_check`] that pins the EventEnvelope unit convention for issue payloads
//! (seconds-not-millis durations; RFC-3339 UTC timestamps) with a loud rejection of a drift.
//!
//! **FLOOR named (VISION §3 name-your-floors): NO Issues data is written yet.** These tokens are
//! **registered** here (the names freeze) but **actually EMITTED only from the OUTBOX** in later
//! prompts — there is **no emit body** in this module:
//! - the state-changing emits (`issue.issue.*`, `issue.relation.*`, `issue.comment.*`,
//!   `issue.field.*`, the rollup/SLA/cycle/milestone families) attach to these constants via
//!   `OutboxTx::emit` in the **silent-data-loss-safe write path — ISS-P06 / P-372** (validate →
//!   check → mutate → emit in one tx), on top of the **issue-spine migrations — ISS-P05 / P-371**;
//! - the `issue.*.erased` tombstones emit from the H-Issues holder erasure path (§7, ISS-P07);
//! - the `issue.*.snapshot` reindex-from-source events emit from `replay(scope, since)` — all via
//!   the outbox, never a direct publish.
//!
//! ## Why this is data (a `&'static str` token table), not an emit seam
//! Registration at M2 is a **names freeze** so dependents (Refs' edge builder for the TE-7
//! `issue.relation.*` mirror, Search's `declare_indexable` projection — ISS-P04, Notif's
//! `define_notif_rule` reason set — ISS-P04, the rollup/forecast consumers) compile against the
//! NAMED issue tokens, never literals (the names anchor, X-5). The emit paths attach to these
//! constants in the later prompts above; when they do, they assert against THESE names — one token
//! language, no drift (EI-01 §7).

use myelin_events::validate_event_type;

// ===========================================================================
// §1 — the complete issue.* event token constants (the registration; dotted grammar)
//
// Names are taken verbatim from arch 03 §1 (the owned taxonomy table). Every constant is asserted
// grammatical against the Bus validator in `tests` below (0 ungrammatical). The trailing comment on
// each group is the Aggregate (ordering key) from arch §5 — **the issue is the aggregate**, so
// per-issue ordering is preserved (contract 2.3 / the D-9 drill) when the emit prompts wire them.
// ===========================================================================

// --- issue lifecycle (aggregate: issue/<PROJECTKEY>-<seqno>) ----------------

/// An issue was created. Aggregate `issue/<key>`. (`issue.create` ToolDef — contract 8.1.)
pub const ISSUE_CREATED: &str = "issue.issue.created";
/// An issue was updated — **carries the field deltas** (the rollup/sync/projection-feeder input).
/// May carry free-text PII in the deltas → `contains_personal_data` set. Aggregate `issue/<key>`.
pub const ISSUE_UPDATED: &str = "issue.issue.updated";
/// An issue transitioned `{from, to, category}` (the named state + the FIXED cross-sub category).
/// Aggregate `issue/<key>`.
pub const ISSUE_TRANSITIONED: &str = "issue.issue.transitioned";
/// An issue was closed. Aggregate `issue/<key>`.
pub const ISSUE_CLOSED: &str = "issue.issue.closed";
/// An issue was reopened. Aggregate `issue/<key>`.
pub const ISSUE_REOPENED: &str = "issue.issue.reopened";
/// An issue was deleted (**soft** — the recoverable tombstone, distinct from the `*.erased` GDPR
/// tombstone below). Aggregate `issue/<key>`.
pub const ISSUE_DELETED: &str = "issue.issue.deleted";
/// A soft-deleted issue was restored. Aggregate `issue/<key>`.
pub const ISSUE_RESTORED: &str = "issue.issue.restored";
/// An issue was (re)assigned. Carries the pseudonymous assignee principal → `data_role` set.
/// Aggregate `issue/<key>`.
pub const ISSUE_ASSIGNED: &str = "issue.issue.assigned";
/// An issue's priority changed. Aggregate `issue/<key>`.
pub const ISSUE_PRIORITY_CHANGED: &str = "issue.issue.priority_changed";
/// An issue's type changed (`issue|epic|sprint|...`). Aggregate `issue/<key>`.
pub const ISSUE_TYPE_CHANGED: &str = "issue.issue.type_changed";
/// An issue's parent changed (the hierarchy edge — feeds the rollup recompute). Aggregate
/// `issue/<key>`.
pub const ISSUE_PARENT_CHANGED: &str = "issue.issue.parent_changed";
/// An issue was archived. Aggregate `issue/<key>`.
pub const ISSUE_ARCHIVED: &str = "issue.issue.archived";
/// An issue was reordered (the `order_key`/LexoRank CAS rank — ISS-P09). Aggregate `issue/<key>`.
pub const ISSUE_REORDERED: &str = "issue.issue.reordered";

// --- triage (on `issue`; agent-assist provenance — always attributed) -------

/// An issue was triaged by the triage agent (agent-assist provenance). Aggregate `issue/<key>`.
pub const ISSUE_TRIAGED: &str = "issue.issue.triaged";
/// The triage agent suspects this issue is a duplicate. Aggregate `issue/<key>`.
pub const ISSUE_DUPLICATE_SUSPECTED: &str = "issue.issue.duplicate_suspected";
/// An issue was labelled by an agent (attributed agent-assist). Aggregate `issue/<key>`.
pub const ISSUE_LABELLED_BY_AGENT: &str = "issue.issue.labelled_by_agent";

// --- relation (the TE-7 typed-edge event Refs mirrors — contract 5.5) -------

/// A typed relation edge was created (`blocked_by`/`closes`/`relates`/…). One event yields BOTH
/// projection directions (Refs mirrors it — contract 5.5). Aggregate `issue/<key>`.
pub const RELATION_CREATED: &str = "issue.relation.created";
/// A typed relation edge was removed. Aggregate `issue/<key>`.
pub const RELATION_REMOVED: &str = "issue.relation.removed";

// --- field (the field-scheme changes; the `#field-<opaqueid>` sub-artifact) -

/// A field was defined on a scheme (`#field-<opaqueid>`). Aggregate `issue/<key>` (scheme-scoped).
pub const FIELD_DEFINED: &str = "issue.field.defined";
/// A field definition was updated. Aggregate `issue/<key>`.
pub const FIELD_UPDATED: &str = "issue.field.updated";
/// A field was removed from a scheme. Aggregate `issue/<key>`.
pub const FIELD_REMOVED: &str = "issue.field.removed";

// --- comment (the `#comment-<opaqueid>` sub-artifact; body is myelin-content) -

/// A comment was created (`#comment-<opaqueid>`; body is `myelin-content`). Free-text PII →
/// `contains_personal_data`/`pii_key_ref` set. Aggregate `issue/<key>`.
pub const COMMENT_CREATED: &str = "issue.comment.created";
/// A comment was updated. Aggregate `issue/<key>`.
pub const COMMENT_UPDATED: &str = "issue.comment.updated";
/// A comment was deleted. Aggregate `issue/<key>`.
pub const COMMENT_DELETED: &str = "issue.comment.deleted";

// --- rollup (the derived aggregate; `input_hash`-suppressed) ----------------

/// The derived rollup aggregate was recomputed (feeds roadmap + the forecast agent;
/// `input_hash`-suppressed — no event if the inputs did not change). Aggregate `issue/<key>`.
pub const ROLLUP_RECOMPUTED: &str = "issue.rollup.recomputed";

// --- cycle (the time axis; burndown / OLAP) ---------------------------------

/// A cycle (sprint) started. Aggregate `issue/<cycle-key>`.
pub const CYCLE_STARTED: &str = "issue.cycle.started";
/// A cycle completed. Aggregate `issue/<cycle-key>`.
pub const CYCLE_COMPLETED: &str = "issue.cycle.completed";
/// An issue was added to a cycle. Aggregate `issue/<cycle-key>`.
pub const CYCLE_ISSUE_ADDED: &str = "issue.cycle.issue_added";
/// An issue was removed from a cycle. Aggregate `issue/<cycle-key>`.
pub const CYCLE_ISSUE_REMOVED: &str = "issue.cycle.issue_removed";

// --- milestone (versions / releases) ----------------------------------------

/// A milestone (version/release) was released. Aggregate `issue/<milestone-key>`.
pub const MILESTONE_RELEASED: &str = "issue.milestone.released";

// --- sla (the compliance feed → OLAP; durations in SECONDS — unit anchor) ---

/// An SLA clock started. Payload `target_seconds` (durations in **seconds**, never millis — the
/// frozen unit, arch §1). Aggregate `issue/<key>`.
pub const SLA_STARTED: &str = "issue.sla.started";
/// An SLA clock paused (e.g. waiting-on-customer). Aggregate `issue/<key>`.
pub const SLA_PAUSED: &str = "issue.sla.paused";
/// An SLA clock resumed. Aggregate `issue/<key>`.
pub const SLA_RESUMED: &str = "issue.sla.resumed";
/// An SLA crossed the at-risk threshold (drives the "tell me when SLA at risk" trigger — §10).
/// Aggregate `issue/<key>`.
pub const SLA_AT_RISK: &str = "issue.sla.at_risk";
/// An SLA was breached. Aggregate `issue/<key>`.
pub const SLA_BREACHED: &str = "issue.sla.breached";
/// An SLA was met (closed within target). Aggregate `issue/<key>`.
pub const SLA_MET: &str = "issue.sla.met";

// --- approval (the HITL gate surface; humanised via contract 7.3) -----------

/// A HITL approval was requested (a gated transition — contract 8.1 `requires_approval`). Aggregate
/// `issue/<key>`.
pub const APPROVAL_REQUESTED: &str = "issue.approval.requested";
/// A HITL approval was granted. Aggregate `issue/<key>`.
pub const APPROVAL_GRANTED: &str = "issue.approval.granted";
/// A HITL approval was rejected. Aggregate `issue/<key>`.
pub const APPROVAL_REJECTED: &str = "issue.approval.rejected";
/// A HITL approval timed out (the `stale_after` durable timer fired — §10). Aggregate `issue/<key>`.
pub const APPROVAL_TIMED_OUT: &str = "issue.approval.timed_out";

// --- initiative (THE REGISTERED `initiative` TYPE TOKEN — recon §2 / §6.2) --

/// **The `initiative` type token, live.** The forecast/drift agent crossed an at-risk threshold →
/// the roadmap "date-at-risk" surface (drives the "tell me when this initiative goes at-risk"
/// trigger — §10). The `initiative` artifact type is the sanctioned §6.2 extension (recon §2;
/// `myelin_events::taxonomy::ARTIFACT_TYPE_TOKENS` already carries `initiative`). Aggregate
/// `issue/<initiative-key>`.
pub const INITIATIVE_HEALTH_CHANGED: &str = "issue.initiative.health_changed";

// --- cross-cutting *.erased tombstones (contract 2.7 / §7 erasure) ----------

/// The `*.erased` GDPR tombstone for an issue (contract 2.7; §7 erase). Consumers tombstone their
/// derived state (Search/Refs/OLAP/Notif). Emitted from the H-Issues holder erasure path (ISS-P07).
pub const ISSUE_ERASED: &str = "issue.issue.erased";
/// The `*.erased` tombstone for a comment sub-artifact (contract 2.7).
pub const COMMENT_ERASED: &str = "issue.comment.erased";

// --- cross-cutting *.snapshot reindex-from-source events (contract 2.6) -----

/// The `*.snapshot` reindex-from-source event for an issue (contract 2.6 — `replay`). Cold == live;
/// imported data rebuilds the same way (one indexing path).
pub const ISSUE_SNAPSHOT: &str = "issue.issue.snapshot";
/// The `*.snapshot` reindex event for a relation edge (contract 2.6 — Refs rebuilds the edge
/// projection drift-free).
pub const RELATION_SNAPSHOT: &str = "issue.relation.snapshot";
/// The `*.snapshot` reindex event for a comment sub-artifact (contract 2.6 — sub-artifact granular).
pub const COMMENT_SNAPSHOT: &str = "issue.comment.snapshot";
/// The `*.snapshot` reindex event for the derived rollup (contract 2.6 — snapshot-emittable for OLAP
/// convenience even though the rollup is DERIVED; the edge truth is `issue_relation`).
pub const ROLLUP_SNAPSHOT: &str = "issue.rollup.snapshot";

/// The complete v1 `issue.*` event-token list this subsystem registers (arch 03 §1) — **including
/// the registered `initiative` type token** carried by [`INITIATIVE_HEALTH_CHANGED`]. The Bus
/// taxonomy (contract 2.9) admits exactly these under the §6.1 grammar; each is PROVEN grammatical
/// by [`tests::every_issue_token_parses_the_bus_grammar`]. **Issues registers; it does not author
/// the grammar** — the validator is [`myelin_events::validate_event_type`] (one grammar, no drift).
///
/// Note: Issues does NOT register foreign-subsystem tokens. The cross-subsystem reflexes (arch §1.1)
/// — `git.branch.created`, `git.pr.merged`, `ci.check.updated`, `chat.message.created`,
/// `identity.member.*` — are **CONSUMED**, never originated by Issues (the acyclic-producer
/// invariant, EI-02 §3); none appear in this list.
pub const ISSUE_EVENT_TOKENS: &[&str] = &[
    // issue lifecycle
    ISSUE_CREATED,
    ISSUE_UPDATED,
    ISSUE_TRANSITIONED,
    ISSUE_CLOSED,
    ISSUE_REOPENED,
    ISSUE_DELETED,
    ISSUE_RESTORED,
    ISSUE_ASSIGNED,
    ISSUE_PRIORITY_CHANGED,
    ISSUE_TYPE_CHANGED,
    ISSUE_PARENT_CHANGED,
    ISSUE_ARCHIVED,
    ISSUE_REORDERED,
    // triage (agent-assist provenance)
    ISSUE_TRIAGED,
    ISSUE_DUPLICATE_SUSPECTED,
    ISSUE_LABELLED_BY_AGENT,
    // relation (TE-7 typed edge; Refs mirror)
    RELATION_CREATED,
    RELATION_REMOVED,
    // field (the field-scheme changes)
    FIELD_DEFINED,
    FIELD_UPDATED,
    FIELD_REMOVED,
    // comment (myelin-content body sub-artifact)
    COMMENT_CREATED,
    COMMENT_UPDATED,
    COMMENT_DELETED,
    // rollup (derived aggregate)
    ROLLUP_RECOMPUTED,
    // cycle (the time axis)
    CYCLE_STARTED,
    CYCLE_COMPLETED,
    CYCLE_ISSUE_ADDED,
    CYCLE_ISSUE_REMOVED,
    // milestone (versions/releases)
    MILESTONE_RELEASED,
    // sla (compliance feed; durations in SECONDS)
    SLA_STARTED,
    SLA_PAUSED,
    SLA_RESUMED,
    SLA_AT_RISK,
    SLA_BREACHED,
    SLA_MET,
    // approval (the HITL gate surface)
    APPROVAL_REQUESTED,
    APPROVAL_GRANTED,
    APPROVAL_REJECTED,
    APPROVAL_TIMED_OUT,
    // initiative (the REGISTERED type token — recon §2 / §6.2)
    INITIATIVE_HEALTH_CHANGED,
    // cross-cutting *.erased tombstones (contract 2.7)
    ISSUE_ERASED,
    COMMENT_ERASED,
    // cross-cutting *.snapshot reindex events (contract 2.6)
    ISSUE_SNAPSHOT,
    RELATION_SNAPSHOT,
    COMMENT_SNAPSHOT,
    ROLLUP_SNAPSHOT,
];

/// Register the complete `issue.*` list against the Bus grammar (contract 2.9). Returns `Ok(())` iff
/// **every** registered token parses the §6.1 grammar via the one Bus validator
/// ([`myelin_events::validate_event_type`]); otherwise the first offending token + its
/// [`myelin_events::TaxonomyError`] (LOUD, never silently coerced). This is the registration check
/// the GATE asserts (**0 ungrammatical tokens**) — Issues REGISTERS its list against the grammar it
/// does not own.
pub fn register_issue_tokens() -> Result<(), (&'static str, myelin_events::TaxonomyError)> {
    for &tok in ISSUE_EVENT_TOKENS {
        validate_event_type(tok).map_err(|e| (tok, e))?;
    }
    Ok(())
}

/// The frozen Issues-side **EventEnvelope unit anchor** (contract 2.1 / arch §1 "Units"). Issue
/// events that carry an SLA target / `stale_after` / any duration express it in **seconds**, and
/// every timestamp is **RFC-3339 UTC** — exactly the frozen units the canonical `EventEnvelope`
/// uses (`myelin-events::envelope`). This module pins the convention so a `*_millis`-style drift in
/// an issue payload is REJECTED loudly, not silently coerced (EI-01 §5; EI-01 §7 reconcile-units).
pub mod unit_check {
    /// Why a unit-bearing value in an issue payload is malformed (contract 2.1 frozen units). Each
    /// variant is a distinct, LOUD reason — the check never silently coerces.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum UnitError {
        /// A duration field was expressed in MILLISECONDS — durations are **seconds** (the frozen
        /// unit). The classic seconds-vs-millis drift; the `*_millis` key is the smell.
        DurationNotSeconds { field: String },
        /// A timestamp was not RFC-3339 UTC (`Z`-suffixed, `T`-separated) — the frozen timestamp
        /// unit. The classic local-time / epoch-millis drift.
        TimestampNotRfc3339Utc { field: String, value: String },
    }

    impl std::fmt::Display for UnitError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                UnitError::DurationNotSeconds { field } => write!(
                    f,
                    "`{field}`: a duration must be expressed in SECONDS (the frozen unit) — a \
                     `*_millis` key is the seconds-vs-millis drift the envelope anchor forbids"
                ),
                UnitError::TimestampNotRfc3339Utc { field, value } => write!(
                    f,
                    "`{field}`: timestamp `{value}` is not RFC-3339 UTC (it must be `T`-separated \
                     and `Z`-suffixed, e.g. `2026-06-21T10:00:00Z`)"
                ),
            }
        }
    }

    /// A timestamp string is the frozen unit iff it is RFC-3339 UTC: `T`-separated and `Z`-suffixed
    /// (the SAME shape the canonical `EventEnvelope` asserts on `occurred_at`/`recorded_at`). A
    /// numeric epoch (`"1718960000000"`) or a local-time string (no `Z`) is NOT the frozen unit.
    pub fn timestamp_is_rfc3339_utc(value: &str) -> bool {
        value.contains('T') && value.ends_with('Z')
    }

    /// **THE Issues-side unit GATE (contract 2.1 frozen units).** Validate that an issue event's
    /// payload uses the frozen units: every duration key is in **seconds** (a `*_millis` key is the
    /// seconds-vs-millis drift → rejected), and every timestamp-bearing key is RFC-3339 UTC.
    ///
    /// The duration check is a NAMING convention enforced loudly: a duration MUST be carried under a
    /// `*_seconds` key (e.g. `target_seconds`, `stale_after_seconds`); a `*_millis` / `*_ms` key is
    /// the explicit smell the check rejects (the frozen unit is seconds — arch §1). This is the
    /// floor `EventEnvelope`-anchored unit discipline the issue payloads conform to; the typed SLA
    /// payload structs land with the SLA write path (ISS-P06+), and they read THIS rule.
    pub fn validate_issue_payload_units(
        payload: &serde_json::Value,
    ) -> Result<(), UnitError> {
        let Some(obj) = payload.as_object() else {
            // a non-object payload carries no unit-bearing keys to check.
            return Ok(());
        };
        for (key, val) in obj {
            // Duration drift: any millis-shaped duration key is the rejection (the frozen unit is
            // seconds). We reject the explicit `*_millis` / `*_ms` smell on a NUMERIC value.
            if (key.ends_with("_millis") || key.ends_with("_ms")) && val.is_number() {
                return Err(UnitError::DurationNotSeconds { field: key.clone() });
            }
            // Timestamp drift: a key that names a timestamp (`*_at`) must carry an RFC-3339 UTC
            // string (never an epoch number, never a local-time string).
            if key.ends_with("_at") {
                match val.as_str() {
                    Some(s) if timestamp_is_rfc3339_utc(s) => {}
                    _ => {
                        return Err(UnitError::TimestampNotRfc3339Utc {
                            field: key.clone(),
                            value: val.to_string(),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::unit_check::{
        timestamp_is_rfc3339_utc, validate_issue_payload_units, UnitError,
    };
    use super::*;

    /// **THE GATE (contract 2.9): 0 ungrammatical tokens.** Every registered `issue.*` token —
    /// **including the `initiative` type token** (`issue.initiative.health_changed`) — parses the
    /// Bus §6.1/§6.2 grammar via the one Bus validator. Issues registers against the grammar it does
    /// not author. The parse is the green artifact.
    #[test]
    fn every_issue_token_parses_the_bus_grammar() {
        for &tok in ISSUE_EVENT_TOKENS {
            assert!(
                validate_event_type(tok).is_ok(),
                "registered issue token `{tok}` is UNGRAMMATICAL: {:?}",
                validate_event_type(tok)
            );
        }
        // The whole-list registration helper agrees (0 ungrammatical).
        assert!(
            register_issue_tokens().is_ok(),
            "register_issue_tokens() must succeed: {:?}",
            register_issue_tokens()
        );
    }

    /// Every registered token carries the canonical `issue` subsystem prefix (§6.2 — `issue` is the
    /// canonical subsystem token; CLI aliases are render-time only and never the stored/registered
    /// token).
    #[test]
    fn every_issue_token_carries_the_issue_subsystem_prefix() {
        for &tok in ISSUE_EVENT_TOKENS {
            let head = tok.split('.').next().expect("non-empty token");
            assert_eq!(head, "issue", "token `{tok}` must carry the `issue` subsystem prefix");
        }
        // ...and `issue` is the canonical subsystem token the Bus knows (the names anchor X-5).
        assert!(
            myelin_events::SUBSYSTEM_TOKENS.contains(&"issue"),
            "`issue` must be a canonical Bus subsystem token"
        );
    }

    /// The **`initiative` type token is registered + live**: the `issue.initiative.health_changed`
    /// event name is in the list, parses the grammar, and the `initiative` artifact-type token is
    /// the one the Bus seed already admits (recon §2 / §6.2 — the sanctioned extension). This is the
    /// ISS-P03 headline alongside the complete `issue.*` taxonomy.
    #[test]
    fn the_initiative_type_token_is_registered_and_grammatical() {
        assert!(ISSUE_EVENT_TOKENS.contains(&INITIATIVE_HEALTH_CHANGED));
        assert!(validate_event_type(INITIATIVE_HEALTH_CHANGED).is_ok());
        // the artifact-type segment is `initiative`, the registered §6.2 extension.
        assert_eq!(INITIATIVE_HEALTH_CHANGED.split('.').nth(1), Some("initiative"));
        assert!(
            myelin_events::ARTIFACT_TYPE_TOKENS.contains(&"initiative"),
            "`initiative` must be a registered Bus artifact-type token (recon §2 / §6.2)"
        );
    }

    /// The list has **no duplicates** — a token registered twice is a contract smell (each name is
    /// minted once; the set is the authoritative registry).
    #[test]
    fn the_issue_token_list_has_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for &tok in ISSUE_EVENT_TOKENS {
            assert!(seen.insert(tok), "issue token `{tok}` is registered more than once");
        }
        assert_eq!(seen.len(), ISSUE_EVENT_TOKENS.len());
    }

    /// The load-bearing cross-subsystem-consumed tokens are present under their NAMED constants (the
    /// names anchor X-5 — Refs/Search/Notif/the rollup+forecast agents consume these by name, never
    /// by literal). A rename/drop here is a contract change every consumer must reconcile.
    #[test]
    fn the_load_bearing_issue_tokens_are_registered() {
        // the rollup/sync/feeder input (carries the field deltas)
        assert!(ISSUE_EVENT_TOKENS.contains(&ISSUE_UPDATED));
        // the transition (the cross-sub "is it done?" category) + the TE-7 typed edge (Refs mirror)
        assert!(ISSUE_EVENT_TOKENS.contains(&ISSUE_TRANSITIONED));
        assert!(ISSUE_EVENT_TOKENS.contains(&RELATION_CREATED));
        // the SLA compliance feed + the initiative health (the trigger drivers — §10)
        assert!(ISSUE_EVENT_TOKENS.contains(&SLA_AT_RISK));
        assert!(ISSUE_EVENT_TOKENS.contains(&INITIATIVE_HEALTH_CHANGED));
        // the cross-cutting *.erased + *.snapshot tokens (contracts 2.7 / 2.6)
        assert!(ISSUE_EVENT_TOKENS.contains(&ISSUE_ERASED));
        assert!(ISSUE_EVENT_TOKENS.contains(&ISSUE_SNAPSHOT));
    }

    /// Issues does NOT register a foreign-subsystem token — the cross-subsystem reflexes (`git.*`,
    /// `ci.*`, `chat.*`, `identity.*`) are CONSUMED, never originated (the acyclic-producer
    /// invariant, EI-02 §3). No registered token leaves the `issue` prefix.
    #[test]
    fn issues_registers_no_foreign_subsystem_tokens() {
        for &tok in ISSUE_EVENT_TOKENS {
            assert!(
                tok.starts_with("issue."),
                "issue must not register the foreign-subsystem token `{tok}`"
            );
        }
    }

    /// **The EventEnvelope unit anchor holds for issue payloads (contract 2.1 frozen units):** a
    /// payload using the frozen units — durations in **seconds** (`*_seconds` keys), timestamps
    /// RFC-3339 UTC — is ACCEPTED; the seconds form validates. This is the GREEN half of the unit
    /// ratchet (durations in seconds, timestamps RFC-3339 UTC validate).
    #[test]
    fn issue_payload_in_frozen_units_validates() {
        // an issue.sla.started payload in the FROZEN units (seconds + RFC-3339 UTC).
        let ok = serde_json::json!({
            "issue": "myelin://acme/issue/issue/ENG-1421",
            "target_seconds": 86_400,          // SLA target in SECONDS (the frozen unit)
            "stale_after_seconds": 2_592_000,  // the trigger stale_after in SECONDS (§10)
            "started_at": "2026-06-21T10:00:00Z" // RFC-3339 UTC (the frozen timestamp unit)
        });
        assert_eq!(
            validate_issue_payload_units(&ok),
            Ok(()),
            "an issue payload in the frozen units (seconds + RFC-3339 UTC) must validate"
        );
    }

    /// **The seconds-vs-millis fixture is REJECTED (contract 2.1 frozen units — the RED half).** A
    /// duration carried in MILLISECONDS (`target_millis`) is the classic unit drift; the check
    /// rejects it LOUDLY with [`UnitError::DurationNotSeconds`], never silently coercing. This is
    /// the dated proof the unit anchor is a real gate (the prompt's named TEST).
    #[test]
    fn seconds_vs_millis_fixture_is_rejected() {
        // the SAME SLA payload, but the duration is in MILLIS — the forbidden drift.
        let drifted = serde_json::json!({
            "issue": "myelin://acme/issue/issue/ENG-1421",
            "target_millis": 86_400_000,        // SECONDS-VS-MILLIS DRIFT — must be rejected
            "started_at": "2026-06-21T10:00:00Z"
        });
        assert_eq!(
            validate_issue_payload_units(&drifted),
            Err(UnitError::DurationNotSeconds { field: "target_millis".into() }),
            "a millis-expressed duration must be REJECTED (the frozen unit is seconds)"
        );
        // the `_ms` short form is the same drift, rejected the same way.
        let drifted_ms = serde_json::json!({ "stale_after_ms": 2_592_000_000u64 });
        assert!(matches!(
            validate_issue_payload_units(&drifted_ms),
            Err(UnitError::DurationNotSeconds { .. })
        ));
    }

    /// A non-UTC / non-RFC-3339 timestamp in an issue payload is REJECTED (the frozen timestamp
    /// unit). An epoch-millis number and a local-time string (no `Z`) both fail loudly.
    #[test]
    fn non_rfc3339_timestamp_in_a_payload_is_rejected() {
        // an epoch-millis NUMBER under a `*_at` key — not the frozen RFC-3339 UTC string.
        let epoch = serde_json::json!({ "occurred_at": 1_718_960_000_000u64 });
        assert!(matches!(
            validate_issue_payload_units(&epoch),
            Err(UnitError::TimestampNotRfc3339Utc { .. })
        ));
        // a local-time string (no Z suffix) — not UTC.
        let local = serde_json::json!({ "transitioned_at": "2026-06-21T10:00:00" });
        assert!(matches!(
            validate_issue_payload_units(&local),
            Err(UnitError::TimestampNotRfc3339Utc { .. })
        ));
        // the frozen-unit helper agrees on the boundary.
        assert!(timestamp_is_rfc3339_utc("2026-06-21T10:00:00Z"));
        assert!(!timestamp_is_rfc3339_utc("2026-06-21T10:00:00"));
        assert!(!timestamp_is_rfc3339_utc("1718960000000"));
    }
}
