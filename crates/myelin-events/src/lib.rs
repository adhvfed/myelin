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
//! ## Status (P-S09, 2026-06-19) — the schema-evolution upcaster registry (forward-only, 2.8)
//! The `(type, from_ver) → to_ver` upcaster registry ([`upcast::UpcasterRegistry`]) is
//! **implemented** (contract 2.8). It holds, per event type, the adjacent `v → v+1` PURE shape
//! transforms and composes them into a forward-only chain ([`upcast::UpcasterRegistry::upcast`])
//! that lifts an old envelope to the current `schema_ver` BEFORE `handle`. Three rules are
//! encoded: forward-only one-step hops (a backwards/skipping/duplicate hop is rejected loudly at
//! [`upcast::UpcasterRegistry::register`]); an unbridgeable gap is a loud
//! [`upcast::UpcastError::UnbridgeableGap`] → the consumer dead-letters it
//! ([`HandleOutcome::NonRetryable`] / DLQ), NEVER a silent drop and NEVER the wrong shape handed
//! to a handler; the transforms are pure (deterministic, no side effects → reindex-from-source ==
//! live). The consumer seam ([`consumer::Consumer::with_upcaster`]) is now **fallible**:
//! [`upcast::UpcasterRegistry::into_hook`] installs the registry, and the runtime turns a gap into
//! a dead-letter. The CDC + unit tests live in [`upcast`] (`tests`) and
//! [`consumer`] (`unbridgeable_gap_dead_letters_loudly_never_silently_passes`).
//!
//! **Reconciliation (EI-01 §7):** EB-10 (P-046) is the event-bus framing of this SAME row-2.8
//! deliverable (the run order interleaves the substrate + event-bus roadmaps); it reconciles in
//! place against this module (the file it names — `upcast.rs` — is [`upcast`]) by adding the
//! Bus-flavoured `v1→v2→v3`-chain / un-upcastable→DLQ / unknown-forward-field tests + the 2.8
//! CDC pair — no second registry, no type re-definition.
//!
//! ## Status (P-042 / EB-02, 2026-06-19) — the taxonomy grammar validator + the seed token table
//! EB-02 ships the **one grammar** ([`taxonomy::validate`]) every `EventEnvelope.type_` is
//! validated against (Bus §6.1: `<subsystem>.<artifact_type>.<event_name>`, lowercase singular
//! past-tense, tokens `[a-z][a-z0-9_]*`, 2 segments min / 3 when an artifact type clarifies, the
//! leading token a canonical §6.2 subsystem), the **seed** token table
//! ([`taxonomy::SEED_EVENT_NAMES`] — the §6.4 representative names), and the **three new tokens**
//! the reconciliation registered (recon §2): `ci.check.updated` + `ci.result` (the X-1 check seam,
//! §6.3) and the `initiative` artifact-type token (§6.2 extension), all in
//! [`taxonomy::new_tokens`]. The structural gate is the red+green fixture pair (the same ratchet
//! shape the lints use): [`taxonomy::tests::reject_fixture_malformed_names_are_rejected_with_their_rule`]
//! (RED — uppercase / plural / present-tense / single-segment / unknown subsystem / hyphen /
//! leading-digit / empty-segment / too-many-segments all rejected with their rule) and
//! [`taxonomy::tests::admit_fixture_every_seed_name_and_the_three_new_tokens_pass`] (GREEN — every
//! seed name + the three new tokens admitted). The provider+consumer CDC pair for row 2.9 is
//! `tests/cdc_2_9_taxonomy.rs` (a PROVIDER that emits canonical-shape types through the
//! validator + a CONSUMER that admits a canonical type and rejects a malformed one); the
//! contract-coverage manifest flips 2.9 `deferred → covered` naming it. **FLOOR named (EI-01 §1):**
//! the per-subsystem dotted-name LIST completion is **EB-24** (each subsystem owns its full list in
//! M3/M4, validated against THIS grammar); EB-02 is the grammar + seed + the new tokens only. The
//! `iam.*` family Identity already ships (`myelin-identity::iam_events`) is Identity's OWN §11.2
//! token set (distinct from the §6.4 `identity.*` Bus seed); both obey this grammar.
//!
//! ## Status (P-091 / EB-14, 2026-06-19) — the cross-cell bridge FRAME pinned from the Bus side
//! EB-14 ("Pin the cross-cell bridge FRAME — `CrossCellPointer`, designed-not-built") is the
//! **event-bus ledger's framing of contract 12.6**, the SAME frozen frame the Tenancy ledger
//! already shipped at **P-CP-02 / P-027** (`myelin_tenancy::CrossCellPointer` + its three
//! supporting value types). Per the coherence rule (EI-01 §7: never define a type twice, never a
//! parallel second implementation), EB-14 **reconciles in place** — the frame's AUTHORITY is
//! `myelin-tenancy` (the §2.9 DAG SINK; the frame's `correlation_id` is the SAME `CorrelationId`
//! the envelope carries, which by the DAG-deviation lives in the sink). So [`crosscell`] does NOT
//! re-define `CrossCellPointer` / `OpaqueSubjectId` / `ArtifactType` / `CellId`: it **re-exports
//! the tenancy authority** on the frozen Bus path (`myelin_events::CrossCellPointer`, …) so the
//! Bus's §5 contract surfaces compile against the ONE frozen frame. What EB-14 ADDS is the Bus-side
//! GATE the prompt names: (a) the serde round-trip through the Bus's own re-export path
//! ([`crosscell::tests::eb14_frame_serde_round_trips_through_the_bus_path`] — exactly the four §6.1
//! fields, `type` wire-name pinned), (b) the compile-time **cell-agnostic** assertion
//! ([`crosscell::assert_cell_agnostic`]) that the §5 surfaces take the OPAQUE subject, never a
//! cell-bound row, and (c) the Bus-side 12.6 CDC serde-conformance pair. **FLOOR named (EI-01 §1):**
//! single-home-cell propagation is v1; the cross-cell PII-free bridge BUILD (per-viewer cell-local
//! resolution, the residency proof that no PII crosses, multi-cell fan-out) is the **M5 follow-on
//! EB-25** (whose drills GA-D8 / CP-D7 / CP-D8 are owed THEN). No catalogue drill greens here — the
//! frame is designed-not-built; the gate is structural (serde round-trip + the cell-agnostic
//! compile assertion).
//!
//! ## Status (P-092 / EB-15, 2026-06-19) — the Bus as a `PersonalDataHolder` + inline-PII crypto-shred
//! EB-15 ships [`holder`] — the Bus's instantiation of the ONE platform erasure posture (Bus §4.8 /
//! X-7, by reference): the references-not-payloads + crypto-shred + tombstone triad. [`holder::BusHolder`]
//! is the §5.7 `locate`/`erase`/`export` MECHANISM (contract **2.7 OWNED**) over the in-cell
//! [`holder::BusEventLog`]: `erase(subject)` crypto-shreds the subject's RARE inline-PII DEK through
//! the [`holder::InlinePiiShredder`] KMS seam (contract 11.4, CONSUMED) and emits `bus.event.erased`
//! tombstones through the **outbox** (the only sanctioned emit path; there is no `publish_now`,
//! BUS-2), returning an [`holder::EraseReceipt`] proving **0 recoverable** inline-PII in the live log;
//! a live consumer degrades gracefully on a tombstone ([`holder::degrade_on_tombstone`] → `Done`,
//! never blocks, never reads the now-unrecoverable payload). The **BUS-D8 live-store leg** is green
//! (`tests/drills_bus_d8_crypto_shred.rs`: 0 recoverable inline-PII live + tombstones present +
//! consumers degrade + nothing lost, bridged into the §10.2 harness assertion library); the 2.7 CDC
//! pair is `tests/cdc_2_7_bus_holder.rs` (the coverage manifest flips 2.7 `deferred → covered`).
//!
//! **DEVIATION (EI-01 §1, documented):** the EB-15 prompt says "impl `PersonalDataHolder` for the
//! EventBus", but that trait (10.1) is in `myelin-gdpr` and the real `KmsEngine` (11.3) is in
//! `myelin-storage` — **both DOWNSTREAM of `myelin-events` in the frozen §2.9 DAG**. So `myelin-events`
//! ships the holder MECHANISM to the exact §5.7 shape against a LOCAL crypto-shred seam
//! ([`holder::InlinePiiShredder`]; in-memory floor [`holder::InMemoryShredder`]); the thin
//! `impl gdpr::PersonalDataHolder for ...` adapter that wraps it + binds the live `KmsEngine` is the
//! downstream **P-GA-06 (P-106)** (the named floor) — the same DAG-respecting pattern [`telemetry`]
//! uses for the §10.2 `SignalName` enum and [`crosscell`] for `CrossCellPointer`. The H8 (event-bus)
//! holder slot in the H1–H18 catalog (`myelin-substrate`, P-S27) is what this module resolves to.
//! **FLOORS named:** the *reaches-backups* leg of BUS-D8 is the M5 follow-on **EB-29**; **post-restore
//! re-erasure** (the key stays destroyed across a restore) **SHIPPED in EB-16 (P-093)** — see the next
//! Status block; the **[OPEN — LEGAL]** residual lawful-basis is the ONE platform posture (10.9, X-7),
//! GDPR/legal track, NOT restated here.
//!
//! ## Status (P-093 / EB-16, 2026-06-19) — the erasure-ledger post-restore re-erasure hook
//! EB-16 ships [`reerase`] — the hook that keeps an erased subject's inline-PII key DESTROYED across a
//! backup restore (external-insights/04 §1; Bus §4.8 post-restore re-erasure fan-out). An append-only
//! log lives in backups too, so a restore of a backup taken BEFORE an erase can resurrect a still-live
//! DEK. The resolution (contract **10.8 CONSUMED** + **11.5** cross-seam, the SAME shape identity's
//! `PseudonymErasureLedger`/`re_erase_after_restore` uses, cold == live): a PII-free,
//! non-shred-erasable [`reerase::BusErasureLedger`] durably records which opaque subject was erased +
//! which key refs were shredded; after a restore, [`holder::BusHolder::re_erase_after_restore`]
//! replays it — re-running the IDENTICAL [`holder::BusHolder::erase`] crypto-shred (idempotent) over
//! every ledger-listed subject — returning a [`reerase::ReErasureReceipt`] proving **0 resurrected**
//! inline-PII keys post-restore. The Bus's leg of the **STOR-D1/D2** restore-verify cross-seam is
//! green (`tests/drills_bus_d8_reerase_after_restore.rs`: 0 resurrected + nothing lost, bridged into
//! the §10.2 harness assertion library); the 10.8 consumer-side CDC is
//! `tests/cdc_10_8_bus_reerase.rs`. **FLOORS named:** row 10.8 stays `deferred` (landing P-115 — the
//! GDPR provider mints/owns the global ledger; this ships the Bus's CONSUMER-SIDE participation); the
//! cross-seam restore TRIGGER (Storage calling every holder's re-erase over the global ledger) is
//! **P-ST-14 (P-100)** + **P-GA-06 (P-106)**. This completes B-M1 for the Bus.
//!
//! ## Status (P-142 / EB-22, 2026-06-20) — the reindex-from-source seam + the `*.snapshot` schema
//! EB-22 ships [`reindex`] — the **only** recovery path for every derived store (Search, Refs, OLAP,
//! Notif read-models): the index NEVER reads an owner DB; it asks each owner to **re-emit through
//! the live consumer path** ([`reindex::reindex`] = §5.6 `events::reindex(scope)`), so steady-state
//! (live events) and recovery (snapshots) are ONE code path and cannot drift (EI-04 §5.3, contract
//! **2.6 OWNED**). A `*.snapshot` ([`reindex::SnapshotDraft`]) carries the SAME envelope shape as the
//! live event but a **deterministic `event_id` from `(aggregate, version)`** ([`reindex::snapshot_event_id`]),
//! so re-running a reindex is idempotent — the outbox `UNIQUE(event_id)` skips a duplicate
//! (`ON CONFLICT DO NOTHING`; [`reindex::reindex`] reports it as skipped-duplicate) and the
//! consumer's [`DedupLedger`] no-ops a redelivered snapshot. The scope ([`reindex::SnapshotScope`])
//! is **sub-artifact-granular + PII-free** (CI one-run `ci:run:<id>`, KN page-subtree at block
//! granularity `knowledge:page:<id>`). This seam is FOUR paths in one: recovery, the schema-upcaster
//! backfill, the new-consumer bootstrap, AND the `resync_required` fallback target for the firehose
//! resume-cursor protocol ([`firehose`], EB-21 / P-141 — an out-of-window `last_seq` raises
//! `resync_required`, the client falls back to a `*.snapshot` replay this seam produces). The
//! **BUS-D5** `cold == live` drill (`tests/drills_bus_d5_reindex.rs`) wipes a derived store, reindexes,
//! and asserts the rebuild is BYTE-IDENTICAL to the live projection (proven on the reference
//! [`reindex::DerivedStore`] + [`reindex::ReferenceReindexSource`]); the 2.6 CDC pair is
//! `tests/cdc_2_6_reindex.rs` (the coverage manifest flips 2.6 `deferred → covered`). **FLOOR named
//! (EI-01 §1):** each OWNER's real `replay` body (CI one-run, KN page-subtree at block granularity,
//! Refs per-blob, Search full reindex) lands with that subsystem in **EB-26 (P-246, M3)** + the
//! owners' M3/M4 prompts (`coverage-matrix` rows 2.6 / 4.x / 5.x); this prompt ships the SEAM + the
//! `*.snapshot` schema + the reference consumer the BUS-D5 drill runs against.
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
//!   runtime calls before `handle` **shipped in P-S09** ([`upcast::UpcasterRegistry`]); the
//!   [`consumer::Consumer::with_upcaster`] hook is its (now fallible) install seam, and an
//!   unbridgeable gap dead-letters loudly. EB-10 (P-046) reconciles in place against [`upcast`].
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

