//! # `envelope` — the canonical `EventEnvelope` (contract 2.1; the X-5 names/units anchor)
//!
//! **EB-01 deliverable location.** The event-bus ledger's EB-01 prompt names
//! `envelope.rs` as the home of the `EventEnvelope` struct it freezes. The substrate
//! ledger's P-S05 (the canonical-envelope prompt) had already shipped this exact struct
//! into the crate root (`lib.rs`); EB-01 is the **Bus-system framing of the same single
//! deliverable** (the global ledger interleaves the substrate and event-bus roadmaps, so
//! the envelope freeze is reached from both — see the run table P-005 / P-011). Per the
//! coherence rule (EI-01 §7: never define a type twice, never build a parallel second
//! implementation), EB-01 **reconciles in place**: the frozen struct + its value types +
//! the causality derivation are MOVED here verbatim from `lib.rs` (no field/name/unit/type
//! change — the frozen public path `myelin_events::EventEnvelope` is preserved by the
//! `pub use envelope::*` re-export in `lib.rs`). What EB-01 ADDS over P-S05 is (1) this
//! module-located home matching the named deliverable, and (2) the EB-01 round-trip GATE
//! artifact ([`tests::eb01_full_field_round_trip_and_depth_derivation_is_lossless`]) that
//! proves the anchor is well-defined in one dated test: every field — including the nested
//! causality triad AND a populated `pii_key_ref` — round-trips lossless, and the
//! depth-derivation (child = parent + 1) computed from a cause is correct.
//!
//! ## Contract 2.1 — `EventEnvelope`, the names/units anchor (X-5)
//! Field list + units are the AUTHORITY in Bus §3.1 / architecture §2.10. Getting this
//! byte-identical is the whole job (a name/type/unit drift calcifies every downstream
//! contract). The struct is `serde`-stable: it is the wire shape every later emitter and
//! consumer reconciles against (the provider-side CDC test pins it).
//!
//! ## Frozen units (architecture §2.10; contract-index "Units (frozen)")
//! - timestamps = RFC-3339 UTC (`occurred_at`, `recorded_at`);
//! - `schema_ver` / `depth` = integers (`u32`); `depth` is the loop ceiling (AG-6) reads it;
//! - `pii_key_ref = kms://<tenant>/<dek-epoch>/<class>`, `<class> ∈ {tenant, subject:<id>, blob}`.
//!
//! ## Mutation-score floor (EB-01 TESTS — mandatory-core)
//! `envelope.rs` is mandatory-core. The floor: **every mutation of a name-, type-, or
//! unit-bearing line of the struct / the derivation must be killed by a test.** The struct
//! shape is killed by [`tests::surface_event_envelope_field_shape_is_frozen`] (a compile-
//! asserting per-field read + the frozen-unit assertions) and the wire-shape CDC test
//! [`tests::cdc_2_1_envelope_wire_shape_is_the_anchor`] (a renamed/dropped/added key fails
//! the frozen key-set assertion); the `derive_envelope` causality logic is killed by the
//! root / caused / deep-chain / saturation tests below (a mutated branch — wrong parent,
//! skipped depth increment, re-seeded root — flips an assertion). A full `cargo-mutants`
//! run is the substrate mutation-harness's job (P-S22/threshold file); the floor stated
//! here is the per-line obligation those structural tests already discharge.

use myelin_identity::Principal;
use myelin_tenancy::{Region, TenantId};
use serde::{Deserialize, Serialize};

/// Re-export of the `ArtifactRef` value type so `myelin_events::ArtifactRef` (and
/// `envelope::ArtifactRef`) is the frozen path (the envelope's `subject` type). Definition
/// site is `myelin-tenancy` (the DAG sink) — see the crate-level DAG-deviation note.
pub use myelin_tenancy::ArtifactRef;

/// The event idempotency key — a ULID (architecture §2.1; ADR-04.1). String-backed in
/// the skeleton; the ULID newtype + ordering invariants land with the outbox (P-S07).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EventId(pub String);

/// The causal-root id (architecture §2.1; BUS-5). Carries through a whole causal chain.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CorrelationId(pub String);

/// The distinct human-action / session ref (architecture §2.1; BUS-5 `caused_by`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CausedBy(pub String);

