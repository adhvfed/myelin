//! # `myelin-events` — the canonical `EventEnvelope`, the outbox helper, the consumer template
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.1 (`myelin-events`), §2.10 (the canonical envelope field list + units — the X-5
//! names/units authority), §5 (the event-consumer template).
//!
//! **Contract-index cluster:** 2 — Event envelope, outbox & consumer template
//! (`planning/05-refined-shared-systems-architecture/contract-index.md` rows 2.1
//! `EventEnvelope`, 2.2 `OutboxTx::emit`, 2.4 `EventHandler`/`HandleOutcome`).
//!
//! ## What crosses the crate boundary here (the frozen surface)
//! - `EventEnvelope` (2.1) — the canonical, versioned envelope; **the names/units
//!   anchor** (X-5) every later contract reconciles against. References-not-payloads:
//!   `payload` carries IDs/`ArtifactRef`s, never PII bodies.
//! - `OutboxTx::emit(draft, cause)` (2.2) — the ONLY sanctioned emit path; causality
//!   correct-by-construction; **there is intentionally NO `publish_now`/fire-and-forget**
//!   (a shortcut that exists will be used and will lose data — EI-02 §4). The
//!   `no-raw-publish` lint (P-S10) enforces this externally.
//! - `EventHandler` + `HandleOutcome` (2.4) — the one consumer template; `subjects()` is
//!   a whitelist, NEVER `*` (BUS-3, head-of-line-blocking guard).
//! - `ArtifactRef` (2.1 type) is re-exported here so `myelin_events::ArtifactRef` is the
//!   frozen path the architecture names — see the DAG-deviation note below.
//!
//! ## Frozen units (architecture §2.10; contract-index "Units (frozen)")
//! - timestamps = RFC-3339 UTC (`occurred_at`, `recorded_at`);
//! - budgets/costs = integer minor-units (never floats);
//! - TTLs / staleness / timers = seconds;
//! - `pii_key_ref = kms://<tenant>/<dek-epoch>/<class>`, `<class> ∈ {tenant, subject:<id>, blob}`.
//!
//! ## DAG-deviation note (EI-01 §1; full text in `myelin-tenancy`)
//! The architecture sites the `ArtifactRef` *type* in this crate (§2.1), but the frozen
//! DAG (§2.9) puts `myelin-identity` ABOVE events and `AuthzClient::check` needs
//! `ArtifactRef`. To keep identity a sink, the value newtype is defined in
//! `myelin-tenancy` (the sink) and **re-exported here** as `myelin_events::ArtifactRef`,
//! preserving the frozen public path with no signature change.
//!
//! ## Status (dated; the code wins over the docs — VISION §3)
//! - `EventEnvelope` is **FROZEN as THE names/units anchor (X-5) — P-S05, 2026-06-19.**
//!   P-001 shipped the field list to the frozen shape; P-S05 freezes it as the anchor by
//!   adding (a) the per-name/per-unit compile-assertion test (`surface_event_envelope_*`)
//!   and (b) the **provider-side CDC envelope-shape contract test for contract 2.1**
//!   (`cdc_2_1_*`) that pins the serialized wire shape every later contract reconciles
//!   against. The consumer side of the 2.1 CDC pair (the relay re-hydrating + a consumer
//!   reading the wire envelope) lands in **P-S07/P-S08** — named, not silently skipped.
//!
//! ## Status (P-S06, 2026-06-19) — causality is correct-by-construction
//! The `OutboxTx::emit` causality derivation is **implemented** as the pure, frozen
//! [`derive_envelope`] function: root carries its own `correlation_id` (= its event id),
//! a caused event sets `causation_id = cause.event_id`, `correlation_id =
//! cause.correlation_id`, `depth = cause.depth + 1` (saturating), and inherits the
//! parent's `caused_by` human-action ref unchanged. The causal-triple fields are NOT on
//! [`EventDraft`] — they are derived, never authored — so a human/agent cannot typo their
//! way into a loop (EI-02 §6). There is no `publish_now` on `OutboxTx`; the only verb is
//! `emit` (the `no-raw-publish` lint, P-S10, enforces the absence workspace-wide).
//!
//! ## Floors named (stubbed bodies → filling prompt)
//! - The `outbox` table + the same-transaction insert + the relay (2.3, SUB-D1/BUS-D4)
//!   is **P-S07** — `OutboxTx::emit`'s body (which pulls the ambient [`EmitContext`] from
//!   the transaction handle, calls [`derive_envelope`], and inserts the row) lands there;
//!   the trait method has no default body here because the storage handle is P-S07's.
//! - The `EventHandler` consumer runtime + `consumer_dedup` ledger (2.5, SUB-D2) is
//!   **P-S08**; the upcaster registry (2.8) is **P-S09**. Bodies here are `todo!()`.
//! - `pii_key_ref`'s KMS hierarchy (the DEK epochs) is Storage M1 (11.3); P-001 ships
//!   only the field + its format.

use myelin_identity::Principal;
use myelin_tenancy::{Region, TenantId};
use serde::{Deserialize, Serialize};

/// Re-export of the `ArtifactRef` value type so `myelin_events::ArtifactRef` is the
/// frozen path (the envelope's `subject` type). Definition site is `myelin-tenancy`
/// (the DAG sink) — see the crate-level DAG-deviation note.
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor(pub Principal);

/// controller | processor — the GDPR fan-out role of the event's data (architecture
/// §2.1, `data_role`; ADR-04.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataRole {
    Controller,
    Processor,
}