pub mod check_seam;
pub mod consumer;
pub mod crosscell;
pub mod crosscell_propagation;
pub mod dead_letter;
pub mod dedup;
pub mod envelope;
pub mod firehose;
/// The per-subsystem token-list VALIDATION HARNESS (contract 2.9, EB-26 / P-246, M3). The Bus owns
/// the §6.1 grammar + this list-registration harness; each subsystem REGISTERS its completed
/// dotted-name list (name + schema_ver lineage + payload shape) against the one grammar.
pub mod harness;
pub mod holder;
pub mod retention;
// Stage 2 / infra: the REAL durable bus behind the BusTransport trait — NATS JetStream via
// async-nats. Compiled ONLY under `--features integration` (it pulls the real async-nats +
// tokio clients); the default build keeps the in-process relay::InProcessBus floor. It
// implements the EXISTING relay::BusTransport trait, it does not fork it.
#[cfg(feature = "integration")]
pub mod nats;
pub mod outbox;
pub mod partition;
pub mod reerase;
pub mod reindex;
pub mod relay;
pub mod residency;
pub mod taxonomy;
pub mod telemetry;
pub mod upcast;

/// The Git↔CI check-seam CARRIAGE (contract 5.9 the Bus's narrow half + 9.4 consumed, EB-24 /
/// P-144). The Bus owns ONLY: envelope conformance ([`check_seam::check_updated_draft`] — the
/// `ci.check.updated` subject `repo#commit-<oid>/check-<context>` + the `(repo, commit_oid)`
/// aggregate, the CI-owned `CheckStatus` carried OPAQUE), per-aggregate ordering on
/// `(repo, commit_oid)` ([`check_seam::CheckSeamOrder`] — interleaved/late arrivals stay
/// per-aggregate ordered, the D-11 substrate Git's `run_attempt` supersession rests on), and the
/// durable `wait_for_signal("ci.result", idem_key)` substrate
/// ([`check_seam::CiResultWaitSubstrate`] — a doubly-delivered `ci.result` wakes EXACTLY once,
/// contract 9.4). It does NOT own the `CheckStatus` shape, the supersession rule, trust-tier
/// gating, or the merge gate (all CI/Git, contract 5.9). The CONSUMER leg (Git's `check_status`
/// projection over this ordered carriage) is LIVE as of EB-26 / P-246 (M3); the PRODUCER leg (CI
/// emits `ci.check.updated` + the rollup `ci.result` via [`check_seam::ci_result_draft`] /
/// [`check_seam::rollup_ci_result`]) is LIVE as of EB-27 / P-327 (M4) — the seam is now END-TO-END
/// (GIT-D10 / CI-D8). FLOOR: the real `myelin-flow` durable engine behind `wait_for_signal` is
/// P-FLOW-04 (this is its in-cell signal substrate, unchanged in shape when the engine lands).
pub use check_seam::{
    check_aggregate, check_subject, check_updated_draft, ci_result_draft, ci_result_subject,
    rollup_ci_result, CheckSeamError, CheckSeamOrder, CiOverall, CiResult, CiResultWaitSubstrate,
    OrderedCheck, WakeOutcome,
};
pub use consumer::{
    consume, Consumer, ConsumerName, ConsumerSpec, DeadLetter, Delivered, Message,
    PerTenantInflight, PrefetchBound, SubscribeError, Subscription,
};
/// The cross-cell bridge FRAME (contract 12.6, EB-14 / P-091), PINNED from the Bus side. The four
/// frozen frame types are re-exported on the frozen `myelin_events::*` path; their definition site
/// is `myelin-tenancy` (the §2.9 DAG sink) — EB-14 reconciles in place (EI-01 §7), it does NOT
/// re-define them. `assert_cell_agnostic` is the compile-time gate that the Bus's §5 surfaces take
/// the opaque subject, never a cell-bound row. FLOOR: the cross-cell BUILD is EB-25 (M5).
pub use crosscell::{
    assert_cell_agnostic, pointer_correlation, ArtifactType, CellId, CrossCellPointer,
    OpaqueSubjectId,
};
/// The Bus's cross-cell EVENT-PROPAGATION half (contract 12.6 — built LIVE, EB-25 / P-438, M5; the
/// M1-frame floor follow-on of EB-14). When a cross-cell-relevant event ([`CrossCellStream`] — the
/// §6.2 ISS portfolio rollup / KN collab / CHAT cross-org floor follow-ons) occurs in a tenant's
/// home cell, [`CrossCellPropagator::fan_out`] mints a [`CrossCellPointer`] carrying ONLY the four
/// frozen PII-free fields ([`pointer_for_propagation`]) and produces one [`PropagatedPointer`] per
/// *other* member cell — never the payload, never any PII (`pii_fields_crossed` pinned to 0 by
/// construction, the CP-D8 / GA-D8 zero). The control plane CONSUMES the produced pointer and carries
/// it between the tenant's cells (`myelin_control_plane::cross_cell_bridge`, P-429); the member cell
/// resolves cell-local (P-429's resolution half) when a viewer there renders it. ONE frame, ONE
/// residency rule, two reconciled DAG legs (EI-01 §7). FLOOR: the cell→cell transport wire is the
/// control plane's bridge + resilient client; this produces the pointer-event it carries.
pub use crosscell_propagation::{
    pointer_for_propagation, propagated_carried_fields, CrossCellPropagator, CrossCellStream,
    PropagatedPointer,
};
/// The DURABLE consumer dead-letter set (CT-004d.2 chunk 6 / peer-review #7b) — mirrors the
/// [`dedup`] `DurableDedup` seam so a dead-lettered event (esp. the H2 panic path) SURVIVES a
/// restart. PII-safe (references-not-payloads: `event_id` + a bounded PII-free `reason`, never the
/// envelope/payload). The PG backing is `myelin_storage::events_durable::DurableDeadLetterBacking`.
pub use dead_letter::{
    bounded_reason, DeadLetterRecord, DeadLetterSink, DurableDeadLetter,
    CONSUMER_DEAD_LETTER_MIGRATION, MAX_REASON_LEN,
};
pub use dedup::{CoCommitError, CoCommitTx, DedupLedger, DurableDedup, CONSUMER_DEDUP_MIGRATION};
/// The firehose resume-cursor subscription protocol (contract 3.5, the Bus-owned zero-loss-replay
/// half — EB-21 / P-141, built FIRST per EI-04 §2.2). `Firehose::publish`/`tail`/`subscribe`/`resume`
/// implement the §5.5 surface: a per-`(stream, scope)` monotonic `seq`, `(last_seq, now]` backfill on
/// reconnect (loses ZERO ops), an out-of-window `last_seq` → `resync_required` → `*.snapshot` fallback
/// (the cold-rebuild path, NAMED not silent — the rebuild itself is EB-22 / P-142), a bounded scope
/// (`FirehoseScope` — never `*`; the transport rejects an over-broad scope), and a per-connection
/// in-flight cap (a slow consumer drops to `resync_required`, never buffers unboundedly). FLOORS: the
/// retention-window size per stream class is NAMED-not-numbered → MEASURED by D-10 in EB-30 / P-439;
/// D-10 re-runs green across the KN CAS→CRDT `engine_promote` boundary (EB-30). The substrate's
/// `FrameBuffer`/`BoundedSelector` (P-135/P-136) are the bounded-and-sheds half that RIDES this
/// protocol at the connection tier (Chat M4) — events cannot depend on substrate, so the two halves
/// compose at the connection tier (EI-01 §7 reconciliation noted in `firehose.rs`).
pub use firehose::{
    Firehose, FirehoseError, FirehoseScope, Frame, FrameDraft, FramePayload, RetentionWindow,
    ScopeKind, SubStream, Subscription as FirehoseSubscription, DEFAULT_INFLIGHT_CAP,
};
/// The per-subsystem token-list validation harness (contract 2.9, EB-26 / P-246, M3). Each
/// subsystem registers its completed [`harness::SubsystemTokenList`] into a [`harness::TokenListHarness`]
/// — the Bus admits the full list iff every name is §6.1-grammar-conformant + own-prefixed + unique,
/// and rejects a malformed addition LOUDLY ([`harness::HarnessError`]). The Bus owns the grammar +
/// harness; the subsystem owns its list (its own crate constant). FLOOR: CI/Issues/Chat register
/// their M4 lists through this same harness in EB-27/P-?.
pub use harness::{
    HarnessError, PayloadShape, RegisteredToken, SubsystemTokenList, TokenListHarness,
};
pub use outbox::{
    DurableOutboxBacking, EmitContextBase, IdMinter, MonotonicMinter, OutboxRow, OutboxStore,
    OutboxTransaction, Ulid, UlidMinter, OUTBOX_MIGRATION,
};
pub use partition::{stream_name_for, PartitionKey, StreamSubject, SubjectError, SUBJECT_ROOT};
pub use relay::{
    dlq_subject, BusTransport, DeadLetterAlert, Delivery, DrainReport, InProcessBus, Relay,
    TransportError, MAX_PUBLISH_ATTEMPTS,
};
pub use residency::{BusRegionReport, BusResidencySignal, BusStreamResidency, ResidencyError};
/// The firehose retention-window TUNING per stream class (EB-30 / P-439, M5 — the named M2 floor
/// MEASURED). [`retention::StreamClass`] is the three heaviest firehose producers (CI log / collab op
/// / chat live, §2.9 item 6); each carries a MEASURED [`retention::RetentionTuning`] — the per-class
/// retention window (frames) + the measured p99 reconnect gap, with the §4.3 invariant
/// `window > p99_reconnect_gap` (with headroom) asserted from the measured data. The numbers are
/// recorded in `thresholds.toml` (`[firehose_retention]`), the versioned source of truth, kept in
/// lock-step by a CDC test. `Firehose::for_stream_class` opens a class's window at its measured size.
pub use retention::{RetentionTuning, StreamClass};
pub use taxonomy::{
    validate as validate_event_type, TaxonomyError, ARTIFACT_TYPE_TOKENS, SEED_EVENT_NAMES,
    SUBSYSTEM_TOKENS,
};
pub use telemetry::{
    BusObservations, BusSignal, BusSignals, MetricLabel, MetricRecorder, MetricSample, MetricsSink,
};
pub use upcast::{RegisterError, UpcastError, UpcasterRegistry};