/// The dotted event type name `<subsystem>.<artifact_type>.<event_name>` (Bus §6 grammar).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventType(pub String);

/// The per-`(aggregate, seq)` ordering key (architecture §2.1; contract 2.3
/// `UNIQUE(aggregate, seq)`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AggregateKey(pub String);

/// The acting principal incl. `on_behalf_of` (architecture §2.1; ADR-13.3). The envelope
/// embeds a `Principal` ref from `myelin-identity`.
///
/// **Actor sub-shape floor (EB-01).** Bus §3.1 sketches the actor as
/// `{principal, kind∈{human|agent|service}, on_behalf_of, session, run}`. The frozen
/// cross-crate `Principal` (owned by `myelin-identity`, P-001) carries `principal`/`kind`,
/// and `on_behalf_of` rides inside `PrincipalKind::Agent`. The `session`/`run` attribution
/// refs land with the Identity-M1 `authenticate`/`mint_run_token` impls (contracts 4.1 /
/// 4.10) — named here, not silently dropped; the envelope embeds the identity-owned ref so
/// it tracks those additions without an envelope shape change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor(pub Principal);

/// controller | processor — the GDPR fan-out role of the event's data (architecture
/// §2.1, `data_role`; ADR-04.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataRole {
    Controller,
    Processor,
}

/// The event's visibility class (architecture §2.1, `visibility`). A HINT for routing —
/// **never** an authz decision (Id decides; Bus §1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    Internal,
    Private,
}

/// `kms://<tenant>/<dek-epoch>/<class>`, `<class> ∈ {tenant, subject:<id>, blob}`
/// (frozen unit, architecture §2.10; contract 2.7). Present only on inline-PII,
/// envelope-encrypted events. **Floor:** the KMS DEK hierarchy is Storage M1 (11.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiiKeyRef(pub String);

/// RFC-3339 UTC timestamp (the frozen unit anchor, architecture §2.10). String-backed in
/// the skeleton so the format is the contract; a typed clock lands with the impl prompts.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Timestamp(pub String);

/// The non-negotiable, versioned envelope (architecture §2.1, §2.10; contract 2.1;
/// ADR-13.2). **The names/units authority (X-5).** Every emitter + consumer aligns to
/// this exact field list. `schema_ver` gates evolution (upcasters bridge forward-only,
/// P-S09). References-not-payloads: `payload` carries IDs/`ArtifactRef`s, never PII bodies.
///
/// Field order + names match the §2.10 / Bus §3.1 frozen anchor. **Frozen as THE anchor by
/// P-S05** (2026-06-19) and **confirmed in `envelope.rs` by EB-01** (the Bus-system freeze
/// of the same single deliverable): the per-name/per-unit compile test + the provider-side
/// CDC envelope-shape contract test (2.1) + the EB-01 full-field round-trip GATE pin it.
/// Any drift from a name/type/unit now fails to compile or fails a CDC/round-trip test.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    /// ULID; the idempotency key (ADR-04.1).
    pub event_id: EventId,
    /// dotted name `<subsystem>.<artifact_type>.<event_name>`.
    pub type_: EventType,
    /// upcasters bridge versions at consume (forward-only).
    pub schema_ver: u32,
    /// partition + residency key (ADR-11) — FIRST-CLASS, never optional.
    pub tenant: TenantId,
    pub region: Region,
    /// Principal ref incl. on_behalf_of (ADR-13.3).
    pub actor: Actor,
    /// what this event is about (ADR-13.1); may carry a #sub anchor (5.7).
    pub subject: ArtifactRef,
    /// the per-(aggregate, seq) ordering key (UNIQUE(aggregate, seq); contract 2.3).
    pub aggregate: AggregateKey,
    /// IMMEDIATE parent (BUS-5: nested, not flat).
    pub causation_id: Option<EventId>,
    /// the causal ROOT — carries through (BUS-5).
    pub correlation_id: CorrelationId,
    /// distinct human-action/session ref (BUS-5).
    pub caused_by: Option<CausedBy>,
    /// causal depth; the loop ceiling reads this (AG-6).
    pub depth: u32,
    /// routes GDPR handling (ADR-04.4).
    pub contains_personal_data: bool,
    /// controller | processor (tenant-content) — GDPR fan-out.
    pub data_role: DataRole,
    pub visibility: Visibility,
    /// kms://<tenant>/<dek-epoch>/<class>; inline-PII events envelope-encrypted (2.7).
    pub pii_key_ref: Option<PiiKeyRef>,
    /// RFC-3339 UTC (the unit anchor).
    pub occurred_at: Timestamp,
    /// RFC-3339 UTC; when the log durably accepted it.
    pub recorded_at: Timestamp,
    /// references-not-payloads: IDs/ArtifactRefs, never PII bodies.
    pub payload: serde_json::Value,
}

