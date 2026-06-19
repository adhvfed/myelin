# Phase 7 — Prompt Ledger: Event Bus + Trigger/Automation Engine (myelin-events)

> Phase-7-A finer-granularity pass. Prompt count: BEFORE 14 (EB-01..EB-14) → AFTER 31 (EB-01..EB-31). Every
> bundled multi-deliverable prompt from the first pass is split into single-deliverable, clean-context,
> independently-committable units; every milestone / contract / drill / floor the first pass covered is
> preserved (the union of the finer prompts covers the old set exactly, plus exposes each bundled
> sub-deliverable as its own prompt). DEPENDS-ON edges are re-threaded across the new finer ids.
>
> Phase: 07-prompts/by-system. The COMPLETE ordered set of implementation prompts that operationalize the
> event-bus shared system's entire Phase-6 roadmap (06-roadmaps/shared/event-bus.md), one clean-context,
> independently-committable prompt per unit of work, each tagged with its master-sequencing band (M0..M6).
> Authored to the template in 00-ledger-overview.md (followed EXACTLY). Frozen architecture (these prompts
> OPERATIONALIZE, they do not redesign): planning/05-refined-shared-systems-architecture/event-bus.md +
> contract-index.md §2/§3/§5.9 + 00-reconciliation-decisions.md. Build order frozen by
> 06-roadmaps/00-master-sequencing.md (M0..M6 bands + the gate invariant). Doctrine carried:
> external-insights/01-process-and-quality-doctrine.md (§1 code-wins-over-docs + name-your-floors; §2
> order-by-non-negotiability + the gate invariant; §3 prove-it-or-it-isn't-real with a quantified drill +
> observability-is-part-of-the-pass; §5 the committed ratchet) and external-insights/04-hard-problems.md
> (§1 erasure-vs-immutability, §2.2 build the resume-cursor transport FIRST, §5.2 column-store seam, §5.3
> reindex-from-source). Identifiers are plain text (no backticks-as-emphasis). Markdown only; no git commits
> by this document. Date: 2026-06-19.
>
> **Stable ids in this file are local (EB-01..EB-31).** The consolidated index (Phase 7-B,
> 01-ledger-index.md) assigns each its global P-NNN ordinal by interleaving all systems in band order; until
> then the EB-NN id is the stable handle DEPENDS-ON edges reference within this file. Each prompt's BAND +
> DEPENDS-ON are the authoritative ordering constraints.

---

## Coverage map (every Bus roadmap milestone → its prompt(s))

The binding guarantee (00-ledger-overview §5): every milestone in 06-roadmaps/shared/event-bus.md §2 maps to
at least one prompt; every floor maps to a floor prompt AND a follow-on prompt.

| Roadmap milestone | Prompts | Notes |
|---|---|---|
| B-M0 (the emit+consume substrate, the silent-data-loss floor) | EB-01, EB-02, EB-03, EB-04, EB-05, EB-06, EB-07, EB-08, EB-09, EB-10, EB-11 | envelope freeze / taxonomy grammar+tokens / outbox+emit / relay+transport / consumer template / dedup ledger / no-raw-publish lint / no-cross-sync-cycle lint / tenant-predicate lint / upcaster seam / survival-signal telemetry+harness self-test |
| B-M1 (tenancy partition + crypto-shred holder seam) | EB-12, EB-13, EB-14, EB-15, EB-16 | partition under (tenant,region) / residency-pin / cross-cell bridge FRAME pinned (built EB-25) / PersonalDataHolder impl + crypto-shred to KMS / erasure-ledger post-restore re-erasure hook |
| B-M2 (reactive layer) | EB-17, EB-18, EB-19, EB-20, EB-21, EB-22, EB-23 | EventMatcher=QueryAst / Signal curation / Automations / Triggers / firehose resume-cursor (built FIRST) / reindex-from-source seam / dispatch tier + check-seam carriage |
| B-M3/B-M4 (per-subsystem tokens + check seam live) | EB-24, EB-26, EB-27, EB-28 | per-subsystem token-list validation harness (M3) / check-seam consumer leg + per-owner replay carriage (M3) / check-seam producer leg end-to-end (M4) |
| B-M5 (world-scale hardening + floor follow-ons) | EB-25, EB-29, EB-30, EB-31 | cross-cell bridge BUILD (follow-on of EB-14 frame) / 30× surge + QPS-order + backups-erasure / retention-window tuning + CRDT-boundary D-10 re-run / E2E spine |
| B-M6 (dogfooding) | covered by EB-31 DoD note + the master M6 dogfood prompts | the Bus adds no new engine in M6; its M6 gate is the self-hosting CI graph green + the truth-up pass — exercised by the dogfood prompts, no Bus-specific build |

Floors (00-master-sequencing §5 / roadmap §3) and their follow-on pairs:
- Per-aggregate ordering CORRECTNESS (EB-03, M0) → ordering AT PRODUCTION QPS / BUS-D9 (EB-29, M5).
- Single-region JetStream log (EB-04, M0) → column-store/time-series seam, measured-not-predicted (EB-31 names it; promotion is post-M5 and only on measured volume — no dated follow-on prompt is owed until volume is measured).
- Crypto-shred in live stores (EB-15, M1) → reaches backups / BUS-D8 (EB-29, M5).
- Resume-cursor transport (EB-21, M2) → the CRDT slots into it (a Knowledge deliverable, M5; EB-21's D-10 is written to re-run green across the engine_promote boundary — re-confirmed in EB-30).
- Firehose retention window, named-not-numbered (EB-21, M2) → measured+tuned by D-10 (EB-30, M5).
- Single-home-cell propagation + pinned bridge FRAME (EB-14, M1) → cross-cell PII-free bridge LIVE (EB-25, M5).
- Erasure-vs-immutability structural floor (EB-15, M1) → the [OPEN — LEGAL] residual lawful-basis (parallel/legal; not an engineering prompt — the structural floor ships in EB-15, the residual is one ratified statement owned by the GDPR/legal track by reference, X-7).

---

## EB-01 — Freeze the EventEnvelope struct (the names/units anchor)

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 (the emit + consume substrate) — the EventEnvelope freeze (the names/units
  anchor). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contract 2.1.
- **DEPENDS-ON.** none (this is a root M0 prompt — the envelope is the names/units anchor every later contract
  compiles against; it requires only the workspace + glue-crate skeleton the substrate M0 prompts lay down,
  and may run as soon as the myelin-events crate skeleton exists).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../../VISION.md (always) + external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors,
    code-wins-over-docs — the envelope is the names/units anchor, X-5; getting it wrong calcifies every
    downstream contract) + §7 (reconcile cross-component contracts at the plan layer — field names AND units).
  - ../../05-refined-shared-systems-architecture/event-bus.md §3.1 (the envelope field list + units — the
    AUTHORITY).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 2.1 (EventEnvelope — the names/units
    anchor) + §0 frozen-units block (timestamps RFC-3339 UTC; costs integer minor-units; pii_key_ref =
    kms://<tenant>/<dek-epoch>/<class>, class in {tenant, subject:<id>, blob}).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-5 (the two names/units
    anchors are unchanged).
- **DELIVERABLE (what to build + exactly where in the repo).** In the glue crate myelin-events:
  - envelope.rs: the EventEnvelope struct exactly as Bus §3.1 lists — event_id (ULID, the idempotency anchor),
    type (canonical dotted name), schema_ver, occurred_at/recorded_at (RFC-3339 UTC, two clocks),
    tenant/region, actor{principal, kind in {human|agent|service}, on_behalf_of, session, run}, subject
    (ArtifactRef) + aggregate (ordering key), the NESTED causality triad correlation_id/causation_id/depth
    (root depth=0, child = parent+1), contains_personal_data/data_role/visibility (a hint, NEVER an authz
    decision) / pii_key_ref (kms://<tenant>/<dek-epoch>/<class>), and the producer-owned versioned payload.
    Serde with a stable JSON encoding; references-not-payloads (payload is small).
  - This FREEZES THE ANCHOR; getting it byte-identical is the whole job. The taxonomy GRAMMAR that validates
    the type field is EB-02 (depends on this struct existing); the per-subsystem token lists are EB-24 (M3/M4).
- **CONTRACTS TO IMPLEMENT.** 2.1 EventEnvelope (owned — the primary contract, the names/units anchor).
  Implement to the frozen shape in Bus §3.1; a needed shape change is a whole-workspace contract PR, escalated
  and written down, never a local divergence (EI-01 §1).
- **GATE / DRILLS (quantified; must be green to call this done).** No catalogue drill greens here (it ships a
  data shape, not a running path). The gate is structural: the envelope serialises/deserialises round-trip
  lossless for EVERY field including the nested causality triad and a populated pii_key_ref; depth-derivation
  (child = parent+1) computed from a cause is correct. This round-trip IS the proof the anchor is well-defined.
- **TESTS (required).** Unit: round-trip serde for the full envelope incl. the nested causality triad and a
  populated pii_key_ref; depth-derivation (child = parent+1) computed from a cause. CDC: this is a
  glue-contract carrier; the provider+consumer CDC pair for row 2.1 is the envelope-shape conformance test (a
  consumer that deserialises an envelope emitted by the canonical encoder, and a producer that emits to the
  canonical shape) — the contract-coverage scanner fails the build if 2.1 lacks both. Mutation: envelope.rs is
  mandatory-core; state the mutation-score floor for envelope.rs and meet it.
- **DEFINITION OF DONE.** envelope.rs compiles in the workspace; 2.1 implemented to the frozen shape; the
  round-trip + depth-derivation unit tests pass; the CDC pair for 2.1 exists; the contract-coverage scanner
  passes for 2.1; all committed lints green; work committed.
- **COMMIT.** Header "P-<NNN> M0: Freeze EventEnvelope (names/units anchor)". Body: contract 2.1 implemented
  to the frozen shape; round-trip serde + depth-derivation green. Branch first if on default; Co-Authored-By
  trailer.

---

## EB-02 — The taxonomy grammar validator + the seed token table + the three new check-seam/initiative tokens

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 — the taxonomy grammar + seed token table (the dotted-name grammar slice).
  Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contract 2.9.
- **DEPENDS-ON.** EB-01 (the EventEnvelope whose type field the grammar validates).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §1 (code-wins-over-docs — the
    token table is the names anchor, X-5) + §7 (one grammar, no per-subsystem drift).
  - ../../05-refined-shared-systems-architecture/event-bus.md §6.1 (the dotted-name grammar), §6.2 (the
    ArtifactRef subsystem/type token table + the new initiative type token), §6.3 (the new ci.check.updated /
    ci.result tokens), §6.4 (the seed event names).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 2.9 (taxonomy + token table; new tokens
    ci.check.updated / ci.result / initiative) + §14 (the ArtifactRef token table = the names anchor; Refs is
    the validator, not a second authority).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §2 (the new tokens registered).
- **DELIVERABLE.** In myelin-events:
  - taxonomy.rs: the grammar validator for type = <subsystem>.<artifact_type>.<event_name> (lowercase,
    singular, past-tense, tokens match [a-z][a-z0-9_]*, two segments min / three when an artifact type
    clarifies); the canonical ArtifactRef subsystem token set (git/ci/issue/knowledge/chat/identity/refs) +
    the type tokens incl. the new initiative; the seed event-name table (the §6.4 representative names) + the
    new tokens ci.check.updated and ci.result REGISTERED.
  - FLOOR named: per-subsystem dotted-name LIST completion is deferred to EB-24 (each subsystem owns its full
    list in M3/M4); EB-02 ships the grammar + seed + the new check-seam tokens only.
- **CONTRACTS TO IMPLEMENT.** 2.9 Event taxonomy + token table (owned — grammar + seed + the three new
  tokens). To the frozen shape in Bus §6.
- **GATE / DRILLS.** No catalogue drill. The gate is structural: the taxonomy validator REJECTS a malformed
  type name (uppercase / plural / present-tense / single-segment / unknown subsystem token) and ADMITS every
  seed name + the three new tokens — a red-fixture + green-fixture pair (the same ratchet shape the lints use).
  This pair IS the proof the grammar is well-defined.
- **TESTS.** Unit: the taxonomy validator's reject-fixture (4+ malformed names) and admit-fixture (every seed
  name + ci.check.updated + ci.result + initiative). CDC: provider+consumer pair for 2.9 (a validator that
  admits a canonical-encoder-emitted type and rejects a malformed one) — the contract-coverage scanner fails
  the build if 2.9 lacks both. Mutation: taxonomy.rs is mandatory-core; state and meet the floor.
- **DEFINITION OF DONE.** taxonomy.rs compiles; 2.9 implemented to the frozen shape; the reject/admit fixture
  pair is green-and-dated; the validator unit tests pass; the CDC pair for 2.9 exists; the contract-coverage
  scanner passes for 2.9; all committed lints green; the floor (per-subsystem token list deferred to EB-24) is
  named in writing; work committed.
- **COMMIT.** Header "P-<NNN> M0: Taxonomy grammar + seed token table + check-seam/initiative tokens". Body:
  contract 2.9 implemented; validator reject/admit fixture green (N malformed rejected, all seed + 3 new
  tokens admitted); floor named (per-subsystem token lists → EB-24). Co-Authored-By trailer.

---

## EB-03 — The transactional outbox table + the OutboxTx::emit same-tx API (per-aggregate ordering correctness)

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 — the outbox table + the same-tx emit API (the Tier-1 silent-data-loss emit
  floor). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contracts 2.2 + 2.3.