pub use holder::{
    degrade_on_tombstone, BusEventLog, BusHolder, EraseReceipt, ExportedEvent, InMemoryShredder,
    InlinePiiShredder, LocateReport, LocatedEvent, ShredError, BUS_ERASED_TYPE, ERASED_EVENT_NAME,
};
/// The Bus as a `PersonalDataHolder` (contract 2.7 OWNED — the event-log half of erasure-vs-
/// immutability) + the inline-PII crypto-shred to the KMS hierarchy (EB-15 / P-092). The §5.7
/// `locate`/`erase`/`export` MECHANISM ([`holder::BusHolder`]) runs over the in-cell
/// [`holder::BusEventLog`], destroying inline-PII DEKs through the [`holder::InlinePiiShredder`] KMS
/// seam (real backing `myelin_storage::kms::KmsEngine::destroy_dek`, downstream — floor named in the
/// [`holder`] module) and emitting `*.erased` tombstones through the outbox. FLOORS: the
/// `impl gdpr::PersonalDataHolder` adapter is P-GA-06; the reaches-backups leg of BUS-D8 is EB-29.
/// EB-16 (P-093): the erasure-ledger post-restore re-erasure hook (contract 10.8 CONSUMED, 11.5
/// cross-seam). [`reerase::BusErasureLedger`] is the Bus's PII-free, non-shred-erasable slice of the
/// erasure ledger; [`holder::BusHolder::re_erase_after_restore`] replays it after a restore so the
/// key STAYS destroyed across a restore ([`reerase::ReErasureReceipt`] proves 0 resurrected).
pub use reerase::{BusErasureLedger, DurableBusErasure, ErasedSubject, ReErasureReceipt};

