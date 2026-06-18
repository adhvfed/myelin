# Doctrine Integration Analysis — `02-platform-substrate.md`

> Phase: `02b-doctrine-integration`. Source doctrine:
> [`external-insights/02-platform-substrate.md`](../../../external-insights/02-platform-substrate.md)
> (treated as DEFAULT per [`external-insights/README.md`](../../../external-insights/README.md)).
> Integrated against: Phase-2 [`architecture-decisions.md`](../../02-holistic-architecture/architecture-decisions.md)
> (ADR-01…ADR-15), [`shared-systems-overview.md`](../../02-holistic-architecture/shared-systems-overview.md),
> and Phase-1 [`technical-structuring.md`](../../01-research/technical-structuring.md).
> Canonical brief: [`VISION.md`](../../../VISION.md).

## 0. Verdict in one paragraph

This is the **heaviest-overlap doctrine doc** and it overwhelmingly **CONFIRMS the Phase-2
spine** — tenant-first partitioning (ADR-11), one polymorphic principal for humans+agents
(ADR-03/08/13), the event log as nervous system with the transactional outbox (ADR-04/13),
causality as a first-class envelope primitive (ADR-13.2), one canonical reference graph with
URN addressing (ADR-13.1), minimal justified storage (ADR-10/14), and crypto-shred /
references-not-payloads (ADR-12). Our spine was built from the same prior art, so most of the
doc is **validation, not work**. The real energy goes into a handful of **SHARPENS** and one
**RESOLVES-OPEN**, almost all of which bind in **Phase 3 (shared systems)** as a *design
discipline* the spine already permits but did not mandate sharply enough, plus three items that
bind in **Phase 5 (testing)** and **Phase 8 (execution discipline)** as drills/gates. There are
**no genuine CONFLICTS**.

The deltas worth real attention, named up front:

1. **§5 — Backpressure with a PROTECTED INTERACTIVE-HUMAN LANE and an explicit shed order**
   (speculative → batch/CI → agent → human-last). We have *agent load governance* (ADR-08.6) but
   no committed human-priority lane or shed ordering. SHARPENS → Phase 3 (Bus + the shared
   service-client) + Phase 5 (the 30× surge drill).
2. **§10 — fail-STATIC (bounded-staleness cache) vs fail-closed for AVAILABILITY** on the
   identity hot path. We commit fail-closed for authz correctness but never distinguish the
   availability axis. This is the single highest blast-radius gap. SHARPENS → Phase 2 back-patch
   (ADR-03 / ADR-11) + Phase 3 (Id) + Phase 5.
3. **§11 — backup-restore-VERIFICATION as a CI gate, with cross-seam (row↔blob) integrity.**
   ADR-12 lists backups as a holder and mentions post-restore re-erasure, but never gates
   *restorability* or row↔blob consistency. NEW → Phase 5 (testing) + Phase 8 (CI gate).
4. **§7 — lifecycle edges MIRRORED to a typed relation table owned by the authoritative
   service** (typed edge is source of truth; backlinks stay event-sourced projections). This is
   a concrete **DEFAULT-TO-BEAT for our open question TE-7.** RESOLVES-OPEN → Phase 3 (Refs) +
   Phase 4 (Issues/Knowledge).
5. **§8 — recursive-CTEs-over-a-graph-DB; forward-only / expand→backfill→contract online
   migrations; measure-lock-against-restore-first; defer sharding until measured.** ADR-10 has
   the storage *tiers* but not the migration/no-graph-DB/lock disciplines. SHARPENS/NEW →
   Phase 2 back-patch (ADR-10 augmentation) + Phase 3 + Phase 8.
6. **§3 — a JetStream-class durable streaming log with durable pull consumers as the SETTLED
   bus building block.** A stronger prior than ADR-04's even-handed Kafka/JetStream/PG-outbox
   list. SHARPENS (narrows the P3 selection prior) → Phase 3 (Bus).
7. **§9 — three-surface service topology (public gateway / internal RPC / metrics-health) as a
   SECURITY boundary, and liveness≠readiness.** We have the gateway (ADR-08/ID §1.4) but never
   committed the public/internal split as a security boundary or the liveness/readiness rule.
   NEW → Phase 2 back-patch (small ADR or ADR-01 augmentation) + Phase 3.