- **DEPENDS-ON.** EB-01 (the EventEnvelope the outbox row stores).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §2 (order-by-non-negotiability:
    silent data loss outranks every feature — this IS the floor) + §3 (prove-it-or-it-isn't-real).
  - external-insights/02-platform-substrate.md §4 (the transactional outbox — the only sanctioned emit path).
  - ../../05-refined-shared-systems-architecture/event-bus.md §2.2 (partition key = aggregate, per-aggregate
    ordering), §2.3 (per-aggregate ordering at production QPS — the two adversarial cases, the D-9 floor),
    §3.2 (the outbox table schema), §5.2 (the OutboxTx::emit surface — same tx; causality correct-by-
    construction; no publish_now).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 2.2 (OutboxTx::emit — the ONLY
    sanctioned emit path; same tx; causality correct-by-construction; no publish_now) and 2.3 (outbox table —
    (event_id UNIQUE, aggregate, seq, subject, envelope); UNIQUE(aggregate, seq)).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row SUB-D1 (the outbox no-ghost/no-loss drill this prompt's same-tx half makes provable; the relay leg is
    EB-04).
- **DELIVERABLE.** In myelin-events:
  - outbox.rs: the outbox table schema (event_id UNIQUE, aggregate, seq, subject, envelope, tenant, ...),
    UNIQUE(aggregate, seq) as the source-of-truth ordering invariant; seq allocated per-aggregate INSIDE the
    producing transaction so it reflects true commit order. The OutboxTx::emit(draft, cause) API — written in
    the SAME db transaction as the state change; causality auto-derived from cause (BUS-5). NO publish_now /
    fire-and-forget path exists on this surface.
  - FLOOR named: per-aggregate ordering CORRECTNESS is shipped here (the UNIQUE(aggregate, seq) construction +
    in-tx seq allocation); proving it holds AT PRODUCTION QPS under a hot-ref / hot-channel burst (BUS-D9) is
    the M5 follow-on EB-29.
- **CONTRACTS TO IMPLEMENT.** 2.2 OutboxTx::emit (owned); 2.3 outbox table (owned). To the frozen shape (Bus
  §3.2/§5.2).
- **GATE / DRILLS.** No standalone running-path drill greens on the table+API alone (the relay that delivers
  is EB-04). The gate is the emit-iff-committed PROPERTY at the source: a rolled-back transaction leaves zero
  outbox rows; a committed transaction leaves exactly one; per-aggregate seq is monotonic and gap-free under
  concurrent emitters to the same aggregate. SUB-D1 / BUS-D4 green fully once EB-04's relay drains the table —
  EB-03 ships the half that makes them possible.
- **TESTS.** Unit: emit-in-same-tx (a rolled-back transaction emits nothing; a committed one emits exactly one
  row); per-aggregate seq monotonicity under concurrent emitters to the same aggregate (no gaps, no dups). CDC:
  provider+consumer pair for 2.2/2.3. Mutation: outbox.rs is mandatory-core; state and meet the mutation-score
  floor.
- **DEFINITION OF DONE.** outbox.rs compiles; 2.2 + 2.3 to the frozen shape; the emit-iff-committed +
  per-aggregate-seq-monotonicity unit tests pass (dated); the CDC pair for 2.2/2.3 exists; coverage scanner +
  all lints green; the QPS-order floor (→ EB-29) named in writing; committed.
- **COMMIT.** Header "P-<NNN> M0: Transactional outbox table + OutboxTx::emit (same-tx emit, no publish_now)".
  Body: contracts 2.2, 2.3; emit-iff-committed at source proven (rolled-back → 0 rows, committed → 1 row);
  per-aggregate seq monotonic; floor named (BUS-D9 QPS-order → EB-29). Co-Authored-By trailer.

---

## EB-04 — The FOR UPDATE SKIP LOCKED relay + the BusTransport trait (no-ghost / no-loss delivery)

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 — the outbox relay + the BusTransport trait (the delivery half of the emit
  floor). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contracts 2.3 (relay) + the BusTransport
  seam.
- **DEPENDS-ON.** EB-03 (the outbox table + the OutboxTx::emit rows the relay drains).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §2 (silent data loss outranks
    every feature) + §3 (prove-it; observability is part of the pass).
  - external-insights/04-hard-problems.md §5.2 (the column-store seam — the BusTransport trait IS that seam,
    built now, promoted only on measured volume).
  - ../../05-refined-shared-systems-architecture/event-bus.md §2.1 (transport: JetStream-class default, escape
    hatch written), §4.1 (the relay: FOR UPDATE SKIP LOCKED, Nats-Msg-Id dedup, DLQ, GC), §7.5 (the
    column-store/time-series seam BUS-6, behind the trait).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 2.3 (relay FOR UPDATE SKIP LOCKED;
    Nats-Msg-Id = event_id broker-side dedup).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows SUB-D1, BUS-D4 (the no-ghost/no-loss drills this prompt greens).
- **DELIVERABLE.** In myelin-events:
  - relay.rs: the stateless, horizontally-replicable relay — claim a batch (FOR UPDATE SKIP LOCKED, ordered by
    aggregate, seq), per-aggregate in seq order transport.put(subject, envelope, dedup_id = event_id) with
    Nats-Msg-Id = event_id for broker-side dedup, mark published_at on ack, retry on failure, dead-letter to
    dlq.<tenant>.<subsystem> after N attempts with a Signal alert, GC published rows after 24h.
  - transport.rs: the BusTransport trait (put/consume/ack/purge) with a JetStream-class reference impl; the
    swap to Kafka/Redpanda is a relay-target change behind this trait, not a consumer rewrite. NO non-durable
    fire-and-forget path exists.
  - FLOOR named: single-region JetStream log; the column-store/time-series seam (BUS-6) is the post-M5,
    measured-not-predicted follow-on (the BusTransport trait IS the seam — built now, not promoted until volume
    is measured; named in EB-31).
- **CONTRACTS TO IMPLEMENT.** 2.3 (the relay half — owned); the BusTransport trait (owned seam). To the frozen
  shape (Bus §4.1/§2.1).
- **GATE / DRILLS.** SUB-D1 → kill the service between commit and publish; the outbox delivers every committed
  event exactly-once-in-effect: 0 ghost, 0 lost; reads outbox-depth drains + the dedup ledger (telemetry, CI).
  BUS-D4 → crash the producer between state-commit and relay-publish; the event is delivered iff the state
  change committed (outbox emit-iff-committed): 0 ghost, 0 lost; reads outbox depth+age (CI). Both thresholds
  are 0 — never weaken to pass; a red gate is a dated "claimed, not proven" scorecard row. (These drills assert
  against the EB-11 telemetry; if EB-11 has not yet landed the survival signals, the drill reads the relay's
  own depth/ack counters and EB-11 wires them into 1.8.)
- **TESTS.** Unit: the relay's SKIP LOCKED claim does not double-publish across two relay workers; a failed
  put retries then dead-letters after N attempts; published rows GC after 24h. Drill scenarios: SUB-D1 and
  BUS-D4 as failure-injection-harness scenarios (kill between commit and publish; assert against outbox-depth +
  dedup telemetry — a drill that survives but emits no signal has FAILED, EI-01 §3). CDC: the BusTransport
  put/consume conformance pair. Mutation: relay.rs is mandatory-core; state and meet the mutation-score floor.
- **DEFINITION OF DONE.** relay.rs/transport.rs compile; the relay + BusTransport trait to the frozen shape;
  SUB-D1 and BUS-D4 each emit a dated green artifact (0 ghost, 0 lost — PROVEN, not CLAIMED); unit + drill +
  CDC tests pass; coverage scanner + all lints green; the column-store-seam floor (→ measured post-M5, named in
  EB-31) named in writing; committed.
- **COMMIT.** Header "P-<NNN> M0: Outbox relay (FOR UPDATE SKIP LOCKED) + BusTransport trait (no-ghost/no-loss
  delivery)". Body: contract 2.3 relay + BusTransport seam; SUB-D1 green (0 ghost / 0 lost, measured); BUS-D4
  green (emit-iff-committed); floor named (column-store seam → measured post-M5). Co-Authored-By trailer.

---

## EB-05 — The idempotent-consumer template (the EventHandler the whole platform is built from)

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 — the idempotent-consumer template (the template EVERY consumer in the platform
  is built from). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contract 2.4.
- **DEPENDS-ON.** EB-01 (the envelope/event_id the handler dedups on), EB-04 (the transport/BusTransport trait
  the template binds to).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §3 (prove-it: the head-of-line
    stall is forced by a drill) + §7 (abstract-at-the-first-copy — there will be dozens of consumers; this
    template is the one primitive).
  - external-insights/03-agent-native-fabric.md §6.1 (the orchestrator/consumer head-of-line-blocking gotcha,
    the whitelist-not-* defence).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.2 (the shared consumer template — the seven
    EI-03 §6 gotchas: whitelist subjects never *, bind-by-name, idempotent on event_id, ack-after-enqueue,
    term non-retryable junk, bounded prefetch + per-tenant in-flight caps, lag is a survival signal), §5.3
    (the subscribe/consume surface).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 2.4 (EventHandler consumer template —
    subjects() whitelist never *, handle → {Done|NonRetryable|Retry}, durable-bind-by-name, ack-after-enqueue,
    dedup ledger, bounded prefetch, lag metric).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows SUB-D2, BUS-D2 (the drills this prompt greens).
- **DELIVERABLE.** In myelin-events:
  - consumer.rs: the EventHandler trait + the consume(ConsumerSpec{durable, subjects /* explicit whitelist,
    NEVER * */, max_ack_pending, per_tenant_inflight}, handler) template. It encodes: (1) whitelist subjects
    via subjects() — never *; (2) bind to a durable consumer by name, never re-assert start policy on
    reconnect; (3) idempotent on event_id (delegating to the EB-06 dedup ledger — see below); (4) ack only
    after the work is durably enqueued (at-least-once to the next stage); (5) term non-retryable junk (handle →
    NonRetryable) so it doesn't burn the redelivery budget; (6) bounded prefetch + bounded handler pool +
    per-tenant in-flight caps; (7) consumer lag (num_pending) exposed as a telemetry survival signal.
  - This is THE template — abstract-at-the-first-copy (EI-01 §7). Every later consumer (Signal engine, Git
    check_status projection, Search indexer, Notif router, the dispatch tier) is built from it, not hand-rolled.
  - NOTE on the seam to EB-06: the dedup LEDGER schema + presence semantics ship in EB-06; this prompt ships
    the TEMPLATE that calls it. EB-05 DEPENDS on EB-06 only for the ledger type — author the trait against the
    ledger interface and let EB-06 land the table; if the index orders EB-06 first, treat EB-06 as merged.
- **CONTRACTS TO IMPLEMENT.** 2.4 EventHandler consumer template (owned). To the frozen shape (Bus §4.2).
- **GATE / DRILLS.** SUB-D2 → drop the broker mid-stream; 0 lost across reconnect (bind-by-name + dedup); a
  slow subject does NOT block others; reads consumer-lag + no-HoL-stall (CI). BUS-D2 → flood a (wrongly)
  *-subscribed consumer with unhandled types; the whitelist-template consumer does NOT stall while the naive
  one does; the lag alarm fires; reads num_pending (CI). Thresholds: 0 lost, lag bounded + alarm fires.
- **TESTS.** Unit: the whitelist consumer ignores subjects it didn't whitelist (no HoL stall); per-tenant
  in-flight cap is honoured; ack-only-after-enqueue (a crash before enqueue redelivers); term routes
  non-retryable junk off the redelivery budget. Drill scenarios: SUB-D2 and BUS-D2 on the failure-injection
  harness, asserting against consumer-lag telemetry. CDC: provider+consumer pair for 2.4. Mutation: consumer.rs
  is mandatory-core; state and meet the floor.
- **DEFINITION OF DONE.** consumer.rs compiles; 2.4 to the frozen shape; SUB-D2 + BUS-D2 each emit a dated
  green artifact (0 lost across reconnect; whitelist consumer does not stall + lag alarm fires); unit + drill +
  CDC tests pass; coverage scanner + lints green; committed.
- **COMMIT.** Header "P-<NNN> M0: Idempotent-consumer template (EventHandler)". Body: contract 2.4; SUB-D2
  green (0 lost across reconnect); BUS-D2 green (whitelist consumer no-stall, lag alarm fires). Co-Authored-By
  trailer.

---

## EB-06 — The consumer_dedup ledger (the effectively-once anchor)

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 — the consumer_dedup ledger (the effectively-once anchor the template keys on).
  Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contract 2.5.
- **DEPENDS-ON.** EB-01 (the envelope/event_id the ledger keys on).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §3 (prove-it — idempotency is a
    forced property, the re-delivery must produce one effect).
  - ../../05-refined-shared-systems-architecture/event-bus.md §3.3 (the consumer_dedup ledger).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 2.5 (consumer_dedup ledger —
    (consumer, event_id) PK).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row SUB-D2 (the dedup half of the no-loss-across-reconnect property).
- **DELIVERABLE.** In myelin-events:
  - dedup.rs: the consumer_dedup ledger ((consumer, event_id) PK; presence == "already handled"); the
    INSERT … ON CONFLICT DO NOTHING primitive the EB-05 template's idempotency rule calls. This is the
    effectively-once anchor every consumer's at-least-once delivery resolves to exactly-once-in-effect through.
- **CONTRACTS TO IMPLEMENT.** 2.5 consumer_dedup ledger (owned). To the frozen shape (Bus §3.3).
- **GATE / DRILLS.** No standalone catalogue drill (the dedup property is greened transitively by SUB-D2 in
  EB-05). The gate is structural: idempotent re-delivery — the same (consumer, event_id) inserted twice yields
  one row and the handler runs once (the ON CONFLICT DO NOTHING property).
- **TESTS.** Unit: idempotent re-delivery (the same event_id delivered twice to the same consumer produces one
  effect via the ledger); two distinct consumers each record the same event_id independently (the consumer
  dimension of the PK). CDC: provider+consumer pair for 2.5. Mutation: dedup.rs is mandatory-core; state and
  meet the floor.
- **DEFINITION OF DONE.** dedup.rs compiles; 2.5 to the frozen shape; the idempotent-re-delivery unit test
  passes (dated); the CDC pair for 2.5 exists; coverage scanner + lints green; committed.
- **COMMIT.** Header "P-<NNN> M0: consumer_dedup ledger (effectively-once anchor)". Body: contract 2.5;
  idempotent re-delivery proven (one effect on double-delivery). Co-Authored-By trailer.

---