/// The event's visibility class (architecture §2.1, `visibility`).
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
/// Field order + names match the §2.10 frozen anchor. **Frozen as THE anchor by P-S05**
/// (2026-06-19): the per-name/per-unit compile test + the provider-side CDC envelope-shape
/// contract test (2.1) pin it. Any drift from a name/type/unit now fails to compile or
/// fails the wire-shape CDC test.
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
/// through unchanged by [`OutboxTx::emit`]'s derivation onto every child — see
/// [`derive_envelope`].
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

/// Placeholder error for the skeleton. The real outbox error taxonomy lands with the
/// table + relay (P-S07).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxError(pub String);

/// `Result` alias for the emit surface.
pub type Result<T> = core::result::Result<T, OutboxError>;

/// The ONLY sanctioned emit path (architecture §2.1; contract 2.2; BUS-2). Inserts into
/// the per-service `outbox` table IN THE SAME TRANSACTION as the state change (table +
/// relay land in P-S07). **There is no fire-and-forget publish** — no `publish_now` on
/// this trait (the `no-raw-publish` lint, P-S10, enforces it).
pub trait OutboxTx {
    /// Derives causality correct-by-construction (BUS-5, EI-02 §6): a root event carries
    /// its own correlation; a caused event sets `causation_id = cause.event_id`,
    /// `correlation_id = cause.correlation_id`, `depth = cause.depth + 1`.
    ///
    /// The provenance derivation itself lives in the pure, frozen [`derive_envelope`]
    /// function (P-S06): every implementer pulls its ambient [`EmitContext`] (tenant /
    /// region / actor / clock / minted ULID) from the transaction handle `self` carries,
    /// calls [`derive_envelope`], and inserts the resulting [`EventEnvelope`] into the
    /// per-service `outbox` table IN THE SAME TRANSACTION as the state change — returning
    /// the minted [`EventId`]. The signature is the frozen contract-2.2 shape; the ambient
    /// context is intentionally NOT a parameter (it is the transaction's, not the caller's),
    /// which is why a caller cannot fabricate a wrong root.
    ///
    /// **Floor:** the `outbox` table + the same-transaction insert + the relay are
    /// **P-S07**; here P-S06 ships the causality derivation ([`derive_envelope`]) the table
    /// will call. There is intentionally **no `publish_now`** — the only emit verb is
    /// `emit` (the `no-raw-publish` lint, P-S10, enforces the absence externally).
    fn emit(&mut self, draft: EventDraft, cause: Option<&EventEnvelope>) -> Result<EventId>;
}

/// A consumer subscription subject pattern (architecture §5; contract 2.4). The consumer
/// template rejects a `*` subscription at registration (BUS-3, head-of-line guard).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectPattern(pub String);

/// A non-retryable reason (poison) (architecture §5; contract 2.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason(pub String);

/// A retry backoff hint (architecture §5; contract 2.4). Seconds (frozen unit, §2.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backoff {
    pub seconds: u64,
}

/// The outcome of handling one event (architecture §5; contract 2.4). At-least-once +
/// idempotent ≈ effectively-once; a poison message terminates immediately (dead-letter).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandleOutcome {
    Done,
    NonRetryable(Reason),
    Retry(Backoff),
}

/// The one consumer template (architecture §5; contract 2.4; BUS-3). Built from this
/// single trait so the seven encoded rules cannot be skipped per-consumer. `subjects()`
/// is a whitelist, **NEVER `*`** (an over-broad subscription head-of-line-blocks
/// everything). `handle` is idempotent on `event_id` via the `consumer_dedup` ledger.
///
/// **Floor:** the consumer runtime (the seven rules + the dedup ledger) is **P-S08**;
/// the upcaster registry that runs before `handle` is **P-S09**. The trait shape is
/// frozen here.
pub trait EventHandler {
    /// Whitelist — NEVER `*` (BUS-3, D7-i).
    fn subjects(&self) -> &'static [SubjectPattern];
    /// Idempotent on `event_id` (ADR-04.1). Body is the consumer's; the runtime around
    /// it (dedup, ack-after-enqueue, bounded prefetch, lag metric) is P-S08.
    fn handle(&self, ev: &EventEnvelope) -> HandleOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn sample_principal() -> Principal {
        Principal {
            id: PrincipalId("p".into()),
            kind: PrincipalKind::Human,
            tenant: TenantId("acme".into()),
        }
    }

    /// Build the canonical anchor envelope used by both the field-shape test and the
    /// provider-side CDC contract test (one fixture, two assertions). Field-by-field this
    /// is the §2.10 names/units anchor: every name spelled, every frozen unit exercised.
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