8. **§4 — "the outbox is the ONLY sanctioned emit path; no fire-and-forget path to regress to."**
   ADR-04.3 makes the outbox the default; the doctrine hardens it to *the only* path. SHARPENS →
   Phase 3 (`myelin-events`) + Phase 8 (lint).

---

## 1. Integration table (every principle in the doc)

Legend for **Binds**: P2bp = Phase-2 back-patch (new/amended ADR); P3 = Phase 3 shared system
(named); P4 = Phase 4 subsystem; P5 = testing; P8 = execution discipline; VISION = canonical.

| # | Doctrine item (§) | Class | Maps to / sharpens | Integration ACTION & WHERE IT BINDS |
|---|---|---|---|---|
| 1 | **§1 Tenant is the unit of everything** — tenant id first column of every table, partition key of every stream/index/cache | CONFIRMS | ADR-11.2 ("every record carries `tenant`+`region` as part of the partitioning key"), shared-systems-overview §0 property 2 | None. Validation. |
| 2 | **§1 No cross-tenant query path; trust token's tenant, never the URL path; cross-tenant access is an IDOR; drill it** | SHARPENS | ADR-11.3 (isolation across all shared systems) commits the *property*; "never trust the URL-path tenant, only the verified token" and "make it a drilled IDOR class" are not stated | (a) Add the **token-not-path tenancy rule** as an explicit invariant. → **P2bp** (one line into ADR-11.3 or ADR-13.3). (b) Make **cross-tenant leakage a named drill**. → **P5** (testing) and **P8** (the no-cross-tenant-predicate lint joins ADR-01's no-cross-DB lint). |
| 3 | **§2 One polymorphic principal for humans+agents; authority in one place; everyone else asks** | CONFIRMS | ADR-03 (one policy engine), ADR-08.1 (agents are `Principal`s), ADR-13.3 (one `Principal`, no subsystem implements auth) | None. Strong validation of our defining seam. |
| 4 | **§2 Per-run, narrowly-scoped, short-lived, auto-revoked agent identity (token life = run life)** | CONFIRMS (mild SHARPEN) | ADR-08 (least-privilege per-run permissions; scoped agent tokens, Id §1.1) | Already committed; the "token TTL == run TTL, teardown revokes" phrasing is a useful **precision** for the **P3 delegation/token-lifecycle design (AG-2)**. → **P3 (Id)**, no new decision. |
| 5 | **§3 Sync calls for queries / async events for reactions; one immutable event per state change; event schema is the integration contract** | CONFIRMS | ADR-04 (semantics), ADR-13.2 (envelope is the contract), tech-structuring §2.2 | None. Validation. |
| 6 | **§3 No circular synchronous dependencies; clear dependency root (identity depends on nothing)** | SHARPENS | Implicit in ADR-13 (one-direction auth) but **never stated as a rule** | Adopt **"no circular sync deps; reactions cross the bus; identity is the dependency root"** as an explicit architectural invariant. → **P2bp** (augment ADR-04 §Decision or ADR-13) + enforce in **P3** service-dependency review. |
| 7 | **§3 Per-subject ordering only; co-locate causally-related events on one subject; no global order** | CONFIRMS | ADR-04.2 (per-aggregate ordering; global ordering explicitly NOT required) | None. Exact match. "Design causally-related events to share a subject" is a good **authoring guideline** for the P3 event-taxonomy work (TE-10). |
| 8 | **§3 Settled building block = JetStream-class durable streaming log with durable PULL consumers + consumer groups (not fire-and-forget pub/sub)** | SHARPENS | ADR-04 §Candidate tech lists Kafka/Redpanda **or** JetStream **or** PG-outbox even-handedly | **Narrow the P3 selection prior**: the doctrine settles on *a durable streaming log with durable pull consumers* as the class, treating non-durable pub/sub as wrong. Record this as a **DEFAULT-TO-BEAT for the Bus transport choice** (JetStream-class is the reference; PG-outbox only acceptable if it provides the same durable-pull/consumer-group semantics). → **P3 (Bus)**, sharpening ADR-04's [OPEN → P3] transport selection. Does not foreclose Kafka/Redpanda (same class). |
| 9 | **§4 Transactional outbox is the only sanctioned emit; dual-write hazard; `FOR UPDATE SKIP LOCKED` relay; stable msg id for dedup; dead-letter after bounded retries** | CONFIRMS (with SHARPEN on mechanics) | ADR-04.3 (outbox by default), `myelin-events` outbox helper (ADR-01), tech-structuring §2.2 | Mechanism is committed. The **concrete relay recipe** (`SKIP LOCKED` claim across replicas, stable message id, bounded-retry dead-letter) is a **DEFAULT-TO-BEAT for the P3 `myelin-events` relay design**. → **P3 (Bus / `myelin-events`)**. |
| 10 | **§4 Make outbox+durable-consumer+idempotent-handler the ONLY pattern — no non-durable path to regress to** | SHARPENS | ADR-04.3 says outbox "by default"; ADR-04.1 makes idempotency platform-wide | Harden "default" → **"only path; no fire-and-forget API exists in `myelin-events`"** (a fire-and-forget shortcut that exists will be used and lose data). → **P2bp** (tighten ADR-04.3 wording) + **P3** (`myelin-events` exposes no non-durable emit) + **P8** (lint: no direct bus-publish outside the outbox helper). |
| 11 | **§4 At-least-once + idempotent + deterministic dedup ≈ effectively-once; do not chase true exactly-once** | CONFIRMS | ADR-04.1 (at-least-once + idempotent; exactly-once *effect* via `event_id` dedup), ADR-04 §Rationale | None. Verbatim agreement. |
| 12 | **§5 Backpressure mandatory everywhere; every queue bounded; bound prefetch, DB pools (fast-fail + statement timeouts), per-tenant in-flight** | SHARPENS | ADR-08.6 governs *agent* load; ADR-11.5 pushes heavy work async — but **"every queue bounded, unbounded anything is a future cascade" is not a committed platform rule** | Adopt **"every queue/pool bounded; fast-fail on saturation; statement timeouts"** as a platform-wide substrate rule, not just an agent-fabric concern. → **P2bp** (new short ADR "Backpressure & overload" or augment ADR-04) + **P3** (the shared service-client / `myelin-events` consumer template carry bounded prefetch + pool limits). |
| 13 | **§5 Principal-aware limiter; PROTECTED LANE for interactive humans; shed order speculative→batch/CI→agent→human-last; agents get 429+Retry-After; our own clients must honour it** | SHARPENS (the headline delta) | ADR-08.6 has budgets/loop-caps/per-tenant breakers for agents; **no human-priority lane, no shed ordering, no Retry-After-honouring contract on our own CLI/agent runtime** | This is the **#1 delta of this doc.** Commit: (a) a **protected interactive-human lane** and the **explicit shed order**; (b) agents/CI get `429 + Retry-After`; (c) **our own clients (CLI, agent runtime) MUST honour Retry-After** or shedding becomes a retry storm. → **P2bp** (the backpressure ADR from #12 states the lane + shed order as policy) + **P3 (Bus + Id rate-limiting + the shared outbound client)** for mechanism + **P5** for the drill (item #14). |
| 14 | **§5 Shared outbound client: per-call timeout, circuit breaker, bounded-concurrency bulkhead, jittered retry for idempotent calls only, never retry through a tripped breaker. Headline drill: 30× agent surge, human lane holds, agent lane sheds, other tenants unaffected** | SHARPENS / NEW | tech-structuring mentions resilience loosely; **no committed shared resilient-client crate, no surge drill** | (a) Add a **shared resilient inter-service client** (timeout/breaker/bulkhead/jittered-retry-idempotent-only) as a named substrate artifact — a sibling of the `myelin-*` glue crates. → **P2bp** (name it in ADR-01's crate table; e.g. `myelin-rpc`/`myelin-client`) + **P3** (build). (b) Adopt the **30× agent-surge drill** as a required scenario. → **P5 (testing)**. |
| 15 | **§6 Causality carried on every event: caused-by (human action/session) + (correlation/root, causation/parent, depth); derive child provenance from cause (correct by construction); propagate in headers with trace context** | CONFIRMS (with SHARPEN) | ADR-13.2 envelope has `causation_id`+`correlation_id`; ADR-08.6 uses `causation_id` depth caps | Envelope fields match. The doctrine **sharpens two things**: (i) the explicit **`caused-by` human-action/session reference** distinct from event-parent, and (ii) **provenance derived *from the cause* (root carries, parent=cause, depth+1) — correct by construction**, plus **header propagation alongside trace context**. → **P2bp** (note the derivation rule + `caused-by`/session field in ADR-13.2 envelope) + **P3 (Bus / `myelin-events`)** for the propagation + derivation helper. |
| 16 | **§6 Loop safety is STRUCTURAL — guard reads platform causality, so no one can typo into a loop (unlike a commit-message convention); depth ceiling + tripwire (N runs share a causal root in a window → trip breaker)** | CONFIRMS (with SHARPEN) | ADR-08.6 (`causation_id` depth caps + cycle detection + per-tenant breakers) | The mechanism is committed. The doctrine adds the **"shared-causal-root within a window → tripwire"** heuristic as a concrete second guard beyond depth caps — a **DEFAULT-TO-BEAT for AG-4** (adversarial loop validation). → **P3 (Agent Fabric)** + **P5** (AG-4 adversarial drill already routed to P5). |
| 17 | **§7 Everything addressable gets a canonical stable URN `scheme://tenant/scope/type/id[/sub]`; one shared library owns parse/format/resolve; services must not re-implement** | CONFIRMS | ADR-13.1 (`ArtifactRef` = `myelin://<tenant>/<subsystem>/<type>/<id>[#sub]`, owned by `myelin-refs`), down to sub-artifact granularity | None. Exact match (our `scope` == `subsystem`). Validation of the moat. |
| 18 | **§7 Cross-boundary/persisted links travel & store as URN; reject ambiguity (short hashes, scope-less names), never guess scope; human display keys (`#42`, `@alice`, `~general`) derived at render, never stored** | SHARPENS | ADR-13.1 commits URN addressing; **"reject ambiguity / never guess scope" and "display keys derived at render, never stored" are not stated** | Adopt as explicit rules: **(a) persisted cross-entity links are URNs, never display keys; (b) the URN library rejects scope-less/ambiguous refs; (c) `#42`/`@alice`/`~general` are render-time projections.** → **P2bp** (augment ADR-13.1) + **P3 (Refs / `myelin-refs`)** for enforcement. (Note: relates to TE-14 human-readable monotonic keys — those keys are *display*, the URN is *storage*.) |
| 19 | **§7 Backlinks are event-sourced projections (rebuild from log); BUT lifecycle-affecting links (closes/blocks/depends/assigns) ALSO mirrored to a typed relation table owned by the authoritative service — the typed edge, not the URN text, is source of truth** | **RESOLVES-OPEN (TE-7)** | TE-7 is *open*: "Does Refs own issue hierarchy/relations… or do subsystems keep local materialised structures projected into Refs?" ADR-13 §Deferred + Refs §3.3 carry it forward | **DEFAULT-TO-BEAT for TE-7:** *backlinks stay event-sourced projections in Refs; lifecycle/semantic edges are mirrored into a typed relation table owned by the authoritative subsystem, and that typed edge is the source of truth (not the URN string).* This is the hybrid both halves of TE-7 wanted — Refs keeps the universal projection; the owning service keeps the authoritative typed edge for rollups/consistency. → **P3 (Refs)** hands this default to **P4 (Issues + Knowledge, joint)** which own the typed relation tables (ties to ADR-06's relation field type). Record in the Refs §3.3 / ADR-13 §Deferred resolution. |
| 20 | **§7 The graph that cascades one action across six subsystems IS the graph a human traverses in four keystrokes — the core moat no bolt-on can copy** | CONFIRMS | VISION §1 (connective tissue), ADR-13 §Rationale (the wedge), Refs §3 | None. Validation of the thesis. |
| 21 | **§8 Keep the storage stack minimal; every engine earns its place; Postgres one-DB-per-service (no shared tables/cross-service joins), S3-compatible object store, cache/coordination store that is NEVER a source of truth** | CONFIRMS | ADR-10 tiers (Postgres OLTP / S3-compatible object / log-firehose), ADR-01 "no subsystem reaches another's DB", ADR-13 | Match. **One precision to fold in:** the **cache/coordination store (Redis/Valkey) is never a source of truth** — implied but worth stating. → **P2bp** (one line into ADR-10) — currently ADR-10 has no explicit cache tier rule. |
| 22 | **§8 Keep SQL visible (thin query layer over heavy ORM — "an ORM hides the data model you most need to see")** | NEW | Not addressed in any ADR (Phase 2 stayed above ORM-vs-query-layer altitude) | Adopt **"thin, visible SQL over a heavy ORM"** as a default substrate discipline. → **P3** (shared bootstrap/data-access convention) + **P8** (a build-time default for service skeletons). Low-controversy; record as guidance, not a hard ADR. |
| 23 | **§8 Prefer recursive CTEs over a dedicated graph DB for the shallow graphs this domain produces (a graph DB buys dual-write sync pain + fragile cross-store transactions)** | SHARPENS / RESOLVES-OPEN-adjacent | ADR-14 Refs row says "edge/back-index store (PG **or graph-index**)" — leaves a graph DB open | **Narrow the prior:** default to **Postgres + recursive CTEs** for Refs' shallow graphs; a dedicated graph DB must beat this default with a measured reason (it adds dual-write/cross-store-txn pain — exactly the hazard §4 warns about). Reinforces #19 (typed table in PG). → **P2bp** (sharpen ADR-14 Refs row) + **P3 (Refs)**. |
| 23b | **§8 Content-addressing (hash-on-write) behind a narrow `put/get/head/delete` trait so filesystem-vs-object-store is a one-line swap** | CONFIRMS | ADR-10 object tier "content-addressed, dedup"; tech-structuring §2.7 | None substantive. The **narrow `put/get/head/delete` blob trait** is a useful concrete shape for the **P3 Storage** object-tier client (and aligns with ADR-12.8 swappable-adapter mandate). → **P3 (Storage)** as a design note. |
| 24 | **§8 Migrations forward-only and online: no rollback migrations (can't un-delete data); expand→backfill→contract; never a blocking `ALTER` on a hot table; measure lock time against a restore first** | NEW | No ADR addresses migration discipline (Phase 2 stayed above it) | Adopt **forward-only / expand→backfill→contract / measure-lock-against-restore-first** as platform migration law. → **P2bp** (augment ADR-10 with a migrations clause, or a short standalone ADR) + **P3** (the shared bootstrap harness wires migrations this way) + **P8** (execution discipline: every schema change is online + forward-only; CI checks for blocking ALTERs). High value, currently a silent gap. |
| 25 | **§8 Defer sharding/partitioning until a hot table is MEASURED, not predicted; read replicas + connection pooling first; a dedicated replica for the auth hot path is usually the first real need** | SHARPENS | ADR-10 "Distributed-SQL only where a single shard outgrows Postgres"; Id §1.6 (authz is highest-QPS) — direction matches but **"measure first, replica for auth first" not stated** | Adopt **"measure before you shard; read-replicas + pooling first; the authn/authz hot path is the likely first replica."** → **P2bp** (sharpen ADR-10) + **P3 (Storage + Id)**. Aligns with ADR-10's existing anti-premature-sharding lean — this names the *first* concrete need. |
| 26 | **§9 Shared bootstrap harness wires config/DB/migrations/event-publisher/telemetry/standard ports in one call; a new service is a thin shell over identical plumbing** | NEW | ADR-01 establishes shared glue *crates* but **no service-bootstrap harness**; "many services maintainable, not many snowflakes" is a new framing | Adopt a **shared service bootstrap harness** (a `myelin-service`/`myelin-bootstrap` crate) wiring config, DB+migrations, the outbox publisher, telemetry, health, and the three ports. → **P2bp** (add to ADR-01's crate table) + **P3** (build) + **P6** (roadmap sequencing: the harness is an early platform-capability deliverable). Strong rework-avoidance lever. |
| 27 | **§9 Three-surface topology per service: public API (only through a thin stateless gateway that authenticates + injects trusted identity headers), internal RPC (inside trust boundary only), metrics/health. The public/internal split is a SECURITY boundary** | NEW | The auth gateway exists (Agent Fabric / Id §1.4 inject trusted identity), but **the three-surface split as a SECURITY boundary is not a committed topology** | Commit the **three-surface service topology** with the **public/internal split as a security boundary** (gateway authenticates + injects identity headers; internal RPC is trust-boundary-only; metrics/health separate). → **P2bp** (short new ADR "Service topology & trust boundary", or augment ADR-01 §Consequences) + **P3** (the bootstrap harness #26 instantiates the three ports). |
| 28 | **§9 Health checks distinguish LIVENESS from READINESS; a service whose critical dependency is dead reports NOT READY and stops taking traffic** | NEW | Not addressed in any ADR | Adopt **liveness≠readiness** as a platform rule: dead critical dependency → not-ready → shed traffic (do not report healthy and keep failing). → **P2bp** (with #27) + **P3** (bootstrap harness exposes both) + **P8** (operational discipline). Interacts with #29 fail-static: readiness gates traffic, fail-static keeps *already-authenticated* traffic alive. |
| 29 | **§10 Blast-radius first: ask "what else dies when it does, and for whom?"; name every stateful component + give it a shared-state/sharding plan; everything else stateless & replaceable** | CONFIRMS (with SHARPEN) | ADR-11.1 (cell = blast-radius unit), ADR-10 (stateful tiers named), ADR-11.5 (stateless front doors) | Property is committed at the *cell* level. The doctrine adds a **per-component discipline**: *enumerate every stateful component and give each an explicit shared-state/sharding plan; everything else must be stateless.* → **P3** (a required "stateful-component register + blast-radius note" per shared system) + **P4** (per subsystem). A design-discipline sharpen, not a new decision. |
| 30 | **§10 Distinguish fail-CLOSED from fail-STATIC. Fail-closed is correct for AUTHORIZATION (deny when unsure). It is the WRONG availability default — if every request fails closed on Id, one shared dep takes the whole platform down. For availability, fail STATIC: serve a bounded-staleness cached answer (e.g. "actor still active") so authenticated traffic keeps working, staleness bounded by the deprovision SLA** | **SHARPENS (the highest blast-radius delta)** | ADR-03 fail-closed for authz correctness is implied; **the availability axis (fail-static, bounded-staleness identity cache) is entirely absent.** Id §1.6 + ADR-11 §Consequences worry about cross-region but not about Id-as-single-point-cascade | This is the **single highest blast-radius gap.** Commit the **fail-closed (authz correctness) vs fail-static (availability)** distinction: Id serves a **bounded-staleness cached "actor active / coarse grants" answer** during an Id-dependency hiccup so already-authenticated traffic survives; the staleness window is bounded by the **deprovision/revocation SLA** (and reconciles with §2.4's short-lived agent tokens — a revoked agent must still fall inside the window). → **P2bp** (new clause in ADR-03 *and* ADR-11, since it is both an authz-semantics and a blast-radius decision) + **P3 (Id)** (the bounded-staleness cache + consistency-token interplay with "zookies") + **P5** ("Id hiccups, platform stays up for authenticated traffic" drill). Tension to flag: fail-static vs GDPR-revocation latency — the staleness bound must be ≤ the revocation SLA; this is a deliberate, written trade-off. |
| 31 | **§11 Continuous log archiving + periodic base backups for a tight RPO; AUTOMATED periodic restore-VERIFICATION that rebuilds and asserts no loss** | NEW | ADR-12.1 lists backups as a `PersonalDataHolder`; Storage §7.5 has `backup restore` + post-restore re-erasure — but **restorability is never gated or verified** | Adopt **automated restore-verification** (rebuild + assert no loss) as a durability gate, not a hope. → **P5 (testing strategy owns it)** + **P8 (wire restore-verify into CI so durability is gated continuously)** + **P3 (Storage)** provides the mechanism. The "a backup that has never been restored is not a backup" rule is net-new platform law. |
| 32 | **§11 Restore must be CONSISTENT across services AND blobs — a restore resurrecting a row that points at a missing blob is silent corruption; assert cross-seam (row↔blob) integrity in the drill** | NEW | Not addressed. ADR-10/12 keep OLTP and object tiers separate with no committed cross-tier restore-consistency assertion | Adopt **cross-seam restore integrity** (OLTP rows ↔ object/blob ↔ search index ↔ event-log offsets must restore to a mutually consistent point). → **P5** (the restore drill asserts row↔blob↔index integrity) + **P3 (Storage + GDPR/Audit)** (define the cross-tier consistency point; interacts with post-restore re-erasure GD-14). |
| 33 | **§11 Wire restore-verify into CI so durability is gated continuously, not just hoped for** | NEW | — | Folded into #31. → **P8 (CI gate)** + **P5**. |
| 34 | **(README) The honesty rule / "name your floors" — partial/untested/deferred must be written down; untested-but-named is fine, silent skipping is the failure** | CONFIRMS | VISION §3 ("Honesty about uncertainty"), §6 (open questions listed); pervasive in `01-process-and-quality-doctrine.md` | None here (this binds primarily through the process doc `01`). Reinforces VISION §3. |

---

## 2. Where the deltas bind — summary by phase

- **VISION (canonical):** nothing in this doc rises to a VISION amendment — it is substrate
  *how*, and VISION already states the *what* (world-scale, GDPR-by-construction, agent-native).
  The honesty rule (#34) reinforces VISION §3 but needs no edit.
- **Phase-2 back-patch (ADRs):** the heaviest landing zone. New/amended ADR material for:
  fail-static-vs-fail-closed (#30 → ADR-03 + ADR-11); a **Backpressure & overload ADR**
  (#12/#13/#14 — protected human lane, shed order, bounded queues, shared resilient client);
  a **Service topology & health ADR** (#27/#28 — three surfaces as a security boundary,
  liveness≠readiness); migration discipline (#24 → ADR-10 clause); plus tightening lines into
  ADR-04 (outbox is the only path #10, no circular sync deps #6), ADR-10 (cache-never-source-of-
  truth #21, measure-before-shard #25, recursive-CTE/no-graph-DB #23), ADR-11/13 (token-not-path
  tenancy #2, URN-not-display-key persistence #18), ADR-13.2 (causality derivation + `caused-by`
  #15), and ADR-01's crate table (the resilient client #14, the bootstrap harness #26).
- **Phase 3 (shared systems):** Bus/`myelin-events` (durable-pull JetStream-class prior #8,
  the relay recipe #9, the only-outbox path #10, causality propagation #15, bounded consumer
  template #12); Id (fail-static cache #30, agent token lifecycle #4, replica-for-auth #25);
  Refs (typed-relation-table answer to TE-7 #19, recursive-CTE default #23, URN ambiguity
  rejection #18); Storage (blob trait #23b, migrations #24, cross-seam restore #32); Agent
  Fabric (causal-root tripwire #16); the bootstrap harness + three-surface topology + health
  (#26/#27/#28); a per-shared-system stateful-component/blast-radius register (#29).
- **Phase 4 (subsystems):** Issues + Knowledge own the **typed relation tables** that back the
  TE-7 resolution (#19, ties to ADR-06 relation field type); each subsystem produces a blast-
  radius note (#29).
- **Phase 5 (testing):** the **30× agent-surge drill** (#13/#14), the **Id-hiccup / fail-static**
  drill (#30), **restore-verification + cross-seam (row↔blob) integrity** drill (#31/#32), the
  **cross-tenant IDOR** drill (#2), and the **causal-loop tripwire** adversarial test (#16,
  already AG-4 → P5).
- **Phase 6 (roadmap sequencing):** the **bootstrap harness** and the **shared resilient client**
  are early platform-capability deliverables (sequence before the per-service work that depends
  on identical plumbing) (#26/#14).
- **Phase 8 (execution discipline):** lints/gates — no-cross-tenant-predicate (#2), no direct
  bus-publish outside the outbox helper (#10), no blocking `ALTER` on hot tables / forward-only
  migrations (#24), restore-verify wired into CI (#31/#33), liveness/readiness operational rule
  (#28).
- **Legal/DPO:** one item to flag — the **fail-static staleness window (#30) must be bounded by
  the deprovision/revocation SLA**, which interacts with GDPR revocation latency; the chosen
  bound is a written, counsel-aware trade-off (relates to existing GD-5/GD-14 carve-outs). No new
  legal question, but the DPO should ratify the staleness bound.

---

## 3. Genuine conflicts

**None.** The doctrine was drawn from the same prior art as our spine; every divergence is a
*sharpening* or a *net-new discipline at a lower altitude than Phase 2 chose to operate*, not a
contradiction. The closest thing to a tension is internal to the doctrine and now made explicit:
**fail-static (#30) trades bounded staleness for availability against GDPR revocation latency** —
resolved by bounding the staleness window to ≤ the deprovision SLA (flagged to DPO above), not a
conflict with any committed ADR.

---

## 4. Prioritized deltas (the 5–8 that matter)

1. **Fail-STATIC vs fail-closed on the identity hot path (§10, #30).** Highest blast-radius gap:
   without a bounded-staleness Id cache, an Id hiccup is a whole-platform cascade. **Binds: P2bp
   (ADR-03 + ADR-11) + P3 (Id) + P5 drill + DPO ratifies the staleness bound.**
2. **Backpressure with a PROTECTED INTERACTIVE-HUMAN LANE + shed order + Retry-After-honouring
   own clients (§5, #12/#13/#14).** We govern agent load but never reserve a human lane or order
   the shedding; the dominant client is now a fleet of agents. **Binds: P2bp (new Backpressure
   ADR) + P3 (Bus/Id/shared resilient client `myelin-rpc`) + P5 (30× surge drill) + P6
   (sequence the client early).**
3. **Backup restore-VERIFICATION as a CI gate + cross-seam (row↔blob) integrity (§11,
   #31/#32/#33).** "A backup that has never been restored is not a backup" — currently a silent
   gap. **Binds: P5 (testing) + P8 (CI gate) + P3 (Storage + GDPR for the consistency point).**
4. **TE-7 RESOLVED: lifecycle edges mirrored to a typed relation table owned by the
   authoritative service; backlinks stay event-sourced projections (§7, #19).** Concrete
   default-to-beat that unifies both halves of our open question. **Binds: P3 (Refs) → P4 (Issues
   + Knowledge own the typed tables).**
5. **Migration discipline: forward-only, expand→backfill→contract, measure-lock-against-restore,
   no blocking ALTER on hot tables; defer sharding until measured; recursive CTEs over a graph DB
   (§8, #24/#25/#23).** Net-new substrate law Phase 2 stayed above; cheap now, brutal to retrofit.
   **Binds: P2bp (ADR-10 augmentation) + P3 + P8 (CI checks).**
6. **The substrate plumbing crates: shared bootstrap harness (§9, #26) + the resilient outbound
   client (§5, #14) + the three-surface security topology and liveness≠readiness (§9, #27/#28).**
   "Many services maintainable, not many snowflakes." **Binds: P2bp (ADR-01 crate table + a
   service-topology ADR) + P3 (build) + P6 (early sequencing).**
7. **JetStream-class durable streaming log with durable PULL consumers as the SETTLED bus class,
   and the outbox as the ONLY emit path (§3/§4, #8/#10).** Narrows ADR-04's open transport list
   to "durable-pull-streaming-log class" and forbids any fire-and-forget regression path. **Binds:
   P2bp (tighten ADR-04) + P3 (Bus) + P8 (lint).**
8. **Token-not-path tenancy + cross-tenant IDOR as a drilled class, and URNs (not display keys)
   as the only persisted cross-entity link (§1/§7, #2/#18).** Two cheap, high-leverage
   invariants. **Binds: P2bp (ADR-11/ADR-13) + P3 (Refs/Id) + P5 (IDOR drill) + P8 (lint).**

---

## 5. Cross-references
- Source: [`external-insights/02-platform-substrate.md`](../../../external-insights/02-platform-substrate.md)
- Spine: [`architecture-decisions.md`](../../02-holistic-architecture/architecture-decisions.md)
  (ADR-01 crates, ADR-03 authz, ADR-04 bus/outbox, ADR-10/14 storage, ADR-11 cells/tenancy,
  ADR-12 GDPR, ADR-13 glue, ADR-15 open-questions/TE-7)
- [`shared-systems-overview.md`](../../02-holistic-architecture/shared-systems-overview.md)
  (§1 Id, §2 Bus, §3 Refs, §7 Storage, §8 GDPR/Audit, §10 inter-system glue)
- [`technical-structuring.md`](../../01-research/technical-structuring.md) (§2 shared systems,
  §3 glue contracts, §5 cells)
- Sibling doctrine-integration analyses (Phase 2b) for `01-process-and-quality`,
  `03-agent-native-fabric`, `04-hard-problems`, `05-ux-and-design`.