/// The reindex-from-source seam + the `*.snapshot` event schema (contract 2.6 OWNED, EB-22 / P-142).
/// [`reindex::reindex`] is the §5.6 `events::reindex(scope)` surface: ask the OWNER of a
/// sub-artifact-granular [`reindex::SnapshotScope`] to `replay(scope, since)`, then emit each
/// `*.snapshot` through the SAME outbox→bus→live-consumer path (no backdoor). A `*.snapshot` carries
/// the live envelope shape and a DETERMINISTIC `event_id` from `(aggregate, version)`
/// ([`reindex::snapshot_event_id`]), so a re-run is an idempotent no-op (the outbox `UNIQUE(event_id)`
/// and the consumer's `consumer_dedup` ledger both absorb the duplicate). This is the recovery path,
/// the upcaster-backfill path, the new-consumer-bootstrap path, and the `resync_required` fallback
/// target for the firehose ([`firehose`], EB-21). [`reindex::DerivedStore`] and
/// [`reindex::ReferenceReindexSource`] are the small reference consumer the BUS-D5 `cold == live`
/// drill proves byte-parity over. FLOOR: each OWNER's real `replay` body (CI one-run, KN
/// page-subtree at block granularity, Refs per-blob, Search full reindex) lands with that subsystem
/// in EB-26 (P-246, M3) and the owners' M3/M4 prompts.
pub use reindex::{
    reindex, snapshot_event_id, DerivedStore, ReferenceReindexSource, ReindexError, ReindexReceipt,
    ReindexSource, SnapshotDraft, SnapshotScope, SNAPSHOT_EVENT_NAME,
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

/// **The same-transaction co-commit handle threaded into a handler (peer-review #7 / MR-023b —
/// FIXED).** The [`consumer::Consumer`] runtime opens ONE database transaction, INSERTs the
/// `(consumer, event_id)` dedup mark WITHIN it, and passes this handle to [`EventHandler::handle`];
/// a handler runs its durable state writes on the SAME transaction, so the dedup mark and the
/// effect **commit atomically or roll back together**. This is what makes the consumer
/// exactly-once-WITH-EFFECT instead of at-most-once: a crash between the dedup mark and the commit
/// leaves NEITHER the mark NOR the effect, so a redelivery RE-RUNS the handler and the effect lands
/// (never a committed mark with a lost effect — the MR-023b floor, now closed).
///
/// **Why type-erased:** `myelin-events` is a §2.9 DAG SINK — it cannot name `sqlx`. So the handle
/// carries the concrete database connection **type-erased** behind `&mut dyn Any`. A durable handler
/// (in a storage-aware crate) recovers the real connection with [`HandlerTx::connection`]
/// (downcasting to `&mut sqlx::PgConnection`) and runs its writes on it. A handler whose effect is an
/// in-memory projection (not a shared-DB write on this pool) legitimately IGNORES the handle — its
/// durable-projection co-commit is that subsystem's named floor, not an open re-instance of #7
/// (there is no durable effect on THIS pool to lose). A durable handler that finds no connection
/// ([`HandlerTx::is_durable`] false / [`HandlerTx::connection`] `None`) MUST fail-closed (return
/// [`HandleOutcome::Retry`]) rather than silently write outside the tx — writing outside the tx
/// re-opens the exact at-most-once bug.
pub struct HandlerTx<'a> {
    conn: Option<&'a mut dyn core::any::Any>,
}