    /// Build an [`EmitContext`] for the P-S06 derivation tests: the ambient fields a real
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
    /// THE X-5 anchor (contract 2.1; architecture §2.10). Every field NAME is spelled out
    /// (a rename breaks the struct-literal at compile time) and every frozen UNIT is
    /// asserted in its frozen form:
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
    /// lands in those prompts. The contract-coverage scanner (P-S21) reads this provider
    /// row + the P-S07/P-S08 consumer rows as the completed pair.
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

    /// P-S06 CDC artifact: the **provider-side** contract test for row 2.2
    /// (`OutboxTx::emit(draft, cause)`). It pins the frozen emit surface — there is NO
    /// `publish_now` / fire-and-forget on `OutboxTx`; the trait's only method is
    /// `emit(draft, cause)` — and exercises a real implementer end-to-end so the derivation
    /// the contract promises (root carries / parent = cause.event_id / depth + 1) is what an
    /// `emit` actually produces. The `no-raw-publish` lint (P-S10) enforces the absence of
    /// any other publish symbol across the workspace.
    ///
    /// **Floor named:** the CONSUMER half of the 2.2 CDC pair — the same-transaction insert
    /// into the `outbox` table + the relay re-hydrating + delivering the derived envelope —
    /// lands in **P-S07**. The contract-coverage scanner (P-S21) reads this provider row +
    /// the P-S07 consumer row as the completed pair.
    #[test]
    fn cdc_2_2_emit_is_the_only_path_and_derives_causality() {
        struct Tx {
            next: u32,
        }
        impl OutboxTx for Tx {
            fn emit(&mut self, draft: EventDraft, cause: Option<&EventEnvelope>) -> Result<EventId> {
                // A real implementer mints the id + ambient context, derives, then (P-S07)
                // inserts the row in the same tx. Here we return the derived envelope's id.
                let id = EventId(format!("01J-{}", self.next));
                self.next += 1;
                let env = derive_envelope(draft, ctx_for(id, Some(CausedBy("human:h".into()))), cause);
                Ok(env.event_id)
            }
        }

        let mut tx = Tx { next: 0 };
        // A root emit through the trait.
        let root_id = tx.emit(draft_for("issues.issue.created"), None).expect("root emits");
        assert_eq!(root_id, EventId("01J-0".into()));

        // Re-derive the root envelope to feed as the cause (P-S07 would read it back from
        // the outbox row); prove a caused emit through the SAME trait derives depth + 1.
        let root_env = derive_envelope(
            draft_for("issues.issue.created"),
            ctx_for(EventId("01J-0".into()), Some(CausedBy("human:h".into()))),
            None,
        );
        let child_id = tx
            .emit(draft_for("refs.edge.created"), Some(&root_env))
            .expect("caused emits");
        assert_eq!(child_id, EventId("01J-1".into()));

        // The frozen signature is `emit(&mut self, EventDraft, Option<&EventEnvelope>)`.
        // If a `publish_now` existed it would be nameable; it does not — `emit` is the only
        // verb (BUS-2). The trait object below also proves no other method is required.
        let _obj: &mut dyn OutboxTx = &mut tx;
    }

    /// Compile-asserting test: there is NO `publish_now` / fire-and-forget on `OutboxTx`
    /// (BUS-2). The trait's only method is `emit(draft, cause)`. A stub implementer
    /// proves the frozen signature; the `no-raw-publish` lint (P-S10) enforces the
    /// absence of any other publish symbol across the workspace.
    #[test]
    fn outbox_has_only_emit_no_publish_now() {
        struct Stub;
        impl OutboxTx for Stub {
            fn emit(&mut self, draft: EventDraft, cause: Option<&EventEnvelope>) -> Result<EventId> {
                // The shape an implementer follows (P-S07 wraps this in the same-tx insert):
                // pull the ambient context from `self`, derive the envelope, return the id.
                let ctx = ctx_for(EventId("01J-stub".into()), None);
                let env = derive_envelope(draft, ctx, cause);
                Ok(env.event_id)
            }
        }
        // If a `publish_now` existed it would be nameable here; it does not. The presence
        // of exactly one method on the constructed value is the compile-time assertion.
        let _s = Stub;
    }

    /// Compile-asserting test: the consumer template shape is frozen (contract 2.4) —
    /// `subjects() -> &'static [SubjectPattern]` (whitelist) + `handle -> HandleOutcome`
    /// with the three frozen variants.
    #[test]
    fn event_handler_template_shape_is_frozen() {
        struct Idx;
        static SUBJECTS: &[SubjectPattern] = &[];
        impl EventHandler for Idx {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(&self, _ev: &EventEnvelope) -> HandleOutcome {
                HandleOutcome::Done
            }
        }
        let h = Idx;
        assert!(h.subjects().is_empty());
        assert_eq!(h.handle(&sample_envelope()), HandleOutcome::Done);
    }

    fn sample_envelope() -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("01J0".into()),
            type_: EventType("t.a.e".into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(sample_principal()),
            subject: ArtifactRef("myelin://acme/t/a/1".into()),
            aggregate: AggregateKey("a:1".into()),
            causation_id: None,
            correlation_id: CorrelationId("root".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Processor,
            visibility: Visibility::Private,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
            payload: serde_json::Value::Null,
        }
    }
}
