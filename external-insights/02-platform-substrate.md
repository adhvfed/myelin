# The Platform Substrate

The shared backbone every subsystem stands on. These are the decisions that are **cheap to
get right at the start and brutal to retrofit**, so they carry the most prescriptive weight.
Get the substrate right and each subsystem becomes a thin projection of it; get it wrong and
every subsystem inherits the wound.

---

## 1. The tenant is the unit of everything — partition by it from the first table

Put the **tenant id (org) as the first column of every table** and the partition key of
every stream, index, and cache. This single decision is the highest-leverage one in the
whole system.

- It makes three otherwise-expensive properties nearly free: **data residency** (a tenant's
  region is its shard key — "EU data stays in EU" and "scale this tenant out" become the same
  mechanism), **fairness/isolation** (the tenant is the blast-radius unit and the thing you
  rate-limit and shed by), and **future sharding** (you already have the shard key).
- There is **no cross-tenant query path.** Enforce it and drill it (cross-tenant data access
  is a top-tier security bug — an "IDOR"). Trust the authenticated principal's tenant from
  the verified token, **never the tenant in the URL path** — path is a routing hint, the
  token is authority.
- **Failure mode if ignored:** retrofitting multi-tenancy/residency means fighting the data
  model on every table at once, and a single missing tenant predicate becomes a cross-tenant
  data leak.

## 2. One identity model for humans *and* agents

Model every actor — human, agent, service account — as **one polymorphic principal** that
flows through the *same* authentication, authorization, attribution, and audit paths. An
agent is a principal with `kind = agent`, **not a special case in the permission code.**

- Permission authority lives in **one place** (the identity service). Every other service
  *asks*; none reimplements the check. The dominant cross-service contract for "may X do Y"
  is that one question.
- Give agents a **per-run, narrowly-scoped, short-lived, auto-revoked identity** — the token's
  life equals the run's life; teardown revokes it. An agent literally cannot exceed its
  identity because the same gateway and the same authorization check stand in front of it as
  a human. There is **no second-class "bot account"** to drift out of sync.
- **Failure mode if ignored:** a parallel agent-permission system that diverges from the
  human one, and agents that can do things no human role can — an audit and security
  nightmare.

## 3. The event log is the nervous system

Adopt a clean split: **synchronous calls for queries** (data a caller needs *right now*),
**asynchronous events for reactions** (everything that happens *in response*).

- Every state change emits **exactly one** immutable, append-only event. Events are the
  source of truth for activity, audit, search projections, notifications, webhooks, and
  causality. Adding a consumer never touches the producer — **the event schema, not RPC, is
  the dominant integration contract.**
- **No circular synchronous dependencies:** if A calls B synchronously, B must never call A
  synchronously (reactions flow either direction over the bus). Keep a clear dependency root
  (identity depends on nothing).
- Partition event subjects by tenant. Guarantee **per-subject ordering only**; design
  causally-related events to share a subject rather than promising global order.
- Settled building block: a **durable streaming log** (a JetStream-class broker) with durable
  pull consumers and consumer groups — not fire-and-forget pub/sub.

## 4. The transactional outbox is the *only* sanctioned way to emit

This is the highest-value correctness decision in the messaging layer, and the one most
often skipped until it causes silent data loss.

- The hazard is the **dual write:** commit to the database, then publish to the bus as a
  separate step. A crash *between* them, or a publish that silently fails, means the entity
  exists but is never indexed, projected, or notified — divergence with no recovery signal.
- The fix: in the **same database transaction** as the state change, insert the event into a
  per-service `outbox` table. A relay drains the outbox to the bus with at-least-once
  delivery, claiming rows with `FOR UPDATE SKIP LOCKED` (safe across replicas), stamping a
  stable message id for broker-side dedup, and dead-lettering after bounded retries.
- **At-least-once delivery + idempotent handlers + deterministic dedup ≈ effectively-once.**
  Make idempotent handlers mandatory; do not chase true exactly-once.
- **If you are starting clean, make the outbox + durable consumer + idempotent handler the
  only pattern, with no non-durable path to regress to.** A fire-and-forget shortcut that
  exists *will* be used and *will* lose data.

## 5. Backpressure is mandatory, everywhere

The dominant client of a modern developer platform is no longer a human at a keyboard — it is
**a fleet of agents and CI jobs that hammer the API at machine speed, fan out across every
surface at once, and do not back off politely.** Design for that load from day one.

- **Every queue is bounded; unbounded anything is a future cascade.** Bound consumer
  prefetch, database connection pools (fast-fail on saturation, with statement timeouts),
  and per-tenant in-flight work.
- Make the limiter **principal-aware.** Reserve a **protected lane for interactive humans** so
  a person never queues behind an agent storm. Shed in order: speculative → batch/CI →
  agent → human (human last). Agents get `429 + Retry-After`; ensure *your own* clients
  (agent runtime, CLI) actually honour it, or shedding becomes a retry storm.
- Wrap every outbound inter-service call in one shared client: per-call timeout, circuit
  breaker, a **bounded-concurrency bulkhead**, and jittered retry *for idempotent calls only*
  (never retry through a tripped breaker).
- **Headline drill:** a 30× agent surge in which the interactive lane holds, the agent lane
  sheds, and other tenants are unaffected.

## 6. Causality is a first-class primitive, not logging