/// The to-be-emitted event before the outbox derives its provenance (architecture §2.1).
///
/// The draft carries the **caller-authored** fields — *what* this event is and what it
/// carries — while the **ambient** fields (tenant/region/actor/timestamps/event_id) come
/// from the emitting transaction's [`EmitContext`] and the **provenance** fields (the
/// causal triple) are derived from the `cause`, not authored. This split is what makes
/// causality correct-by-construction: a caller cannot hand-set `causation_id`,
/// `correlation_id`, or `depth` (they are not on the draft), so a human or agent **cannot
/// typo their way into a loop** (EI-02 §6, BUS-5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventDraft {
    pub type_: EventType,
    pub subject: ArtifactRef,
    pub aggregate: AggregateKey,
    pub payload: serde_json::Value,
    /// GDPR fan-out role of this event's data (controller | processor). Caller-classified
    /// (the emitting subsystem knows whether it is the controller of the subject data).
    pub data_role: DataRole,
    /// The event's visibility class.
    pub visibility: Visibility,
    /// Does the payload carry inline personal data? If so `pii_key_ref` MUST be set
    /// (envelope-encrypted, contract 2.7). References-not-payloads is the default: most
    /// events carry IDs/refs and set this `false`.
    pub contains_personal_data: bool,
    /// Set iff `contains_personal_data`; the `kms://…` URN of the inline-PII envelope key
    /// (the KMS hierarchy is Storage M1 — here only the field travels).
    pub pii_key_ref: Option<PiiKeyRef>,
}

/// The **ambient** per-emit context the emitting transaction supplies: the fields that
/// belong to the actor/tenant/clock, not to the draft and not to the causal chain. The
/// outbox owns minting the `event_id` (a ULID) and stamping `recorded_at` when the row is
/// durably accepted; the caller supplies `tenant`/`region`/`actor`/`occurred_at` and the
/// optional human-action `caused_by` (which, for a ROOT, seeds nothing causal — it is the
/// distinct human-action ref, BUS-5).
///
/// `caused_by` is **only** read for a root emit's own provenance recording and is carried
/// through unchanged by [`OutboxTx::emit`](crate::OutboxTx::emit)'s derivation onto every
/// child — see [`derive_envelope`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmitContext {
    /// The ULID the outbox mints for this event (the idempotency key, ADR-04.1). Generation
    /// (clock + randomness) is the outbox's concern, P-S07; the derivation here is pure in
    /// the supplied id so the causality logic stays deterministically testable.
    pub event_id: EventId,
    /// The partition + residency key (ADR-11) — first-class, never optional.
    pub tenant: TenantId,
    pub region: Region,
    /// The acting principal incl. on_behalf_of (ADR-13.3).
    pub actor: Actor,
    /// schema version of the emitted type (forward-only; upcasters bridge at consume).
    pub schema_ver: u32,
    /// RFC-3339 UTC; when the action happened (the unit anchor, §2.10).
    pub occurred_at: Timestamp,
    /// RFC-3339 UTC; when the log durably accepted it.
    pub recorded_at: Timestamp,
    /// The distinct human-action / session ref (BUS-5). Carried through the whole causal
    /// chain unchanged once a root sets it.
    pub caused_by: Option<CausedBy>,
}

