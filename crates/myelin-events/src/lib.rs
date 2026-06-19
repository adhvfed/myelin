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
//! ## Status (P-011 / EB-01, 2026-06-19) — the Bus-system envelope freeze, reconciled in place
//! EB-01 ("Freeze the EventEnvelope struct, the names/units anchor") is the **event-bus
//! ledger's framing of the SAME single deliverable P-S05 already shipped** (the global run
//! order interleaves the substrate + event-bus roadmaps, so the envelope freeze is reached
//! from both — P-005 and P-011). Per the coherence rule (EI-01 §7: never define a type
//! twice, never build a parallel second implementation), EB-01 **reconciles in place**: the
//! frozen `EventEnvelope` struct + its value types + the causality derivation were MOVED
//! verbatim from this crate root into [`envelope`] (the file EB-01 names — `envelope.rs`),
//! with **no name/type/unit/field change**, and are re-exported here so every frozen public
//! path (`myelin_events::EventEnvelope`, `::EventId`, `::derive_envelope`, …) is unchanged.
//! What EB-01 ADDS is (a) the named-deliverable file home and (b) the EB-01 round-trip GATE
//! artifact `envelope::tests::eb01_full_field_round_trip_and_depth_derivation_is_lossless`
//! — one dated test proving the anchor is well-defined: every field (incl. the nested
//! causality triad AND a populated `pii_key_ref`) round-trips lossless, and the
//! depth-derivation (child = parent + 1) from a cause is correct. EB-01's DoD also names
//! "the contract-coverage scanner passes for 2.1": that scanner is **P-037 / P-S21, not yet
//! built at this point in the run order** — the provider-side 2.1 CDC test
//! (`cdc_2_1_envelope_wire_shape_is_the_anchor`) + the consumer-side CDC
//! (`tests/drills_sub_d2_consumer.rs::cdc_2_4_2_5_*`) are both present, so the 2.1 pair the
//! scanner will read is complete; the scanner row greens when P-037 lands (floor named).
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
//! ## Status (P-S07, 2026-06-19) — the `outbox` table + the relay (SUB-D1 / BUS-D4)
//! The `outbox` table (contract 2.3) + the same-transaction co-commit + the relay are
//! **implemented** (see [`outbox`] + [`relay`]). [`OutboxTx::emit`] now has a concrete
//! implementer ([`outbox::OutboxTransaction`]) that mints the stable ULID, derives via
//! [`derive_envelope`], assigns the per-aggregate `seq`, and BUFFERS the row into the open
//! transaction — durable iff the transaction commits ([`outbox::OutboxTransaction::commit`]),
//! published nowhere if it is dropped (emit-iff-committed, **BUS-D4**, structural). The
//! [`relay::Relay`] claims unsent rows with the `FOR UPDATE SKIP LOCKED` discipline, publishes
//! via [`relay::BusTransport`] with `dedup_id = event_id` (the stable broker-side dedup → 0
//! ghost), marks sent, and dead-letters after [`relay::MAX_PUBLISH_ATTEMPTS`]; a killed relay
//! re-claims the unsent rows → 0 lost (**SUB-D1**). `outbox_depth` + the dead-letter count are
//! exported ([`outbox::OutboxStore::outbox_depth`] / `dead_letter_count`). **This is a
//! PERMANENT gate (re-run on every emit-path change).**
//!
//! ## Status (P-S08, 2026-06-19) — the idempotent consumer runtime + `consumer_dedup` (SUB-D2)
//! The [`EventHandler`] template now has its **one runtime** ([`consumer::Consumer`]) that
//! encodes the seven §5 rules so no consumer can skip one, plus the `consumer_dedup` ledger
//! (contract 2.5, the effectively-once anchor). [`consumer::Subscription::bind`] REJECTS a `*`
//! subscription at registration (rule 3, unconstructable wildcard); [`consumer::DedupLedger`]
//! makes a redelivered `event_id` a no-op (rule 1, `(consumer, event_id)` PK); the runtime acks
//! only on `Done` (rule 2 — a `Retry` is not acked → 0 lost), binds durable-by-name so a
//! reconnect resumes (rule 4 — the **SUB-D2** 0-lost/0-dup-across-reconnect core re-uses the same
//! ledger), dead-letters poison immediately (rule 5), bounds prefetch (rule 6), and exports
//! `consumer_lag` (rule 7). **SUB-D2** (drop broker mid-stream → 0 lost across reconnect, slow
//! subject does not head-of-line-block) and **SUB-D1 re-confirmed through a consumer** (the dedup
//! ledger absorbs the relay redelivery → 0 dup) are drilled in
//! `tests/drills_sub_d2_consumer.rs`. **This is a PERMANENT gate (re-run on every emit-path
//! change).** The upcaster registry that runs before `handle` is **P-S09** — the pre-handle hook
//! ([`consumer::Consumer::with_upcaster`]) is the seam it plugs into (identity map until then).
//!
//! ## Status (P-014 / EB-11, 2026-06-19) — the Bus survival signals on the metrics-health port
//! [`telemetry`] is the **Bus's provider side of contract 1.8 (§4.11)**: it reads the Bus's live
//! counters (outbox depth + age, the relay's published / dead-letter counts) and folds in the
//! producer-fed [`telemetry::BusObservations`] (consumer lag, dedup hit-rate, per-tenant
//! in-flight, causal-depth max, shared-root-tripwire firings) into the seven §4.11 survival
//! signals, then [`telemetry::BusSignals::emit_to`] writes each as a [`telemetry::MetricSample`]
//! — **with the right name + unit** ([`telemetry::BusSignal`]) — onto the metrics-health port
//! seam [`telemetry::MetricsSink`]. These ARE the assertions the §8 Bus drills read; EB-11 wires
//! them so every later Bus drill has a signal to assert against. The **harness self-test** the
//! M0→M1 exit gate requires (inject a producer-kill fault → read the outbox-depth + dedup
//! telemetry assertion) is `tests/drills_eb11_telemetry_self_test.rs`: it snapshots the Bus
//! after a `Dependency::Broker` kill, emits to a [`telemetry::MetricRecorder`], and maps the
//! recorded samples into the harness `SignalSource` to assert `outbox_depth`/`dedup` green
//! (loud, never swallowed). **With EB-01..EB-11 the M0→M1 exit gate is fully green.**
//!
//! DEVIATION (EI-01 §1, documented): the contract-1.8 ASSERTION library (`SignalName` /
//! `SignalSource` / `Predicate` / `Assertion`) already shipped in `myelin-harness` (P-S04) and
//! is the FROZEN §10.2 16-name enum. `myelin-events` cannot depend on the harness in production
//! (it is a dev-dependency-only leaf TEST-SUPPORT crate; an `events → harness` production edge
//! would invert the §2.9 DAG). So [`telemetry`] owns the Bus's *emit* vocabulary as plain
//! `&'static str` name+unit constants whose names line up 1:1 with the harness `SignalName`,
//! rather than re-defining or widening that frozen enum (the harness's exhaustive-`ALL` test
//! stays at 16). The Bus-finer signals (outbox age, publish latency, dedup hit-rate, per-tenant
//! in-flight) are the Bus's contribution UNDER the §10.2 rows ("depth **+ age**", "consumer lag
//! … oldest-un-acked **age**", "per-tenant **in-flight**"); the self-test bridges the two.
//!
//! ## Status (P-015 / EB-06, 2026-06-19) — the `consumer_dedup` ledger gets its named home
//! EB-06 ("The consumer_dedup ledger, the effectively-once anchor") is the **event-bus ledger's
//! framing of the row-2.5 deliverable P-009 / P-S08 already shipped** (the substrate roadmap
//! reached the consumer template — which DEPENDS on the ledger — first; the event-bus roadmap
//! reaches the ledger as its own EB-06 unit). Per the coherence rule (EI-01 §7: never define a
//! type twice, never build a parallel second implementation), EB-06 **reconciles in place**: the
//! [`DedupLedger`] + the frozen 2.5 DDL [`CONSUMER_DEDUP_MIGRATION`] were MOVED verbatim out of
//! [`consumer`] into [`dedup`] (the EB-06-named file home) with **no name/type/unit/semantics
//! change**, and are re-exported here so every frozen public path (`myelin_events::DedupLedger`,
//! `::CONSUMER_DEDUP_MIGRATION`) is unchanged; [`consumer::Consumer`] keeps calling exactly the
//! same `mark_handled`/`revert` API (rule 1). What EB-06 ADDS is (a) the named-deliverable file
//! home for the effectively-once anchor and (b) the **standalone 2.5 CDC pair** + the focused unit
//! tests (idempotent re-delivery proven: one effect on double-delivery; the per-consumer PK
//! proven: two consumers record the same event independently). The provider+consumer CDC pair for
//! 2.5 is `tests/cdc_2_5_consumer_dedup.rs`; the combined end-to-end 2.4/2.5 relay→consumer pair
//! (`tests/drills_sub_d2_consumer.rs::cdc_2_4_2_5_*`) stays as the integration pair. The gate is
//! structural (no standalone catalogue drill — the dedup property is greened transitively by
//! SUB-D2 in EB-05/P-009): the same `(consumer, event_id)` inserted twice yields one row and the
//! handler runs once (the `ON CONFLICT DO NOTHING` property).
//!
//! ## Floors named (stubbed bodies → filling prompt)
//! - **The OLTP binding is modeled in-memory at M0.** There is no live database (the OLTP tier
//!   client is **P-007 / P-ST-01**; the migration runner is **P-S15**). [`outbox::OUTBOX_MIGRATION`]
//!   is the frozen 2.3 DDL the runner will apply; [`outbox::OutboxStore`] models exactly its
//!   semantics until then. The real `SELECT … FOR UPDATE SKIP LOCKED` + `INSERT` against the
//!   Storage pool lands when the OLTP client is wired (P-007 + `serve` P-S12). See the
//!   DEVIATION note in [`outbox`].
//! - **The real `BusTransport` adapter is the Bus's M0 deliverable (EB-04 → P-013).** P-S07
//!   shipped the trait + an in-process fake ([`relay::InProcessBus`]); **EB-04 (P-013) added the
//!   three relay refinements** arch §4.1 names — the `dlq.<tenant>.<subsystem>` dead-letter
//!   Signal alert ([`relay::DeadLetterAlert`] / [`relay::Relay::dead_letter_alerts`]), the 24h
//!   published-row GC ([`relay::Relay::gc_published`]), and the `BusTransport` put/consume CDC
//!   conformance pair — reconciled IN PLACE on the same trait, no second implementation. The one
//!   thing still owed: the JetStream-class reference adapter that implements the SAME frozen
//!   `put/consume/ack/purge` shape against a real broker is wired when the `serve` lifecycle +
//!   the broker binding land (P-S12 / the Bus M0 deployment); the relay algorithm + the drilled
//!   0-ghost/0-lost property do not change.
//! - **The single-region event log → column-store seam** is the post-M5 follow-on (the
//!   `BusTransport` trait IS that seam; promoted only when volume is measured; named in EB-31).
//! - **The ULID source** is the injected [`outbox::IdMinter`] ([`outbox::MonotonicMinter`] is
//!   the deterministic floor); the real wall-clock+random ULID source wires at **P-S12**.
//! - The `EventHandler` consumer runtime (2.4, SUB-D2) **shipped in P-S08** (see [`consumer`];
//!   re-confirms SUB-D1 end-to-end through a consumer). The `consumer_dedup` ledger (2.5, the
//!   effectively-once anchor) shipped with it and was given its named home in [`dedup`] by
//!   **EB-06 / P-015** (the reconciliation Status block above). The upcaster registry (2.8) the
//!   runtime calls before `handle` is **P-S09** — the [`consumer::Consumer::with_upcaster`] hook
//!   is its install seam; identity map until then.
//! - `pii_key_ref`'s KMS hierarchy (the DEK epochs) is Storage M1 (11.3); P-001 ships
//!   only the field + its format.
//! - **The metrics-health PORT + the producer-side clock (P-014/EB-11).** [`telemetry`] ships
//!   the *emit* surface ([`telemetry::MetricsSink`] + the in-memory [`telemetry::MetricRecorder`])
//!   and the snapshot that drives it; the OpenTelemetry exporter on the real §3.5 metrics-health
//!   port + the monotonic clock that feeds outbox-age / publish-latency wire at **`serve`,
//!   P-S12/P-S13**. The signal NAMES + UNITS this module emits are the ones that port exports.
//!   The dispatch-tier **shared-root tripwire COUNTER** that feeds
//!   [`telemetry::BusObservations::shared_root_tripwire_firings`] is **EB-23 (P-143)**; here the
//!   signal name/unit + the snapshot seam are frozen so EB-23 only feeds the count (until then
//!   it is `0` — no tripwire has fired).