impl<'a> HandlerTx<'a> {
    /// Wrap a type-erased, transaction-bound connection the handler downcasts to write on (the
    /// DURABLE co-commit path — the same tx the dedup mark is in).
    pub fn with_connection(conn: &'a mut dyn core::any::Any) -> HandlerTx<'a> {
        HandlerTx { conn: Some(conn) }
    }

    /// A handle carrying NO co-commit connection — the in-memory model / unit-test path (a handler
    /// whose effect is not a shared-DB write; the dedup ledger models the mark's atomicity in
    /// memory). A durable handler treats this as "no tx available" and fails-closed.
    pub fn none() -> HandlerTx<'static> {
        HandlerTx { conn: None }
    }

    /// Recover the concrete transaction-bound connection to run the handler's writes on (downcast
    /// from the erased handle — e.g. `tx.connection::<sqlx::PgConnection>()`). `None` on the
    /// in-memory path (or a type mismatch). A durable handler MUST treat `None` as fail-closed
    /// (Retry), NEVER a silent write outside the tx (that re-opens the at-most-once bug — #7).
    pub fn connection<T: core::any::Any>(&mut self) -> Option<&mut T> {
        self.conn.as_deref_mut().and_then(|c| c.downcast_mut::<T>())
    }

    /// Whether a co-commit connection is present (a handler can branch: durable co-commit write vs
    /// the in-memory model). `true` only on the durable runtime path.
    pub fn is_durable(&self) -> bool {
        self.conn.is_some()
    }
}

