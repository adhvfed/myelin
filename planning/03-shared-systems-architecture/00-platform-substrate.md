# Phase 3 — Platform Substrate & Foundations (the crates every service stands on)

> Phase: `03-shared-systems-architecture`. Deliverable `00` — the **foundational** doc the other
> Phase-3 systems consume. Canonical brief: [`VISION.md`](../../VISION.md). Doctrine (binding):
> [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) (EI-02)
> and [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) (EI-04).
> Binds: **ADR-01, ADR-04, ADR-16, ADR-17, ADR-18** (with cross-refs to ADR-11/12/13); directives
> **X-1…X-5, BUS-2/BUS-3, STOR-1/STOR-2/STOR-3, ID-1, GD-3**, decision-record **§(c) D1/D2/D3/D7** and
> **§(e) stronger priors**. Phase-2 springboard:
> [`shared-systems-overview.md`](../02-holistic-architecture/shared-systems-overview.md).
>
> **What this doc is.** The shared **crates + conventions** every Myelin service is built from: the
> trait/type surface of the eight glue crates, the **bootstrap harness** that makes a new service a thin
> shell, the **three-surface topology**, the **event-consumer template**, the **resilient inter-service
> client**, **backpressure/shedding**, the **fail-static** primitives, **forward-only online migrations**,
> and the **observability baseline**. It defines contracts other Phase-3 systems (Id, Bus, Refs, Search,
> Notif, Agents, Storage, GDPR/Audit) and all Phase-4 subsystems consume. It does **not** re-decide an ADR;
> where it sharpens one it cites it and stays inside the decision.
>
> **Altitude.** Phase 3 is *detailed*: concrete trait surfaces, schemas, algorithms, wire shapes, failure
> modes + drills owed, and the explicit hand-off of what remains `[OPEN → P4]`. Illustrative Rust-shaped
> snippets appear for the **contract surface**; they are signatures, not implementations.

---

## 0. Purpose, responsibilities, and the one-paragraph thesis

The substrate is the set of crates and conventions that are **cheap to get right at the start and brutal
to retrofit** (EI-02 preamble). Get them right and every shared system and subsystem becomes a thin
projection over identical plumbing; get them wrong and every service inherits the wound. This doc owns
**nine substrate concerns**, each a named section below:

1. **The eight glue crates** (§2) — `myelin-events`, `-identity`, `-refs`, `-agent`, `-gdpr`, `-content`,
   `-query`, `-tenancy`, plus the Phase-2b-added `-client` (ADR-16) and a small `-substrate` harness crate.
   Each crate's *responsibility* and its *trait/type surface* (the contract; internals are the owning
   system's Phase-3/4 doc).
2. **The bootstrap harness** (§3) — config / DB pool / migrations / outbox-publisher / telemetry / the
   three ports, wired in **one call** (EI-02 §9; directive X-2).
3. **The three-surface topology** (§4) — public gateway / internal RPC / metrics-health; the public↔internal
   split as a **security boundary**; **liveness ≠ readiness** (EI-02 §9; directive X-2).
4. **The event-consumer template** (§5) — ADR-04 semantics + the D7 orchestrator gotchas, encoded once
   (BUS-2, BUS-3).
5. **The resilient inter-service client** (§6) — timeout / circuit-breaker / bulkhead / jittered-retry, in
   one crate every caller links (ADR-16 §4; directive X-3).
6. **Backpressure + principal-aware shedding** (§7) — bounded everything, the protected human lane, the
   shed order (ADR-16; directive X-3).
7. **Fail-static primitives** (§8) — the bounded-staleness cache pattern (ADR-17; directive ID-1, GD-3).
8. **Forward-only online migrations** (§9) — expand → backfill → contract, no blocking `ALTER` on a hot
   table (EI-02 §8; directive STOR-2).
9. **The observability baseline** (§10) — traces carry causality + tenant; the telemetry the Phase-5 drills
   read as a survival signal (EI-02 §6; directive X-1).

**Thesis.** *A new Myelin service is a `main.rs` that calls `myelin_substrate::serve(AppSpec)` and supplies
its handlers, its migrations, and its event-consumer registrations. Everything load-bearing for correctness —
the outbox, idempotency, tenant-scoping, the resilient client, the shed lane, fail-static, the three ports,
the trace context — comes from the crates, so it cannot drift between services and cannot be skipped.* This
is the mechanical embodiment of ADR-01 (contracts that cannot drift) and EI-02 §9 (many services, not many
snowflakes).

### 0.1 The non-negotiables every crate and service inherits (not repeated per section)

Carried from EI-02 / ADR-11 / ADR-12 / the prompt's standing rules:

- **Tenant is the first column / partition key of everything** (EI-02 §1; ADR-11.2). There is **no
  cross-tenant query path**. The authenticated principal's tenant comes from the **verified token, never the
  URL path** (directive ID-3). A missing tenant predicate is a top-tier security bug (an IDOR).
- **Every store is residency-pinned, per-tenant envelope-encrypted, crypto-shred-capable, and a
  `PersonalDataHolder`** (ADR-11/12). The substrate provides the seams; it never lets a service opt out.
- **No subsystem/shared-system reads another's store** (ADR-01, ADR-13). Cross-system interaction is via the
  contracts in this doc. Enforced by the `no-cross-db` architecture lint (§2.11, E-5).
- **The transactional outbox is the ONLY sanctioned emit path** (EI-02 §4; BUS-2). `myelin-events` exposes
  **no fire-and-forget publish**; a shortcut that exists will be used and will lose data.

---

## 1. Prior art this substrate stands on (cited once, referenced throughout)

| Concern | Prior art / proven system | Where it lands |
|---|---|---|
| Transactional outbox / dual-write hazard | Microservices outbox pattern (Richardson, *Microservices Patterns* 2018, ch. 3.2; Kleppmann, *DDIA* 2017 ch. 11 "dual writes"); Debezium outbox; `FOR UPDATE SKIP LOCKED` claim (PostgreSQL ≥ 9.5 docs) | §2.1, §5 |
| Durable streaming log + durable pull consumers | NATS **JetStream** (durable consumers, ack policies, consumer-lag/`num_pending`); Apache **Kafka** consumer groups; Redpanda — the "durable streaming log" class (ADR-04; §(e) prior) | §2.1, §5 |
| At-least-once + idempotent ≈ effectively-once | Kafka/JetStream delivery semantics; Helland "Idempotence Is Not a Medical Condition" (2012); Kleppmann *DDIA* ch. 11 | §5.2 |
| Circuit breaker / bulkhead / timeout / retry | Nygard, *Release It!* (2nd ed. 2018) — Circuit Breaker, Bulkhead, Timeout, Steady-State; Netflix **Hystrix** wiki; AWS Builders' Library "Timeouts, retries, backoff with jitter" (Brooker) | §6 |
| Jittered exponential backoff | Marc Brooker, "Exponential Backoff And Jitter" (AWS Architecture Blog, 2015) — *full jitter* | §6.3 |
| Backpressure / bounded queues / load shedding | Little's Law; Welsh et al. **SEDA** (SOSP 2001); AWS Builders' Library "Using load shedding to avoid overload"; Google **SRE** ch. 21 (handling overload), ch. 22 (cascading failures) | §7 |
| Fairness / priority lanes under overload | Weighted fair queueing (Demers/Keshav/Shenker, SIGCOMM 1989); LIFO-under-load (Facebook/Meta); per-tenant quotas | §7.2 |
| Fail-static / bounded-staleness cache | Google SRE ch. 22 "degrade gracefully"; CDN/DNS stale-while-revalidate (RFC 5861); Zanzibar consistency tokens ("zookies", Pang et al. OSDI 2019) | §8 |
| Liveness vs readiness | Kubernetes liveness/readiness/startup probes (k8s docs); 12-factor "disposability" | §4.3 |
| Online schema change / expand-contract | Stripe "Online migrations at scale" (2017); GitHub **gh-ost** (2016); PlanetScale/Vitess online DDL; Sato et al.; Parallel-change / expand-contract (Fowler, "ParallelChange") | §9 |
| Distributed tracing + causality propagation | **OpenTelemetry** spec (trace context, W3C **`traceparent`** Rec 2020); Google **Dapper** (2010); ADR-13.2 causality triple | §10 |
| Content-addressed blob store | Git object model (content addressing); S3-compatible `put/get/head/delete`; ADR-10/STOR-1 | §2.5 |
| One polymorphic principal | Zanzibar (OSDI 2019); ADR-03/08/13.3 | §2.2 |
| RED/USE telemetry method | Tom Wilkie "RED method"; Brendan Gregg "USE method" | §10.2 |