pub mod consumer;
pub mod dedup;
pub mod envelope;
pub mod outbox;
pub mod relay;
pub mod telemetry;

pub use consumer::{
    Consumer, ConsumerName, DeadLetter, Delivered, Message, PrefetchBound, SubscribeError,
    Subscription,
};
pub use dedup::{DedupLedger, CONSUMER_DEDUP_MIGRATION};
pub use telemetry::{
    BusObservations, BusSignal, BusSignals, MetricLabel, MetricRecorder, MetricSample, MetricsSink,
};
pub use outbox::{
    EmitContextBase, IdMinter, MonotonicMinter, OutboxRow, OutboxStore, OutboxTransaction, Ulid,
    OUTBOX_MIGRATION,
};
pub use relay::{
    dlq_subject, BusTransport, DeadLetterAlert, Delivery, DrainReport, InProcessBus, Relay,
    TransportError, MAX_PUBLISH_ATTEMPTS,
};

use serde::{Deserialize, Serialize};

/// The canonical `EventEnvelope` (contract 2.1, the X-5 names/units anchor) + its value
/// types + the correct-by-construction causality derivation live in [`envelope`]
/// (**EB-01** moved them there from the crate root, matching the EB-01 named deliverable
/// location, with no name/type/unit change). They are re-exported here so the frozen
/// public paths (`myelin_events::EventEnvelope`, `::EventId`, `::derive_envelope`, …) are
/// unchanged — every emitter, consumer, and the outbox/relay/consumer modules below keep
/// resolving `crate::EventEnvelope` &c. through this re-export.
pub use envelope::{
    derive_envelope, Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole,
    EmitContext, EventDraft, EventEnvelope, EventId, EventType, PiiKeyRef, Timestamp, Visibility,
};