## EB-07 — The no-raw-publish lint (red + green fixtures, wired into CI)

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 — the no-raw-publish lint (the Bus's slice of the twelve committed lints).
  Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contract 1.6 (this lint).
- **DEPENDS-ON.** EB-03 (no-raw-publish forbids any emit path that isn't OutboxTx::emit — the lint needs the
  sanctioned path to exist), EB-04 (the transport.put it must forbid being called directly in a write path).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §5 (the ratchet — an uncommitted
    gate is no gate; make violations loud, never silently swallowed; no `... || true`).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.2 (the no-raw-publish discipline — the only
    sanctioned emit is OutboxTx::emit), §5.2 (the emit surface the lint protects).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 1.6 (the twelve architecture lints —
    the Bus owns no-raw-publish).
- **DELIVERABLE.** In the workspace lint crate (the architecture-lint harness laid down by the substrate M0
  prompts; the lints are committed CI gates, contract 1.6):
  - no-raw-publish: a compile-time lint making a bare broker publish / publish_now a compile error — the only
    sanctioned emit is OutboxTx::emit. Red fixture: a direct transport.put / publish_now in a write path (must
    be REJECTED). Green fixture: an OutboxTx::emit (must be ADMITTED).
  - WIRED INTO CI, loud, never `... || true` (an uncommitted lint is no lint — wire it in, do not leave it on
    disk).
- **CONTRACTS TO IMPLEMENT.** 1.6 (the Bus's no-raw-publish lint — owned slice; the lint harness is shared
  substrate).
- **GATE / DRILLS.** The lint green WITH BOTH FIXTURES, wired into CI loud-never-swallowed: it rejects its red
  fixture (proves it forbids) AND admits its green fixture (proves it doesn't over-reject). A lint that only
  rejects (or only admits) is not proven — both fixtures are the pass condition. The CI job's lint-pass
  artifact is the dated green proof.
- **TESTS.** Unit: the red fixture compiles to a lint error; the green fixture compiles clean. The CI wiring is
  the gate — assert the workflow fails (loudly, non-zero exit) when the red fixture is present (no `|| true`
  swallow). CDC: not a runtime contract — the fixture pair IS the test obligation.
- **DEFINITION OF DONE.** the no-raw-publish lint exists in the lint crate, is wired into CI
  loud-never-swallowed, and is green with both fixtures (PROVEN: red rejected, green admitted, dated); the
  CI-fails-on-red wiring test passes; committed.
- **COMMIT.** Header "P-<NNN> M0: no-raw-publish lint (red+green fixtures)". Body: contract 1.6 (no-raw-publish
  slice); lint green with red+green fixtures, wired into CI loud-never-swallowed. Co-Authored-By trailer.

---

## EB-08 — The no-cross-sync-cycle lint (red + green fixtures, wired into CI)

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 — the no-cross-sync-cycle lint (the Bus's slice of the twelve committed lints).
  Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contract 1.6 (this lint).
- **DEPENDS-ON.** EB-03 (the sanctioned async emit path the lint steers callers toward), EB-05 (the consumer/
  projection surface that IS the sanctioned cross-subsystem read the lint admits).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §5 (the ratchet — loud, never
    swallowed).
  - ../../05-refined-shared-systems-architecture/event-bus.md §7.1 (cell-local, no synchronous cross-system
    call in the write path — the no-cross-sync-cycle rule).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 1.6 (the Bus owns no-cross-sync-cycle).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md for the acyclicity rule the
    lint enforces (Git never synchronously calls CI; every cross-subsystem dependency is an async
    event/projection).
- **DELIVERABLE.** In the workspace lint crate:
  - no-cross-sync-cycle: a lint rejecting a synchronous cross-subsystem call in a write path (a subsystem
    synchronously calling another to ask "is it green"). Red fixture: a sync cross-subsystem call in a write
    path (must be REJECTED). Green fixture: an async event/projection read (must be ADMITTED).
  - WIRED INTO CI, loud, never `... || true`.
- **CONTRACTS TO IMPLEMENT.** 1.6 (the Bus's no-cross-sync-cycle lint — owned slice).
- **GATE / DRILLS.** The lint green WITH BOTH FIXTURES, wired into CI loud-never-swallowed: rejects the red
  fixture AND admits the green fixture. The CI job's lint-pass artifact is the dated green proof.
- **TESTS.** Unit: the red fixture compiles to a lint error; the green fixture compiles clean. The CI wiring is
  the gate — assert the workflow fails loudly on the red fixture (no `|| true`). CDC: the fixture pair IS the
  test obligation.
- **DEFINITION OF DONE.** the no-cross-sync-cycle lint exists, is wired into CI loud-never-swallowed, and is
  green with both fixtures (PROVEN, dated); the CI-fails-on-red wiring test passes; committed.
- **COMMIT.** Header "P-<NNN> M0: no-cross-sync-cycle lint (red+green fixtures)". Body: contract 1.6
  (no-cross-sync-cycle slice); lint green with red+green fixtures, wired into CI loud-never-swallowed.
  Co-Authored-By trailer.

---

## EB-09 — The Bus slice of the tenant-predicate-on-streams lint (red + green fixtures, wired into CI)

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 — the tenant-predicate lint as it applies to streams/consumers (the Bus's slice
  of the twelve committed lints). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contract 1.6.
- **DEPENDS-ON.** EB-05 (the consume/subscribe surface this lint checks must exist to be linted).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §5 (the ratchet — loud, never
    swallowed).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.2 (the whitelist-not-* rule the tenant-
    predicate slice enforces) + §7.1–§7.3 (per-(tenant, subsystem) streams).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 1.6 (the Bus owns the tenant-predicate
    slice for streams).
- **DELIVERABLE.** In the workspace lint crate:
  - the Bus slice of tenant-predicate: a lint rejecting a stream/consumer/subscribe without a (tenant,
    subsystem) scope. Red fixture: an unscoped subscribe / a subscribe with scope = * (must be REJECTED). Green
    fixture: a (tenant, subsystem)-scoped stream (must be ADMITTED).
  - WIRED INTO CI, loud, never `... || true`. (Note: this is the Bus's slice of the shared tenant-predicate
    lint, contract 1.6 — the data-store slices of the same lint are owned by other systems' M0 prompts.)
- **CONTRACTS TO IMPLEMENT.** 1.6 (the Bus's tenant-predicate-on-streams slice — owned slice).
- **GATE / DRILLS.** The lint green WITH BOTH FIXTURES, wired into CI loud-never-swallowed: rejects the red
  fixture AND admits the green fixture. The CI job's lint-pass artifact is the dated green proof. (Together
  with EB-07 + EB-08 this completes the Bus's three of the twelve-lint M0 gate.)
- **TESTS.** Unit: the red fixture (unscoped / scope=* subscribe) compiles to a lint error; the green fixture
  ((tenant, subsystem) stream) compiles clean. The CI wiring is the gate — assert the workflow fails loudly on
  the red fixture (no `|| true`). CDC: the fixture pair IS the test obligation.
- **DEFINITION OF DONE.** the tenant-predicate-on-streams lint exists, is wired into CI loud-never-swallowed,
  and is green with both fixtures (PROVEN, dated); the CI-fails-on-red wiring test passes; committed. (Note:
  this is the third of the Bus's three M0 lints; EB-07 + EB-08 + EB-09 together complete the Bus's slice of the
  twelve-lint M0 ratchet.)
- **COMMIT.** Header "P-<NNN> M0: tenant-predicate-on-streams lint (Bus slice, red+green fixtures)". Body:
  contract 1.6 (tenant-predicate stream slice); lint green with red+green fixtures, wired into CI
  loud-never-swallowed. Co-Authored-By trailer.

---

## EB-10 — The schema-evolution upcaster seam (forward-only; un-upcastable → DLQ)

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 — the schema-evolution seam (the upcaster registry + the forward-only
  discipline). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contract 2.8.
- **DEPENDS-ON.** EB-01 (the envelope/schema_ver the upcasters operate on), EB-05 (the consumer at whose
  consume-time the upcasters apply).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors — the
    forward-only discipline; an un-upcastable version is term'd, never silently dropped) + §5 (the
    forward-only-migration lint is a committed gate).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.10 (schema evolution / upcasting:
    expand→migrate→contract, forward-only, (type, from_ver)→to_ver pure fns at consume, an un-upcastable
    schema_ver is term'd to DLQ never silently dropped, no rollback migrations).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 2.8 (schema evolution / upcasters —
    forward-only).
- **DELIVERABLE.** In myelin-events:
  - upcast.rs: the upcaster registry ((type, from_ver) → to_ver pure functions applied at consume); the
    expand→migrate→contract forward-only discipline (new optional fields only, consumers ignore unknowns, no
    rollback migrations); an un-upcastable schema_ver is term'd to the DLQ, never silently dropped. The
    forward-only-migration lint applies to envelope evolution (this prompt wires the Bus's compliance; the lint
    itself is the substrate M0 forward-only-migration lint).
- **CONTRACTS TO IMPLEMENT.** 2.8 schema evolution / upcasters (owned). To the frozen shape (Bus §4.10).
- **GATE / DRILLS.** No standalone catalogue drill. The gate is structural: an upcaster chain (v1→v2→v3)
  applied at consume produces the current shape; an un-upcastable schema_ver fixture is term'd to the DLQ
  (asserted: 0 silently dropped); a consumer ignores an unknown forward-added field.
- **TESTS.** Unit: an upcaster chain (v1→v2→v3) applied at consume produces the current shape; an
  un-upcastable schema_ver lands in the DLQ (not dropped — 0 silently dropped); a consumer ignores an unknown
  forward-added field. CDC: provider+consumer pair for 2.8. Mutation: upcast.rs is mandatory-core; state and
  meet the floor.
- **DEFINITION OF DONE.** upcast.rs compiles; 2.8 to the frozen shape; the upcaster-chain + un-upcastable→DLQ
  (0 silently dropped) unit tests pass (dated); the CDC pair for 2.8 exists; coverage scanner + lints (incl.
  forward-only-migration) green; committed.
- **COMMIT.** Header "P-<NNN> M0: Schema-evolution upcaster seam (forward-only; un-upcastable → DLQ)". Body:
  contract 2.8; upcaster chain proven; un-upcastable schema_ver → DLQ (0 silently dropped). Co-Authored-By
  trailer.

---

## EB-11 — The Bus survival signals into the telemetry contract + the failure-injection harness self-test

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 — wiring the Bus's survival signals into the telemetry contract (1.8) + the
  harness self-test that closes the M0→M1 exit gate. Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M0",
  contract 1.8 (the Bus's contribution; §4.11).
- **DEPENDS-ON.** EB-03 (the outbox whose depth+age it instruments), EB-04 (the relay whose publish+dead-letter
  rate it instruments), EB-05 (the consumer whose lag it instruments), EB-06 (the dedup hit-rate it
  instruments).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §3 (observability is part of the
    pass condition — a drill that emits no signal has failed; the Bus is the largest single contributor to 1.8;
    build the failure-injection harness early — the harness self-test is the unit-of-proof).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.11 (the telemetry contract — the exact
    survival signals the §8 drills read).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 1.8 (telemetry signal set — consumer-
    lag, outbox-depth, breaker-state, causal-depth, per-tenant in-flight: the Phase-5 drill survival signals).
  - ../../06-roadmaps/00-master-sequencing.md §2 M0 exit gate ("the harness can inject a fault and read a
    telemetry assertion").
- **DELIVERABLE.** In myelin-events:
  - telemetry.rs: emit the Bus's survival signals to the metrics port (contract 1.8) — consumer lag
    (num_pending per durable consumer), outbox depth + age, relay publish + dead-letter rate, per-aggregate
    publish latency (recorded_at → broker-ack), dedup hit-rate, per-tenant in-flight, causal-depth histogram +
    shared-root-tripwire counter. These ARE the assertions the §8 drills read — wire them so every later Bus
    drill has a signal to assert against.
  - The harness self-test wiring: the failure-injection harness can inject a producer-kill fault and READ the
    resulting outbox-depth + dedup telemetry assertion (the harness self-test the M0→M1 boundary requires —
    master §2 M0 exit gate). This is the observability precondition for EB-04's SUB-D1/BUS-D4 and every later
    Bus drill.
- **CONTRACTS TO IMPLEMENT.** 1.8 telemetry signal set (the Bus's contribution — consumed/emitted; the Bus is
  the largest single contributor). To the frozen shape (Bus §4.11).
- **GATE / DRILLS.** No standalone catalogue drill greens here, but this prompt is the OBSERVABILITY gate: the
  failure-injection harness can inject a producer-kill fault and READ the resulting outbox-depth + dedup
  telemetry assertion (the harness self-test). Threshold: each named survival signal is emitted with the right
  name/unit; the self-test reads an outbox-depth assertion after an injected kill.
- **TESTS.** Telemetry: assert each survival signal is emitted with the right name/unit to the metrics port
  (the §4.11 list); the harness can read an outbox-depth assertion after an injected kill (the self-test).
  CDC: the metrics-port conformance for the 1.8 signal set the Bus contributes.
- **DEFINITION OF DONE.** telemetry.rs compiles; the Bus's 1.8 signals are emitted with correct names/units;
  the harness self-test (inject a kill, read an outbox-depth/dedup assertion) emits a dated green artifact;
  telemetry + CDC tests pass; coverage scanner + lints green; committed. This prompt completes the B-M0
  milestone — with EB-01..EB-11 the M0→M1 exit gate (SUB-D1, SUB-D2, BUS-D4, BUS-D2, the three lints, the
  harness self-test) is fully green.
- **COMMIT.** Header "P-<NNN> M0: Bus survival signals into telemetry + harness self-test". Body: contract 1.8
  (Bus contribution); the harness self-test green (inject kill → read outbox-depth/dedup); all §4.11 survival
  signals emitted with correct names/units. Co-Authored-By trailer.

---

## EB-12 — Partition the streams under the (tenant, region) key

- **BAND.** M1.
- **ROADMAP MILESTONE.** B-M1 (tenancy partition + the crypto-shred holder seam) — the partition slice.
  Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M1", contract 12.1 (consumed).
- **DEPENDS-ON.** EB-04 (the streams/relay being partitioned), EB-05 (the per-tenant in-flight cap that now
  becomes tenant-real). Upstream (must be merged first): the M1 Tenancy contract 12.1 (the (tenant, region)
  partition key) — owned by the Tenancy system's M1 prompts; EB-12 consumes it, so it cannot start until the
  Tenancy partition key exists.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §2 (the tenant/residency
    partition is a Tier-5 keystone — true by construction, not bolted on).
  - ../../05-refined-shared-systems-architecture/event-bus.md §2.2 (the subject encodes routing + ordering),
    §7.1–§7.3 (cell-local, per-(tenant, subsystem) streams, the tenant as the blast-radius + fairness unit).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 12.1 (the (tenant, region) partition
    key — consumed; injected by the harness).
- **DELIVERABLE.** In myelin-events:
  - Partition the streams under the (tenant, region) key: provision per-(tenant, subsystem) streams
    (cell-local), partitioned internally by aggregate_id; the subject encodes both routing and ordering as
    evt.<tenant>.<subsystem>.<aggregate_type>.<aggregate_id>.<event_name>. The tenant becomes the
    blast-radius + fairness unit (the EB-05 per-tenant in-flight cap is now tenant-real — one tenant's surge is
    isolated from another's stream).
- **CONTRACTS TO IMPLEMENT.** 12.1 (consumed — the partition key the streams are keyed under).
- **GATE / DRILLS.** No standalone catalogue drill on the partition alone (residency is EB-13). The gate is
  structural: the subject grammar round-trips the routing + ordering key; the per-tenant in-flight cap isolates
  one tenant's surge from another's stream (the bulkhead property under the tenant key).
- **TESTS.** Unit: the subject grammar round-trips the (tenant, subsystem, aggregate_type, aggregate_id,
  event_name) routing + ordering key; the per-tenant in-flight cap isolates one tenant's surge from another's
  stream. CDC: the consumer side of 12.1 (the Bus calls the partition key).
- **DEFINITION OF DONE.** Streams partitioned under (tenant, region); 12.1 consumed correctly; the subject
  round-trip + per-tenant isolation unit tests pass; the CDC consumer side of 12.1 exists; coverage scanner +
  lints green; committed.
- **COMMIT.** Header "P-<NNN> M1: Partition streams under (tenant, region)". Body: contract 12.1 consumed;
  subject round-trip + per-tenant in-flight isolation proven. Co-Authored-By trailer.

---

## EB-13 — Residency-pin the Bus streams (no cross-region read path)

- **BAND.** M1.
- **ROADMAP MILESTONE.** B-M1 — the residency-pin slice. Roadmap: ../../06-roadmaps/shared/event-bus.md §2
  "B-M1", contract 12.4 (consumed).
- **DEPENDS-ON.** EB-12 (the (tenant, region)-partitioned streams being residency-pinned). Upstream (must be
  merged first): the M1 Tenancy contract 12.4 (residency_verify) — owned by the Tenancy system's M1 prompts.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/04-hard-problems.md §1 (residency: region-pinning, no cross-region
    query path).
  - ../../05-refined-shared-systems-architecture/event-bus.md §7.1–§7.3 (per-(tenant, subsystem) streams,
    region-pinned, no cross-region stream read path — the residency-pin lint applies).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 12.4 (residency_verify — consumed;
    every store reports the tenant's region, the no-global-pool property attestable).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows CP-D3 / STOR-D5 (residency — the Bus's slice).
- **DELIVERABLE.** In myelin-events:
  - Residency-pin: no cross-region stream read path (the residency-pin lint, substrate M0, applies); a write
    where row.region ≠ cell.region is rejected; residency_verify attestation passes for the Bus's streams
    (the stream-provisioning code reports its region).
- **CONTRACTS TO IMPLEMENT.** 12.4 (consumed — residency_verify for the Bus's streams).
- **GATE / DRILLS.** The Bus's slice of CP-D3 / STOR-D5 (residency): a write where row.region ≠ cell.region is
  rejected; no cross-region stream read path exists; residency_verify attestation passes for the Bus's streams
  (CI/SCHED). Threshold: 0 cross-region stream reads; the residency-pin lint green on the Bus's
  stream-provisioning code.
- **TESTS.** Unit: a stream provisioned for (tenant, region=eu-west) rejects a read routed from a different
  region. Residency drill: the Bus's slice of CP-D3 on the harness (an out-of-region write is rejected,
  asserted against the residency telemetry). CDC: the consumer side of 12.4 (the Bus calls residency_verify).
- **DEFINITION OF DONE.** Streams residency-pinned; 12.4 consumed correctly; the residency-pin lint green on
  the Bus code; the Bus's CP-D3 slice emits a dated green artifact (0 cross-region reads); unit + drill + CDC
  tests pass; coverage scanner + lints green; committed.
- **COMMIT.** Header "P-<NNN> M1: Residency-pin the Bus streams". Body: contract 12.4 consumed; CP-D3 (Bus
  slice) green (0 cross-region reads); residency-pin lint green. Co-Authored-By trailer.

---

## EB-14 — Pin the cross-cell bridge FRAME (CrossCellPointer; designed-not-built)

- **BAND.** M1.
- **ROADMAP MILESTONE.** B-M1 — pin the cross-cell propagation bridge frame (the named floor; built in EB-25).
  Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M1", contract 12.6 (frame pinned).
- **DEPENDS-ON.** EB-12 (the partition the cross-cell frame extends from). Upstream context: the M1 Tenancy
  control-plane frame (12.6) — the frame is cell-agnostic by construction; the BUILD is EB-25/M5.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/04-hard-problems.md §1 (residency: no cross-region query path) +
    external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors: pin the frame now, build it
    later, never silently).
  - ../../05-refined-shared-systems-architecture/event-bus.md §7.4 (the cross-cell propagation FLOOR —
    designed-not-built; the bridge frame now PINNED; the §5 contracts are cell-agnostic so it extends without a
    rewrite).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 12.6 (CrossCellPointer{subject (opaque),
    type, correlation_id, home_cell} — the frame pinned here, BUILT in EB-25).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-I (the cross-cell frame).
- **DELIVERABLE.** In myelin-events:
  - PIN the cross-cell bridge frame: the frozen CrossCellPointer{subject: OpaqueSubjectId, type: ArtifactType,
    correlation_id: CorrelationId, home_cell: CellId} type — designed-not-built. The §5 contracts are
    cell-agnostic so it extends without a rewrite.
  - FLOOR named: single-home-cell propagation is v1; the cross-cell PII-free bridge BUILD (per-viewer
    cell-local resolution, the residency proof that no PII crosses, multi-cell fan-out) is the M5 follow-on
    EB-25.
- **CONTRACTS TO IMPLEMENT.** 12.6 (the CrossCellPointer frame pinned — owned-as-frame, built in EB-25).
- **GATE / DRILLS.** No catalogue drill (the frame is designed-not-built; its drills GA-D8/CP-D7/CP-D8 are owed
  by EB-25 when multi-cell ships). The gate is structural: the CrossCellPointer frame serde round-trips; the §5
  contract surfaces are cell-agnostic (a compile-time assertion that they take the opaque subject, never a
  cell-bound row).
- **TESTS.** Unit: the CrossCellPointer frame's serde round-trip; a compile-time check that the §5 contract
  surfaces are cell-agnostic. CDC: the CrossCellPointer frame's serde conformance.
- **DEFINITION OF DONE.** The CrossCellPointer frame pinned + serde-round-trips; 12.6 frame-pinned; the
  single-home-cell FLOOR + its M5 EB-25 follow-on named in writing; unit + CDC tests pass; coverage scanner +
  lints green; committed.
- **COMMIT.** Header "P-<NNN> M1: Pin cross-cell bridge frame (CrossCellPointer, designed-not-built)". Body:
  contract 12.6 frame pinned; CrossCellPointer serde round-trips; floor named (cross-cell BUILD → EB-25).
  Co-Authored-By trailer.

---

## EB-15 — Register the Bus as a PersonalDataHolder + wire inline-PII crypto-shred to the KMS hierarchy

- **BAND.** M1.
- **ROADMAP MILESTONE.** B-M1 — the crypto-shred holder seam (the event-log half of erasure-vs-immutability,
  the structural floor). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M1", contract 2.7 (owned) +
  10.1/11.3/11.4 (consumed).
- **DEPENDS-ON.** EB-01 (the envelope's pii_key_ref + contains_personal_data fields), EB-03 (the outbox the
  *.erased tombstones emit through), EB-12 (the (tenant, region) partition the holder operates within).
  Upstream (must be merged first): the M1 GDPR/Storage contracts 10.1 (PersonalDataHolder trait + harness
  auto-registration), 11.3/11.4 (the KMS hierarchy + per-subject DEK) — owned by the GDPR/Storage M1 prompts;
  EB-15 wires to them. (The erasure-ledger post-restore re-erasure hook is EB-16.)
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §2 (silent data loss / the
    holder list must be exhaustive before real data) + external-insights/04-hard-problems.md §1
    (erasure-vs-immutability: references-not-payloads + pseudonymous actor.principal + crypto-shred + tombstone
    — the structural answer).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.8 (retention + crypto-shred + tombstones —
    the references-not-payloads + crypto-shred + tombstone triad; this is the Bus's instantiation of the ONE
    platform erasure posture, X-7, by reference NOT restated) + §5.7 (the PersonalDataHolder impl signature:
    locate/erase/export).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 2.7 (crypto-shred / tombstone on the
    log — owned), 10.1 (PersonalDataHolder — consumed), 11.3 (KMS hierarchy + KeyOrigin — consumed), 11.4
    (crypto-shred granularity / per-subject DEK — consumed).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-7 (the ONE platform erasure
    posture — the [OPEN — LEGAL] residual is flagged, not an engineering gate).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row BUS-D8 (the erasure drill this prompt greens in the live-store leg; the reaches-backups leg is EB-29/M5).
- **DELIVERABLE.** In myelin-events:
  - holder.rs: impl PersonalDataHolder for the EventBus — locate(subject) → inline-PII events + tombstone
    status; erase(subject) → crypto-shred inline-PII keys (destroy the pii_key_ref DEK) + emit *.erased
    tombstones via the outbox; export(subject) → the subject's events (references resolved via owners).
    Auto-registered by the harness so the H1–H18 holder list is EXHAUSTIVE before any real tenant data is
    written.
  - Wire the inline-PII crypto-shred to the M1 KMS hierarchy: the rare inline-PII event is envelope-encrypted
    with pii_key_ref = kms://<tenant>/<dek-epoch>/<class> (per-tenant, optionally per-subject DEK); erasure =
    destroy the key. The references-not-payloads default means MOST events carry contains_personal_data =
    false (erasing the person tombstones the identity, not the fact).
  - This is the STRUCTURAL FLOOR (the event-log half of erasure). FLOOR named: the reaches-backups leg of
    BUS-D8 is the M5 follow-on EB-29 (re-confirmed with STOR-D4). FLOOR named by reference: the [OPEN — LEGAL]
    residual lawful-basis is the ONE platform posture (10.9, X-7) — handled by the GDPR/legal track, not
    restated here; the structural floor ships regardless. (The erasure-ledger re-erasure hook is EB-16.)
- **CONTRACTS TO IMPLEMENT.** 2.7 crypto-shred / tombstone on the log (owned); 10.1 PersonalDataHolder
  (consumed — the Bus implements the trait), 11.3 KMS hierarchy (consumed), 11.4 crypto-shred granularity
  (consumed).
- **GATE / DRILLS.** BUS-D8 (live-store leg) → erase a subject; inline-PII events unrecoverable (the DEK
  destroyed); *.erased tombstones emitted; consumers degrade gracefully; reads erase-receipt + tombstone count
  (SCHED). Threshold: 0 recoverable inline-PII in the live log; tombstones present. The reaches-backups leg is
  EB-29/M5 (re-confirmed with STOR-D4). The Bus's holder participates in the STOR-D1/D2 restore-verify
  cross-seam (the event-log offset is the cross-seam cursor) — must be consistent at the restored point.
- **TESTS.** Unit: erase(subject) destroys the pii_key_ref DEK and renders the inline-PII payload
  unrecoverable; *.erased tombstones are emitted via the outbox; a consumer degrades gracefully on a tombstone;
  export(subject) returns the subject's events with references resolved. Drill scenario: BUS-D8 (live leg) on
  the harness, asserting against erase-receipt + tombstone-count telemetry. CDC: provider side of 2.7 +
  consumer side of 10.1/11.3/11.4. Mutation: holder.rs is mandatory-core; state and meet the floor.
- **DEFINITION OF DONE.** holder.rs compiles; 2.7 owned + 10.1/11.3/11.4 wired; BUS-D8 (live-store leg) emits
  a dated green artifact (0 recoverable inline-PII live; tombstones present — PROVEN); the holder participates
  correctly in the restore-verify cross-seam; unit + drill + CDC tests pass; coverage scanner + lints green;
  the reaches-backups FLOOR (→ EB-29) + the [OPEN — LEGAL] residual (by reference to X-7) named in writing;
  committed.
- **COMMIT.** Header "P-<NNN> M1: Bus PersonalDataHolder + inline-PII crypto-shred to KMS". Body: contract 2.7;
  10.1/11.3/11.4 consumed; BUS-D8 live-store leg green (0 recoverable inline-PII live, tombstones present);
  floors named (reaches-backups → EB-29; [OPEN — LEGAL] residual → X-7 by reference). Co-Authored-By trailer.

---

## EB-16 — Hook the erasure ledger for post-restore re-erasure (the key stays destroyed across a restore)

- **BAND.** M1.
- **ROADMAP MILESTONE.** B-M1 — the erasure-ledger post-restore re-erasure hook. Roadmap:
  ../../06-roadmaps/shared/event-bus.md §2 "B-M1", contract 10.8 (consumed).
- **DEPENDS-ON.** EB-15 (the holder + the crypto-shred path the re-erasure re-applies). Upstream (must be
  merged first): the M1 GDPR contract 10.8 (the erasure ledger) — owned by the GDPR M1 prompts.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/04-hard-problems.md §1 (erasure-vs-immutability: the key stays
    destroyed even after a backup is restored).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.8 (retention + crypto-shred + tombstones —
    post-restore re-erasure fan-out).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 10.8 (erasure ledger — consumed;
    drives post-restore re-erasure GD-14), 11.5 (backup/restore cross-seam — the event-log offset is the
    cross-seam cursor; post_restore_reerase).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows STOR-D1/STOR-D2 (restore-verify — the Bus's holder participates) + BUS-D8 (re-erasure leg).
- **DELIVERABLE.** In myelin-events:
  - Hook the erasure ledger (10.8) for post-restore re-erasure: when Storage restores an older backup, the
    Bus's holder participates in the re-erasure fan-out (the key stays destroyed across a restore) — the
    erasure ledger (PII-free, opaque subject id) drives the re-application so a restored backup does not
    resurrect an erased subject's inline-PII keys.
- **CONTRACTS TO IMPLEMENT.** 10.8 erasure ledger (consumed — the Bus participates in the re-erasure fan-out).
- **GATE / DRILLS.** The Bus's leg of the STOR-D1/D2 restore-verify cross-seam: after a backup restore the
  re-erasure fan-out re-destroys the keys for every ledger-listed erased subject; the Bus's outbox/streams are
  consistent at the restored point and an erased subject's inline-PII stays unrecoverable post-restore (SCHED).
  Threshold: 0 resurrected inline-PII keys after a restore.
- **TESTS.** Unit: a post-restore re-erasure pass over the ledger re-destroys the keys for a previously-erased
  subject (the restored backup does not resurrect the key). Drill scenario: the Bus's leg of STOR-D1/D2 on the
  harness — restore an older backup, run the re-erasure fan-out, assert 0 resurrected keys. CDC: the consumer
  side of 10.8.
- **DEFINITION OF DONE.** The erasure-ledger re-erasure hook is wired; 10.8 consumed; the Bus's STOR-D1/D2
  restore-verify leg emits a dated green artifact (0 resurrected inline-PII keys post-restore); unit + drill +
  CDC tests pass; coverage scanner + lints green; committed. This completes B-M1.
- **COMMIT.** Header "P-<NNN> M1: Erasure-ledger post-restore re-erasure hook". Body: contract 10.8 consumed;
  Bus restore-verify leg green (0 resurrected inline-PII keys post-restore). Co-Authored-By trailer.

---

## EB-17 — The EventMatcher = the frozen myelin-query QueryAst (bounded, permission-aware interpreter)

- **BAND.** M2.
- **ROADMAP MILESTONE.** B-M2 (the reactive layer) — the EventMatcher freeze (the prerequisite for
  Signals/Automations/Triggers all sharing one predicate surface). Roadmap:
  ../../06-roadmaps/shared/event-bus.md §2 "B-M2", contract 3.4.
- **DEPENDS-ON.** EB-01 (the envelope the matcher evaluates over). Upstream (must be merged first): the M2
  frozen myelin-query QueryAst (13.3, co-owned by Issues/Knowledge) — the EventMatcher IS its predicate core,
  so the freeze must land first; the M1 Identity list_objects SetExpr push-down (4.3) — the matcher composes
  with it for permission-awareness.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §7 (reconcile cross-component
    contracts at the plan layer — the matcher, saved views, Search, Notif prefs must not drift; one grammar).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.5 (the EventMatcher = the frozen QueryAst:
    the grammar And/Or/Not/Cmp/In/Has/Text/Ref, ops eq..within; the bounded interpreter — no UDFs/loops/
    recursion, statically cost-bounded, side-effect-free; permission-aware by construction — always composes
    with list_objects(viewer, read, type); one grammar, four compile targets incl. the JetStream subject
    filter + the bounded interpreter).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 3.4 (EventMatcher = the frozen
    QueryAst), 13.3 (myelin-query primitive frozen byte-identical — consumed; the matcher IS the QueryAst
    predicate core), 4.3 (list_objects SetExpr push-down — consumed; the matcher composes with it).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-3 (the QueryAst freeze) and
    OQ-C.
- **DELIVERABLE.** In myelin-events:
  - matcher.rs: the EventMatcher as the predicate core of the frozen myelin-query QueryAst (And/Or/Not/Cmp/In/
    Has/Text/Ref, ops eq | ne | lt | lte | gt | gte | contains | starts_with | within; Literal = Str | Num |
    Bool | Date | Principal | Ref | Null), serialised as JSON, evaluated by a custom BOUNDED interpreter — NOT
    raw CEL or JSONLogic. The validator REJECTS an AST whose worst-case cost exceeds a budget; evaluation is
    side-effect-free; no UDFs, no loops, no unbounded recursion.
  - The compile path: (a) compile to a JetStream subject filter where possible (cheap server-side prefilter),
    then (b) the bounded interpreter over the envelope for the residual predicate.
  - Permission-aware BY CONSTRUCTION: a matcher always composes with list_objects(viewer, read, type) (the
    frozen SetExpr push-down, 4.3) so no matcher/saved-view/search surface can select artifacts the subject
    can't see. The Has/Ref predicates over projection state express the relational conditions
    (CI's on: pull_request / issue.transitioned; Issues' "all blocked_by resolved").
  - Freeze the grammar BYTE-IDENTICAL with 13.3 — assert the serialisation matches the myelin-query crate's so
    matcher / saved-views / Search / Notif-prefs cannot drift.
- **CONTRACTS TO IMPLEMENT.** 3.4 EventMatcher (owned — = the frozen QueryAst); composes with 4.3
  list_objects (consumed) and shares 13.3 myelin-query (consumed, byte-identical).
- **GATE / DRILLS.** No standalone catalogue drill (the matcher is greened transitively by BUS-D6 in EB-23
  and the Signal/Trigger drills). The gate is structural: (a) the cost validator REJECTS an over-budget AST
  (a fixture exceeding the bound is rejected — the DoS-hardening property); (b) byte-identical serialisation
  with the myelin-query crate (a cross-crate round-trip fixture); (c) permission-awareness — a matcher over a
  type the viewer can't see returns zero matches after composing with list_objects (0 leak).
- **TESTS.** Unit: every operator total + side-effect-free; the cost validator rejects an over-budget AST;
  byte-identical serde with myelin-query (the no-drift property); a Has/Ref predicate over projection state
  evaluates correctly ("all blocked_by resolved"); the permission compose returns 0 matches for an unviewable
  type. CDC: provider+consumer pair for 3.4; the byte-identical 13.3 conformance test (a Bus-serialised
  QueryAst deserialised by the myelin-query crate, round-trip equal). Mutation: matcher.rs is mandatory-core;
  state and meet the floor.
- **DEFINITION OF DONE.** matcher.rs compiles; 3.4 to the frozen shape, byte-identical with 13.3; the cost
  validator rejects over-budget ASTs (dated); the permission-compose returns 0 leak; unit + CDC tests pass;
  coverage scanner + lints green; committed.
- **COMMIT.** Header "P-<NNN> M2: EventMatcher = frozen QueryAst (bounded, permission-aware)". Body: contract
  3.4; byte-identical with 13.3 (no drift); cost validator rejects over-budget; permission-compose 0 leak.
  Co-Authored-By trailer.

---

## EB-18 — Signal curation (match / severity-rank / dedup-window / auto-resolve / publish)

- **BAND.** M2.
- **ROADMAP MILESTONE.** B-M2 — Signal curation (3.1, the upstream defence). Roadmap:
  ../../06-roadmaps/shared/event-bus.md §2 "B-M2", contract 3.1.
- **DEPENDS-ON.** EB-05 (the consumer template the Signal engine is built from), EB-17 (the EventMatcher =
  QueryAst the matcher column stores).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/03-agent-native-fabric.md §2 (the four primitives — Event / Signal /
    Automation / Trigger, each a different author/lifetime/store; don't collapse them — Signal is the curated,
    deduped, severity-ranked subset).
  - ../../05-refined-shared-systems-architecture/event-bus.md §1.2 (the four primitives), §4.4 (Signal
    curation / dedup / severity-ranking — match, severity-rank info<notice<warning<error<critical, dedup
    within window with count=N, auto-resolve, publish to sig.<tenant>.<severity>.<rule>; the upstream defence).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 3.1 (define_signal_rule(SignalRule
    {matcher, severity, dedup_key_tpl, dedup_window}) — curated/deduped/ranked subset; consumers subscribe to
    Signals, never evt.*).
- **DELIVERABLE.** In myelin-events:
  - signals.rs: the Signal engine (an infra consumer on the raw evt.* firehose — one of the excepted
    full-firehose consumers, built from the EB-05 template). define_signal_rule(SignalRule{matcher /* QueryAst
    */, severity, dedup_key_tpl, dedup_window}): match the EventMatcher; severity-rank
    (info<notice<warning<error<critical); dedup within window (dedup_key = render(tpl, envelope); ON CONFLICT …
    count = count+1 — N identical failures collapse to one Signal with count=N, the storm-control primitive
    Notif relies on); auto-resolve (a resolving matcher resolves the matching Signal); publish to
    sig.<tenant>.<severity>.<rule>. The UPSTREAM DEFENCE: product consumers + agents subscribe to curated
    Signals, never the raw firehose.
- **CONTRACTS TO IMPLEMENT.** 3.1 define_signal_rule (owned). To the frozen shape (Bus §4.4).
- **GATE / DRILLS.** No standalone catalogue drill (Signal correctness is greened transitively by BUS-D3
  replay-determinism in EB-23). The gate for THIS prompt: the dedup-window collapse is correct (N identical
  events → one Signal count=N); severity-ranking ordering is correct; auto-resolve resolves the matching Signal.
- **TESTS.** Unit: dedup-window collapse (10 identical failures → one Signal count=10); severity-ranking
  ordering; auto-resolve (a ci.run.passed resolves the matching ci.run.failed). CDC: provider+consumer pair for
  3.1. Mutation: signals.rs (the dedup-window collapse) is mandatory-core; state and meet the floor.
- **DEFINITION OF DONE.** signals.rs compiles; 3.1 to the frozen shape; the dedup-collapse (count=N) +
  severity-ranking + auto-resolve unit tests pass (dated); the CDC pair for 3.1 exists; coverage scanner +
  lints green; committed.
- **COMMIT.** Header "P-<NNN> M2: Signal curation (dedup-window / severity-rank / auto-resolve)". Body:
  contract 3.1; dedup-collapse (count=N) + severity-ranking + auto-resolve proven. Co-Authored-By trailer.

---

## EB-19 — Automations (the stateless per-event reflex over the matcher)

- **BAND.** M2.
- **ROADMAP MILESTONE.** B-M2 — Automations (3.2, the stateless per-event reflex). Roadmap:
  ../../06-roadmaps/shared/event-bus.md §2 "B-M2", contract 3.2.
- **DEPENDS-ON.** EB-05 (the consumer template the Automation engine is built from), EB-17 (the EventMatcher =
  QueryAst the matcher column stores). Upstream (must be merged first): the M2 myelin-flow durable workflow
  surface (9.1/9.2) — Automation action.kind=workflow delegates to it.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/03-agent-native-fabric.md §2 (the four primitives — Automation is
    the stateless per-event reflex; don't collapse it with Trigger, which is the stateful per-person promise).
  - ../../05-refined-shared-systems-architecture/event-bus.md §1.2 (the four primitives), §3.5 (the automation
    store), §4.7 (the reactive surface automations run on — for context; the dispatch tier is EB-23).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 3.2 (register_automation(AutomationRule
    {matcher, action, run_as, delegation, budget, gates}) — stateless per-event reflex; may invoke a durable
    workflow), 9.1/9.2 (the durable workflow surface — consumed, for action.kind=workflow).
- **DELIVERABLE.** In myelin-events:
  - automations.rs: register_automation(AutomationRule{matcher, action, run_as, delegation, budget, gates}) —
    the stateless per-event reflex; match the EventMatcher; on a match, run the action under run_as +
    delegation within budget + gates; action.kind=workflow invokes myelin-flow (DurableExecutor::start, 9.1).
    No per-person state (that is Trigger, EB-20); each event is matched independently.
- **CONTRACTS TO IMPLEMENT.** 3.2 register_automation (owned); 9.1/9.2 the durable workflow surface (consumed,
  for action.kind=workflow).
- **GATE / DRILLS.** No standalone catalogue drill (Automation correctness is greened transitively by BUS-D3
  replay-determinism in EB-23). The gate for THIS prompt: a matching event fires the automation exactly once
  per delivery (idempotent on event_id via the EB-06 dedup ledger); action.kind=workflow delegates to
  myelin-flow (not reinvented); run_as + delegation + budget + gates are honoured.
- **TESTS.** Unit: a matching event fires the automation (and a non-matching one does not); action.kind=
  workflow invokes the myelin-flow DurableExecutor (delegated, not reinvented); the automation honours run_as +
  budget. CDC: provider+consumer pair for 3.2; the consumer side of 9.1/9.2. Mutation: automations.rs is
  mandatory-core; state and meet the floor.
- **DEFINITION OF DONE.** automations.rs compiles; 3.2 to the frozen shape; 9.1/9.2 consumed; the
  match-fires-once + workflow-delegation unit tests pass (dated); CDC pairs exist; coverage scanner + lints
  green; committed.
- **COMMIT.** Header "P-<NNN> M2: Automations (stateless per-event reflex)". Body: contracts 3.2; 9.1/9.2
  consumed; match-fires-once + workflow delegation proven. Co-Authored-By trailer.

---

## EB-20 — Triggers (the stateful per-person promise; fire-once-per-arming guarded UPDATE)

- **BAND.** M2.
- **ROADMAP MILESTONE.** B-M2 — Triggers (3.3, the stateful per-person promise). Roadmap:
  ../../06-roadmaps/shared/event-bus.md §2 "B-M2", contract 3.3.
- **DEPENDS-ON.** EB-05 (the consumer template the Trigger engine is built from), EB-17 (the EventMatcher =
  QueryAst the condition column stores). Upstream (must be merged first): the M2 myelin-flow durable timer
  wheel (9.3) — Trigger stale_after delegates to it.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/03-agent-native-fabric.md §2 (the four primitives — Trigger is the
    stateful per-person promise; armed → {resolved|stale|disarmed}; don't collapse it with the stateless
    Automation).
  - ../../05-refined-shared-systems-architecture/event-bus.md §1.2 (the four primitives), §3.6 (the trigger
    store), §4.6 (the Trigger state machine armed → {resolved|stale|disarmed}, fire-once-per-arming via the
    atomic guarded UPDATE; condition = the frozen QueryAst over projection state; stale_after delegated to the
    myelin-flow timer wheel).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 3.3 (arm_trigger/disarm_trigger
    (Trigger{owner, condition, arms_subject, on_resolve, stale_after}); condition = the frozen QueryAst over
    projection state, "all blocked_by resolved"), 9.3 (the durable timer wheel — consumed).
- **DELIVERABLE.** In myelin-events:
  - triggers.rs: arm_trigger/disarm_trigger(Trigger{owner, condition /* QueryAst over projection state */,
    arms_subject, on_resolve, stale_after}) — the stateful per-person promise; armed → {resolved|stale|
    disarmed}; fire-once-per-arming via the atomic guarded UPDATE (UPDATE trigger SET state='resolved',
    resolved_by=:event_id WHERE id=:id AND state='armed'); armed→stale on a myelin-flow durable timer set to
    stale_after; armed→disarmed on owner cancel; re-arming creates a new arming (idempotency is per-arming).
- **CONTRACTS TO IMPLEMENT.** 3.3 arm_trigger/disarm_trigger (owned); 9.3 the timer wheel (consumed).
- **GATE / DRILLS.** No standalone catalogue drill (Trigger correctness is greened transitively by BUS-D3
  replay-determinism in EB-23). The gate for THIS prompt: the Trigger fires EXACTLY ONCE per arming under
  concurrent resolving events (the atomic guard — a fire-once property test); stale_after delegates to the
  myelin-flow timer (not reinvented); re-arming creates a fresh arming.
- **TESTS.** Unit: the Trigger fire-once-per-arming under two concurrent resolving events (only one wins the
  guarded UPDATE); armed→stale on the myelin-flow timer (delegated, not reinvented); armed→disarmed on owner
  cancel; re-arming creates a fresh arming (idempotency is per-arming). CDC: provider+consumer pair for 3.3;
  the consumer side of 9.3. Mutation: triggers.rs (the fire-once guard) is mandatory-core; state and meet the
  floor.
- **DEFINITION OF DONE.** triggers.rs compiles; 3.3 to the frozen shape; 9.3 consumed; the fire-once-per-arming
  + stale_after-delegation + re-arming unit tests pass (dated); CDC pairs exist; coverage scanner + lints green;
  committed.
- **COMMIT.** Header "P-<NNN> M2: Triggers (stateful per-person promise, fire-once-per-arming)". Body:
  contracts 3.3; 9.3 consumed; Trigger fire-once-per-arming + stale_after delegation proven. Co-Authored-By
  trailer.

---

## EB-21 — The firehose resume-cursor subscription protocol (built FIRST)

- **BAND.** M2.
- **ROADMAP MILESTONE.** B-M2 — the firehose transport + resume-cursor protocol (3.5, EI-04 §2.2 "build it
  FIRST"). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M2", contract 3.5.
- **DEPENDS-ON.** EB-04 (the durable bus/transport carrying the pointer events), EB-05 (the consumer/dedup the
  resume path leans on). This is the durable real-time transport the KN CAS floor (M3) and CRDT (M5) slot into
  — it MUST be green before any subsystem live surface rides it. (The reindex-from-source seam that is the
  resync_required fallback target is EB-22.)
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/04-hard-problems.md §2.2 (BUILD THE RESUME-CURSOR TRANSPORT FIRST,
    before the CRDT — the CRDT slots into this transport).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.3 (the firehose split + the resume-cursor
    protocol: subscribe(stream, scope, cursor?)/resume(stream, scope, last_seq); per-(stream, scope) monotonic
    seq; (last_seq, now] backfill on reconnect loses ZERO ops; resync_required → *.snapshot fallback;
    bounded-scope discipline never *, board:/doc:/channel:; per-connection in-flight caps, slow consumer →
    resync_required not unbounded buffering), §5.5 (the firehose contract surface).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 3.5 (firehose transport + resume-cursor
    protocol — owned-seam; KN owns the collab CRDT).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-J (the firehose protocol).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row D-10 (firehose reconnect loses zero ops — architecture §8 D-10).
- **DELIVERABLE.** In myelin-events:
  - firehose.rs: firehose::publish(stream, frame) / tail(stream, range); the NEW subscribe(stream, scope,
    cursor?) → SubStream (frames carry a per-(stream, scope) monotonic seq) and resume(stream, scope,
    last_seq) → backfill (last_seq, now] from a bounded retention window then live (LOSES ZERO OPS); an
    out-of-window last_seq yields resync_required → a *.snapshot replay fallback (the cold-rebuild path, NAMED
    not silent; the *.snapshot schema + reindex seam is EB-22). scope is a BOUNDED selector, NEVER * (board:
    <id>/doc:<id>/channel:<id>); the transport REJECTS an unbounded/over-broad scope (the whitelist-not-* rule
    generalised); a huge board paginates its scope. Per-connection in-flight frame caps; a slow consumer drops
    to resync_required rather than buffering unboundedly.
  - FLOOR named: the firehose retention window per stream class is named-not-numbered (the window must exceed
    the p99 reconnect gap); it is MEASURED + tuned by D-10 in M5 (EB-30). FLOOR named: D-10 is written to
    re-run green across the KN CAS→CRDT engine_promote boundary (re-confirmed in EB-30).
- **CONTRACTS TO IMPLEMENT.** 3.5 firehose transport + resume-cursor protocol (owned-seam). To the frozen shape
  (Bus §4.3).
- **GATE / DRILLS.** D-10 (firehose) → drop a subscribed connection mid-stream on a hot board/doc/channel;
  resume(last_seq) backfills (last_seq, now] then live, 0 OPS LOST; an out-of-window last_seq yields
  resync_required → *.snapshot fallback; reads firehose seq gaps + resync count (CI). Threshold: 0 ops lost;
  resync path correct. The transport REJECTS an over-broad scope (a fixture: scope=* is rejected). (The
  *.snapshot rebuild target lands in EB-22; D-10's resync_required leg asserts the signal is raised — the
  rebuild is proven cold==live by BUS-D5 in EB-22.)
- **TESTS.** Unit: per-(stream, scope) monotonic seq; backfill (last_seq, now] then live loses 0 ops; an
  out-of-window last_seq → resync_required; an over-broad scope is rejected; a slow consumer drops to
  resync_required (no unbounded buffering). Drill scenario: D-10 (firehose reconnect, 0 ops lost) on the
  harness, asserting against firehose-seq-gap + resync-count telemetry. CDC: provider+consumer pair for 3.5.
  Mutation: firehose.rs (the resume path) is mandatory-core; state and meet the floor.
- **DEFINITION OF DONE.** firehose.rs compiles; 3.5 to the frozen shape; D-10 emits a dated green artifact
  (0 ops lost; resync path raises resync_required correctly — PROVEN); the over-broad-scope rejection is proven;
  unit + drill + CDC tests pass; coverage scanner + lints green; the retention-window FLOOR (→ EB-30 measured) +
  the CRDT-boundary re-run note (→ EB-30) named in writing; committed.
- **COMMIT.** Header "P-<NNN> M2: Firehose resume-cursor protocol (built first)". Body: contract 3.5; D-10
  green (0 ops lost, resync correct); over-broad scope rejected; floors named (retention window → EB-30;
  CRDT-boundary re-run → EB-30). Co-Authored-By trailer.

---

## EB-22 — The reindex-from-source seam + the *.snapshot event schema (cold == live)

- **BAND.** M2.
- **ROADMAP MILESTONE.** B-M2 — reindex-from-source (2.6 — the only recovery path + the resync_required
  fallback target). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M2", contract 2.6.
- **DEPENDS-ON.** EB-03 (the outbox the *.snapshot events re-emit through), EB-21 (the firehose whose
  resync_required leg this seam is the rebuild target for). The per-owner replay(scope, since) lands with each
  owner in EB-26 (M3/M4); this prompt ships the SEAM + schema + a reference consumer to prove cold==live.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/04-hard-problems.md §5.3 (reindex-from-source — the only recovery
    path for derived stores).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.9 (reindex-from-source: events::reindex(scope)
    → owner replay(scope, since) emits *.snapshot via the SAME outbox→bus path; sub-artifact-granular).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 2.6 (reindex-from-source — owned seam +
    every subsystem's replay; sub-artifact-granular CI one-run / KN page-subtree at block granularity).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row BUS-D5 (reindex-from-cold parity — cold == live).
- **DELIVERABLE.** In myelin-events:
  - reindex.rs: the events::reindex(scope) seam + the *.snapshot event schema (idempotent on a deterministic
    event_id from (aggregate, version)); sub-artifact-granular (CI one-run, KN page-subtree at block
    granularity). Each owner implements replay(scope, since) → emits *.snapshot via the outbox; the per-owner
    replay lands with each owner in EB-26 (M3/M4) — this prompt ships the SEAM + the *.snapshot schema + a
    small reference consumer to prove cold==live.
- **CONTRACTS TO IMPLEMENT.** 2.6 reindex-from-source (owned-seam + the *.snapshot schema). To the frozen shape
  (Bus §4.9).
- **GATE / DRILLS.** BUS-D5 (reindex) → wipe a derived store, reindex(scope) → the rebuilt store byte-matches
  live (cold == live); reads the reindex-parity hash (SCHED — provable at M2 against a small derived consumer;
  the per-subsystem full reindex lands with each owner in EB-26). Threshold: cold == live (byte-match).
- **TESTS.** Unit: *.snapshot idempotency on the deterministic event_id from (aggregate, version) (a replayed
  snapshot produces one effect); the reference consumer rebuilds byte-identically from a *.snapshot replay.
  Drill scenario: BUS-D5 (reindex cold==live on a small derived consumer) on the harness, asserting against the
  reindex-parity telemetry. CDC: provider+consumer pair for 2.6. Mutation: reindex.rs is mandatory-core; state
  and meet the floor.
- **DEFINITION OF DONE.** reindex.rs compiles; 2.6 to the frozen shape; BUS-D5 emits a dated green artifact
  (cold==live on a small consumer — PROVEN); unit + drill + CDC tests pass; coverage scanner + lints green;
  the per-owner-replay note (each owner's replay → EB-26) named in writing; committed.
- **COMMIT.** Header "P-<NNN> M2: Reindex-from-source seam + *.snapshot schema (cold==live)". Body: contract
  2.6; BUS-D5 green (cold==live); per-owner replay → EB-26. Co-Authored-By trailer.

---

## EB-23 — The reactive/dispatch tier (nested causality + structural loop guards + reserve/settle)

- **BAND.** M2.
- **ROADMAP MILESTONE.** B-M2 — the reactive/dispatch tier (3.6, separately-reviewed D7). Roadmap:
  ../../06-roadmaps/shared/event-bus.md §2 "B-M2", contract 3.6.
- **DEPENDS-ON.** EB-18 (Signals — the dispatch tier consumes curated Signals), EB-21 (the firehose), EB-22
  (the reindex seam). Upstream (must be merged first): the M2 Agent EventInbox::deliver explicit-first dispatch
  (8.6) — the dispatch tier's delivery target; the M1 reserve/settle cost gate (11.7) — the dispatch cost gate.
  (The check-seam carriage is EB-24.)
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/03-agent-native-fabric.md §5.3 + §6 (the orchestrator gotchas —
    nested causality, structural loop guards, bounded dispatch, explicit-first dispatch) +
    external-insights/01-process-and-quality-doctrine.md §3 (the loop-safety drill forces the runaway and
    observability watches the breaker trip).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.7 (the reactive/dispatch tier: nested
    causality causation_id=dispatch_event.event_id depth=+1 — flat threading FORBIDDEN; structural loop guards
    — self-guard, reference gate only an artifact_ref node re-triggers never raw text, causal-depth ceiling
    default 12, shared-causal-root tripwire → per-tenant breaker; bounded dispatch pool drops over-cap;
    explicit-first dispatch CHAT-1; reserve/settle before any run; the per-surface shed budgets OQ-K, the
    agent-mention storm).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 3.6 (reactive/dispatch tier — owned),
    8.6 (EventInbox::deliver explicit-first — consumed), 11.7 (reserve/settle — consumed).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-K (shed budgets).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows BUS-D1, BUS-D3, BUS-D6.
- **DELIVERABLE.** In myelin-events:
  - dispatch.rs: the reactive/dispatch tier — consumes Signals, delivers to the Agent EventInbox (8.6).
    Disciplines: NESTED causality (a dispatched action derives causation_id = dispatch_event.event_id,
    correlation_id carried, depth = +1 — flat threading forbidden, it breaks depth-capping); STRUCTURAL loop
    guards (self-guard: drop an event whose actor.principal == the consumer's agent; reference gate: only a
    structured artifact_ref content node re-triggers, never raw text; causal-depth ceiling default 12;
    shared-causal-root tripwire: >K events on one correlation_id in a short window → per-tenant circuit
    breaker); BOUNDED dispatch worker pool that drops over-cap (a mention/event storm is bounded, never forked
    unboundedly, with a Signal); EXPLICIT-FIRST dispatch (CHAT-1: a mention notifies, does not auto-spawn a
    costed run); RESERVE/SETTLE before any run (11.7 — no balance → no execution). The tier inherits the
    per-surface shed budgets (OQ-K) — the agent-mention storm sheds the agent lane with 429 + Retry-After; the
    Bus owns the discipline, the numbers are tuned in M5 (EB-29).
- **CONTRACTS TO IMPLEMENT.** 3.6 reactive/dispatch tier (owned); 8.6, 11.7 (consumed).
- **GATE / DRILLS.** BUS-D1 → kill a consumer + sever the broker during sustained publish → 0 lost, 0
  duplicate effects on reconnect (CI). BUS-D3 (replay) → replay a correlation_id tree → deterministic,
  idempotent re-drive, causality preserved (replay == original, exactly once) (CI). BUS-D6 (F9) → a
  self-triggering automation → the depth ceiling (12) + the shared-root tripwire trip the per-tenant breaker
  (halts ≤ ceiling; breaker trips) (CI). All thresholds exact — never weaken. (The M2 hard go/no-go AG-D4 is
  owned by Agent/CI; the Bus's dispatch tier must not deliver to a run executing untrusted code before AG-D4 is
  green — honoured via the reserve/settle + the runner gate, not by the Bus.)
- **TESTS.** Unit: nested causality (a dispatched action's depth = parent+1; flat threading rejected); the
  self-guard drops the agent's own event; the reference gate re-triggers only on an artifact_ref node (raw text
  does not); the depth ceiling parks at 12; the shared-root tripwire trips the per-tenant breaker;
  explicit-first (a mention notifies, 0 auto-spawn); reserve/settle blocks a no-balance run. Drill scenarios:
  BUS-D1, BUS-D3, BUS-D6 on the harness, asserting against per-tenant in-flight + shed-counts + causal-depth
  histogram + shared-root-tripwire telemetry. CDC: provider+consumer pair for 3.6; consumer side of 8.6/11.7.
  Mutation: dispatch.rs (the loop guards) is mandatory-core; state and meet the floor.
- **DEFINITION OF DONE.** dispatch.rs compiles; 3.6 owned + 8.6/11.7 consumed; BUS-D1, BUS-D3, BUS-D6 each
  emit a dated green artifact (0 lost/0 dup; replay==original; halts ≤ ceiling + breaker trips — all PROVEN);
  unit + drill + CDC tests pass; coverage scanner + lints green; the OQ-K shed-budget numbers FLOOR (→ EB-29
  tuned) named in writing; committed.
- **COMMIT.** Header "P-<NNN> M2: Dispatch tier (causality + loop guards + reserve/settle)". Body: contract
  3.6; 8.6/11.7 consumed; BUS-D1/BUS-D3/BUS-D6 green (with measured numbers); floor named (OQ-K shed budgets →
  EB-29). Co-Authored-By trailer.

---

## EB-24 — The check-seam carriage (ci.check.updated per-aggregate ordering + the ci.result wait_for_signal substrate)

- **BAND.** M2.
- **ROADMAP MILESTONE.** B-M2 — the check-seam carriage (2.9/§4.12, the Bus's narrow role in contract 5.9).
  Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M2", contracts 5.9 (carriage) + 9.4 (consumed).
- **DEPENDS-ON.** EB-23 (the dispatch tier + the reactive surface), EB-21 (the firehose), EB-02 (the
  ci.check.updated / ci.result tokens registered). Upstream (must be merged first): the M2 myelin-flow durable
  signal (9.4) — the merge-queue ci.result wait substrate.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §7 (reconcile cross-component
    contracts at the plan layer — the X-1 check seam, the most load-bearing cross-subsystem contract; the
    Bus's role stays NARROW).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.12 (the check-seam carriage — what the Bus
    carries: ci.check.updated aggregate=(repo, commit_oid), ci.result the rollup signal; the Bus owns ONLY
    envelope conformance + per-aggregate ordering + at-least-once + the durable wait_for_signal substrate; it
    does NOT own the CheckStatus shape, run_attempt supersession, trust-tier gating, or the merge gate — all
    CI/Git).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 5.9 (the Git↔CI CheckStatus seam — the
    Bus CARRIES it, CI+Git own it), 9.4 (durable signal — consumed, the ci.result wait), 2.9 (the
    ci.check.updated / ci.result tokens, registered in EB-02).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-1 (the check seam).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row D-11 (check-seam ordering, architecture §8 D-11).
- **DELIVERABLE.** In myelin-events:
  - check_seam.rs: carry ci.check.updated (envelope subject = repo#commit-<oid>/check-<context>, aggregate =
    (repo, commit_oid) so all checks for one commit are per-aggregate ordered) and ci.result (the rollup signal
    {commit_oid, overall, contexts, idem_token} the merge-queue durable workflow waits on via
    wait_for_signal("ci.result", idem_key=<merge_attempt_id>), contract 9.4). The Bus guarantees per-aggregate
    ordering on (repo, commit_oid) + at-least-once delivery + the durable wait_for_signal substrate; it does
    NOT evaluate run_attempt supersession, trust-tier gating, or the merge gate (CI/Git own those). The
    consumer (Git's check_status projection) lands EB-26/M3; the producer (CI) lands EB-27/M4. "A shaping, not
    a new engine."
- **CONTRACTS TO IMPLEMENT.** 5.9 check-seam CARRIAGE (the Bus's narrow half — envelope + ordering +
  at-least-once + the wait_for_signal substrate; CI/Git own the rest); 9.4 (consumed).
- **GATE / DRILLS.** D-11 (check-seam ordering, X-1) → emit interleaved/late-arriving ci.check.updated for one
  (repo, commit_oid) across contexts + re-run attempts → per-aggregate ordering holds so Git's run_attempt
  supersession is well-defined; a stale lower-attempt re-delivery is droppable (aggregate order preserved;
  supersession deterministic) (CI). All thresholds exact — never weaken. (The end-to-end seam GIT-D10/CI-D8 is
  EB-27/M4; this prompt proves the Bus's ordering substrate D-11 the seam rests on.)
- **TESTS.** Unit: check-seam per-aggregate ordering on (repo, commit_oid) (interleaved/late arrivals stay
  per-aggregate ordered); the wait_for_signal("ci.result", idem_key) substrate wakes exactly once on a
  doubly-delivered ci.result. Drill scenario: D-11 on the harness, asserting against per-aggregate-publish-
  latency telemetry. CDC: the Bus's carriage half of 5.9; consumer side of 9.4.
- **DEFINITION OF DONE.** check_seam.rs compiles; the 5.9 carriage half + 9.4 consumed; D-11 emits a dated
  green artifact (aggregate order preserved + supersession deterministic — PROVEN); unit + drill + CDC tests
  pass; coverage scanner + lints green; committed. This completes B-M2.
- **COMMIT.** Header "P-<NNN> M2: Check-seam carriage (ci.check.updated ordering + ci.result wait_for_signal)".
  Body: contract 5.9 (carriage), 9.4 consumed; D-11 green (aggregate order preserved, supersession
  deterministic). Co-Authored-By trailer.

---

## EB-25 — Build the cross-cell PII-free pointer bridge (the M1-frame floor follow-on)

- **BAND.** M5.
- **ROADMAP MILESTONE.** B-M5 (world-scale hardening + floor follow-ons) — the cross-cell bridge BUILD (the
  follow-on of the EB-14 pinned frame). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M5", contract
  12.6 live.
- **DEPENDS-ON.** EB-14 (the pinned CrossCellPointer frame). Upstream (must be merged first): the M5
  control-plane multi-cell build (12.6 live) — the cross-cell bridge needs the control plane to carry the
  pointer between a tenant's cells.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/04-hard-problems.md §1 (residency: no PII crosses a cell boundary)
    + external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors: this is the named follow-on
    of the M1 frame).
  - ../../05-refined-shared-systems-architecture/event-bus.md §7.4 (cross-cell propagation — the bridge frame
    CrossCellPointer{subject, type, correlation_id, home_cell}; the control plane carries ONLY this pointer,
    never payload or PII, control-plane-pii-free lint; resolution is ALWAYS cell-local — the home cell renders
    + permission-checks, only the already-filtered projection crosses, or a tombstone).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 12.6 (the cross-cell PII-free pointer
    bridge — built live here).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-I.
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows GA-D8 / CP-D7 / CP-D8 (the FLOOR drills, owed when multi-cell ships).
- **DELIVERABLE.** In myelin-events (+ the control-plane integration the Tenancy system owns):
  - The cross-cell event propagation: the control plane carries ONLY CrossCellPointer{subject (opaque), type,
    correlation_id, home_cell} between a tenant's cells — never payload or PII (the control-plane-pii-free lint
    holds). Resolution is ALWAYS cell-local: a viewer in cell A wanting to render a pointer homed in cell B has
    cell B resolve(ref, viewer, mode) in B, permission-checked in B, returning only the already-rendered,
    already-permission-filtered projection (or a tombstone) — never raw rows, never PII that should stay in B.
  - The multi-cell fan-out for the streams the floor follow-ons ride (ISS cross-cell portfolio rollup, KN
    cross-cell collab, CHAT cross-org channels — the Bus carries the pointer events; the subsystems own their
    surfaces).
- **CONTRACTS TO IMPLEMENT.** 12.6 cross-cell PII-free pointer bridge (the Bus's event-propagation half —
  built live; the control plane owns the pointer transport).
- **GATE / DRILLS.** GA-D8 / CP-D7 / CP-D8 (the FLOOR drills, now owed) → multi-cell erasure fan-out per-cell
  receipt set; cell→cell migration 0 loss; the cross-cell ref carries ONLY the PII-free bridge (0 PII crosses)
  (SCHED). Threshold: 0 PII crosses a cell boundary; per-cell receipt set complete; 0 loss on migration. The
  control-plane-pii-free lint green on the bridge code.
- **TESTS.** Unit: the control plane rejects a pointer carrying anything beyond CrossCellPointer (no payload,
  no PII); cell-local resolution returns only the permission-filtered projection from the home cell; a
  tombstone is returned for an erased subject. Drill scenarios: GA-D8 / CP-D7 / CP-D8 on the harness,
  asserting against per-cell receipt + migration-loss + PII-crossing telemetry. CDC: the cross-cell pointer
  conformance test (provider+consumer).
- **DEFINITION OF DONE.** The cross-cell bridge is BUILT and live; 12.6 implemented; GA-D8/CP-D7/CP-D8 each
  emit a dated green artifact (0 PII crosses; per-cell receipts; 0 migration loss — PROVEN); the
  control-plane-pii-free lint green; unit + drill + CDC tests pass; coverage scanner + lints green; committed.
- **COMMIT.** Header "P-<NNN> M5: Build cross-cell PII-free pointer bridge". Body: contract 12.6 live;
  GA-D8/CP-D7/CP-D8 green (0 PII crosses, per-cell receipts, 0 migration loss); control-plane-pii-free lint
  green. Co-Authored-By trailer.

---

## EB-26 — Per-subsystem token-list validation harness + the check-seam consumer leg + per-owner replay carriage (M3)

- **BAND.** M3.
- **ROADMAP MILESTONE.** B-M3 (per-subsystem tokens — Git/KN; the check-seam consumer half; per-owner replay).
  Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M3 / B-M4", contracts 2.9 (per-subsystem M3 completion)
  + 5.9 (consumer half) + 2.6 (per-owner replay).
- **DEPENDS-ON.** EB-24 (the check-seam carriage + the per-aggregate ordering substrate), EB-21 (the firehose),
  EB-22 (the reindex seam + *.snapshot schema), EB-02 (the taxonomy grammar each subsystem's list validates
  against). Upstream (must be merged first, per band): Git's check_status projection (M3) + KN's collab
  op-streams (M3) — owned by the Git/KN subsystem prompts; this Bus prompt provides the carriage they ride.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §7 (reconcile cross-component
    contracts at the plan layer — the X-1 check seam is split producer/consumer across two bands; the Bus's
    role stays NARROW).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.12 (the check seam — the Bus's narrow role:
    the consumer projection is built from the EB-05 idempotent template) + §6.1/§6.4 (the grammar each
    subsystem's dotted-name list validates against) + §4.9 (the per-owner replay for reindex).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 2.9 (each subsystem completes its
    list), 5.9 (the Git↔CI CheckStatus seam — Git ships the consumer/projection in M3; the Bus carries), 2.6
    (per-owner replay).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows GIT-D9 / KN-D7 / KN-D1 (the Bus's per-aggregate ordering + firehose under Git/KN producers) + D-11
    (the ordering substrate the consumer leg rests on).
- **DELIVERABLE.** In myelin-events (the Bus's carriage; the subsystems own their lists/projection):
  - Provide the grammar VALIDATION HARNESS: validate each M3 subsystem's completed dotted-name event list
    (Git/KN) against the §6.1 grammar, with its schema_ver lineage and payload shapes (the Bus owns the grammar
    + seed; the subsystem owns its complete list). Each subsystem registers its list; the harness admits the
    full list + rejects a malformed addition.
  - Carry the check-seam CONSUMER leg (5.9, M3): Git ships the check_status CONSUMER/projection (built from the
    EB-05 idempotent template, idempotent on event_id, applying CI/Git's run_attempt supersession over the
    Bus's per-aggregate-ordered ci.check.updated) + awaits CI — the Bus carries it (envelope conformance,
    per-aggregate ordering on (repo, commit_oid), at-least-once delivery). The PRODUCER (CI) lands EB-27/M4.
  - Carry per-owner replay (2.6, M3 owners): Git/KN each implement replay(scope, since) so reindex-from-source
    covers them sub-artifact-granular (KN page-subtree at block granularity) — the Bus carries the *.snapshot;
    the per-owner replay is each subsystem's. The firehose's M3 producers come online (KN collab op-streams)
    over the EB-21 transport; the durable bus carries only the pointer events.
- **CONTRACTS TO IMPLEMENT.** 2.9 (the per-subsystem M3 list validation — the Bus's grammar harness); 5.9
  consumer-leg carriage (the Bus's narrow half, M3); 2.6 per-owner replay (the Bus carries the *.snapshot for
  Git/KN).
- **GATE / DRILLS.** The Bus's per-aggregate ordering holds under Git's push events (GIT-D9 — git.ref.updated
  emitted iff the ref move committed, the Bus carries it) and under KN's block commits (KN-D7 — block commit ↔
  relay-publish outbox emit-iff-committed; KN-D1 — resume(scope=doc, last_seq) loses 0 ops, re-run across the
  CRDT boundary) (CI). Thresholds: emit-iff-committed; 0 ops lost on resume.
- **TESTS.** Unit: the grammar validator admits each M3 subsystem's full list + rejects a malformed addition;
  the check_status projection is idempotent on event_id and per-aggregate ordered. Drill scenarios: GIT-D9 /
  KN-D7 / KN-D1 on the harness — the Bus's carriage half asserted against per-aggregate-publish-latency +
  dedup + firehose-seq-gap telemetry. CDC: the consumer-leg carriage half of 5.9 + the per-subsystem M3 2.9
  list conformance + 2.6 per-owner replay (Git/KN).
- **DEFINITION OF DONE.** The grammar validation harness is live + each M3 subsystem's list validated; the
  check-seam consumer leg is live (Git's projection over the Bus's per-aggregate-ordered ci.check.updated);
  GIT-D9/KN-D7/KN-D1 each emit a dated green artifact (emit-iff-committed; 0 ops lost — PROVEN); unit + drill +
  CDC tests pass; coverage scanner + lints green; the producer-leg follow-on (CI → EB-27/M4) named in writing;
  committed.
- **COMMIT.** Header "P-<NNN> M3: Token-list validation harness + check-seam consumer leg + per-owner replay".
  Body: contracts 2.9 (M3 lists), 5.9 (consumer leg), 2.6 (Git/KN replay); GIT-D9/KN-D7/KN-D1 green (with
  measured numbers); producer leg → EB-27/M4. Co-Authored-By trailer.

---

## EB-27 — The check-seam producer leg goes live end-to-end (M4) + CI/Issues/Chat token lists + their replay

- **BAND.** M4.
- **ROADMAP MILESTONE.** B-M4 (per-subsystem tokens — CI/Issues/Chat; the check seam goes live end-to-end; the
  M4 producers' replay). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M3 / B-M4", contracts 2.9
  (per-subsystem M4 completion) + 5.9 (end-to-end) + 2.6 (per-owner replay).
- **DEPENDS-ON.** EB-26 (the M3 consumer leg — the producer leg DEPENDS-ON the consumer leg per the X-1 split;
  the seam is never collapsed into one band). Upstream (must be merged first, per band): CI's check producer +
  the merge-queue durable workflow (M4) + Issues/Chat event producers — owned by the CI/Issues/Chat subsystem
  prompts; this Bus prompt provides + drills the carriage they ride.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §7 (the X-1 check seam goes
    end-to-end in M4; the Bus's role stays NARROW).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.12 (the check seam — the Bus's narrow role:
    envelope conformance, per-aggregate ordering, at-least-once, the durable wait_for_signal substrate) +
    §6.1/§6.4 (the grammar each subsystem's list validates against) + §4.9 (per-owner replay).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 2.9 (CI/Issues/Chat complete their
    lists), 5.9 (the Git↔CI CheckStatus seam end-to-end — CI producer + Git gate; the Bus carries; an
    untrusted_fork success is neutral for gating until endorsed/re-run-trusted), 9.4 (the merge-queue ci.result
    wait substrate), 2.6 (per-owner replay).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows GIT-D10 / CI-D8 (the X-1 check seam end-to-end) + D-11 (the Bus's ordering substrate it rests on) +
    CHAT-D1 / CHAT-D13 (the Bus's firehose + co-commit under Chat's load).
- **DELIVERABLE.** In myelin-events (the Bus's carriage; the subsystems own their producers/lists):
  - Validate each M4 subsystem's completed dotted-name event list (CI/Issues/Chat) against the §6.1 grammar via
    the EB-26 harness; each registers its list (admit the full list + reject a malformed addition).
  - Carry the check-seam PRODUCER leg end-to-end (5.9, M4): CI ships the PRODUCER (emits ci.check.updated per
    (commit_oid, context) + the rollup ci.result signal the merge-queue durable workflow waits on via the EB-24
    wait_for_signal substrate). With the EB-26 Git consumer already live, the seam is now END-TO-END. The Bus's
    role stays narrow: envelope conformance, per-aggregate ordering on (repo, commit_oid), at-least-once
    delivery, the durable wait_for_signal substrate. (CI's producer + Git's supersession rule are owned by the
    CI/Git subsystem prompts; this Bus prompt provides + drills the carriage.)
  - Carry per-owner replay (2.6, M4 owners): CI/Issues/Chat each implement replay(scope, since) so
    reindex-from-source covers them sub-artifact-granular (CI one-run). The firehose's heaviest producers come
    online (ci.log.appended, Chat presence/live) over the EB-21 transport; the durable bus carries only the
    pointer events.
- **CONTRACTS TO IMPLEMENT.** 2.9 (the per-subsystem M4 list validation); 5.9 end-to-end carriage (the Bus's
  narrow half, producer leg); 2.6 per-owner replay (CI/Issues/Chat); 9.4 the merge-queue wait substrate (the
  carriage the producer's ci.result lands on).
- **GATE / DRILLS.** GIT-D10 / CI-D8 (the X-1 check seam end-to-end — out-of-order/dup ci.check.updated →
  run_attempt supersession; fork self-green neutral; doubly-delivered ci.result → merge-queue wakes EXACTLY
  ONCE; 0 double-merge — the Bus's D-11 ordering guarantee is the substrate this rests on) + CHAT-D1 (sever
  gateway↔firehose → resume 0 lost/0 dup, the Bus's firehose under Chat's load) + CHAT-D13 (message-persist ↔
  event co-commit) (CI). Thresholds: 0 double-merge; 0 lost/0 dup; emit-iff-committed.
- **TESTS.** Unit: the grammar validator admits each M4 subsystem's full list + rejects a malformed addition;
  the merge-queue wakes exactly once on a doubly-delivered ci.result. Drill scenarios: GIT-D10/CI-D8/CHAT-D1/
  CHAT-D13 on the harness — the Bus's carriage half asserted against per-aggregate-publish-latency + dedup +
  firehose-seq-gap telemetry. CDC: the producer-leg carriage half of 5.9 + the per-subsystem M4 2.9 list
  conformance + 2.6 per-owner replay (CI/Issues/Chat) + consumer side of 9.4.
- **DEFINITION OF DONE.** The check seam is live END-TO-END (Git consumer M3 via EB-26, CI producer M4 here)
  with the Bus carrying it; GIT-D10/CI-D8/CHAT-D1/CHAT-D13 each emit a dated green artifact (0 double-merge;
  0 lost/0 dup; emit-iff-committed — PROVEN); each M4 subsystem's list validated; unit + drill + CDC tests
  pass; coverage scanner + lints green; the producer/consumer split kept across bands (M4 producer DEPENDS-ON
  M3 consumer); committed.
- **COMMIT.** Header "P-<NNN> M4: Check seam live end-to-end + CI/Issues/Chat token lists + their replay".
  Body: contracts 2.9 (M4 lists), 5.9 (end-to-end carriage), 2.6 (CI/Issues/Chat replay), 9.4; GIT-D10/CI-D8/
  CHAT-D1/CHAT-D13 green (with measured numbers); producer/consumer split kept across bands. Co-Authored-By
  trailer.

---

## EB-28 — Reserved (folded into EB-26/EB-27)

> NOTE. The first pass bundled M3 + M4 into one cross-band prompt (old EB-13). This pass splits it into the M3
> consumer leg (EB-26) and the M4 producer leg (EB-27) — the X-1 producer/consumer split kept across bands per
> 00-ledger-overview §3.2 step 3. No standalone EB-28 deliverable remains; this id is intentionally void so the
> later M5 ids (EB-29..EB-31) keep their declared meaning. The index assigns no P-<NNN> to a void id.

---

## EB-29 — World-scale: the 30× agent surge + per-aggregate order at QPS + crypto-shred reaches backups

- **BAND.** M5.
- **ROADMAP MILESTONE.** B-M5 — the world-scale hardening drills (the surge, the QPS-order floor follow-on,
  the backups-erasure floor follow-on). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M5", drills
  BUS-D7 / BUS-D9 / BUS-D8 (backups leg).
- **DEPENDS-ON.** EB-23 (the dispatch tier whose shed lanes BUS-D7 drills), EB-03 (the per-aggregate ordering
  BUS-D9 proves at QPS), EB-15 (the live-store crypto-shred BUS-D8 extends to backups), EB-16 (the erasure
  ledger the re-erasure leg uses), EB-27 (the check seam + all five subsystems live, the load the surge
  exercises). Upstream (must be merged first): Storage restore-verify at cell scale (STOR-D2/STOR-D4) — the
  Bus's holder participates in the cross-seam restore.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §3 (prove-it at world-scale: the
    30× surge + the QPS-order + the backups-erasure forced by drills, observability watching the human lane
    hold).
  - ../../05-refined-shared-systems-architecture/event-bus.md §2.3 (per-aggregate ordering at production QPS —
    the D-9 floor), §4.7 (the OQ-K shed budgets the surge tunes), §4.8 (crypto-shred reaching backups).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 2.3 (per-aggregate ordering at QPS),
    2.7 (crypto-shred to backups), 11.5 (the restore cross-seam), 11.7 (reserve/settle the surge sheds against).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows BUS-D7 (30× surge), BUS-D9 (per-aggregate order at QPS), BUS-D8 (erasure reaches backups) + STOR-D4.
- **DELIVERABLE.** In myelin-events:
  - Tune + prove the OQ-K shed budgets (the named M2 floor): the 30× agent-surge family — the protected
    human/control lane holds while the agent lane sheds (429 + Retry-After honoured) and other tenants are
    unaffected (the per-tenant bulkhead).
  - Prove per-aggregate ordering AT PRODUCTION QPS (the M0 correctness floor's follow-on): burst force-pushes
    to one hot ref + a burst of sends to one hot channel under load → per-ref + per-conversation order
    preserved at target QPS, parallel across aggregates.
  - Extend crypto-shred to backups (the M1 live-store floor's follow-on): inline-PII events unrecoverable in
    backups (the key excluded from backup), not just live DBs — re-confirmed with STOR-D4.
- **CONTRACTS TO IMPLEMENT.** No new contract surface — this prompt HARDENS the owned contracts (2.3 at QPS,
  2.7 to backups, 3.6 shed budgets) and implements the measured numbers the earlier prompts named as floors.
- **GATE / DRILLS.** BUS-D7 (F6) → 30× agent publish surge on one tenant → the human/control lane holds, the
  agent lane sheds (429 + Retry-After), other tenants unaffected; reads shed-counts/lane + per-tenant RED
  (SCHED). BUS-D9 → burst force-pushes to one hot ref + sends to one hot channel under load → per-aggregate
  order preserved at target QPS, parallel across aggregates; reads per-aggregate publish latency (SCHED).
  BUS-D8 (reaches-backups leg) + STOR-D4 → 0 recoverable inline-PII in the log AND backups; tombstones present
  (SCHED). STOR-D2 at cell scale re-confirmed (the Bus's holder consistent at the restored point under
  world-scale load). All thresholds exact — never weaken; a red gate is a dated "claimed, not proven" scorecard
  row.
- **TESTS.** Drill scenarios (the bulk of this prompt): BUS-D7, BUS-D9, BUS-D8-backups — all on the
  failure-injection harness at 30× load with mixed principal kinds, asserting against the §4.11 survival
  signals (shed-counts, per-tenant in-flight, per-aggregate publish latency, holder erase receipts). CDC: the
  hardened-contract conformance re-runs for 2.3 / 2.7.
- **DEFINITION OF DONE.** BUS-D7, BUS-D9, BUS-D8-backups each emit a dated green artifact (human lane holds +
  agent sheds + cross-tenant 0; QPS-order preserved; 0 recoverable inline-PII in backups — all PROVEN);
  STOR-D2 at cell scale re-confirmed; coverage scanner + lints green; committed.
- **COMMIT.** Header "P-<NNN> M5: Bus world-scale hardening (30× surge / QPS-order / backups-erasure)". Body:
  BUS-D7/BUS-D9/BUS-D8-backups green (with measured numbers); STOR-D2 cell-scale re-confirmed. Co-Authored-By
  trailer.

---

## EB-30 — Tune the firehose retention window + re-green D-10 across the KN CAS→CRDT engine_promote boundary

- **BAND.** M5.
- **ROADMAP MILESTONE.** B-M5 — the firehose retention-window floor follow-on (measured here) + the
  CRDT-boundary D-10 re-run. Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M5", drill D-10-at-scale +
  the retention-window floor.
- **DEPENDS-ON.** EB-21 (the firehose whose retention window D-10 tunes). Upstream (must be merged first): the
  M5 KN CRDT (the engine_promote boundary D-10 re-runs across) — owned by the Knowledge M5 prompts.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/04-hard-problems.md §2.2 (the CRDT slots into the resume-cursor
    transport — D-10 re-runs across the engine_promote boundary) + external-insights/01-process-and-quality-
    doctrine.md §1 (name-your-floors: the retention window measured here, the floor's promotion itself drilled).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.3 (the retention-window sizing the D-10 drill
    measures — too short forces expensive resync_required, too long costs storage; the window must exceed the
    p99 reconnect gap).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 3.5 (the firehose retention window).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row D-10 (firehose reconnect, re-run across the engine_promote boundary).
- **DELIVERABLE.** In myelin-events:
  - Tune the firehose retention window per stream class (the named M2 floor → measured here): so the
    (last_seq, now] backfill exceeds the p99 reconnect gap per stream class (CI log vs collab op vs chat live)
    — D-10 measures it; record the measured window per stream class in the thresholds file.
  - Re-run D-10 GREEN across the KN CAS→CRDT engine_promote boundary (the floor's promotion is itself drilled):
    the resume-cursor transport survives the CRDT promotion (0 ops lost) — the transport is unchanged; the
    drill proves the floor's promotion does not regress the zero-ops-lost property.
- **CONTRACTS TO IMPLEMENT.** No new contract surface — this prompt HARDENS the owned contract 3.5 (retention
  window) and implements the measured numbers EB-21 named as the retention-window floor.
- **GATE / DRILLS.** D-10 RE-GREEN across the KN CAS→CRDT engine_promote boundary → the resume-cursor transport
  survives the CRDT promotion (0 ops lost) (CI/SCHED). Threshold: 0 ops lost across the engine_promote; the
  per-stream-class retention window measured > p99 reconnect gap (asserted from measured data). All thresholds
  exact — never weaken.
- **TESTS.** Drill scenario: D-10-across-the-CRDT-boundary on the failure-injection harness, asserting against
  the firehose-seq-gap + resync-count telemetry. Unit: the retention window > p99 reconnect gap is asserted
  from measured data (per stream class). CDC: the 3.5 conformance re-run across the boundary.
- **DEFINITION OF DONE.** D-10-across-the-CRDT-boundary emits a dated green artifact (0 ops lost across the
  engine_promote — PROVEN); the retention window is measured + tuned per stream class and recorded in the
  thresholds file; coverage scanner + lints green; committed.
- **COMMIT.** Header "P-<NNN> M5: Firehose retention-window tuning + D-10 re-green across the CRDT boundary".
  Body: D-10-across-boundary green (0 ops lost across engine_promote); retention window measured + tuned per
  stream class. Co-Authored-By trailer.

---

## EB-31 — The Bus as the E2E spine + the column-store seam measurement gate

- **BAND.** M5.
- **ROADMAP MILESTONE.** B-M5 — the Bus as the spine of all four E2E scenarios + the column-store/time-series
  seam named (measured, not built). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M5", the four E2E
  scenarios + BUS-6.
- **DEPENDS-ON.** EB-27 (the check seam + all five subsystems live, the spine the E2E ride), EB-29 (the surge +
  erasure hardening E2E-2/E2E-4 lean on), EB-22 (the reindex-from-cold path E2E-3 rides), EB-04 (the
  BusTransport trait the column-store seam promotes behind).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §3 (prove-it at world-scale — the
    Bus is the spine each E2E rides; observability watches each chained mutation) +
    external-insights/04-hard-problems.md §5.2 (the column-store seam — measured-not-predicted).
  - ../../05-refined-shared-systems-architecture/event-bus.md §7.5 (the column-store/time-series seam BUS-6 —
    promoted ONLY on measured volume, behind the BusTransport trait).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    the four E2E scenarios (E2E-1..E2E-4, the Bus is the spine each rides).
- **DELIVERABLE.** In myelin-events:
  - The Bus as the spine of all four E2E scenarios (it carries every chained mutation): E2E-1 (the PR pane's
    live check-update + the per-ref cache bust), E2E-2 (the flagship — the Signal that wakes the triage agent,
    the durable ci.result wait, the nested-causality run), E2E-3 (reindex-from-cold == live via the *.snapshot
    path), E2E-4 (the bus as a holder in the DSAR fan-out — inline-PII events crypto-shredded). The subsystems
    own the E2E orchestration; the Bus proves its carriage holds under each.
  - The column-store/time-series seam (BUS-6): NAMED, not built — promotion to a ClickHouse-class tier behind
    the BusTransport trait happens ONLY if a per-stream volume is MEASURED to outgrow JetStream at degraded
    latency (post-M5, measured-not-predicted). This prompt records the measurement gate (the per-stream volume
    threshold + the degraded-latency criterion in the thresholds file), not a build.
- **CONTRACTS TO IMPLEMENT.** No new contract surface — this prompt proves the E2E spine over the already-owned
  contracts and records the BUS-6 measurement gate (behind the EB-04 BusTransport trait).
- **GATE / DRILLS.** The four E2E scenarios green (E2E-1..E2E-4) — the Bus is the spine each rides; each emits
  its named green artifact (SCHED). Threshold: each E2E green (the Bus's carriage holds under each chained
  mutation). The column-store promotion gate is RECORDED (measured, not predicted) — no build is owed until a
  per-stream volume is measured to outgrow the tier.
- **TESTS.** Drill scenarios: the Bus's slice of E2E-1..E2E-4 on the failure-injection harness at world-scale
  load with mixed principal kinds, asserting against the §4.11 survival signals (per-aggregate publish latency,
  causal-depth, holder erase receipts, firehose seq gaps). Unit: the column-store promotion gate is recorded
  (the measurement criterion, not a build). CDC: the E2E-spine carriage conformance re-runs.
- **DEFINITION OF DONE.** E2E-1..E2E-4 each emit a dated green artifact (the four E2E green; the Bus's carriage
  holds under each — PROVEN); the column-store seam is NAMED with its measured promotion gate recorded in the
  thresholds file (not built); coverage scanner + lints green; committed. This completes B-M5; the Bus's M6
  contribution (the self-hosting CI graph green + the truth-up pass confirming 0 red earlier-band Bus gates) is
  exercised by the master M6 dogfood prompts — the Bus adds no new engine in M6.
- **COMMIT.** Header "P-<NNN> M5: Bus E2E spine + column-store seam measurement gate". Body: E2E-1..E2E-4 green
  (the Bus's carriage holds under each); column-store seam named (measured promotion gate recorded, not built).
  Co-Authored-By trailer.

---

## Digest (this file)

31 prompts (EB-01..EB-31, with EB-28 a void id kept for ordinal stability) operationalize the entire
event-bus roadmap at single-deliverable granularity — every milestone B-M0..B-M6 covered with no gap, floors
paired to their follow-ons. Prompt count: 14 (first pass) → 31 (this pass; 30 live + 1 void). By band:
- **M0 (11):** EB-01 envelope freeze · EB-02 taxonomy grammar+tokens · EB-03 outbox+emit · EB-04 relay+transport (SUB-D1/BUS-D4) · EB-05 consumer template (SUB-D2/BUS-D2) · EB-06 dedup ledger · EB-07 no-raw-publish lint · EB-08 no-cross-sync-cycle lint · EB-09 tenant-predicate-on-streams lint · EB-10 upcaster seam · EB-11 survival signals + harness self-test. Together they green the M0→M1 exit gate.
- **M1 (5):** EB-12 (tenant,region) partition · EB-13 residency-pin (CP-D3) · EB-14 cross-cell frame pinned · EB-15 PersonalDataHolder + crypto-shred to KMS (BUS-D8 live leg) · EB-16 erasure-ledger re-erasure hook (STOR-D1/D2 leg).
- **M2 (8):** EB-17 EventMatcher = frozen QueryAst · EB-18 Signal curation · EB-19 Automations · EB-20 Triggers · EB-21 firehose resume-cursor (built FIRST, D-10) · EB-22 reindex seam (BUS-D5) · EB-23 dispatch tier (BUS-D1/BUS-D3/BUS-D6) · EB-24 check-seam carriage (D-11).
- **M3 (1):** EB-26 token-list validation harness + check-seam consumer leg + per-owner replay (GIT-D9/KN-D7/KN-D1).
- **M4 (1):** EB-27 check seam live end-to-end + CI/Issues/Chat token lists + replay (GIT-D10/CI-D8/CHAT-D1/CHAT-D13).
- **M5 (4):** EB-25 cross-cell PII-free bridge BUILD (GA-D8/CP-D7/CP-D8) · EB-29 30× surge BUS-D7 / per-aggregate order at QPS BUS-D9 / crypto-shred to backups BUS-D8 · EB-30 retention-window tuning + CRDT-boundary D-10 re-green · EB-31 the four E2E spine + the column-store seam measurement gate.
- **M6:** no new Bus engine; the Bus's M6 gate (self-hosting CI graph green + truth-up pass) is exercised by the master dogfood prompts.

Drill coverage (unchanged from the first pass — preserved at finer granularity): SUB-D1, SUB-D2, BUS-D1..BUS-D9,
D-10, D-11 — all 13 owed Bus drills are greened by a named prompt's GATE/DRILLS field; the permanent
STOR-D1/D2 restore-verify cross-seam (the Bus's holder is the event-log-offset cursor) is participated in by
EB-04/EB-15/EB-16 and re-confirmed at cell scale by EB-29. Floor coverage (every first-pass floor preserved):
QPS-order (EB-03→EB-29), column-store seam (EB-04→EB-31 named), crypto-shred-to-backups (EB-15→EB-29),
resume-cursor-transport + CRDT-boundary (EB-21→EB-30), retention-window (EB-21→EB-30), cross-cell
(EB-14→EB-25), erasure-vs-immutability structural + [OPEN — LEGAL] residual (EB-15, residual by reference X-7).