Carry provenance on every event as platform-owned metadata: a **caused-by** reference to the
originating human action/session, plus a causal triple (**correlation/root**,
**causation/parent event**, **depth**). Derive a child's provenance *from its cause* so it is
**correct by construction** (root carries through, parent = cause id, depth + 1). Propagate it
across service hops in headers, alongside trace context.

- One mechanism then powers **audit** (walk the graph, don't guess), the **"why did this
  happen?"** view, distributed tracing, and **loop safety**.
- **Loop safety is structural:** because the loop guard reads platform causality metadata,
  **a human (or agent) can never typo their way into a loop** the way a commit-message
  convention would allow. Add a depth ceiling and a tripwire (e.g. if N runs share a causal
  root within a short window, trip a breaker).
- **Failure mode if ignored:** "why did this fire?" becomes archaeology, and an automation
  that triggers itself has nothing structural to stop it.

## 7. One canonical reference graph

Everything addressable gets a **canonical, stable URN** (`scheme://tenant/scope/type/id[/sub]`).
A single shared library owns parsing, formatting, and resolution — services must not
re-implement it.

- **If a reference crosses a service boundary or is persisted as a cross-entity link, it
  travels and is stored as a URN.** Reject ambiguity (short hashes, scope-less names) — never
  guess scope. Human-friendly display keys (`#42`, `@alice`, `~general`) are a UI concern,
  derived at render time, never stored.
- **Backlinks are event-sourced projections**, not stored edges — rebuild them from the event
  log. But links that affect lifecycle (closes / blocks / depends / assigns) must *also* be
  mirrored into a **typed relation table owned by the authoritative service**, and that typed
  edge — not the URN text — is the source of truth.
- The payoff: **the same graph that lets one action cascade across six subsystems with no
  human effort is the graph that lets a person jump from a failing test to the line of code
  to the issue to the conversation in four keystrokes.** Build the graph once; both the
  machines and the humans win. This is the platform's core moat — an integration no
  bolt-on can copy.

## 8. Keep the storage stack minimal — justify every engine

Every additional data engine is permanent operational cost. Default to the smallest set that
works and make each addition earn its place.

- A strong default: **PostgreSQL as one database per service** (system of record, no shared
  tables, no cross-service joins — logical references validated through the owning service);
  an **S3-compatible object store** for blobs, packs, logs, and content-addressed artifacts;
  and a cache/coordination store (Redis/Valkey-class) that is **never a source of truth**.
- Keep the **SQL visible and intentional** (favour a thin query layer over a heavy ORM —
  "an ORM hides the data model you most need to see"). Prefer **recursive CTEs over a
  dedicated graph database** for the shallow graphs this domain produces (a separate graph DB
  buys dual-write sync pain and fragile cross-store transactions).
- Use **content-addressing** (hash-on-write) for blobs behind a narrow `put/get/head/delete`
  trait, so the filesystem-vs-object-store choice is a one-line swap.
- **Migrations are forward-only and online:** no rollback migrations (you can't un-delete
  data); expand → backfill → contract; never a blocking `ALTER` on a hot table — measure lock
  time against a restore first.
- **Defer sharding and partitioning until a hot table is *measured*, not predicted.**
  Premature sharding is its own outage. Reach for read replicas and connection pooling first;
  a dedicated replica for the authentication hot path is usually the first real need.

## 9. Make many services maintainable, not many snowflakes

If the system is many services, invest early in a **shared bootstrap harness** that wires
configuration, database, migrations, the event publisher, telemetry, and the standard ports
in one call. A new service should be a thin shell over identical plumbing.

- Adopt a **three-surface topology per service:** a public API (reached only through a thin,
  stateless gateway that authenticates and injects trusted identity headers), an internal RPC
  surface (inside the trust boundary only), and a metrics/health surface. The public/internal
  split is a **security boundary**, not just organisation.
- **Health checks must distinguish liveness from readiness.** A service whose critical
  dependency is dead must report *not ready* and stop taking traffic — not report healthy and
  keep failing requests.

## 10. Blast-radius first — and fail *static*, not *closed*, for availability

For every component ask not "will it fail?" but **"what else dies when it does, and for
whom?"** Name every stateful component up front and give each an explicit shared-state or
sharding plan; everything else should be stateless and replaceable.

- **Distinguish fail-closed from fail-static.** Fail-*closed* is correct for *authorization*
  (deny when unsure). It is the wrong *availability* default: if every request fails closed on
  the identity service, one shared dependency takes the whole platform to a hard stop — the
  textbook single-point cascade. For availability, **fail static**: serve a bounded-staleness
  cached answer (e.g. "this actor is still active") so already-authenticated traffic keeps
  working while the dependency recovers, with the staleness window bounded by your
  deprovision SLA.
- **Failure mode if ignored:** the identity service (or its database) hiccups and the entire
  platform returns errors — the highest-priority blast-radius problem these systems have.

## 11. A backup that has never been restored is not a backup

- Continuous log archiving + periodic base backups for a tight recovery-point objective;
  **automated, periodic restore verification** that actually rebuilds and asserts no loss.
- Restore must be **consistent across services *and* blobs** — a restore that resurrects a row
  pointing at a missing blob is silent corruption. Assert cross-seam integrity in the drill.
- Wire the restore-verify into CI so durability is gated continuously, not just hoped for.