/// **The causality derivation — correct-by-construction (P-S06; BUS-5; EI-02 §6).**
///
/// This is the single function every emit path routes through, so the causal triple is
/// *derived*, never hand-authored. Given a [`EventDraft`], the ambient [`EmitContext`], and
/// the optional `cause` (the parent envelope this emit is a reaction to):
///
/// - **root** (`cause == None`): the event is its own causal root.
///   `causation_id = None` (no parent), `correlation_id = <this event's id>`
///   (it carries its own root), `depth = 0`, and `caused_by` is whatever the context's
///   human-action ref is (the root *defines* the human action for the chain).
/// - **caused** (`cause == Some(parent)`): provenance is taken *from the parent*.
///   `causation_id = Some(parent.event_id)` (the IMMEDIATE parent, nested-not-flat),
///   `correlation_id = parent.correlation_id` (the ROOT carries through),
///   `depth = parent.depth + 1` (saturating — the loop ceiling, AG-6, reads this), and
///   `caused_by = parent.caused_by` (the originating human action is inherited unchanged —
///   a deep reactive chain still attributes to the human who started it, BUS-5).
///
/// Because `causation_id`/`correlation_id`/`depth`/`caused_by` are computed here and are
/// **not fields on `EventDraft`**, a caller has no API surface on which to fabricate a wrong
/// parent, skip a depth increment, or forge a root — the loop guard's invariant holds
/// structurally, not by convention.
pub fn derive_envelope(
    draft: EventDraft,
    ctx: EmitContext,
    cause: Option<&EventEnvelope>,
) -> EventEnvelope {
    let (causation_id, correlation_id, depth, caused_by) = match cause {
        // Root: carries its own correlation; depth 0; no parent.
        None => (
            None,
            CorrelationId(ctx.event_id.0.clone()),
            0,
            ctx.caused_by.clone(),
        ),
        // Caused: provenance is inherited from the parent (correct-by-construction).
        Some(parent) => (
            Some(parent.event_id.clone()),
            parent.correlation_id.clone(),
            // saturating so a pathological chain can never wrap to 0 and defeat the ceiling.
            parent.depth.saturating_add(1),
            // The originating human action is the parent's — inherited unchanged through
            // the whole chain (a child does NOT re-seed it from its own context).
            parent.caused_by.clone(),
        ),
    };

    EventEnvelope {
        event_id: ctx.event_id,
        type_: draft.type_,
        schema_ver: ctx.schema_ver,
        tenant: ctx.tenant,
        region: ctx.region,
        actor: ctx.actor,
        subject: draft.subject,
        aggregate: draft.aggregate,
        causation_id,
        correlation_id,
        caused_by,
        depth,
        contains_personal_data: draft.contains_personal_data,
        data_role: draft.data_role,
        visibility: draft.visibility,
        pii_key_ref: draft.pii_key_ref,
        occurred_at: ctx.occurred_at,
        recorded_at: ctx.recorded_at,
        payload: draft.payload,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn sample_principal() -> Principal {
        Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, TenantId("acme".into()))
    }

    /// Build the canonical anchor envelope used by the field-shape test, the provider-side
    /// CDC contract test, AND the EB-01 round-trip GATE (one fixture, several assertions).
    /// Field-by-field this is the §2.10 / Bus §3.1 names/units anchor: every name spelled,
    /// every frozen unit exercised, the nested causality triad populated, `pii_key_ref` set.
    fn anchor_envelope() -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("01J0".into()),
            type_: EventType("issues.issue.created".into()),
            schema_ver: 1u32,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(sample_principal()),
            subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            causation_id: Some(EventId("01J-parent".into())),
            correlation_id: CorrelationId("root".into()),
            caused_by: Some(CausedBy("session:abc".into())),
            depth: 4u32,
            contains_personal_data: true,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: Some(PiiKeyRef("kms://acme/3/subject:u42".into())),
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            payload: serde_json::json!({ "ref": "myelin://acme/issues/issue/PROJ-1" }),
        }
    }

    /// Build an [`EmitContext`] for the derivation tests: the ambient fields a real
    /// transaction would supply. `caused_by` is the optional originating human-action ref.
    fn ctx_for(event_id: EventId, caused_by: Option<CausedBy>) -> EmitContext {
        EmitContext {
            event_id,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(sample_principal()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            caused_by,
        }
    }

    /// A minimal caller-authored draft (references-not-payloads; no inline PII).
    fn draft_for(type_: &str) -> EventDraft {
        EventDraft {
            type_: EventType(type_.into()),
            subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            payload: serde_json::json!({ "ref": "myelin://acme/issues/issue/PROJ-1" }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }

    /// **EB-01 GATE artifact (2026-06-19).** The single dated proof the anchor is
    /// well-defined: (a) the full envelope — INCLUDING the nested causality triad
    /// (`causation_id` immediate parent + `correlation_id` root + `caused_by`) AND a
    /// populated `pii_key_ref` — round-trips through the canonical JSON encoder LOSSLESS
    /// (every field survives byte-for-byte); and (b) the depth-derivation computed from a
    /// cause is correct (child = parent + 1). This round-trip IS the proof the EB-01 prompt
    /// names ("the round-trip IS the proof the anchor is well-defined").
    #[test]
    fn eb01_full_field_round_trip_and_depth_derivation_is_lossless() {
        // (a) full-field lossless round-trip, every field populated incl. the triad + pii.
        let env = anchor_envelope();
        // pre-conditions: the round-trip is only meaningful if the fixture actually
        // populates the OPTIONAL / nested fields EB-01 calls out.
        assert!(env.causation_id.is_some(), "fixture exercises the immediate-parent leg");
        assert!(env.caused_by.is_some(), "fixture exercises the human-action ref");
        assert!(env.pii_key_ref.is_some(), "fixture exercises a populated pii_key_ref");
        assert_ne!(env.depth, 0, "fixture exercises a non-root depth");

        let json = serde_json::to_string(&env).expect("envelope serialises");
        let back: EventEnvelope = serde_json::from_str(&json).expect("envelope deserialises");
        assert_eq!(back, env, "every field round-trips lossless (the X-5 anchor is well-defined)");
        // explicitly re-assert the nested triad + pii survived (not just the derived Eq).
        assert_eq!(back.causation_id, env.causation_id);
        assert_eq!(back.correlation_id, env.correlation_id);
        assert_eq!(back.caused_by, env.caused_by);
        assert_eq!(back.depth, env.depth);
        assert_eq!(back.pii_key_ref, env.pii_key_ref);

        // (b) depth-derivation (child = parent + 1) computed from a cause is correct.
        let parent = derive_envelope(
            draft_for("issues.issue.created"),
            ctx_for(EventId("01J-root".into()), Some(CausedBy("session:abc".into()))),
            None,
        );
        assert_eq!(parent.depth, 0, "a root is at depth 0");
        let child = derive_envelope(
            draft_for("refs.edge.created"),
            ctx_for(EventId("01J-child".into()), None),
            Some(&parent),
        );
        assert_eq!(child.depth, parent.depth + 1, "child depth = parent depth + 1 (BUS-5)");
        assert_eq!(
            child.causation_id,
            Some(parent.event_id.clone()),
            "causation_id is the immediate parent (nested-not-flat)"
        );
        assert_eq!(
            child.correlation_id, parent.correlation_id,
            "correlation_id (the root) carries through unchanged"
        );

        // the derived child also round-trips lossless (closes the loop: a derived envelope
        // is a wire-shape envelope, no special-casing).
        let cjson = serde_json::to_string(&child).expect("child serialises");
        let cback: EventEnvelope = serde_json::from_str(&cjson).expect("child deserialises");
        assert_eq!(cback, child, "a derived (caused) envelope round-trips lossless too");
    }

    /// P-S06 GATE artifact (1/3): a ROOT emit (`cause == None`) is its own causal root.
    /// It carries its own `correlation_id` (= its event id), has no immediate parent, and
    /// sits at `depth = 0`. The originating human action comes from the context.
    #[test]
    fn emit_root_carries_its_own_correlation_at_depth_zero() {
        let id = EventId("01J-root".into());
        let ctx = ctx_for(id.clone(), Some(CausedBy("session:abc".into())));
        let env = derive_envelope(draft_for("issues.issue.created"), ctx, None);

        assert_eq!(env.event_id, id, "the minted id is carried onto the envelope");
        assert_eq!(env.causation_id, None, "a root has no immediate parent");
        assert_eq!(
            env.correlation_id,
            CorrelationId("01J-root".into()),
            "a root carries its OWN id as the correlation/root (BUS-5)"
        );
        assert_eq!(env.depth, 0, "a root is at causal depth 0");
        assert_eq!(
            env.caused_by,
            Some(CausedBy("session:abc".into())),
            "the root defines the human-action ref for the chain"
        );
    }

    /// P-S06 GATE artifact (2/3): a CAUSED emit (`cause == Some(parent)`) derives its whole
    /// provenance FROM the parent — correct-by-construction. `causation_id` is the parent's
    /// id (the immediate parent, nested-not-flat), `correlation_id` is the parent's root
    /// (carries through), `depth = parent.depth + 1`, and the human-action ref is inherited.
    #[test]
    fn emit_caused_derives_provenance_from_the_parent() {
        // The parent: a root at depth 0.
        let parent = derive_envelope(
            draft_for("issues.issue.created"),
            ctx_for(EventId("01J-root".into()), Some(CausedBy("session:abc".into()))),
            None,
        );

        // The child reacts to the parent. Its OWN context's caused_by is intentionally a
        // DIFFERENT value to prove the derivation IGNORES it and inherits the parent's.
        let child = derive_envelope(
            draft_for("refs.edge.created"),
            ctx_for(EventId("01J-child".into()), Some(CausedBy("session:WRONG".into()))),
            Some(&parent),
        );

        assert_eq!(
            child.causation_id,
            Some(EventId("01J-root".into())),
            "causation_id = the IMMEDIATE parent's event id"
        );
        assert_eq!(
            child.correlation_id,
            CorrelationId("01J-root".into()),
            "correlation_id = the parent's ROOT, carried through unchanged"
        );
        assert_eq!(child.depth, 1, "depth = parent.depth + 1");
        assert_eq!(
            child.caused_by,
            Some(CausedBy("session:abc".into())),
            "the originating human action is INHERITED from the parent, not re-seeded \
             from the child's own context (a deep chain still attributes to the human)"
        );
    }

    /// P-S06 GATE artifact (3/3): a deep chain monotonically increments depth and never
    /// loses the root — the property the loop ceiling (AG-6) and audit walk rely on. Built
    /// by chaining `derive_envelope` end-to-end (a sequence property, EI-01 §4).
    #[test]
    fn emit_deep_chain_keeps_root_and_increments_depth_monotonically() {
        let root = derive_envelope(
            draft_for("issues.issue.created"),
            ctx_for(EventId("01J-0".into()), Some(CausedBy("human:h1".into()))),
            None,
        );

        let mut prev = root.clone();
        for i in 1..=10u32 {
            let next = derive_envelope(
                draft_for("refs.edge.created"),
                ctx_for(EventId(format!("01J-{i}")), Some(CausedBy("human:DECOY".into()))),
                Some(&prev),
            );
            // depth strictly increases by 1 each hop.
            assert_eq!(next.depth, i, "depth increments by exactly 1 per hop");
            assert!(next.depth > prev.depth, "depth is monotonically increasing");
            // the ROOT is preserved across the whole chain.
            assert_eq!(
                next.correlation_id, root.correlation_id,
                "the causal root carries through the entire chain"
            );
            // the immediate parent is the previous hop (nested, not flat).
            assert_eq!(next.causation_id, Some(prev.event_id.clone()));
            // the originating human action is preserved from the root, decoys ignored.
            assert_eq!(next.caused_by, Some(CausedBy("human:h1".into())));
            prev = next;
        }
    }

    /// P-S06: depth derivation saturates rather than wrapping — a pathological chain can
    /// never wrap `u32` back to 0 and slip under the loop ceiling (AG-6). We assert the
    /// `saturating_add` boundary directly so the invariant is pinned independent of how
    /// deep a real chain could plausibly get.
    #[test]
    fn emit_depth_saturates_never_wraps() {
        // A synthetic parent already at u32::MAX depth.
        let mut maxed = derive_envelope(
            draft_for("issues.issue.created"),
            ctx_for(EventId("01J-deep".into()), None),
            None,
        );
        maxed.depth = u32::MAX;

        let child = derive_envelope(
            draft_for("refs.edge.created"),
            ctx_for(EventId("01J-deeper".into()), None),
            Some(&maxed),
        );
        assert_eq!(child.depth, u32::MAX, "depth saturates at u32::MAX, never wraps to 0");
    }

    /// P-S06: the caller-authored fields (type/subject/aggregate/payload/classification)
    /// pass through unchanged; only the causal triple + ambient fields are derived. Proves
    /// the derivation does not mangle the caller's content.
    #[test]
    fn emit_passes_caller_authored_fields_through_unchanged() {
        let draft = draft_for("issues.issue.created");
        let expected_type = draft.type_.clone();
        let expected_subject = draft.subject.clone();
        let expected_payload = draft.payload.clone();

        let env = derive_envelope(draft, ctx_for(EventId("01J".into()), None), None);

        assert_eq!(env.type_, expected_type);
        assert_eq!(env.subject, expected_subject);
        assert_eq!(env.payload, expected_payload);
        assert_eq!(env.data_role, DataRole::Controller);
        assert_eq!(env.visibility, Visibility::Internal);
        // ambient fields come from the context.
        assert_eq!(env.tenant, TenantId("acme".into()));
        assert_eq!(env.region, Region("eu-west".into()));
        assert_eq!(env.schema_ver, 1);
    }

    /// P-S05 GATE artifact: the compile-asserting names/units test that makes the envelope
    /// THE X-5 anchor (contract 2.1; architecture §2.10; Bus §3.1). Every field NAME is
    /// spelled out (a rename breaks the struct-literal at compile time) and every frozen
    /// UNIT is asserted in its frozen form:
    ///   - `occurred_at` / `recorded_at` are `Timestamp` = RFC-3339 UTC (`Z`-suffixed);
    ///   - `depth: u32` (integer causal depth — the loop ceiling reads it, AG-6);
    ///   - `schema_ver: u32` (integer; upcasters gate evolution forward-only);
    ///   - `causation_id` (IMMEDIATE parent) + `correlation_id` (ROOT) — the nested triple;
    ///   - `pii_key_ref` = `kms://<tenant>/<dek-epoch>/<class>`, class ∈ {tenant,subject:<id>,blob};
    ///   - `payload: serde_json::Value` (references-not-payloads — IDs/refs, never a PII body).
    ///
    /// Drift from any name or unit stops this test compiling or failing — this is the freeze.
    #[test]
    fn surface_event_envelope_field_shape_is_frozen() {
        let env = anchor_envelope();

        // --- names + types (the struct literal above already pins every NAME) ---
        // Re-read each by name so a future rename also breaks here, not only at construction.
        let _: &EventId = &env.event_id;
        let _: &EventType = &env.type_;
        let _: &u32 = &env.schema_ver;
        let _: &TenantId = &env.tenant;
        let _: &Region = &env.region;
        let _: &Actor = &env.actor;
        let _: &ArtifactRef = &env.subject;
        let _: &AggregateKey = &env.aggregate;
        let _: &Option<EventId> = &env.causation_id;
        let _: &CorrelationId = &env.correlation_id;
        let _: &Option<CausedBy> = &env.caused_by;
        let _: &u32 = &env.depth;
        let _: &bool = &env.contains_personal_data;
        let _: &DataRole = &env.data_role;
        let _: &Visibility = &env.visibility;
        let _: &Option<PiiKeyRef> = &env.pii_key_ref;
        let _: &Timestamp = &env.occurred_at;
        let _: &Timestamp = &env.recorded_at;
        let _: &serde_json::Value = &env.payload;

        // --- frozen units (§2.10) ---
        // timestamps = RFC-3339 UTC: `T`-separated, `Z`-suffixed (UTC), parseable shape.
        for ts in [&env.occurred_at.0, &env.recorded_at.0] {
            assert!(ts.contains('T'), "timestamp must be RFC-3339 (date T time): {ts}");
            assert!(ts.ends_with('Z'), "timestamp must be UTC (Z-suffixed): {ts}");
        }
        // depth is an integer causal depth (u32) — the loop ceiling (AG-6) reads it.
        assert_eq!(env.depth, 4u32);
        // the causal triple: immediate parent + root carry through (BUS-5, nested-not-flat).
        assert_eq!(env.causation_id, Some(EventId("01J-parent".into())));
        assert_eq!(env.correlation_id, CorrelationId("root".into()));
        // pii_key_ref format: kms://<tenant>/<dek-epoch>/<class>, class ∈ {tenant,subject:<id>,blob}.
        let pkr = &env.pii_key_ref.as_ref().expect("anchor sets pii_key_ref").0;
        assert!(pkr.starts_with("kms://"), "pii_key_ref must be a kms:// URN: {pkr}");
        let rest = pkr.strip_prefix("kms://").unwrap();
        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        assert_eq!(parts.len(), 3, "kms://<tenant>/<dek-epoch>/<class>: {pkr}");
        assert_eq!(parts[0], "acme", "tenant segment");
        assert!(parts[1].parse::<u64>().is_ok(), "dek-epoch is an integer: {}", parts[1]);
        let class = parts[2];
        assert!(
            class == "tenant"
                || class == "blob"
                || class.strip_prefix("subject:").is_some_and(|id| !id.is_empty()),
            "class ∈ {{tenant, subject:<id>, blob}}: {class}"
        );
        // references-not-payloads: payload is a JSON value (IDs/refs), not a typed PII body.
        assert!(env.payload.is_object());
    }

    /// P-S05 CDC artifact: the **provider-side** envelope-shape contract test for 2.1.
    /// The Bus is the provider of the 2.1 envelope; every emitter+consumer reconciles
    /// against this wire shape (the X-5 anchor). This test pins the serialized JSON shape:
    /// the exact set of top-level keys (a rename/add/drop is caught) and the frozen unit
    /// renderings (timestamps as `Z`-suffixed strings, `depth`/`schema_ver` as integers,
    /// `pii_key_ref` as the kms:// URN, `payload` as a nested object — references, not a
    /// PII body). It round-trips to prove the shape is the contract.
    ///
    /// **Floor named:** the CONSUMER half of the 2.1 CDC pair — the relay re-hydrating the
    /// stored envelope (P-S07) and a consumer reading it through the template (P-S08) —
    /// already lands in those prompts (`tests/drills_sub_d2_consumer.rs`
    /// `cdc_2_4_2_5_consumer_reads_relayed_envelope_and_dedups`). The contract-coverage
    /// scanner (P-S21 / P-037, not yet built) reads this provider row + the consumer rows
    /// as the completed pair.
    #[test]
    fn cdc_2_1_envelope_wire_shape_is_the_anchor() {
        let env = anchor_envelope();
        let json = serde_json::to_value(&env).expect("envelope serializes");
        let obj = json.as_object().expect("envelope is a JSON object");

        // The frozen top-level key set (the §2.10 names, in struct-field spelling). A drift
        // — a dropped, added, or renamed field — changes this set and fails the contract.
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut expected = [
            "event_id",
            "type_",
            "schema_ver",
            "tenant",
            "region",
            "actor",
            "subject",
            "aggregate",
            "causation_id",
            "correlation_id",
            "caused_by",
            "depth",
            "contains_personal_data",
            "data_role",
            "visibility",
            "pii_key_ref",
            "occurred_at",
            "recorded_at",
            "payload",
        ];
        expected.sort_unstable();
        assert_eq!(keys, expected, "the 2.1 envelope wire key set is frozen (X-5 anchor)");

        // Frozen unit renderings on the wire.
        assert!(obj["schema_ver"].is_u64(), "schema_ver is an integer on the wire");
        assert!(obj["depth"].is_u64(), "depth is an integer on the wire");
        // timestamps are RFC-3339 UTC strings (Z-suffixed).
        assert_eq!(obj["occurred_at"], serde_json::json!("2026-06-19T00:00:00Z"));
        assert_eq!(obj["recorded_at"], serde_json::json!("2026-06-19T00:00:01Z"));
        // pii_key_ref renders as the kms:// URN string (Option::Some).
        assert_eq!(obj["pii_key_ref"], serde_json::json!("kms://acme/3/subject:u42"));
        // payload is a nested object of references, never a flat PII body.
        assert!(obj["payload"].is_object(), "payload carries references, not a PII body");

        // The shape IS the contract: round-trip is lossless.
        let back: EventEnvelope = serde_json::from_value(json).expect("envelope round-trips");
        assert_eq!(back, env, "the wire shape round-trips to the anchor (no lossy field)");
    }
}