These are the **defaults-to-beat** and the **justified adoptions**. Where Myelin deviates (e.g. the
specific shed order, the fail-static staleness bound) it is called out in writing in the relevant section.

---

## 2. The shared crates — responsibility + trait/type surface

The crate table is ADR-01's, ratified. This section gives each crate its **responsibility** and its
**exposed surface** (the contract other systems link). Internals (the tuple store, the index engine, the
DSR orchestrator algorithm) belong to the owning system's own Phase-3 deliverable; here we pin only what
crosses a crate boundary, because **that** is the thing that must not drift (ADR-01).

> **Convention.** Snippets are Rust-shaped signatures for concreteness (ADR-02: glue crates are Rust,
> non-negotiably). Cross-language services consume the same surface as a **wire contract** (protobuf/JSON
> over the internal RPC, ADR-02 §Consequences), generated from these types. Field **names and units** are
> reconciled at the plan layer before either side ships (directive X-5); §2.10 is the canonical envelope
> field list that reconciliation anchors on.

### 2.1 `myelin-events` — envelope, `ArtifactRef`, outbox helper, consumer template

**Responsibility.** Owns the **canonical event envelope** (ADR-13.2), the **`ArtifactRef`** type
(co-owned conceptually with `myelin-refs`; the *type* lives here because the envelope embeds it), the
**transactional-outbox helper** (the ONLY emit path, BUS-2), the **event-consumer template** (§5), and the
**event taxonomy *types*** (not the dotted-name registry, which is `[OPEN → P3 Bus]`).

**Surface (the contract):**

```rust
/// The non-negotiable envelope (ADR-13.2). schema_ver gates evolution.
pub struct EventEnvelope {
    pub event_id: EventId,            // ULID; the idempotency key (ADR-04.1)
    pub type_: EventType,             // dotted name, e.g. "git.pr.opened" (registry → P3 Bus)
    pub schema_ver: u32,
    pub tenant: TenantId,             // partition + residency key (ADR-11) — FIRST-CLASS, never optional
    pub region: Region,
    pub actor: Actor,                 // Principal ref incl. on_behalf_of (ADR-13.3)
    pub subject: ArtifactRef,         // what this event is about (ADR-13.1)
    pub causation_id: Option<EventId>,    // IMMEDIATE parent (BUS-5: nested, not flat)
    pub correlation_id: CorrelationId,    // the causal ROOT — carries through (BUS-5)
    pub caused_by: Option<CausedBy>,      // distinct human-action/session ref (BUS-5)
    pub depth: u32,                       // causal depth; loop ceiling reads this (AG-6)
    pub contains_personal_data: bool,     // routes GDPR handling (ADR-04.4)
    pub visibility: Visibility,
    pub occurred_at: Timestamp,
    pub payload: serde_json::Value,   // references-not-payloads: IDs/ArtifactRefs, never PII bodies
}

/// The ONLY sanctioned emit path (BUS-2). Inserts into the per-service `outbox` table
/// IN THE SAME TRANSACTION as the state change. There is NO fire-and-forget publish.
pub trait OutboxTx {
    /// Derives causality from the cause so it is correct-by-construction (BUS-5, EI-02 §6):
    /// root carries, parent = cause.event_id, depth = cause.depth + 1.
    fn emit(&mut self, draft: EventDraft, cause: Option<&EventEnvelope>) -> Result<EventId>;
}

/// Canonical URN. Parsing/formatting/resolution live in myelin-refs (REF-3); the TYPE lives here.
pub struct ArtifactRef(/* myelin://<tenant>/<subsystem>/<type>/<id>[#sub] */ String);
```