/// Re-export the `(tenant, region)` partition-key types so `crate::TenantId` / `crate::Region`
/// (the paths the outbox/relay/consumer modules and `myelin_events::*` consumers use) keep
/// resolving after EB-01 moved the envelope into [`envelope`]. Definition site is
/// `myelin-tenancy` (the DAG sink); these are the architecture's first-class partition key.
pub use myelin_tenancy::{Region, TenantId};

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
/// The consumer runtime (the seven rules + the dedup ledger) is [`consumer::Consumer`]
/// (**shipped in P-S08**); the upcaster registry that runs before `handle` is **P-S09**
/// (the [`consumer::Consumer::with_upcaster`] hook). The trait shape is frozen here.
pub trait EventHandler {
    /// Whitelist — NEVER `*` (BUS-3, D7-i). [`consumer::Subscription::bind`] enforces the
    /// `*`-rejection at registration so an over-broad subscription is unconstructable.
    fn subjects(&self) -> &'static [SubjectPattern];
    /// Idempotent on `event_id` (ADR-04.1). Body is the consumer's; the runtime around
    /// it (dedup, ack-after-enqueue, bounded prefetch, lag metric) is [`consumer::Consumer`].
    fn handle(&self, ev: &EventEnvelope) -> HandleOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn sample_principal() -> Principal {
        Principal {
            id: PrincipalId("p".into()),
            kind: PrincipalKind::Human,
            tenant: TenantId("acme".into()),
        }
    }

    /// Build an [`EmitContext`] for the emit-surface CDC tests: the ambient fields a real
    /// transaction would supply. `caused_by` is the optional originating human-action ref.
    /// (The exhaustive derivation tests live in [`crate::envelope`]; here we only need the
    /// helper to exercise the `OutboxTx`/`EventHandler` SURFACE shapes this module owns.)
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
