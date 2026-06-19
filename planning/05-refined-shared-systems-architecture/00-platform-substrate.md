# Phase 5 — Platform Substrate & Foundations (the refined, canonical shared-system architecture)

> Phase: `05-refined-shared-systems-architecture`. Deliverable `00` — the **refined, canonical** substrate
> doc that Phase 6 (roadmaps) and Phase 7/8 (build) stand on. It **supersedes**
> [`planning/03-shared-systems-architecture/00-platform-substrate.md`](../03-shared-systems-architecture/00-platform-substrate.md).
> Canonical brief: [`VISION.md`](../../VISION.md) (single source of truth, never contradicted). Binding
> doctrine: [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
> (EI-02 — the substrate's home doc) and
> [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) (EI-04).
> Reconciliation spine: [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) (X-1..X-7,
> OQ-A..OQ-L) + [`contract-index.md`](./contract-index.md) (the frozen build-to surface). Spine ADRs:
> [`architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md) (ADR-01, ADR-04,
> ADR-16, ADR-17, ADR-18; cross-refs ADR-11/12/13) +
> [`02b-doctrine-integration/integration-directives.md`](../02b-doctrine-integration/integration-directives.md)
> (X-1…X-5, BUS-2/3, STOR-1/2/3/4, ID-1/3/4, GD-3, AG-1/2/6). Date: 2026-06-19.
>
> **What this doc is.** The shared **crates + conventions** every Myelin service is built from: the
> trait/type surface of the glue crates, the **bootstrap harness** (`serve(AppSpec)`), the **three-surface
> topology**, the **event-consumer template**, the **resilient inter-service client**,
> **backpressure/shedding**, the **fail-static** primitives, **forward-only online migrations**, and the
> **observability baseline**. It is the contract floor all other shared systems and all subsystems consume.
> It does **not** re-decide an ADR; **no ADR is reversed** (none was requested — change-requests §14.1). The
> Phase-5 work here is **confirmation + additive sharpening + applying the reconciliation decisions**.

---

## Changes vs Phase 3 (every change, with its kind)

The substrate is the doctrine's "cheap to get right, brutal to retrofit" layer; almost all of its Phase-3
design is **CONFIRMED unchanged**. The genuine Phase-5 deltas are additive sharpenings and a few new
seam-shapes the subsystems' Phase-4 work surfaced. Tags: **CONFIRM** (ratified as written), **SHARPEN**
(stood, now frozen concrete), **NEW** (named first in Phase 5). Each cites the contract-index row it serves.

| # | Change | Kind | Where (this doc / index row) |
|---|---|---|---|
| C-1 | **Lint set extended** — the §2.11 lint table gains six lints the Phase-4 work made load-bearing: `residency-pin`, `control-plane-pii-free`, `search-requires-acl-filter`, `no-llm-in-platform`, `no-untagged-personal-data`, `flow-determinism`. | SHARPEN | §2.11 / index 1.6 |
| C-2 | **Cross-language harness shim frozen** — the minimum a non-Rust subsystem (Chat BEAM/TE-21 connection tier) must satisfy is pinned as a frozen *divergence contract*, not left `[OPEN → P4]`. A no-op if Chat stays Rust; the hatch is real either way. | SHARPEN | §3.7 / index 1.7 |
| C-3 | **Hot-table declaration frozen** — every subsystem declares its hot tables for the `forward-only-migration` lint; the seed set (KN `block`/`db_row`/`doc_op`; all high-write subsystems) is named. | SHARPEN | §9.4 / index 1.5 |
| C-4 | **Per-surface shed-budget floor table (OQ-K)** — the shed order is CONFIRMED; Phase 5 names the per-surface v1 budget *floor* (CI-surge / collab op-stream / connection-storm / agent-mention-storm) tuned by drills. | SHARPEN (floors named) | §7.6 / index 1.11 |
| C-5 | **Telemetry signal set pinned to the drill survival signals** — the §10.2 set is unchanged but is now explicitly the Phase-5 drill pass-condition surface, with consumer-lag / outbox-depth / breaker-state / fail-static / shed-counts / causal-depth named as drill inputs. | CONFIRM (pinned) | §10.2 / index 1.8 |
| C-6 | **Fail-static bound is `[OPEN — LEGAL]` (DPO ratifies)** — the mechanism + `static_max ≤ revocation SLA ≥ agent-token TTL` constraint are CONFIRMED; the *value* is explicitly a DPO-ratified call (L-1), flagged for counsel. | CONFIRM (legal flag) | §8.2 / index 1.10, 4.11 |
| C-7 | **Firehose resume-cursor transport is a substrate-adjacent seam (OQ-J)** — the durable resume-cursor / scope-bounded subscription protocol (`subscribe`/`resume`/`scope`) is Bus-owned but rides the substrate's bounded-queue + shed discipline; the substrate's role (per-connection in-flight caps, `resync_required` shed) is named here. | SHARPEN (seam named) | §7.7 / index 3.5 |
| C-8 | **`myelin-content` WASM compile target** — the one editor render path reuses the Rust `myelin-content` core client-side (`render(parse(md)) === md`); the substrate build-system carries the WASM target. | CONFIRM (build-system) | §2.5 / index 13.1 |
| C-9 | **`EventEnvelope` field-name alignment to the frozen anchor** — the §2.1 envelope is restated to match the canonical field list `00 §2.10` + the index 2.1 anchor exactly (adds `recorded_at`, `aggregate`, `data_role`, `pii_key_ref` names already implied; no semantic change). The envelope remains **the names/units authority** (X-5). | CONFIRM (alignment) | §2.1, §2.10 / index 2.1, 14 |
| C-10 | **`list_objects` result shape note** — the substrate's `AuthzClient::list_objects` returns the frozen `Ids | Filter{set_expr, zookie}` (OQ-E); the substrate pins only the call shape, the `SetExpr` algebra lives in Identity's doc. | SHARPEN (shape pinned) | §2.2 / index 4.3 |
| C-11 | **`#sub` granularity hand-off resolved** — Phase-3 §13 Q4 (per-subsystem `#sub` scheme) is no longer open; the unified grammar + tombstone ladder is frozen in Refs (X-4/OQ-D). The substrate `ArtifactRef` *type* is unchanged. | CONFIRM (open item closed) | §2.1 / index 5.7 |
| C-12 | **Phase-3 open-question disposition** — §13's five Phase-4 open questions are all now resolved/closed by the reconciliation; §13 here records their disposition rather than re-opening them. | CONFIRM | §13 |

**Unchanged from Phase 3 (the bulk).** The eight glue crates + their trait surfaces (§2), the bootstrap
harness `serve(AppSpec)` (§3), the three-surface topology + liveness≠readiness (§4), the event-consumer
template + the seven encoded rules (§5), the resilient client's four primitives + `Retry-After` honouring
(§6), the shed order + protected human lane (§7.2-7.5), the fail-static mechanism (§8), expand→backfill→
contract forward-only migrations (§9), the observability baseline + causality-on-every-trace (§10), the
stateful-component register (§11.1), and the dependency root (§2.9). These are **cited, not re-derived**
below; where a section is verbatim-confirmed it says so.

---

## 0. Purpose, responsibilities, and the one-paragraph thesis

**Unchanged from Phase 3 §0.** The substrate is the set of crates and conventions that are **cheap to get
right at the start and brutal to retrofit** (EI-02 preamble). Get them right and every shared system and
subsystem becomes a thin projection over identical plumbing; get them wrong and every service inherits the
wound. The nine substrate concerns are: the glue crates (§2), the bootstrap harness (§3), the three-surface
topology (§4), the event-consumer template (§5), the resilient inter-service client (§6), backpressure +
principal-aware shedding (§7), the fail-static primitives (§8), forward-only online migrations (§9), and the
observability baseline (§10).

**Thesis (verbatim from Phase 3).** *A new Myelin service is a `main.rs` that calls
`myelin_substrate::serve(AppSpec)` and supplies its handlers, its migrations, and its event-consumer
registrations. Everything load-bearing for correctness — the outbox, idempotency, tenant-scoping, the
resilient client, the shed lane, fail-static, the three ports, the trace context — comes from the crates, so
it cannot drift between services and cannot be skipped.* This is the mechanical embodiment of ADR-01
(contracts that cannot drift) and EI-02 §9 (many services, not many snowflakes).

### 0.1 The non-negotiables every crate and service inherits (CONFIRMED, Phase 3 §0.1)

Carried unchanged from EI-02 / ADR-11 / ADR-12:

- **Tenant is the first column / partition key of everything** (EI-02 §1; ADR-11.2). **No cross-tenant query
  path.** Tenant comes from the **verified token, never the URL path** (ID-3). A missing tenant predicate is
  a top-tier security bug (an IDOR), caught by the `tenant-predicate` lint at compile time.
- **Every store is residency-pinned, per-tenant envelope-encrypted, crypto-shred-capable, and a
  `PersonalDataHolder`** (ADR-11/12). The substrate provides the seams; it never lets a service opt out. The
  `residency-pin` and `no-untagged-personal-data` lints (§2.11, new) make this structural.
- **No subsystem/shared-system reads another's store** (ADR-01, ADR-13). Cross-system interaction is via the
  frozen contracts only. Enforced by `no-cross-db`.
- **The transactional outbox is the ONLY sanctioned emit path** (EI-02 §4; BUS-2). `myelin-events` exposes
  **no fire-and-forget publish**; a shortcut that exists will be used and will lose data.

---

## 1. Prior art this substrate stands on (CONFIRMED, Phase 3 §1)

The cited-prior-art table is **unchanged from Phase 3 §1** and remains the defaults-to-beat. The load-bearing
citations, in brief (see Phase-3 §1 for the full table with editions):

| Concern | Prior art / proven system |
|---|---|
| Transactional outbox / dual-write hazard | Richardson *Microservices Patterns* (2018) ch. 3.2; Kleppmann *DDIA* (2017) ch. 11; Debezium outbox; `FOR UPDATE SKIP LOCKED` (PostgreSQL ≥ 9.5) |
| Durable streaming log + durable pull consumers | NATS **JetStream**; Kafka consumer groups; Redpanda (ADR-04) |
| At-least-once + idempotent ≈ effectively-once | Helland "Idempotence Is Not a Medical Condition" (2012); *DDIA* ch. 11 |
| Circuit breaker / bulkhead / timeout / retry | Nygard *Release It!* (2nd ed. 2018); Netflix Hystrix; AWS Builders' Library (Brooker) |
| Jittered exponential backoff | Brooker, "Exponential Backoff And Jitter" (AWS, 2015) — *full jitter* |
| Backpressure / bounded queues / shedding | Little's Law; Welsh et al. **SEDA** (SOSP 2001); AWS load-shedding; Google **SRE** ch. 21/22 |
| Fairness / priority lanes | Weighted fair queueing (Demers/Keshav/Shenker, SIGCOMM 1989); per-tenant quotas |
| Fail-static / bounded-staleness | SRE ch. 22; stale-while-revalidate (RFC 5861); Zanzibar zookies (Pang et al., OSDI 2019) |
| Liveness vs readiness | Kubernetes probe semantics; 12-factor disposability |
| Online schema change / expand-contract | Stripe "Online migrations at scale" (2017); GitHub **gh-ost** (2016); Vitess online DDL; Fowler ParallelChange |
| Tracing + causality propagation | **OpenTelemetry** / W3C Trace Context (Rec 2020); Google **Dapper** (2010); ADR-13.2 causality triple |
| Content-addressed blob store | Git object model; S3-compatible `put/get/head/delete`; ADR-10/STOR-1 |
| One polymorphic principal | Zanzibar (OSDI 2019); ADR-03/08/13.3 |
| RED/USE telemetry | Wilkie "RED method"; Gregg "USE method" |
| **Resume-cursor / scope-bounded streaming (NEW seam, §7.7)** | stale-while-revalidate + durable-cursor replay (the EI-04 §2.2 "build the resume-cursor transport FIRST" mandate); JetStream durable-consumer-by-name resume |

Where Myelin deviates (the specific shed order §7.3, the fail-static staleness bound §8.2) it is called out
in writing in the relevant section. No deviation is introduced in Phase 5; the existing ones stand.

---

## 2. The shared crates — responsibility + trait/type surface

The crate table is ADR-01's, ratified and **unchanged**. This section pins only what crosses a crate boundary
(ADR-01: that is the thing that must not drift). Internals belong to each owning system's own doc.

> **Convention (CONFIRMED).** Snippets are Rust-shaped signatures (ADR-02: glue crates are Rust,
> non-negotiably). Cross-language services consume the same surface as a **wire contract** generated from
> these types. §2.10 is the canonical envelope field list + units that reconciliation anchors on (X-5); it is
> **unchanged** and remains the names/units authority (index §14).

### 2.1 `myelin-events` — envelope, `ArtifactRef`, outbox helper, consumer template (CONFIRM; envelope aligned to the anchor, C-9)

**Responsibility (unchanged).** Owns the **canonical event envelope** (ADR-13.2), the **`ArtifactRef`** type
(the envelope embeds it; parse/format/resolve live in `myelin-refs`), the **transactional-outbox helper**
(the ONLY emit path, BUS-2), the **event-consumer template** (§5), and the event taxonomy *types*.

**Surface (the contract) — restated to match the frozen anchor (index 2.1, `00 §2.10`):**

```rust
/// The non-negotiable, versioned envelope (ADR-13.2). schema_ver gates evolution.
/// This field list IS the names/units authority (X-5); every emitter + consumer aligns to it.
pub struct EventEnvelope {
    pub event_id: EventId,            // ULID; the idempotency key (ADR-04.1)
    pub type_: EventType,             // dotted name "<subsystem>.<artifact_type>.<event_name>" (Bus §6 grammar)
    pub schema_ver: u32,              // upcasters bridge versions at consume (forward-only)
    pub tenant: TenantId,             // partition + residency key (ADR-11) — FIRST-CLASS, never optional
    pub region: Region,
    pub actor: Actor,                 // Principal ref incl. on_behalf_of (ADR-13.3)
    pub subject: ArtifactRef,         // what this event is about (ADR-13.1); may carry a #sub anchor (5.7)
    pub aggregate: AggregateKey,      // the per-(aggregate, seq) ordering key (UNIQUE(aggregate, seq); index 2.3)
    pub causation_id: Option<EventId>,    // IMMEDIATE parent (BUS-5: nested, not flat)
    pub correlation_id: CorrelationId,    // the causal ROOT — carries through (BUS-5)
    pub caused_by: Option<CausedBy>,      // distinct human-action/session ref (BUS-5)
    pub depth: u32,                       // causal depth; loop ceiling reads this (AG-6)
    pub contains_personal_data: bool,     // routes GDPR handling (ADR-04.4)
    pub data_role: DataRole,              // controller | processor (tenant-content) — GDPR fan-out
    pub visibility: Visibility,
    pub pii_key_ref: Option<PiiKeyRef>,   // kms://<tenant>/<dek-epoch>/<class>; inline-PII events envelope-encrypted (index 2.7)
    pub occurred_at: Timestamp,           // RFC-3339 UTC (the unit anchor)
    pub recorded_at: Timestamp,           // RFC-3339 UTC; when the log durably accepted it
    pub payload: serde_json::Value,   // references-not-payloads: IDs/ArtifactRefs, never PII bodies
}

/// The ONLY sanctioned emit path (BUS-2). Inserts into the per-service `outbox` table IN THE SAME
/// TRANSACTION as the state change. There is NO fire-and-forget publish (`no-raw-publish` lint).
pub trait OutboxTx {
    /// Derives causality from the cause so it is correct-by-construction (BUS-5, EI-02 §6):
    /// root carries, parent = cause.event_id, depth = cause.depth + 1.
    fn emit(&mut self, draft: EventDraft, cause: Option<&EventEnvelope>) -> Result<EventId>;
}

/// Canonical URN. Parsing/formatting/resolution live in myelin-refs (REF-3); the TYPE lives here.
/// myelin://<tenant>/<subsystem>/<type>/<id>[#sub] — #sub grammar frozen in Refs (index 5.7, X-4).
pub struct ArtifactRef(String);
```

**Why these choices (unchanged from Phase 3 §2.1).** ULID `event_id` gives the broker a stable dedup key and
the DB an index-friendly PK without coordination. `causation_id` (immediate parent) + `correlation_id` (root)
is the nested model BUS-5/D7(iv) mandate. `emit(draft, cause)` **derives** child provenance so a human or
agent **cannot typo their way into a loop** (EI-02 §6). The outbox helper is the only API; there is
intentionally no `publish(event)`.

**Phase-5 alignment (C-9, C-11).** The field names above match the frozen anchor (index 2.1) exactly —
`aggregate`, `data_role`, `recorded_at`, `pii_key_ref` are named (they were implied in Phase 3; no semantic
change). The `subject` may carry a `#sub` sub-anchor whose grammar is **now frozen** in Refs (the unified
`#sub` grammar + tombstone ladder, X-4/OQ-D) — Phase-3 §13 Q4 ("per-subsystem `#sub` scheme") is therefore
**closed**, not open. New event tokens `ci.check.updated`, `ci.result`, and the type token `initiative` are
registered under the §6 grammar (index 2.9) — the grammar itself is unchanged.

### 2.2 `myelin-identity` — `Principal`, capability types, the authz client (CONFIRM; `list_objects` shape pinned, C-10)

**Responsibility (unchanged).** The **one polymorphic `Principal`** (Human / Agent / Service — kind is data,
not a code branch; EI-02 §2), the capability/permission types, and the **authz client** every service links
(no service re-implements the check; ADR-13.3). The tuple store and decision engine are Identity's own doc.

**Surface — the call shapes pinned to the frozen contracts (index 4.2/4.3/4.5):**

```rust
pub struct Principal { pub id: PrincipalId, pub kind: PrincipalKind, pub tenant: TenantId, /* … */ }
pub enum PrincipalKind { Human, Agent { runtime_ref: RuntimeRef, on_behalf_of: Option<PrincipalId> }, Service }

pub trait AuthzClient {
    /// Per-action gate, fail-closed (ADR-03). The CaveatContext carries field/transition ABAC,
    /// evaluated HERE (off the hot list_objects path) — OQ-E. None for the common case.
    fn check(&self, subject: &Principal, perm: Permission, object: &ArtifactRef,
             at: Consistency, caveat: Option<CaveatContext>) -> Result<Decision>;
    /// The leak-free pre-filter (ADR-03). Returns a materialised id set OR a pushdownable Filter.
    /// The SetExpr algebra is frozen in Identity (OQ-E); the substrate pins only the call shape.
    fn list_objects(&self, subject: &Principal, perm: Permission, ty: ObjectType, at: Consistency)
        -> Result<ListObjectsResult>;          // Ids{ids, zookie} | Filter{set_expr, zookie}
    fn delegation(&self, agent: &Principal, trigger_actor: &Principal)
        -> Result<EffectivePolicy>;            // agent ∩ delegation ∩ tenant (ADR-08.3, monotone)
}

/// Consistency token ("zookie", Zanzibar) for read-your-writes; also the input to fail-static (§8).
pub struct Consistency { pub at_least: Zookie, pub mode: ConsistencyMode /* Strong | BoundedStale */ }
```

The substrate's contribution is unchanged: **`check`/`list_objects` flow through the resilient client (§6)
and are the canonical fail-static surface (§8).** Phase 5 pins the `list_objects` return as the frozen
`Ids | Filter{set_expr, zookie}` (the most load-bearing inter-system contract, OQ-E) and the optional
`CaveatContext` on `check` — but the `SetExpr` algebra and the reverse-index JOIN target are **Identity's**
deliverable (contract 4.3), not the substrate's. The substrate only guarantees the call rides the resilient
client and the fail-static cache honours the zookie revision watermark (index 4.10).

### 2.3 `myelin-refs` — URN parse/format/resolve + edge client (CONFIRM; `#sub` closed, C-11)

**Responsibility (unchanged).** Owns **`ArtifactRef` parsing/formatting/resolution** (one library; services
must not re-implement — REF-3) and the **edge/backlink client**. Rejects scope-less/ambiguous refs and
**never guesses scope**. Display keys (`#42`, `@alice`, `~general`) are render-time projections, never stored.

```rust
pub trait Refs {
    fn parse(s: &str) -> Result<ArtifactRef>;           // rejects ambiguity; never guesses scope (REF-3)
    fn format(r: &ArtifactRef) -> String;
    fn edges(&self, r: &ArtifactRef) -> Result<Vec<Edge>>;                  // outbound
    fn backlinks(&self, r: &ArtifactRef, viewer: &Principal) -> Result<Vec<Edge>>; // permission-filtered (REF-1)
}
```

Edge creation is **`refs.edge.created` emission via the outbox** (no separate edge-write API). Backlinks are
event-sourced projections; lifecycle/semantic edges are *also* mirrored to a typed table owned by the
authoritative subsystem (REF-1/TE-7). Defaults to Postgres + recursive CTEs for the shallow graph (REF-2).
**Phase-5:** the substrate `ArtifactRef` *type* is unchanged; the **`#sub` sub-artifact grammar + the 4-step
tombstone ladder are now frozen** in Refs (X-4/OQ-D, index 5.7), closing Phase-3 §13 Q4. The substrate does
not own the grammar — it owns the type that carries it.

### 2.4 `myelin-agent` — runtime/agent/tool traits (CONFIRM)

**Unchanged from Phase 3 §2.4.** The **strategy-pattern boundary** (VISION §3): the small trait set behind
which `MockAgentRuntime` lives now and `LlmAgentRuntime` lives later. **No LLM SDK / prompt / model name
appears in platform code** (ADR-08.2; the new `no-llm-in-platform` lint, §2.11, makes this structural). The
brain is a *stateless* `step` (AG-1); the hands is `exec` with **no host-execution bypass** (AG-2).

```rust
pub trait AgentRuntime { fn step(&self, conv: &Conversation) -> StepOutcome; }      // STATELESS brain (AG-1)
pub enum StepOutcome { UseTools(Vec<ToolCall>), Submit(Submission) }
pub trait ToolHands { fn exec(&self, cmd: Command) -> ToolResult; }                 // hands; no bypass (AG-2)
pub trait ToolSurface { fn register_tool(&mut self, def: ToolDef); }               // one catalogue (ADR-08.4)
```

**Phase-5 note (X-6, index 8.4).** `ToolHands::exec` **is** the CI runner's `kind=agent` job on the unified
sandbox; the real-kernel escape drill gates both kinds. The four uniform guarantees (cost gate, per-run-token
attribution, HITL withhold, isolation floor+drill) and the frozen `requires_approval` defaults table are the
Agent Fabric's deliverable — the substrate only owns the trait shape so subsystems register tools against a
stable surface.

### 2.5 `myelin-content`, `myelin-query`, `myelin-gdpr`, `myelin-tenancy`, `myelin-client` (substrate-relevant seams)

Owned by other docs; the **substrate-relevant** seams only:

- **`myelin-content`** (ADR-05) — block/inline taxonomy + the three platform-load-bearing inline nodes
  `mention(Principal)`, `artifact_ref(ArtifactRef)`, `embed(ArtifactRef)`. Inline content is a
  **markdown-subset string** (KN-2/D10); the three structured nodes are stored structured. Substrate role:
  these nodes are the **producers of `refs.edge.created`** (emitted via the outbox), and the crate has a
  **WASM compile target** (C-8, index 13.1) so the *one editor render path* reuses the Rust core client-side
  with `render(parse(md)) === md` on identical code (the D10 round-trip gate runs on the same bytes server-
  and client-side). The taxonomy itself is **frozen** (X-2/OQ-B) — Knowledge owns it, Chat/Issues consume
  strict subsets — but that freeze lives in Knowledge's doc; the substrate carries the WASM build target.
- **`myelin-query`** (ADR-06/07) — the single query AST + field/view primitive; **permission-aware by
  construction** (always composes with `list_objects`; ADR-07). Substrate role: the `EventMatcher` predicate
  core (Bus triggers) and saved-view filters **share the same `QueryAst`** (now frozen byte-identical,
  X-3/OQ-C) — one safe-evaluation engine, one DoS-hardening surface. The substrate provides the
  bounded-evaluation guard (§7.5): no Turing-complete predicates, no UDFs/loops/recursion, statically
  cost-bounded.
- **`myelin-gdpr`** (ADR-12) — the `PersonalDataHolder` trait (`locate/export/rectify/restrict/erase`) + DSR
  client + KMS/crypto-shred abstraction. Substrate role: **the harness auto-registers every store a service
  opens as a holder** (§3.4) — "we forgot a store" is structurally impossible (GD-3 + the new
  `no-untagged-personal-data` lint). The content-addressed `BlobStore` trait lives at this seam (§2.7).
- **`myelin-tenancy`** (ADR-11) — `TenantId` / `Region` / residency tags / cell-routing client types.
  Substrate role: the **tenant-scoping guard** every query path threads (the `tenant-predicate` lint) + the
  residency assertion (the new `residency-pin` lint, §2.11).
- **`myelin-client`** (ADR-16 §4) — the **shared resilient inter-service client** (§6). The one place
  timeout/breaker/bulkhead/retry is correct for every caller.

### 2.6 `myelin-substrate` — the bootstrap harness crate (CONFIRM)

**Unchanged.** The harness crate (working name `myelin-substrate`) depends on all of the above and exposes the
**one-call `serve`** (§3). It is the only crate a service's `main.rs` must wire by hand.

### 2.7 The content-addressed blob trait (STOR-1; CONFIRM, with the Phase-5 BlobStore extensions noted)

```rust
pub trait BlobStore {                                    // hash-on-write; fs-vs-object is a one-line swap
    fn put(&self, bytes: &[u8]) -> Result<ContentHash>;  // content address = the hash (BLAKE3; per-tenant dedup)
    fn get(&self, h: &ContentHash) -> Result<Vec<u8>>;
    fn head(&self, h: &ContentHash) -> Result<BlobMeta>;
    fn delete(&self, h: &ContentHash) -> Result<()>;     // crypto-shred is the real erasure (ADR-12.3)
}
```

The trait is **unchanged**. Content-addressing gives dedup + integrity for free; the narrow trait keeps the
fs-vs-S3 choice a one-line swap (EI-02 §8). Erasure is **crypto-shred** (destroy the per-tenant/per-subject
key), not blob delete, for immutable/backup tiers (ADR-12.3). **Phase-5 (index 11.2, Storage's deliverable,
noted for completeness):** Storage adds an object-backed pack/delta seam (Git), a within-EU CDN clone/bundle
blob class, and **trust-tier/branch-scoped cache namespaces** (an `UntrustedFork` write cannot reach the
trusted cache scope — the poisoned-cache defence tied to X-1). These are **Storage's** additions over this
trait, not substrate changes; the trait surface above is what the substrate exposes.

### 2.8 What is deliberately NOT a crate (CONFIRM)

**Unchanged.** No shared "storage API" crate spanning subsystems (EI-02 §8). Each service owns its schema and
opens its own pool; the boundary is the `no-cross-db` lint, not a shared data-access crate. **Thin, visible
SQL over a heavy ORM** ("an ORM hides the data model you most need to see") — the harness provides a thin
query layer (a query builder + typed rows), not an ORM.

### 2.9 The dependency root (no cycles) (CONFIRM)

**Unchanged.** ADR-01/EI-02 §3: **no circular synchronous dependencies; identity depends on nothing.** The
crate DAG, root-last: `myelin-tenancy` → `myelin-identity` → `myelin-events` (+ `-refs`, `-content`,
`-query`) → `myelin-agent`, `myelin-gdpr` → `myelin-client` → `myelin-substrate`. Synchronous *service*
calls obey the same rule (if A calls B sync, B reacts to A only over the bus). The `no-cross-sync-cycle`
lint (E-5) enforces it.

### 2.10 The canonical envelope field list (the X-5 reconciliation anchor) (CONFIRM — the names/units authority)

**Unchanged and binding (index §14).** The envelope fields in §2.1 are the single canonical list. Units,
**frozen, never re-litigated** (restated once, aligned to the reconciliation §0):

- **timestamps** = RFC-3339 UTC (`occurred_at`, `recorded_at`);
- **budgets/costs** = integer minor-units (never floats — CI-2/D8 metering);
- **TTLs / staleness windows / timers** = seconds;
- **resilient-client timeouts** = milliseconds (§6.3);
- **`pii_key_ref`** = `kms://<tenant>/<dek-epoch>/<class>`, `<class> ∈ {tenant, subject:<id>, blob}`.

Every cross-component contract field reconciles against this section. This is one of the two highest-fan-in
reconciliation anchors (the other is the `ArtifactRef` token table, Bus §6.2) and is **CONFIRMED unchanged**.

### 2.11 Enforced architecture lints (E-5) — extended (SHARPEN, C-1)

The substrate ships these as `cargo`-level architecture tests / clippy-style lints, committed to CI (an
uncommitted gate is no gate). The Phase-3 six are **unchanged**; Phase 5 adds six the Phase-4 work made
load-bearing (these are the lints the reconciliation/index 1.6 froze):

| Lint | Rule | Status | Citation |
|---|---|---|---|
| `no-cross-db` | a service crate must not depend on another service's storage module | CONFIRM | ADR-01 |
| `no-raw-publish` | no bus publish outside `OutboxTx::emit` | CONFIRM | BUS-2 |
| `tenant-predicate` | every query-builder call carries a `TenantId` bound; a tenant-less query fails to compile | CONFIRM | EI-02 §1, ID-3 |
| `no-host-exec` | no host-execution path bypassing `ToolHands::exec` (= the unified sandbox, X-6) | CONFIRM | AG-2 |
| `forward-only-migration` | no rollback migration file; no blocking `ALTER` on a flagged-hot table (§9.4) | CONFIRM | STOR-2, §9 |
| `no-cross-sync-cycle` | the sync call graph is acyclic; identity is a sink | CONFIRM | EI-02 §3 |
| **`residency-pin`** | every store/stream/index/cache declares a region; no global pool; outbound transfer is gated (index 10.5) | **NEW** | ADR-11; recon §10 |
| **`control-plane-pii-free`** | the control plane (routing, cross-cell pointers) carries opaque ids only — never a name/email/body | **NEW** | ADR-11; recon §OQ-I |
| **`search-requires-acl-filter`** | every search/list query conjoins the `list_objects` `Filter` before scoring — pre-filter, never post-filter | **NEW** | ADR-03; recon §OQ-E |
| **`no-llm-in-platform`** | no LLM SDK / prompt / model name in platform code; the runtime is behind the `AgentRuntime` strategy seam | **NEW** | ADR-08.2; VISION §3 |
| **`no-untagged-personal-data`** | every schema field carrying PII is `#[personal_data(...)]`-tagged; an untagged PII column fails to compile | **NEW** | ADR-12; recon §10.2 |
| **`flow-determinism`** | a `myelin-flow` workflow body uses only the deterministic `WfCtx` surface; non-determinism journals | **NEW** | index 9.2; recon §OQ-F |

The new lints make structural the properties the subsystems' Phase-4 docs depended on (residency, the
control-plane PII-free rule, ACL-pre-filtered search, the no-LLM-in-platform strategy boundary, exhaustive
PII tagging, and workflow determinism). They are **additive**; no existing lint changes.

---

## 3. The shared bootstrap harness (a new service is a thin shell)

**§3.1–§3.6 are unchanged from Phase 3 §3** and are cited, not re-derived. §3.7 is the Phase-5 cross-language
shim freeze (SHARPEN, C-2).

### 3.1 The one call (CONFIRM, verbatim)

```rust
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
ports → serve until signalled → **graceful drain** (stop intake, finish in-flight, ack-then-exit). Non-zero
on failed boot.

### 3.2 Config (CONFIRM). Env-first (12-factor), typed, validated at boot (fail fast). Injects DB URL(s),
broker endpoint + durable-consumer names, KMS endpoint, region + cell id (ADR-11), telemetry endpoint, the
resilient-client defaults, and the shed-lane budgets (§7). Secrets are **resolved by reference inside the
trust boundary** (ADR-20), never baked into images.

### 3.3 DB pool + outbox publisher (CONFIRM). Bounded pool with a **statement timeout** and **fast-fail on
saturation** (the first bounded queue, §7.1). The **outbox relay** starts automatically (BUS-2): claims
unsent rows with `FOR UPDATE SKIP LOCKED` (safe across replicas), stamps the stable `event_id` for
broker-side dedup, publishes, marks sent, dead-letters after bounded retries. The relay is the *only*
component on the broker's publish side. **Read-replica awareness**: the pool routes reads to a replica; the
authz hot path is the likely first dedicated replica (ID-4) — measure before sharding (ADR-10).

### 3.4 PersonalDataHolder auto-registration (CONFIRM, GD-3). Every store the harness opens (OLTP schema, any
blob prefix, any cache namespace, the search index if owned) is **automatically registered as a
`PersonalDataHolder`**. The new `no-untagged-personal-data` lint (§2.11) plus this auto-registration make
"we forgot a store / a column" a **structural failure**, not a review miss.

### 3.5 Telemetry + the three ports (CONFIRM). The harness initialises the OpenTelemetry tracer/meter/logger
(§10), installs the **causality+tenant trace context** middleware on every surface, and opens the **three
ports** (§4) with the standard middleware stack.

### 3.6 What the harness deliberately does NOT do (CONFIRM). No business logic, schema, or event semantics; it
does not hide the SQL (thin query layer, not an ORM); it provides no fire-and-forget publish (BUS-2).
*Identical plumbing, visible logic.*

### 3.7 The cross-language harness shim — frozen as the divergence contract (SHARPEN, C-2; index 1.7)

ADR-02 lets a subsystem diverge from Rust where it genuinely calls for it; the named candidate is the **Chat
connection tier** (a BEAM/Elixir tier for the connection-storm load, TE-21). Phase 3 left "what the wire
contract + per-language shim must provide" as `[OPEN → P4]`. Phase 5 **freezes** it: a non-Rust subsystem's
shim **must** provide, to the same guarantee the Rust harness does:

1. **Three-surface topology** (§4) — public (gateway-fronted, identity-injected) / internal RPC (trust
   boundary only) / metrics-health; the public↔internal split as a security boundary.
2. **Liveness ≠ readiness** (§4.3) — a dead critical dependency reports *not ready* and sheds.
3. **No fire-and-forget emit** — the outbox pattern (BUS-2): same-transaction insert + a relay; **no**
   `publish_now` path exists in the divergent language either.
4. **`PersonalDataHolder` registration** (§3.4) — every store the shim opens registers for DSR fan-out.
5. **The resilient-client behaviour** (§6) — per-call timeout / breaker / bulkhead / jittered-retry-
   idempotent-only / **`Retry-After` honouring** on every outbound inter-service call.
6. **The principal-aware shed order** (§7) — speculative → batch/CI → agent → human-last, with the protected
   human lane and per-surface budgets (§7.6).
7. **Forward-only online migrations** (§9) — expand→backfill→contract, no rollback files, no blocking
   `ALTER` on a hot table; the discipline is a **substrate law**, not a Rust-library feature.

This is the **frozen divergence contract**: a no-op if Chat stays Rust, but the escape hatch is now *real and
specified* — a divergent subsystem cannot ship without satisfying these seven, and the cross-language wire
shapes are generated from the same `myelin-events`/`myelin-identity` types (ADR-02 §Consequences). The
non-negotiables cannot be quietly dropped at a language boundary.

---

## 4. The three-surface service topology (CONFIRM — unchanged from Phase 3 §4)

**§4.1–§4.4 are unchanged.** Summarised; see Phase-3 §4 for the full text.

- **§4.1 Public surface (behind the gateway).** A thin, stateless gateway: authenticates the credential →
  resolves a `Principal` via Id; injects trusted identity+causality headers; **trusts the token's tenant,
  never the URL path** (ID-3 — a mismatch is an IDOR → rejected + audited); applies the principal-aware shed
  lane (§7) at the edge. Holds no per-request state and no personal data of record.
- **§4.2 Internal RPC surface (trust boundary only).** Carries injected identity headers forward; a service
  does not re-authenticate but **does re-authorize** (`check`/`list_objects` always called — trusting the
  header for *identity* is fine, for *authorization* is not). The internal surface is a **security boundary**
  (mTLS / signed internal credential — Id's detail; the rule is here): we do not presume "internal = safe."
  All internal calls go through the resilient client (§6).
- **§4.3 Metrics-health (liveness ≠ readiness).** Liveness = "not wedged" → restart on fail; **must not**
  check dependencies. Readiness = "can serve correct traffic now" → a dead critical dependency reports *not
  ready* and stops taking traffic (never reports healthy and keeps failing). Startup = boot/migration
  incomplete → not ready, not killed. Interaction with fail-static (§8): readiness handles a *sustained*
  outage; fail-static buys the seconds-to-minutes of a *transient* hiccup.
- **§4.4 Topology diagram** — unchanged (Phase-3 §4.4): public stateless gateway → trust boundary (services
  re-authorize every call, metrics-health per service) → reactions flow both directions **only over the bus**
  (no sync cycles, §2.9).

---

## 5. The event-consumer template (CONFIRM — unchanged from Phase 3 §5)

**Unchanged in full.** Every consumer is built from **one template** in `myelin-events`, so the correctness
rules cannot be skipped per-consumer. The template shape and the seven encoded rules are verbatim from
Phase-3 §5; summarised:

```rust
pub trait EventHandler {
    fn subjects(&self) -> &'static [SubjectPattern];   // whitelist — NEVER "*" (BUS-3, D7-i)
    fn handle(&self, ev: &EventEnvelope) -> HandleOutcome;  // idempotent on event_id (ADR-04.1)
}
pub enum HandleOutcome { Done, NonRetryable(Reason), Retry(Backoff) }
```

The seven encoded rules (each maps to a directive/gotcha): **(1)** idempotent on `event_id` via the
per-consumer `consumer_dedup` ledger `(consumer, event_id) PK` (at-least-once + idempotent ≈
effectively-once; Myelin does **not** chase true exactly-once); **(2)** ack-after-enqueue, never before;
**(3)** whitelist subjects, never `*` (an over-broad subscription head-of-line-blocks everything); **(4)**
bind the durable consumer **by name**, never re-declare its start policy on reconnect (the single most
operationally expensive JetStream/Kafka mistake); **(5)** terminate non-retryable (poison) messages
immediately (dead-letter, don't burn the redelivery budget); **(6)** bounded prefetch; **(7)** monitor
consumer lag (`num_pending` / oldest-un-acked age) as a first-class metric.

**§5.3 Causality through the consumer (CONFIRM).** A reaction event calls
`OutboxTx::emit(draft, cause = Some(incoming))` so root carries, `causation_id` = incoming `event_id`,
`depth` = incoming `depth + 1`. This is what makes the loop guard **structural** (AG-6): the depth ceiling
and the shared-causal-root-within-a-window tripwire read these fields, so no convention/typo can defeat them.

**§5.4 The reactive/dispatch tier is a separate design (CONFIRM, ADR-19/D7).** The Signal-curation /
Automation-rule / stateful-Trigger tier sits **above** this template and is the Bus deliverable's separately-
reviewed design. **Phase-5 note:** the trigger/automation predicate core is now the **frozen `QueryAst`**
(= the `EventMatcher`, X-3/OQ-C, index 3.4) — there is no per-subsystem CEL/trigger DSL; one grammar, one
bounded interpreter, four compile targets. This consolidation is the Bus's, not the substrate's; the template
floor below it is unchanged.

---

## 6. The shared resilient inter-service client (`myelin-client`) (CONFIRM — unchanged from Phase 3 §6)

**Unchanged in full.** One client every outbound inter-service call goes through, so
timeout/breaker/bulkhead/retry is correct in exactly one place.

```rust
impl ResilientClient {
    /// Every call: per-call TIMEOUT, BULKHEAD (bounded concurrency), through the BREAKER.
    /// Retry ONLY if idempotent, with full jitter, and NEVER through a tripped breaker.
    fn call<R>(&self, target: Target, req: Req, idem: Idempotency) -> Result<R>;
}
```

The four primitives (all mandatory, all on by default): **(1)** per-call timeout (deadlines propagate);
**(2)** circuit breaker (closed→open→half-open; **never retry through a tripped breaker** — the textbook
retry-storm amplifier); **(3)** bounded-concurrency bulkhead (a semaphore per target; saturation fast-fails,
never queues unboundedly); **(4)** jittered retry — **idempotent calls only** (full jitter, Brooker 2015; a
`NonIdempotent` call is never retried).

**§6.2 `Retry-After` honouring (CONFIRM).** Our own clients **MUST honour `Retry-After`** — the resilient
client respects the header as the floor of its backoff. This is a hard requirement on the agent runtime and
the CLI too (they link this client), so shedding (§7) cannot become a retry storm and the protected human
lane holds.

**§6.3 Defaults (CONFIRM, the X-5 unit reconciliation).** Timeouts in **milliseconds**; breaker thresholds as
a failure ratio over a rolling window + a minimum request count; bulkhead as an integer concurrency cap;
backoff base in ms with full jitter. Concrete per-target values are each consuming system's call; the shape
and on-by-default posture are fixed.

---

## 7. Backpressure + principal-aware shedding (the protected human lane)

**§7.1–§7.5 are unchanged from Phase 3 §7** (cited below). §7.6 is the Phase-5 per-surface shed-budget floor
table (OQ-K, SHARPEN, C-4). §7.7 is the Phase-5 firehose resume-cursor seam discipline (OQ-J, SHARPEN, C-7).

### 7.1 Bounded everything (CONFIRM). **Every queue and pool is bounded; unbounded anything is a future
cascade** (EI-02 §5). The substrate bounds: consumer prefetch, the DB pool (fast-fail + statement timeout),
the bulkhead per target, per-tenant in-flight work, and the HTTP intake queue. A bounded queue that fills
**fast-fails** (sheds) rather than growing latency unboundedly (Little's Law: unbounded queue = unbounded
latency = indistinguishable from down).

### 7.2 The principal-aware limiter + protected human lane (CONFIRM). The limiter reads `Principal.kind` + the
run's class from the injected headers and reserves a **protected lane for interactive humans**. The shed
order under pressure:

```
shed first  →  speculative  →  batch / CI  →  agent  →  human  ←  shed last (protected)
```

Speculative work is dropped first (never promised); batch/CI and agent lanes shed next with
**`429 + Retry-After`** (and our clients honour it, §6.2); the human lane is shed **last** and only in true
saturation. Shedding is **per-tenant** so one tenant's surge does not shed another tenant's humans (EI-02 §1
blast-radius).

### 7.3 Why this order (CONFIRM). The order encodes *promise strength*: speculative made no promise; batch/CI/
agents are machine clients that can and should back off; humans are the interactive principal the product
exists for. This is Myelin's deliberate adopt-verbatim of the doctrine order; we do **not** shed by raw FIFO
(which would starve humans behind an agent burst).

### 7.4 Agent-generated load (CONFIRM). The dispatch worker pool is **bounded and drops over-cap (never
forks)** (AG-6); combined with the causal-depth ceiling, the shared-root-within-a-window tripwire, per-tenant
circuit breakers, and the **reserve/settle cost gate** (no balance → no execution), agent load is
structurally capped.

### 7.5 Bounded predicate evaluation (CONFIRM). The shared `EventMatcher`/saved-view predicate core
(`myelin-query`, now the frozen `QueryAst`, X-3) is **declarative and safe-to-evaluate — no Turing-complete
predicates on hot paths**, no UDFs/loops/recursion, statically cost-bounded. The substrate enforces a
step/time ceiling per predicate so a crafted matcher cannot DoS the trigger engine.

### 7.6 Per-surface shed budgets — the v1 floor table (SHARPEN, C-4; OQ-K, index 1.11)

Phase 3 left the concrete per-surface budgets `[OPEN → P4]`. Phase 5 **names the v1 budget floor** — these
are **named floors**, tuned by the drills (T-5), not claimed-final. The discipline (ADR-16: every surface is
bounded, has a reserved human lane, and applies the shed order) is the contract; the numbers are each
subsystem's P4 budget call asserted by the drills:

| Surface | Storm profile | In-flight cap (per tenant) | Protected-human-lane reservation | Shed order applied |
|---|---|---|---|---|
| **CI dispatch** | CI-surge (30× agent) | bounded run-queue per tenant; runners pull-bounded | n/a (CI is the batch lane) | speculative → batch/CI → agent → human-last; CI + agent share the wallet (no special CI-before-agent rule) |
| **Collab op-stream (KN)** | hot-doc edit/read storm | per-doc op in-flight cap; read-fanout bounded | a reserved fraction for **active editors** vs passive viewers | viewers shed before editors; agents shed before humans |
| **Connection tier (Chat)** | connection-storm | per-tenant connection cap + per-connection frame cap | reserved connection slots for interactive humans | presence/speculative shed first; message delivery last |
| **Agent-mention (Chat/all)** | agent-mention-storm | per-tenant agent-run in-flight cap (reserve/settle refuses over-cap) | humans never queue behind agent runs (the lane) | agent lane sheds with `429 + Retry-After`; the agent runtime honours it (§6.2) |

The **floor** is "every one of these is bounded, has a reserved human lane, and applies the shed order" — an
unbounded one is the cascade (EI-02 §5). The 30×-agent-surge / connection-storm drills (D-3, plus the
connection-storm drill the Chat tier owns) assert against these.

### 7.7 The firehose resume-cursor seam — the substrate's backpressure role (SHARPEN, C-7; OQ-J, index 3.5)

The **firehose** (Bus contract 3.5) is the shared transport for high-volume, ephemeral-ish streams: CI logs,
KN collab op-streams + presence, Chat live delivery + presence + agent partials. The durable bus carries only
pointer events; the firehose carries the frames. Phase 5 freezes the **resume-cursor / scope-bounded
subscription protocol** (OQ-J), co-designed once for ISS huge-board / KN hot-doc / Chat hot-channel:

```
subscribe(stream, scope, cursor?) → SubStream     // scope BOUNDS what frames arrive (board:/doc:/channel:, never *)
SubStream yields Frame { seq: u64, ... }           // seq is per-(stream, scope) monotonic
resume(stream, scope, last_seq) → backfill (last_seq, now] then live    // the gap is replayed, never lost
```

This protocol is **Bus-owned** (Bus's deliverable), but it **rides the substrate's backpressure discipline**,
which is the substrate's stake here:

- **Per-connection in-flight frame caps** (§7.1 bounded-everything, generalised to the firehose): a
  subscription's frame buffer is bounded; over-cap sheds in the firehose's own bounded queue.
- **A slow consumer is dropped to `resync_required`, never buffered unboundedly** — the slow consumer falls
  back to a full `*.snapshot` replay (the cold-rebuild path, named not silent), rather than the transport
  growing memory to hold its gap. This is the bounded-queue rule applied to streaming.
- **Scope is a bounded selector, never `*`** (the whitelist-not-`*` rule, BUS-3, generalised): a 50k-row
  board paginates its scope (the visible window + margin); the firehose delivers only that slice's frames.
- **The per-surface shed budgets (§7.6) apply** — presence/speculative frames shed before message delivery;
  agents shed before humans.

The **resume cursor itself** (zero ops lost across a reconnect — the T-5 "reconnect-loses-zero-ops" drill)
is the doctrine's "build the durable resume-cursor transport FIRST, the CRDT slots into it" (EI-04 §2.2). The
substrate guarantees the *bounded-and-sheds* half; the Bus guarantees the *zero-loss-replay* half. Together
they make a dropped connection lose nothing while one client cannot subscribe to the whole tenant's firehose.

---

## 8. Fail-static primitives (CONFIRM — mechanism unchanged from Phase 3 §8; bound is `[OPEN — LEGAL]`, C-6)

**§8.1, §8.3 unchanged.** **Distinguish fail-closed from fail-static**: fail-closed is correct for
*authorization correctness* (deny when unsure, ADR-03 unchanged); **fail-static is the correct *availability*
default** — on a transient dependency hiccup, serve a **bounded-staleness cached answer** so
*already-authenticated* traffic keeps working, rather than failing every request closed and turning one
shared dependency (Id) into a whole-platform cascade (the textbook single-point cascade, EI-02 §10).

```rust
pub struct FailStatic<T> {
    fresh_ttl: Seconds,          // serve fresh within this
    static_max: Seconds,         // serve STALE (degraded marker) up to here on a hiccup; ≤ revocation SLA, ≥ agent-token TTL
}
impl<T> FailStatic<T> { fn get(&self, key: K, refresh: impl Fn() -> Result<T>) -> Answer<T>; } // Fresh | Static(degraded) | Closed
```

On a hiccup: within `fresh_ttl` → fresh; between → serve stale + mark degraded + background-refresh
(stale-while-revalidate); past `static_max` → **fail closed** (the staleness budget is exhausted; deny is now
correct). For authorization, the static answer is the coarse "actor still active / coarse grants" — **never
an escalation of access** (we never fail *open*). The zookie/`Consistency` mode expresses the staleness; a
security-sensitive read passes the zookie so it **bypasses the fail-static cache** (index 4.10) and the authz
reverse index honours the zookie revision watermark.

### 8.2 The staleness bound (CONFIRM mechanism; `[OPEN — LEGAL]` value, C-6)

`static_max ≤ the deprovision/revocation SLA` and **must contain the short-lived agent-token TTL** (ID-1,
GD-3, ADR-17). This makes the window a **written, bounded exposure**: a revoked actor still falls inside the
window and is denied once it closes; an agent token (life == run life) expires inside the window. **The DPO
ratifies the chosen bound** (L-1; it is the residual GDPR-revocation exposure window) — this is a
decision-shaped call handed to Legal/DPO, not an engineering default set silently. The mechanism and the
`≤ revocation-SLA ≥ agent-token-TTL` constraint ship regardless; the **value** is `[OPEN — LEGAL]`, flagged
for counsel (index 4.11).

### 8.3 Interaction with readiness (CONFIRM). Fail-static handles a **transient** hiccup; **readiness** (§4.3)
handles a **hard-down** sustained outage (not-ready + shed new traffic). The two compose; both are asserted
by the Id-hiccup drill (D-4 / T-5).

---

## 9. Forward-only online migrations (expand → backfill → contract)

**§9.1–§9.3 are unchanged from Phase 3 §9** (cited). §9.4 is the Phase-5 hot-table declaration freeze
(SHARPEN, C-3).

### 9.1 The algorithm (CONFIRM). A hot-table schema change is **three deploys**, never one blocking `ALTER`:
**(1) Expand** — add the new shape additively + non-blockingly (nullable column; `CREATE INDEX
CONCURRENTLY`; new table); app writes both old and new behind a flag. **(2) Backfill** — populate in bounded,
throttled, resumable batches off the hot path (idempotent, re-runnable; shares the event-replay posture).
**(3) Contract** — switch reads to the new shape, stop writing the old, drop the old in a later non-blocking
deploy. **Forward-only**: no down migrations (you can't un-delete data); "rollback" is a *new forward*
migration.

### 9.2 Measure lock time against a restore first (CONFIRM). Before any DDL on a hot table, **measure its lock
time against a restored production-scale copy** (STOR-2) — a "quick `ALTER`" can be a minutes-long exclusive
lock at scale. This ties to the restore-verification machinery (ADR-18, §11).

### 9.3 Online-DDL for cross-language services (CONFIRM). A non-Rust subsystem still obeys
expand→backfill→contract and forward-only; the rule is a **substrate law** (now part of the frozen
cross-language shim, §3.7), not a Rust-library feature.

### 9.4 Hot-table declaration — frozen (SHARPEN, C-3; index 1.5)

Phase 3 left "which tables are hot enough to require expand→backfill→contract" as a per-subsystem `[OPEN →
P4]` (§13 Q2). Phase 5 **freezes the declaration mechanism**: every subsystem **declares its hot tables** in
its `AppSpec`, and the `forward-only-migration` lint reads that declaration to enforce no-blocking-`ALTER` on
exactly those tables. The seed set (named, measured-not-predicted per ADR-10):

- **Knowledge**: `block`, `db_row`, `doc_op` (the highest-write collaborative tables).
- **All high-write subsystems** declare theirs (Git ref/object metadata at push QPS, CI `run`/`step`/log
  index, Issues `issue`/`issue_relation`, Chat `message`/`channel_membership`).

The declaration is **per-subsystem and measured** (a table is flagged hot when its write rate warrants
expand→backfill→contract, not predicted); the *mechanism* (declare → lint enforces) is the frozen contract.

---

## 10. The observability baseline (CONFIRM — unchanged from Phase 3 §10; pinned as the drill survival signal, C-5)

**§10.1, §10.3 unchanged.** **Causality is a first-class primitive, not logging** (EI-02 §6). The harness
trace middleware propagates, on **every** surface and hop, alongside W3C `traceparent`: **`tenant` +
`region`** (so blast-radius scoping is mechanical) and **the causality triple** (`correlation_id` root,
`causation_id` immediate parent, `depth`) + the distinct **`caused_by`** human-action ref. These are the
**same fields the events carry** (§2.1), so the "why did this happen?" view, audit walk, distributed trace,
and loop guard are **one mechanism**, not four. **§10.3:** the tamper-evident audit log (AG-7/ADR-12.9) is a
**separate**, retention-bounded `PersonalDataHolder` — not the telemetry stream (telemetry is operational +
sampled; audit is complete + tamper-evident).

### 10.2 The telemetry signal set (CONFIRM, pinned as the drill survival signal, C-5; X-1, index 1.8)

Every service exports, on the metrics-health port, a **standard signal set** the Phase-5 drills read as a
**survival signal**. The set is unchanged from Phase 3; Phase 5 pins it as the drill pass-condition surface
(a shared-system doc that omits any of these fails X-1, and the drills **assert against** these signals,
which is what makes "proven" mean proven, T-4):

| Signal | Method | Drill it feeds |
|---|---|---|
| Request rate / errors / duration, **per principal-kind + per tenant** | RED | 30× agent-surge (human lane holds) |
| Utilisation / Saturation / Errors of each pool/queue | USE | overload / cascade |
| **Consumer lag** (`num_pending`, oldest-un-acked age) per consumer | — | event-loss / head-of-line (D7-i) |
| **Outbox depth** / dead-letter count | — | silent-data-loss (BUS-2) |
| **Breaker state** (open/half/closed), bulkhead rejections, `Retry-After` issuance | — | retry-storm / cascade |
| **Fail-static**: fresh/stale/closed answer ratio, staleness age | — | Id-hiccup / fail-static |
| **Shed counts** per lane (speculative/CI/agent/human) + per-surface budget (§7.6) | — | agent-surge / human-lane-holds / connection-storm |
| **Causal-depth** tripwire firings, dispatch-pool drops | — | causal-loop tripwire (AG-6) |
| Firehose: per-(stream,scope) frame lag, `resync_required` count (§7.7) | — | reconnect-loses-zero-ops (T-5) |

The last row is the only addition (the firehose resume-cursor seam's survival signal, C-7); the rest are
verbatim from Phase 3 §10.2.

---

## 11. Failure modes + the drills owed

**The drill table is unchanged from Phase 3 §11** (the substrate enumerates the drills it owes; Phase 5 owns
the platform testing strategy + thresholds). Each drill emits a **green artifact** when it passes; until then
the property is **claimed, not proven** (T-4, EI-04 §4.3). Restated with the two Phase-5 additions noted:

| # | Property / failure mode | Drill owed (quantified) | Owner | Directive |
|---|---|---|---|---|
| D-1 | **Silent data loss** (dual write) | Kill a service *between* DB commit and publish; assert the relay delivers every committed event effectively-once (zero ghost, zero lost). | Bus + substrate | BUS-2, ADR-04.3 |
| D-2 | **Event loss across reconnect / head-of-line** | Drop the broker connection mid-stream; assert zero loss (durable consumer resumes by name, dedup absorbs) + a slow subject does not block others. | consumer template | BUS-3 |
| D-3 | **30× agent surge** | Drive a 30× agent/CI surge on one tenant; assert the **human lane holds**, the **agent lane sheds** (429+Retry-After, clients honour it), **other tenants unaffected** — against the §7.6 budgets. | §7 | ADR-16 |
| D-4 | **Id-hiccup / fail-static** | Inject a transient Id hiccup; assert already-authenticated traffic stays up on the bounded-staleness cache, staleness never exceeds `static_max ≤ revocation SLA`, a revoked actor is denied once the window closes. | §8 | ADR-17, ID-1 |
| D-5 | **Retry storm** | Trip a downstream breaker under load; assert callers fail fast (no retry through the tripped breaker) and honour `Retry-After`. | §6 | ADR-16 §4 |
| D-6 | **Restore + cross-seam integrity** | Rebuild from backups; assert no loss and OLTP rows ↔ blob ↔ search index ↔ event-log offsets restore to one consistent point (no row → missing blob). | Storage + GDPR | ADR-18, STOR-4 |
| D-7 | **Cross-tenant IDOR** | Attempt a cross-tenant read via path-tenant ≠ token-tenant; assert zero cross-tenant read + the `tenant-predicate` lint catches a tenant-less query at compile time. | §2.11, §4.1 | EI-02 §1, ID-3 |
| D-8 | **Causal-loop tripwire** | Adversarially construct an agent→agent loop; assert the depth ceiling + shared-root tripwire + bounded dispatch (drops over-cap, never forks) stop it. | §5.3, §7.4 | AG-6 |
| D-9 | **Liveness ≠ readiness** | Kill a critical dependency; assert the instance reports not-ready and sheds (not healthy-but-failing), and liveness does not restart-storm. | §4.3 | X-2 |
| D-10 | **Online migration safety** | Run an expand→backfill→contract migration on a restored production-scale copy under load; assert no blocking lock beyond budget, zero downtime. | §9 | STOR-2 |
| **D-11** | **Firehose reconnect-loses-zero-ops (NEW, §7.7)** | Drop a firehose subscription mid-stream on a hot board/doc/channel; assert `resume(last_seq)` backfills the gap with **zero ops lost**, an over-retention gap yields `resync_required` → `*.snapshot` (named, not silent), and a slow consumer is dropped (not buffered unboundedly). | Bus + substrate (bounded-queue half) | OQ-J, T-5 |

D-11 is the only new drill (the firehose resume-cursor seam, C-7); D-1..D-10 are unchanged. The substrate's
job is to make every drill *possible* by exposing the telemetry (§10.2) and the failure-injection seams (a
scoped, reversible dependency break — T-3).

### 11.1 The stateful-component register + blast-radius note (CONFIRM, unchanged from Phase 3 §11.1)

| Stateful component | Sharding / shared-state plan | Blast radius if it dies |
|---|---|---|
| Per-service OLTP pool + **outbox table** | per-service DB, tenant-partitioned; read replica for hot paths (ID-4) | that service's writes + un-drained events (recovered by the relay on restart) |
| **Per-consumer dedup ledger** | per-consumer, tenant-partitioned | at worst a redelivery is re-processed (idempotency absorbs); no loss |
| **Durable consumer cursors** (in the broker) | bound by name; never re-declared (D7-ii) | resumes on reconnect; no loss |
| **Fail-static cache** | per-service, in-memory + backing store | cold cache → brief fail-closed until warm; bounded by `static_max` |
| Breaker/bulkhead state | per-service, in-memory | resets on restart (fails safe: closed-but-probing) |
| **Firehose subscription buffers** (NEW, §7.7) | per-connection, bounded; per-(stream,scope) cursor | a dropped slow consumer → `resync_required` (cold rebuild); no loss, no unbounded memory |

Everything else (gateways, handlers, the resilient client, the migration runner) is **stateless and
horizontally replaceable** (EI-02 §10). The control plane holds **zero in-region personal data** (ADR-11; the
new `control-plane-pii-free` lint, §2.11, makes this structural).

---

## 12. The contracts this doc exposes (the stable surface — aligned to the refined index)

The explicit, stable surface other shared systems and all subsystems consume. Changing any of these is a
single workspace PR that breaks every consumer's build *now* (ADR-01), never silently in production. Each row
cites its **refined contract-index** entry (the index, not this table, is the canonical map):

| Contract | Crate | Index row | Status |
|---|---|---|---|
| `serve(AppSpec)` bootstrap | `myelin-substrate` | 1.1 | CONFIRM |
| Three-surface topology + public↔internal security boundary | convention + harness | 1.2 | CONFIRM |
| Liveness ≠ readiness | harness | 1.3 | CONFIRM |
| `PersonalDataHolder` auto-registration | harness | 1.4 | CONFIRM |
| Forward-only online migrations + **hot-table declaration** | migration runner + each owner | 1.5 | SHARPEN (C-3) |
| **Architecture lints** (the six Phase-3 + six new) | substrate/CI | 1.6 | SHARPEN (C-1) |
| **Cross-language harness shim** (the frozen divergence contract) | the diverging subsystem | 1.7 | SHARPEN (C-2) |
| **Telemetry signal set** (the drill survival signals) | harness | 1.8 | CONFIRM (pinned, C-5) |
| `ResilientClient::call` + `Retry-After` honouring | `myelin-client` | 1.9 | CONFIRM |
| `FailStatic<T>` + the staleness-bound rule (`[OPEN — LEGAL]` value) | substrate | 1.10 | CONFIRM (C-6) |
| **Protected-human-lane shed order + per-surface budget floors** | harness + gateway + each subsystem | 1.11 | SHARPEN (C-4) |
| `EventEnvelope` + the canonical field list (the names/units anchor) | `myelin-events` | 2.1, 14 | CONFIRM (aligned, C-9) |
| `OutboxTx::emit(draft, cause)` — the ONLY emit path | `myelin-events` | 2.2 | CONFIRM |
| `EventHandler` template + `HandleOutcome` (the seven rules) | `myelin-events` | 2.4 | CONFIRM |
| `Principal` / `AuthzClient::{check (w/ `CaveatContext`), list_objects (`Ids \| Filter`), delegation}` | `myelin-identity` | 4.2/4.3/4.5 | CONFIRM (shapes pinned, C-10) |
| `ArtifactRef` parse/format + `Refs::{edges,backlinks}` (`#sub` grammar frozen in Refs) | `myelin-refs` | 5.1/5.3/5.7 | CONFIRM (C-11) |
| `AgentRuntime::step` / `ToolHands::exec` / `ToolSurface::register_tool` | `myelin-agent` | 8.1/8.3/8.4 | CONFIRM |
| `PersonalDataHolder` + `BlobStore` put/get/head/delete | `myelin-gdpr` | 10.1/11.2 | CONFIRM |
| The `myelin-content` taxonomy seam + **WASM compile target** | `myelin-content` (KN-led) | 13.1 | CONFIRM (C-8) |
| The `myelin-query` `EventMatcher` / saved-view shared core (bounded eval) | `myelin-query` | 13.3/3.4 | CONFIRM |
| The expand→backfill→contract + forward-only rule | migration runner | 1.5 | CONFIRM |
| **Firehose resume-cursor seam** (substrate backpressure half) | Bus (protocol) + substrate (bounds) | 3.5 | SHARPEN (C-7) |

---

## 13. Open questions — Phase-3 §13 disposition + what remains for Phase 6

The Phase-3 §13 open questions handed to Phase 4 are **all resolved/closed** by the reconciliation; their
disposition (no longer open):

| Phase-3 §13 OQ | Disposition |
|---|---|
| **Q1 — Cross-language harness parity** | **CLOSED** → frozen as the divergence contract (§3.7, index 1.7, C-2). |
| **Q2 — Per-table "hot" flagging** | **CLOSED** → the declaration mechanism is frozen, seed set named (§9.4, index 1.5, C-3). |
| **Q3 — Per-surface shed budgets** | **CLOSED** → the v1 budget floor table named (§7.6, OQ-K, index 1.11, C-4). |
| **Q4 — Sub-artifact `#sub` granularity** | **CLOSED** → the unified `#sub` grammar + tombstone ladder frozen in Refs (X-4/OQ-D, index 5.7). |
| **Q5 — Service-to-service authn inside the trust boundary** | **CONFIRMED as Id's detail** (mTLS vs signed internal credential is Id's Phase-3/4 deliverable; the *rule* — assert the boundary, don't presume it — is fixed §4.2). Not a substrate open item. |

### 13.1 Remaining open items the substrate carries into Phase 6 (the honesty register)

- **`[OPEN — LEGAL]` — the fail-static staleness bound value (L-1).** The mechanism + `≤ revocation-SLA ≥
  agent-token-TTL` constraint ship regardless; the **value** is a DPO-ratified call (§8.2, index 4.11).
  Flagged to counsel/DPO. *This is the one substrate-owned legal flag.* (The broader `[OPEN — LEGAL]` posture
  — free-text/immutable erasure, worklog sensitivity, build-data-as-training — is GDPR/Audit's deliverable,
  not the substrate's; the substrate provides the structural floor those instantiate, per reconciliation
  §X-7/§OQ-H.)
- **Concrete resilient-client per-target values** (timeouts, breaker thresholds) remain **each consuming
  system's call**; the shape + on-by-default posture are fixed (§6.3). Not a blocker — a default-set Phase 6
  must tune per target via drills (the auth hot path gets a tighter timeout than a batch indexer).
- **The per-surface shed budget *numbers*** (§7.6) are **named v1 floors tuned by the drills** (D-3, D-11,
  the connection-storm drill); the *floor discipline* (bounded + reserved human lane + shed order) is the
  contract. Phase 6/8 tunes the numbers against the drills, not by prediction (EI-02 §8 measured-not-
  predicted).
- **Hot-table flags are measured, not predicted** (§9.4): the seed set is named, but a table is flagged hot
  on measured write rate; Phase 6 roadmaps each subsystem's measurement gate.

These are **named floors / decision-shaped calls**, not silent gaps: each names its follow-on and its owner
(VISION §3, EI-04 §4).

---

## 14. Cross-references

- [`VISION.md`](../../VISION.md) — world-scale, GDPR-by-construction, agent-native, top-tier UX, Rust-default.
- [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) — the keystone (X-1..X-7, OQ-A..OQ-L);
  the rationale for every Phase-5 shape applied here.
- [`contract-index.md`](./contract-index.md) — the refined, frozen build-to surface; the canonical contract
  map this doc's §12 aligns to (supersedes the Phase-3 index).
- [`../03-shared-systems-architecture/00-platform-substrate.md`](../03-shared-systems-architecture/00-platform-substrate.md)
  — the Phase-3 base this refines (superseded).
- [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) (EI-02) —
  the home doctrine (§1 tenant, §3 bus, §4 outbox, §5 backpressure, §6 causality, §8 storage/migrations, §9
  harness/topology, §10 fail-static/blast-radius, §11 restore).
- [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) (EI-04) — the honesty
  discipline + reindex-from-source + the resume-cursor-transport-FIRST mandate (§2.2 → §7.7 here).
- Spine: [`../02-holistic-architecture/architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md)
  (ADR-01 crates, ADR-04 bus, ADR-16 backpressure, ADR-17 fail-static, ADR-18 restore-verify);
  [`../02b-doctrine-integration/integration-directives.md`](../02b-doctrine-integration/integration-directives.md)
  (X-1…X-5, BUS-2/3, STOR-1/2/3/4, ID-1/3/4, GD-3, AG-1/2/6).
- [`../04-subsystem-architectures/cross-subsystem-change-requests.md`](../04-subsystem-architectures/cross-subsystem-change-requests.md)
  — §11 (substrate change requests, all CONFIRM-grade) + the OQ-J/OQ-K seams folded here.
```