**Why these choices.** ULID `event_id` (lexicographically sortable, time-prefixed) gives the broker a stable
dedup key and the DB an index-friendly PK without a coordination round-trip (vs UUIDv4's random scatter).
`causation_id` as *immediate parent* + `correlation_id` as *root* is the nested model BUS-5/D7(iv) mandate —
flat-to-root breaks both depth-capping and the "why did this happen" walk (EI-02 §6). `emit(draft, cause)`
**derives** child provenance from the cause so a human (or agent) **cannot typo their way into a loop**
(EI-02 §6 "correct by construction"). The outbox helper is the only API; there is intentionally no
`publish(event)`.

### 2.2 `myelin-identity` — `Principal`, capability types, the authz client

**Responsibility.** The **one polymorphic `Principal`** (Human / Agent / Service — kind is data, not a code
branch; EI-02 §2, ADR-08.1), the capability/permission types, and the **authz client** every service links
(no service re-implements the check; ADR-13.3). Crate is a *client* surface; the tuple store and decision
engine are Id's own Phase-3 doc.

**Surface (the contract Search/Refs/Agents/every gateway consume):**

```rust
pub struct Principal { pub id: PrincipalId, pub kind: PrincipalKind, pub tenant: TenantId, /* … */ }
pub enum PrincipalKind { Human, Agent { runtime_ref: RuntimeRef, on_behalf_of: Option<PrincipalId> }, Service }

pub trait AuthzClient {
    fn check(&self, subject: &Principal, perm: Permission, object: &ArtifactRef, at: Consistency)
        -> Result<Decision>;                       // per-action gate (ADR-03)
    fn list_objects(&self, subject: &Principal, perm: Permission, ty: ObjectType, at: Consistency)
        -> Result<ObjectFilter>;                   // the leak-free pre-filter (ADR-03; §10.2 overview)
    fn delegation(&self, agent: &Principal, trigger_actor: &Principal)
        -> Result<EffectivePolicy>;                // agent ∩ delegation ∩ tenant (ADR-08.3)
}

/// Consistency token ("zookie", Zanzibar) for read-your-writes; also the input to fail-static (§8).
pub struct Consistency { pub at_least: Zookie, pub mode: ConsistencyMode /* Strong | BoundedStale */ }
```

The substrate's contribution: **the `check`/`list_objects` calls flow through the resilient client (§6) and
are the canonical fail-static surface (§8)**. The full tuple algebra, `Agent`-vs-`Service` (AG-1), and the
delegation algebra (AG-2) are Id's `[OPEN → P3]` — this crate pins only the call shape so callers compile
against a stable surface now.

### 2.3 `myelin-refs` — URN parse/format/resolve + edge client

**Responsibility.** Owns **`ArtifactRef` parsing/formatting/resolution** (one library; services must not
re-implement — REF-3), and the **edge/backlink client**. Rejects scope-less / ambiguous refs and **never
guesses scope** (REF-3). Display keys (`#42`, `@alice`, `~general`) are render-time projections, never stored.

**Surface:**

```rust
pub trait Refs {
    fn parse(s: &str) -> Result<ArtifactRef>;           // rejects ambiguity; never guesses scope (REF-3)
    fn format(r: &ArtifactRef) -> String;
    fn edges(&self, r: &ArtifactRef) -> Result<Vec<Edge>>;                  // outbound
    fn backlinks(&self, r: &ArtifactRef, viewer: &Principal) -> Result<Vec<Edge>>; // permission-filtered (REF-1)
}
```

Edge creation is **`ref.created` emission via the outbox** (ADR-13: events are authoritative for edges) —
there is no separate edge-write API. Backlinks are event-sourced projections; lifecycle/semantic edges are
*also* mirrored to a typed table owned by the authoritative subsystem (REF-1). Defaults to **Postgres +
recursive CTEs** for the shallow graph (REF-2 / §(e) prior). Internals → Refs Phase-3 doc.

### 2.4 `myelin-agent` — runtime/agent/tool traits + `MockAgentRuntime`

**Responsibility.** The **strategy-pattern boundary** (VISION §3): the small trait set behind which
`MockAgentRuntime` lives now and `LlmAgentRuntime` lives later. **No LLM SDK / prompt / model name appears
in platform code** (ADR-08.2). The brain is a *stateless* `step` (AG-1); the hands is `exec` with **no
host-execution bypass** (AG-2). Detailed in the Agent Fabric Phase-3 doc; the substrate pins the trait shape
so subsystems can register tools against a stable surface.

**Surface (concrete shapes from D6/AG-1/AG-2):**

```rust
pub trait AgentRuntime { fn step(&self, conv: &Conversation) -> StepOutcome; }      // STATELESS brain (AG-1)
pub enum StepOutcome { UseTools(Vec<ToolCall>), Submit(Submission) }
pub trait ToolHands { fn exec(&self, cmd: Command) -> ToolResult; }                 // hands; no bypass (AG-2)
pub trait ToolSurface { fn register_tool(&mut self, def: ToolDef); }               // one catalogue (ADR-08.4)
```

### 2.5 `myelin-content`, `myelin-query`, `myelin-gdpr`, `myelin-tenancy`, `myelin-client` (substrate-relevant surface)

These are owned by other Phase-3/4 docs; here only the **substrate-relevant** seams:

- **`myelin-content`** (ADR-05) — block/inline taxonomy + the platform-load-bearing inline nodes
  `mention(Principal)`, `artifact_ref(ArtifactRef)`, `embed(ArtifactRef)`. Inline content is a
  **markdown-subset string** (KN-2/D10); structured ref/mention/embed nodes preserved. Substrate role: these
  nodes are the **producers of `ref.created`** (ADR-05 §Consequences), emitted via the outbox.
- **`myelin-query`** (ADR-06/07) — the single query AST + field/view primitive; **permission-aware by
  construction** (always composes with `list_objects`; ADR-07). Substrate role: the `EventMatcher` predicate
  core (Bus triggers) and saved-view filters share it — **one safe-evaluation engine, one DoS-hardening
  surface** (AG-7). Substrate provides the bounded-evaluation guard (no Turing-complete predicates on hot
  paths; §7.5).
- **`myelin-gdpr`** (ADR-12) — the `PersonalDataHolder` trait (`locate/export/rectify/restrict/erase`) +
  DSR client + **KMS/crypto-shred abstraction**. Substrate role: **the bootstrap harness registers every
  store a service opens as a holder automatically** (§3.4) so "we forgot the search index" is structurally
  impossible (GD-3, ADR-12.1). The **content-addressed blob trait** `put/get/head/delete` (STOR-1) lives at
  this seam so filesystem↔object-store is a one-line swap.
- **`myelin-tenancy`** (ADR-11) — `TenantId` / `Region` / residency tags / cell-routing client types.
  Substrate role: the **tenant-scoping guard** every query path threads (§2.11) and the residency assertion.
- **`myelin-client`** (ADR-16 §4, added Phase-2b) — the **shared resilient inter-service client** (§6). The
  one place timeout/breaker/bulkhead/retry is correct for every caller.

### 2.6 `myelin-substrate` — the bootstrap harness crate (new, §3)

The harness itself is a crate (working name `myelin-substrate`; it may be folded into a `bootstrap` module).
It depends on all of the above and exposes the **one-call `serve`** (§3). It is the only crate a service's
`main.rs` must wire by hand.

### 2.7 The content-addressed blob trait (STOR-1, lives in `myelin-gdpr`/`myelin-storage` seam)

```rust
pub trait BlobStore {                                    // hash-on-write; fs-vs-object is a one-line swap
    fn put(&self, bytes: &[u8]) -> Result<ContentHash>;  // content address = the hash (Git object model)
    fn get(&self, h: &ContentHash) -> Result<Vec<u8>>;
    fn head(&self, h: &ContentHash) -> Result<BlobMeta>;
    fn delete(&self, h: &ContentHash) -> Result<()>;     // crypto-shred is the real erasure (ADR-12.3)
}
```

Content-addressing (hash-on-write) gives dedup and integrity for free (Git's object model); the narrow trait
keeps the filesystem-vs-S3 choice a one-line swap (EI-02 §8; STOR-1). Erasure is **crypto-shred** (destroy
the per-tenant key), not blob delete, for anything in immutable/backup tiers (ADR-12.3).

### 2.8 What is deliberately NOT a crate

There is **no shared "storage API" crate spanning subsystems** (EI-02 §8; overview §7.4). Each service owns
its own schema and opens its own pool; the boundary is enforced by the `no-cross-db` lint (§2.11), not by a
shared data-access crate that would invite cross-service reads. **Thin, visible SQL over a heavy ORM**
(§(e) prior; "an ORM hides the data model you most need to see") — the harness provides a thin query layer
(a query builder + typed rows), not an ORM.

### 2.9 The dependency root (no cycles)

ADR-01/EI-02 §3: **no circular synchronous dependencies; identity depends on nothing.** The crate dependency
DAG, root-last: `myelin-tenancy` → `myelin-identity` → `myelin-events` (+ `-refs`, `-content`, `-query`) →
`myelin-agent`, `myelin-gdpr` → `myelin-client` → `myelin-substrate`. Synchronous *service* calls obey the
same rule: if A calls B synchronously, B reacts to A **only over the bus**, never with a synchronous call
back (EI-02 §3). The `no-cross-sync-cycle` check (E-5) is an architecture-test obligation.

### 2.10 The canonical envelope field list (the X-5 reconciliation anchor)

For directive X-5 (reconcile names **and units** before either side ships), the envelope fields above are
the single canonical list. Two unit conventions are pinned here so no two systems disagree: **timestamps are
RFC-3339 UTC** (`occurred_at`), **budgets/costs are integer minor-units** (cents-equivalent; never floats —
CI-2/D8 metering), **TTLs and staleness windows are seconds**. Every cross-component contract field
(`ArtifactRef` shape, authz `list_objects` result, budget/quota/SLA units) reconciles against this section.

### 2.11 Enforced architecture lints (E-5, made concrete here)

The substrate ships these as `cargo`-level architecture tests / clippy-style lints, committed to CI (E-4:
an uncommitted gate is no gate):

| Lint | Rule | Citation |
|---|---|---|
| `no-cross-db` | a service crate must not depend on another service's storage module | ADR-01 §Consequences |
| `no-raw-publish` | no bus publish outside the `OutboxTx::emit` helper | BUS-2, E-5 |
| `tenant-predicate` | every query builder call carries a `TenantId` bound; a tenant-less query fails to compile | EI-02 §1, ID-3 |
| `no-host-exec` | no host-execution path bypassing `ToolHands::exec` | AG-2, E-5 |
| `forward-only-migration` | no rollback migration file; no blocking `ALTER` on a flagged-hot table | STOR-2, §9 |
| `no-cross-sync-cycle` | the sync call graph is acyclic; identity is a sink | EI-02 §3 |

---

## 3. The shared bootstrap harness (a new service is a thin shell)

EI-02 §9 / directive X-2 / §(e) prior: invest early in a harness that wires **config, DB, migrations, the
event publisher, telemetry, and the three ports in one call**. Prior art: the 12-factor app (config in env),
Spring Boot / Quarkus "starter" auto-wiring, Go's `http.Server` + chi/grpc middleware stacks — but assembled
to *force* the Myelin non-negotiables rather than make them optional.

### 3.1 The one call

```rust
// A new service's entire main.rs, modulo its handlers:
fn main() -> Result<()> {
    myelin_substrate::serve(AppSpec {
        name: "issues",
        config:     Config::from_env(),                  // §3.2
        migrations: migrations::EMBEDDED,                 // §9; forward-only, run at boot
        public:     public_routes(),                     // §4.1 — behind the gateway, identity-injected
        internal:   internal_rpc(),                      // §4.2 — inside the trust boundary only
        consumers:  vec![ index_consumer(), refs_consumer() ], // §5 — durable, idempotent, whitelisted
        holders:    AppSpec::auto,                        // §3.4 — every opened store auto-registered (GD-3)
        outbox:     OutboxSpec::default(),                // §3.3 — relay started automatically (BUS-2)
    })
}
```

`serve` blocks, owning the lifecycle: boot → migrate → start outbox relay → start consumers → open the three
ports → serve until signalled → **graceful drain** (stop intake, finish in-flight, ack-then-exit). It returns
non-zero on a failed boot (a missing migration, an unreachable critical dependency at startup).

### 3.2 Config

Config is **env-first** (12-factor), typed, validated at boot (fail fast on a missing/invalid value, never
at first use). The harness injects: DB URL(s), broker endpoint + durable-consumer names, KMS endpoint,
region + cell id (ADR-11), telemetry exporter endpoint, the resilient-client defaults (timeouts, breaker
thresholds — §6), and the shed-lane budgets (§7). Secrets are **resolved by reference inside the trust
boundary** (ADR-20 secrets discipline carried to all services), never baked into images.

### 3.3 DB pool + outbox publisher

- **Bounded pool** with a **statement timeout** and **fast-fail on saturation** (EI-02 §5; X-3). The pool is
  the first bounded queue (§7.1); unbounded connection acquisition is forbidden.
- **The outbox relay** starts automatically (BUS-2): a background task that claims unsent rows with
  `SELECT … FOR UPDATE SKIP LOCKED` (safe across replicas — PostgreSQL docs), stamps the stable
  `event_id` for broker-side dedup, publishes, marks sent, and **dead-letters after bounded retries**
  (Richardson outbox; Debezium). The relay is the *only* component that talks to the broker's publish side;
  application code only ever calls `OutboxTx::emit` (§2.1).
- **Read-replica awareness** (§(e) prior): the pool can route reads to a replica; the harness exposes a
  `replica()` handle. The authn/authz hot path is the likely first dedicated replica (ID-4) — measure before
  sharding (ADR-10).

### 3.4 PersonalDataHolder auto-registration (GD-3, ADR-12.1)

Every store the harness opens (the OLTP schema, any blob prefix, any cache namespace, the search index if the
service owns one) is **automatically registered with the DSR orchestrator as a `PersonalDataHolder`**. "We
forgot the search index" becomes a *structural failure*, not a review miss (ADR-12.1; GD-3). A service that
opens a store the harness didn't wrap fails the `holder-registered` architecture test.

### 3.5 Telemetry + the three ports

The harness initialises the OpenTelemetry tracer/meter/logger (§10), installs the **causality+tenant trace
context** middleware on every surface, and opens the **three ports** (§4) with the standard middleware stack
(auth-header trust on public; trust-boundary check on internal; liveness/readiness on metrics-health).

### 3.6 What the harness deliberately does NOT do

It does not own business logic, schema, or event semantics — those are the service's. It does not hide the
SQL (thin query layer, not an ORM; §(e) prior). It does not provide a fire-and-forget publish (BUS-2). The
goal is *identical plumbing, visible logic*.

---

## 4. The three-surface service topology

EI-02 §9 / directive X-2: **public API (via a thin stateless gateway) / internal RPC (trust boundary only) /
metrics-health**. The public↔internal split is a **security boundary**, not organisation. Prior art:
the API-gateway pattern (Richardson ch. 8), BFF, Kubernetes probe semantics, the "trusted internal network"
deperimeterisation caveat (we do *not* assume the internal network is safe — see §4.2).

### 4.1 Public surface (behind the gateway)

Reached **only** through a thin, stateless **gateway** that:
1. **Authenticates** the credential (token, session, SSH key, CI job token, agent token) → resolves a
   `Principal` via Id (§2.2).
2. **Injects trusted identity headers** (`x-myelin-principal`, `x-myelin-tenant`, `x-myelin-actor`,
   on-behalf-of, the causality/trace headers — §10) that the internal services trust **because they came
   through the gateway**, not from the caller.
3. **Trusts the token's tenant, never the URL path** (ID-3). The path tenant is a routing hint; the token is
   authority. A mismatch is an IDOR attempt → rejected + audited.
4. Applies the **principal-aware shed lane** (§7) at the edge: a human request enters the protected lane; an
   agent/CI request enters its shed-able lane and receives `429 + Retry-After` under pressure.

The gateway holds no per-request state and no personal data of record (ADR-11 control-plane discipline
applied locally); it is horizontally scalable and replaceable.

### 4.2 Internal RPC surface (inside the trust boundary only)

Service-to-service calls. **Not exposed publicly.** It carries the injected identity headers forward (a
service does **not** re-authenticate from scratch but **does** re-authorize — `check`/`list_objects` are
always called; trusting the header for *identity* is fine, trusting it for *authorization* is not). Two
security notes:

- The internal surface is a **security boundary**: a service must refuse a call that did not transit the
  gateway or a peer inside the boundary (mTLS / a signed internal credential — mechanism is Id's Phase-3
  detail; the *rule* is here). We do **not** assume "internal = safe" naively; the boundary is asserted, not
  presumed.
- All internal calls go through the **resilient client** (§6) — timeout, breaker, bulkhead, retry.

### 4.3 Metrics-health surface (liveness ≠ readiness)

The crisp rule (EI-02 §9; X-2; k8s probe semantics):

- **Liveness** = "the process is not wedged." Fails → the orchestrator **restarts** it. Liveness must **not**
  check dependencies (a dependency outage must not cause a restart storm).
- **Readiness** = "this instance can serve correct traffic right now." A service whose **critical dependency
  is dead reports *not ready* and stops taking traffic** — it does **not** report healthy and keep failing
  requests (the EI-02 §9 anti-pattern). Readiness checks the DB pool, the broker connection, and the authz
  client's reachability.
- **Startup** = boot/migration not yet complete → not ready, not killed.

**Interaction with fail-static (§8):** readiness gates on a *dead* critical dependency; fail-static keeps
*already-authenticated* traffic alive through a *transient* hiccup of the **authz** dependency. The two are
complementary (ADR-17 §Consequences): a hard-down Id → Search reports not-ready for *new* unauthenticated
requests; a brief Id hiccup → already-authenticated traffic survives on the bounded-staleness cache. The
metrics surface also exports the RED/USE/queue-depth/consumer-lag signals the Phase-5 drills read (§10, X-1).

### 4.4 Topology diagram

```
            ┌─────────── Public (Internet / CLI / git wire / API) ───────────┐
            │   thin STATELESS gateway: authenticate → inject trusted        │
            │   identity+causality headers → principal-aware shed lane (§7)  │
            └───────────────────────────┬───────────────────────────────────┘
                                        │ (trusted headers; tenant from TOKEN, not path)
   ┌───────────────────────── TRUST BOUNDARY ─────────────────────────┐
   │  Service A ── internal RPC (resilient client §6) ── Service B     │
   │     │  re-AUTHORIZES every call (check/list_objects) — never      │
   │     │  trusts the header for authorization (§4.2)                 │
   │     └── metrics-health port: liveness≠readiness (§4.3) ───────────┤
   └──────────────────────────────────────────────────────────────────┘
   reactions flow A→B and B→A only over the BUS (no sync cycles, §2.9)
```

---

## 5. The event-consumer template (ADR-04 + the D7 orchestrator gotchas, encoded once)

Every consumer (Search indexer, Refs builder, Notif router, OLAP appender, trigger engine, audit consumer,
agent inbox) is built from **one template** in `myelin-events`, so the correctness rules cannot be skipped
per-consumer. The template encodes ADR-04's semantics and the four D7 gotchas (BUS-3) as the *only* way to
write a consumer.

### 5.1 The template shape

```rust
pub trait EventHandler {
    /// Whitelist: the subjects THIS handler consumes — NEVER "*" (BUS-3, D7-i).
    fn subjects(&self) -> &'static [SubjectPattern];
    /// Idempotent on event_id (ADR-04.1). Returns Enqueued | Done | NonRetryable.
    fn handle(&self, ev: &EventEnvelope) -> HandleOutcome;
}

pub enum HandleOutcome {
    Done,                 // work is durably enqueued/applied → ack (D7-iii)
    NonRetryable(Reason), // malformed/poison → TERMINATE immediately, do not burn redelivery budget (D7-iii)
    Retry(Backoff),       // transient → nack with backoff
}
```

The template (not the handler) owns: binding the **durable consumer by name**, the ack, the dedup table, the
lag metric, and the prefetch bound. The handler owns only the idempotent work.

### 5.2 The encoded rules (each maps to a directive/gotcha + prior art)

1. **Idempotent on `event_id`** (ADR-04.1; BUS-3). The template keeps a per-consumer **dedup table**
   `(consumer, event_id) PRIMARY KEY` (or a bounded dedup cache backed by it); a re-delivered event whose id
   is present is a no-op `Done`. This is what makes at-least-once + idempotent ≈ effectively-once (Helland
   2012; *DDIA* ch. 11) — Myelin **does not chase true exactly-once** (EI-02 §4).
2. **Ack-after-enqueue, never before** (BUS-3; D7-iii). The template acks **only** after `handle` returns
   `Done` (work durably enqueued/applied). Acking before risks silent loss; acking after risks a
   redelivery — which idempotency (rule 1) absorbs. (Kafka/JetStream manual-ack semantics.)
3. **Whitelist subjects; never `*`** (BUS-3; D7-i). A handler declares the exact subjects it consumes. An
   over-broad subscription **head-of-line-blocks everything** behind one slow subject (EI-02/EI-03 §6.1).
   Consumers subscribe to **curated Signals**, not the raw Event firehose (BUS-4/ADR-19), except infra
   indexers/refs-builders that genuinely need the firehose — and those are reviewed explicitly.
4. **Bind the durable consumer by name; never re-declare its start policy on reconnect** (BUS-3; D7-ii). On
   reconnect the template **looks up** the durable consumer by name and resumes; it never re-asserts a start
   position (re-declaring `DeliverAll`/`DeliverNew` on reconnect can wedge the broker or replay the world).
   This is the single most operationally expensive JetStream/Kafka mistake the doctrine names.
5. **Terminate non-retryable (malformed) messages immediately** (BUS-3; D7-iii). A poison message returns
   `NonRetryable` → the template **terminates** it (acks + dead-letters with the reason) rather than nacking
   it back into the redelivery budget, where it would starve the consumer. (Dead-letter pattern.)
6. **Bounded prefetch** (EI-02 §5; X-3). The consumer's in-flight window is bounded; an unbounded prefetch is
   a future cascade (§7).
7. **Monitor consumer lag** (BUS-3; D7-i). The template exports `pending` / `num_pending` / age-of-oldest-
   un-acked as a first-class metric (§10); consumer-lag is a Phase-8 ops gate (E-5). Lag is the early-warning
   signal that a handler has fallen behind or a subject is blocking.

### 5.3 Causality through the consumer (BUS-5)

When a handler emits a *reaction* event, it calls `OutboxTx::emit(draft, cause = Some(incoming))` so the
child's `correlation_id` (root) carries, `causation_id` = the incoming `event_id`, and `depth` = incoming
`depth + 1` (§2.1; EI-02 §6). This is what makes the **loop guard structural** (AG-6): the depth ceiling and
the shared-causal-root-within-a-window tripwire read these fields, so no convention/typo can defeat them.

### 5.4 The reactive/dispatch tier is a separate design (ADR-19 / D7)

Per ADR-19 and D7, the **Signal curation / Automation-rule / stateful-Trigger** tier (above the raw consumer
template) gets an **explicit, separately-reviewed design** in the Bus/trigger-engine Phase-3 doc — it is
*not* folded into this template. This template is the **floor every consumer stands on**; the dispatch tier
is the named follow-on owned by the Bus deliverable.

---

## 6. The shared resilient inter-service client (`myelin-client`)

ADR-16 §4 / directive X-3 / EI-02 §5: **one** client every outbound inter-service call goes through, so
timeout/breaker/bulkhead/retry is correct in exactly one place. Prior art: Nygard *Release It!* (Circuit
Breaker, Bulkhead, Timeout); Netflix Hystrix; AWS Builders' Library timeouts/retries/backoff; gRPC
deadlines.

### 6.1 The four primitives (all mandatory, all on by default)

```rust
pub struct ResilientClient { /* per-target config */ }
impl ResilientClient {
    /// Every call: per-call TIMEOUT, BULKHEAD (bounded concurrency), through the BREAKER.
    /// Retry ONLY if idempotent, with jitter, and NEVER through a tripped breaker.
    fn call<R>(&self, target: Target, req: Req, idem: Idempotency) -> Result<R>;
}
pub enum Idempotency { Idempotent, NonIdempotent }   // retry gate (ADR-16 §4)
```

1. **Per-call timeout** (deadline). No unbounded wait; a slow dependency must not pin a caller's resources
   indefinitely (the classic cascade trigger — *Release It!*; SRE ch. 22). Deadlines propagate (a downstream
   call inherits the remaining budget).
2. **Circuit breaker** (closed → open → half-open). After a threshold of failures/timeouts the breaker
   **opens** and calls fail fast; after a cool-down it goes **half-open** to probe. **Never retry through a
   tripped breaker** (ADR-16 §4) — retrying into an open breaker is the textbook retry-storm amplifier.
3. **Bounded-concurrency bulkhead** (a semaphore per target). Caps in-flight calls to one dependency so its
   slowness can't exhaust the caller's threads/connections and take the caller down with it (*Release It!*
   Bulkhead). Saturation **fast-fails** (X-3), it does not queue unboundedly (§7).
4. **Jittered retry — idempotent calls only** (ADR-16 §4). Retries use **full jitter** exponential backoff
   (Brooker 2015: `sleep = random(0, base · 2^attempt)`) to avoid synchronised retry waves. A
   **`NonIdempotent`** call is **never** retried (a retried non-idempotent write is a duplicate-effect bug).

### 6.2 Retry-After honouring (the anti-retry-storm rule)

ADR-16 §3 / X-3 / EI-02 §5: **our own clients MUST honour `Retry-After`.** When a target sheds with
`429 + Retry-After` (§7), the resilient client **respects the header** as the floor of its backoff — a client
that ignores `Retry-After` turns shedding into a retry storm and defeats the protected human lane. This is a
hard requirement on the agent runtime and the CLI too (they link this client), not just service-to-service.

### 6.3 Defaults (the X-5 unit reconciliation for the client)

Timeouts in **milliseconds**; breaker thresholds as a failure *ratio over a rolling window* + a minimum
request count; bulkhead as an integer concurrency cap; backoff base in **ms** with full jitter. Concrete
values are per-target config (the auth hot path gets a tighter timeout than a batch indexer) and are
**[OPEN → P3]** per consuming system, but the *shape* and the *on-by-default* posture are fixed here.

---

## 7. Backpressure + principal-aware shedding (ADR-16; the protected human lane)

EI-02 §5 / ADR-16 / directive X-3. The dominant client is **a fleet of agents and CI jobs that hammer the
API at machine speed**; design for that load from day one. Prior art: Little's Law (queue depth ↔ latency),
SEDA (Welsh 2001, bounded stages), SRE ch. 21/22 (overload, cascading failures), AWS load-shedding,
weighted-fair-queueing (Demers 1989).

### 7.1 Bounded everything (the cascade-prevention floor)

**Every queue and pool is bounded; unbounded anything is a future cascade** (EI-02 §5). The substrate bounds:
consumer prefetch (§5.2-6), the DB connection pool (§3.3, fast-fail + statement timeout), the bulkhead per
inter-service target (§6.1), per-tenant in-flight work, and the HTTP intake queue. A bounded queue that fills
**fast-fails** (sheds) rather than growing latency unboundedly — Little's Law makes "unbounded queue" mean
"unbounded latency," which is indistinguishable from down.

### 7.2 The principal-aware limiter + the protected human lane

The limiter is **principal-aware** (it reads the `Principal.kind` and the run's class from the injected
identity headers, §4.1). It maintains separate admission lanes and **reserves a protected lane for
interactive humans** so a person never queues behind an agent storm (EI-02 §5; weighted-fair-queueing /
priority classes). The **shed order under pressure** (ADR-16, doctrine §5):

```
shed first  →  speculative  →  batch / CI  →  agent  →  human  ←  shed last (protected)
```

- **Speculative** work (prefetch, optimistic precompute) is dropped first — it was never promised.
- **Batch/CI** and **agent** lanes shed next, receiving **`429 + Retry-After`** (and our clients honour it,
  §6.2).
- The **human lane** is shed **last** and only in true saturation; the goal is that an interactive human
  request survives a 30× agent surge (the headline drill, §11 / T-5).
- **Per-tenant fairness**: shedding is also **per-tenant** so one tenant's surge does not shed another
  tenant's humans (EI-02 §1 blast-radius; the drill asserts "other tenants unaffected").

### 7.3 Why this order (the justification, not just the list)

The order encodes *promise strength*: speculative work made no promise; batch/CI/agents are machine clients
that can and should back off; humans are the interactive principal the product exists for. This is Myelin's
**deliberate deviation-or-confirmation point**: the doctrine names exactly this order, and we adopt it
verbatim because the dominant-client analysis (agents at machine speed) makes "protect the human last-resort
lane" the only ordering that keeps the product usable under the load it is actually built for. We do **not**
shed by raw FIFO (which would starve humans behind an agent burst) — that is the anti-pattern being beaten.

### 7.4 Agent-generated load specifically

Agents wake agents; an unbounded dispatch fans out into a cascade. The substrate's dispatch worker pool is
**bounded and drops over-cap (never forks)** (AG-6); combined with the causal-depth ceiling and the
shared-root-within-a-window tripwire (§5.3, AG-6) and per-tenant circuit breakers (ADR-08.6), agent load is
structurally capped. The reserve/settle cost gate (D8/CI-2) is the economic backstop: **no balance → no
execution**, so a runaway loop is self-limiting.

### 7.5 Bounded predicate evaluation (the query-AST DoS surface)

The shared `EventMatcher`/saved-view predicate core (`myelin-query`, ADR-07) is **declarative and
safe-to-evaluate — no Turing-complete predicates on hot paths** (ADR-07; AG-7). The substrate enforces a
bounded-evaluation guard (step/time ceiling per predicate) so a crafted matcher cannot DoS the trigger
engine — one safe-evaluation engine, one DoS-hardening surface.

---

## 8. Fail-static primitives (ADR-17; bounded-staleness, not fail-closed)

ADR-17 / directive ID-1 / GD-3 / EI-02 §10. **Distinguish fail-closed from fail-static**:

- **Fail-closed is correct for *authorization correctness*** — deny when genuinely unsure (ADR-03 unchanged).
- **Fail-static is the correct *availability* default** — on a transient dependency hiccup, serve a
  **bounded-staleness cached answer** so *already-authenticated* traffic keeps working, rather than failing
  every request closed and turning one shared dependency (Id) into a whole-platform cascade — *the textbook
  single-point cascade* (EI-02 §10; SRE ch. 22 "degrade gracefully").

Prior art: CDN/HTTP **stale-while-revalidate** (RFC 5861), DNS serve-stale, SRE graceful degradation,
Zanzibar consistency tokens (the staleness is *expressed* via the zookie/`Consistency` mode, §2.2).

### 8.1 The primitive

```rust
/// A bounded-staleness cache around a critical dependency answer (esp. authz: "actor active / coarse grants").
pub struct FailStatic<T> {
    fresh_ttl: Seconds,          // serve fresh within this
    static_max: Seconds,         // serve STALE (with a degraded marker) up to here on dependency hiccup
    // static_max MUST be ≤ the deprovision/revocation SLA (ID-1, GD-3) and ≥ the agent-token TTL
}
impl<T> FailStatic<T> {
    fn get(&self, key: K, refresh: impl Fn() -> Result<T>) -> Answer<T>; // Fresh | Static(degraded) | Closed
}
```

On a dependency hiccup: within `fresh_ttl` → fresh; between `fresh_ttl` and `static_max` → **serve stale,
mark the answer degraded, attempt background refresh** (stale-while-revalidate); past `static_max` → **fail
closed** (the staleness budget is exhausted; deny is now correct). For **authorization**, the static answer
is the coarse "actor still active / coarse grants" — never an *escalation* of access (we never fail *open*).

### 8.2 The staleness bound (the deliberate, DPO-ratified trade-off)

`static_max ≤ the deprovision/revocation SLA` and **must contain the short-lived agent-token TTL** (ID-1, GD-3,
ADR-17). This makes the window a written, bounded exposure: a *revoked* actor still falls inside the window
and is denied once the window closes; an agent token (whose life == run life) expires inside the window.
**The DPO ratifies the chosen bound** (L-1; it is the residual GDPR-revocation exposure window). The bound is
a **decision-shaped call** handed to Legal/DPO, not an engineering default we set silently.

### 8.3 Interaction with readiness (§4.3)

Fail-static handles a **transient** hiccup (keep already-authenticated traffic alive). A **hard-down**
critical dependency is handled by **readiness** (the instance reports not-ready and sheds new traffic). The
two compose: fail-static buys the seconds-to-minutes of a blip; readiness handles a sustained outage. Both
are asserted by the Id-hiccup drill (§11 / T-5).

---

## 9. Forward-only online migrations (expand → backfill → contract)

EI-02 §8 / directive STOR-2 / §(e) prior. **Migrations are forward-only and online: no rollback migrations
(you can't un-delete data); never a blocking `ALTER` on a hot table; measure lock time against a restore
first.** Prior art: Stripe "Online migrations at scale" (2017), GitHub **gh-ost** (2016), Vitess/PlanetScale
online DDL, Fowler's **ParallelChange / expand-contract**, the Strong-Migrations rule set.

### 9.1 The expand → backfill → contract algorithm

A schema change that touches a hot table is **three deploys**, never one blocking `ALTER`:

1. **Expand** — add the new column/table/index *additively and non-blockingly* (a nullable column; a
   `CREATE INDEX CONCURRENTLY`; a new table). The app writes **both** old and new (dual-write at the app
   layer, behind a flag). No reads of the new shape yet.
2. **Backfill** — populate the new shape in **bounded batches** (chunked by tenant/PK range, throttled,
   resumable), off the hot path. The backfill is idempotent and re-runnable (it shares the event-replay
   posture, E-6).
3. **Contract** — once the new shape is fully backfilled and verified, switch reads to it, stop writing the
   old shape, and *only then* drop the old column/table in a later deploy. The drop is itself
   non-blocking/low-lock.

**Forward-only** (no down migrations): you can't un-delete data, so "rollback" is a *new forward* migration,
not a reverse one (STOR-2). The `forward-only-migration` lint (§2.11) bans rollback files and blocking
`ALTER`s on flagged-hot tables.

### 9.2 Measure lock time against a restore first

Before any DDL on a hot table, **measure its lock time against a restored copy** of production-scale data
(STOR-2) — a "quick `ALTER`" on a small dev DB can be a minutes-long exclusive lock at scale. This ties
directly to the restore-verification machinery (ADR-18, §11): the restore drill produces the realistic copy
the migration is rehearsed against.

### 9.3 Online-DDL contract for cross-language services

A non-Rust subsystem (ADR-02 divergence) still obeys expand→backfill→contract and forward-only; the rule is a
**substrate law**, not a Rust-library feature. The harness's migration runner enforces the file convention;
the discipline is checked in CI (E-5).

---

## 10. The observability baseline (traces carry causality + tenant)

EI-02 §6 / directive X-1 / X-2. **Causality is a first-class primitive, not logging** (EI-02 §6).
Observability is part of the **pass condition** of every Phase-5 drill (T-1: "a property does not exist until
a drill forces the failure and observability watches the system survive"). Prior art: OpenTelemetry +
W3C Trace Context (`traceparent`/`tracestate`), Google **Dapper**, the **RED** (Wilkie) and **USE** (Gregg)
methods.

### 10.1 Every trace carries causality + tenant

The harness's trace middleware (§3.5) propagates, on **every** surface and every hop, alongside the W3C
`traceparent`:
- **`tenant`** + **`region`** (ADR-11) — so every span/metric/log is filterable by tenant and the breach/
  blast-radius scoping is mechanical.
- **The causality triple** — `correlation_id` (root), `causation_id` (immediate parent), `depth` — and the
  distinct **`caused_by`** human-action/session ref (BUS-5; ADR-13.2). These ride in headers next to the
  trace context (EI-02 §6) and are **the same fields the events carry** (§2.1), so the "why did this happen?"
  view, audit walk, distributed trace, and loop guard are **one mechanism**, not four (EI-02 §6).

This is the structural payoff: an audit/why-view *walks the causality graph* rather than guessing, and the
loop guard reads `depth`/`correlation_id` directly — a human can't typo into a loop (EI-02 §6).

### 10.2 The telemetry contract every shared system implements (X-1)

Every service exports, on the metrics-health port (§4.3), a **standard signal set** the Phase-5 drills read
as a **survival signal** (X-1):

| Signal | Method | Drill it feeds |
|---|---|---|
| Request rate / errors / duration, **per principal-kind + per tenant** | RED (Wilkie) | 30× agent-surge (human lane holds) |
| Utilisation / Saturation / Errors of each pool/queue | USE (Gregg) | overload / cascade |
| **Consumer lag** (`num_pending`, oldest-un-acked age) per consumer | — | event-loss / head-of-line (D7-i) |
| Outbox depth / dead-letter count | — | silent-data-loss (BUS-2) |
| Breaker state (open/half/closed), bulkhead rejections, `Retry-After` issuance | — | retry-storm / cascade |
| Fail-static: fresh/stale/closed answer ratio, staleness age | — | Id-hiccup / fail-static |
| Shed counts per lane (speculative/CI/agent/human) | — | agent-surge / human-lane-holds |
| Causal-root tripwire firings, dispatch-pool drops | — | causal-loop tripwire (AG-6) |

These are **named, not optional**: a shared-system Phase-3 doc that omits them fails X-1. The drills don't
just run the failure — they **assert against these signals**, which is what makes "proven" mean proven (T-4).

### 10.3 Tamper-evident audit is distinct from telemetry

Per AG-7/ADR-12.9: the **tamper-evident audit log** (every human + agent action) is a *separate*,
retention-bounded `PersonalDataHolder`, **not** the telemetry stream. Telemetry is operational and sampled;
audit is complete and tamper-evident. The substrate keeps them distinct so neither weakens the other.

---

## 11. Failure modes + the drills owed

For each property that can fail, the **quantified drill** that proves it (Phase 5 owns the strategy and the
thresholds; this section **enumerates the drills the substrate owes**, per the PROVE-IT mandate and T-5).

| # | Property / failure mode | Drill owed (quantified) | Owner | Directive/ADR |
|---|---|---|---|---|
| D-1 | **Silent data loss** (dual write) | Kill a service *between* DB commit and publish; assert the outbox relay delivers every committed event exactly-once-in-effect (zero ghost, zero lost). | Bus + substrate | BUS-2, ADR-04.3 |
| D-2 | **Event loss across reconnect / head-of-line** | Drop the broker connection mid-stream; assert **zero messages lost across reconnect** (durable consumer resumes by name, dedup absorbs redelivery) and a slow subject does not block others. | consumer template | BUS-3, T-5 |
| D-3 | **30× agent surge** | Drive a 30× agent/CI surge on one tenant; assert the **human lane holds** (interactive latency within budget), the **agent lane sheds** (429+Retry-After, clients honour it), and **other tenants are unaffected**. | §7 | ADR-16, T-5 |
| D-4 | **Id-hiccup / fail-static** | Inject a transient Id-dependency hiccup; assert **already-authenticated traffic stays up** on the bounded-staleness cache, the staleness never exceeds `static_max ≤ revocation SLA`, and a revoked actor is denied once the window closes. | §8 | ADR-17, ID-1, T-5 |
| D-5 | **Retry storm** | Trip a downstream breaker under load; assert callers **fail fast (no retry through the tripped breaker)** and honour `Retry-After` — no amplification. | §6 | ADR-16 §4 |
| D-6 | **Restore + cross-seam integrity** | Rebuild from backups; assert **no loss** and that OLTP rows ↔ blob ↔ search index ↔ event-log offsets restore to **one mutually consistent point** (no row pointing at a missing blob). | Storage + GDPR | ADR-18, STOR-4, T-5 |
| D-7 | **Cross-tenant IDOR** | Attempt a cross-tenant read via a path-tenant ≠ token-tenant; assert **zero cross-tenant read** and the `tenant-predicate` lint catches a tenant-less query at compile time. | §2.11, §4.1 | EI-02 §1, ID-3, T-5 |
| D-8 | **Causal-loop tripwire** | Adversarially construct an agent→agent loop; assert the depth ceiling + shared-root-within-window tripwire + bounded dispatch pool stop it (drops over-cap, never forks). | §5.3, §7.4 | AG-6, T-5 |
| D-9 | **Liveness≠readiness** | Kill a critical dependency; assert the instance reports **not-ready and sheds** (does not report healthy and keep failing), and liveness does **not** restart-storm. | §4.3 | X-2 |
| D-10 | **Online migration safety** | Run an expand→backfill→contract migration on a restored production-scale copy under load; assert **no blocking lock** beyond budget and zero downtime. | §9 | STOR-2 |

Each drill emits a **green artifact** when it passes; until then the property is **claimed, not proven**
(T-4, EI-04 §4.3). The substrate's job here is to make every one of these drills *possible* by exposing the
telemetry (§10.2) and the failure-injection seams (a scoped, reversible dependency break — T-3).

### 11.1 The stateful-component register + blast-radius note (X-4)

Per X-4, the substrate enumerates its **stateful** components and a sharding/blast-radius plan; everything
else is stateless and replaceable:

| Stateful component | Shared-state / sharding plan | Blast radius if it dies |
|---|---|---|
| Per-service OLTP pool + **outbox table** | per-service DB, tenant-partitioned; read replica for hot paths (ID-4) | that service's writes + its un-drained events (recovered by the relay on restart) |
| **Per-consumer dedup table** | per-consumer, tenant-partitioned | at-worst a redelivery is re-processed (idempotency absorbs); no loss |
| **Durable consumer cursors** (in the broker) | bound by name; never re-declared (D7-ii) | resumes on reconnect; no loss |
| **Fail-static cache** | per-service, in-memory + backing store | a cold cache → brief fail-closed until warm; bounded by `static_max` |
| Breaker/bulkhead state | per-service, in-memory | resets on restart (fails safe: starts closed-but-probing) |

Everything else in the substrate (gateways, handlers, the resilient client, the migration runner) is
**stateless and horizontally replaceable** (EI-02 §10). The control plane holds **zero in-region personal
data** (ADR-11).

---

## 12. The contracts this doc exposes (the stable surface other Phase-3 systems link)

The explicit, stable list other Phase-3 systems and Phase-4 subsystems consume. Changing any of these is a
single workspace PR that breaks every consumer's build *now* (ADR-01), never silently in production.

| Contract | Crate | Consumed by |
|---|---|---|
| `EventEnvelope` + the canonical field list (§2.1, §2.10) | `myelin-events` | every emitter + every consumer |
| `OutboxTx::emit(draft, cause)` — the ONLY emit path | `myelin-events` | every state-changing handler |
| `EventHandler` template + `HandleOutcome` (§5) | `myelin-events` | every consumer (Search/Refs/Notif/OLAP/Agents/Audit) |
| `Principal` / `AuthzClient::{check,list_objects,delegation}` (§2.2) | `myelin-identity` | every gateway, Search, Refs, Agents |
| `ArtifactRef` parse/format + `Refs::{edges,backlinks}` (§2.3) | `myelin-refs` | every subsystem (projection API), Refs |
| `AgentRuntime::step` / `ToolHands::exec` / `ToolSurface::register_tool` (§2.4) | `myelin-agent` | Agent Fabric, every tool-contributing subsystem |
| `PersonalDataHolder` + `BlobStore` put/get/head/delete (§2.5, §2.7) | `myelin-gdpr` | every store, DSR orchestrator |
| `ResilientClient::call` + `Retry-After` honouring (§6) | `myelin-client` | every inter-service caller, CLI, agent runtime |
| `serve(AppSpec)` bootstrap (§3) | `myelin-substrate` | every service `main.rs` |
| The three-surface topology + liveness/readiness rule (§4) | convention + harness | every service |
| The principal-aware shed lane + shed order (§7) | harness + gateway | every public surface |
| `FailStatic<T>` + the staleness-bound rule (§8) | substrate | Id (primary), any critical-dependency caller |
| The expand→backfill→contract + forward-only rule (§9) | migration runner | every schema owner |
| The telemetry signal set (§10.2) | harness | every shared system (X-1), every Phase-5 drill |
| The architecture lints (§2.11) | CI | every crate |

---

## 13. Open questions handed to Phase 4 (and the within-Phase-3 floors)

**Named floors (within Phase 3, follow-ons owned elsewhere):**
- The **reactive/dispatch tier** (Signal curation / Automation-rule / stateful-Trigger) is a **named floor
  here** — the consumer template (§5) is the floor; the dispatch tier is the Bus Phase-3 deliverable's
  separately-reviewed design (ADR-19/D7/§5.4).
- The **concrete resilient-client values** (timeouts, breaker thresholds per target) are `[OPEN → P3]` per
  consuming system; the *shape* and *defaults-on* posture are fixed (§6.3).
- The **fail-static staleness bound value** is a **decision-shaped call for the DPO** (L-1); the *mechanism*
  and the `≤ revocation-SLA` constraint are fixed (§8.2).

**Open questions for Phase 4 (subsystems):**
1. **Cross-language harness parity** — a subsystem that diverges from Rust (ADR-02; chat connection tier
   TE-21) needs an equivalent of `serve(AppSpec)` in its language that enforces the same non-negotiables
   (outbox, three ports, liveness/readiness, forward-only migrations). What is the minimum the wire contract
   + a thin per-language shim must provide? (Owner: the diverging subsystem's P4 agent.)
2. **Per-table "hot" flagging** for the `forward-only-migration` lint (§9) — which tables are hot enough to
   require expand→backfill→contract is per-subsystem and measured, not predicted (ADR-10 anti-premature-
   shard). Each subsystem flags its hot tables.
3. **Per-surface shed budgets** (§7) — the concrete per-tenant in-flight caps and the human-lane reservation
   size are per-subsystem load profiles; CI (heaviest) and Chat (connection storms) will differ.
4. **Sub-artifact `ArtifactRef` granularity** (§2.3) — each subsystem must expose stable IDs down to the
   sub-artifact (a PR comment, a doc block, a CI step); the exact `#sub` scheme is per-subsystem (REF-3).
5. **Service-to-service authn mechanism inside the trust boundary** (§4.2) — mTLS vs a signed internal
   credential is Id's Phase-3 detail; the *rule* (assert the boundary, don't presume it) is fixed here.

**Within-Phase-3 dependencies this doc creates** (the other P3 agents must honour these contracts): Id (§2.2
authz client + fail-static surface + ID-4 replica), Bus (§2.1 envelope/outbox + §5 consumer template + §5.4
dispatch tier), Refs (§2.3), Search (§5 + §10.2 lag + reindex-from-source SEARCH-1), Notif (§5 + NOTIF-1
backend humanisation), Agents (§2.4 + §7.4 + AG-6), Storage (§2.7 blob trait + §9 migrations + STOR-4
cross-seam point), GDPR/Audit (§3.4 auto-registration + §8.2 staleness ratification + §10.3 audit-distinct).

---

## 14. Cross-references

- [`VISION.md`](../../VISION.md) — world-scale, GDPR-by-construction, agent-native, top-tier UX, Rust-default.
- [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) (EI-02) —
  the doctrine this doc operationalises (§1 tenant, §3 bus, §4 outbox, §5 backpressure, §6 causality, §8
  storage/migrations, §9 harness/topology, §10 fail-static/blast-radius, §11 restore).
- [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) (EI-04) — the
  honesty discipline + reindex-from-source as a resilience primitive (§5.3).
- [`architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md) — ADR-01 (crates),
  ADR-04 (bus semantics), ADR-11/12/13 (cells/GDPR/glue), **ADR-16** (backpressure), **ADR-17** (fail-static),
  **ADR-18** (restore-verification).
- [`02b-doctrine-integration/integration-directives.md`](../02b-doctrine-integration/integration-directives.md)
  — X-1…X-5, BUS-2/3, STOR-1/2/3/4, ID-1/3/4, GD-3, AG-1/2/6.
- [`02b-doctrine-integration/decision-record.md`](../02b-doctrine-integration/decision-record.md) — §(c)
  D1/D2/D3/D7, §(e) stronger priors.
- [`shared-systems-overview.md`](../02-holistic-architecture/shared-systems-overview.md) — the §10 inter-
  system glue + §12 P3 backlog this doc's contracts serve.
- **Seeds the rest of Phase 3:** every other `03-*` doc links the §12 contracts and honours the §13
  dependencies.
```