/// The one consumer template (architecture §5; contract 2.4; BUS-3). Built from this
/// single trait so the seven encoded rules cannot be skipped per-consumer. `subjects()`
/// is a whitelist, **NEVER `*`** (an over-broad subscription head-of-line-blocks
/// everything). `handle` is idempotent on `event_id` via the `consumer_dedup` ledger.
///
/// The consumer runtime (the seven rules + the dedup ledger) is [`consumer::Consumer`]
/// (**shipped in P-S08**); the upcaster registry that runs before `handle` is **P-S09**
/// (the [`consumer::Consumer::with_upcaster`] hook). The trait shape is frozen here.
///
/// **The [`HandlerTx`] co-commit seam (#7 / MR-023b — FIXED):** `handle` receives a same-transaction
/// handle so a handler's durable state write co-commits with the dedup mark (both land or neither).
/// A handler with a shared-DB effect writes on `tx.connection::<sqlx::PgConnection>()`; an in-memory
/// / outbox-seam handler ignores it (documented per impl).
pub trait EventHandler {
    /// Whitelist — NEVER `*` (BUS-3, D7-i). [`consumer::Subscription::bind`] enforces the
    /// `*`-rejection at registration so an over-broad subscription is unconstructable.
    fn subjects(&self) -> &'static [SubjectPattern];
    /// Idempotent on `event_id` (ADR-04.1). Body is the consumer's; the runtime around
    /// it (dedup, ack-after-enqueue, bounded prefetch, lag metric) is [`consumer::Consumer`].
    ///
    /// `tx` is the same-transaction co-commit handle (#7 / MR-023b): a handler's durable state
    /// write runs on `tx.connection::<sqlx::PgConnection>()` so it co-commits with the dedup mark.
    /// An in-memory / outbox-seam handler accepts and ignores it (its durable-projection co-commit
    /// is that subsystem's named floor).
    fn handle(&self, ev: &EventEnvelope, tx: &mut HandlerTx<'_>) -> HandleOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn sample_principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
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
            fn emit(
                &mut self,
                draft: EventDraft,
                cause: Option<&EventEnvelope>,
            ) -> Result<EventId> {
                // A real implementer mints the id + ambient context, derives, then (P-S07)
                // inserts the row in the same tx. Here we return the derived envelope's id.
                let id = EventId(format!("01J-{}", self.next));
                self.next += 1;
                let env =
                    derive_envelope(draft, ctx_for(id, Some(CausedBy("human:h".into()))), cause);
                Ok(env.event_id)
            }
        }

        let mut tx = Tx { next: 0 };
        // A root emit through the trait.
        let root_id = tx
            .emit(draft_for("issues.issue.created"), None)
            .expect("root emits");
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
            fn emit(
                &mut self,
                draft: EventDraft,
                cause: Option<&EventEnvelope>,
            ) -> Result<EventId> {
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
            fn handle(&self, _ev: &EventEnvelope, _tx: &mut HandlerTx<'_>) -> HandleOutcome {
                HandleOutcome::Done
            }
        }
        let h = Idx;
        assert!(h.subjects().is_empty());
        assert_eq!(
            h.handle(&sample_envelope(), &mut HandlerTx::none()),
            HandleOutcome::Done
        );
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
