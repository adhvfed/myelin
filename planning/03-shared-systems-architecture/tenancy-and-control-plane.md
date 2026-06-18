# Phase 3 — Tenancy & Control Plane (the cell topology, in detail)

> Phase: `03-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md).
> Doctrine (binding): [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
> §1 (tenant-is-the-unit) / §10 (blast-radius, fail-static) — always — and
> [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1 (residency as
> region-pinning, no cross-region query path). Spine: **ADR-11** (cell topology — the doc this resolves),
> with ADR-01/03/04/12/13/16/17. Directives: ID-1/ID-3, X-2/X-4/X-5, STOR-1…STOR-4, GD-3, BUS-2.
> Foundational Phase-3 docs consumed (their contracts are NOT re-invented here):
> [`00-platform-substrate.md`](./00-platform-substrate.md) (bootstrap harness, three-surface topology,
> consumer template, fail-static, blast-radius register),
> [`identity-and-access.md`](./identity-and-access.md) (the multi-cell **home-cell-authoritative +
> cross-cell read-through** seam, §14; zookies; `list_objects`),
> [`event-bus.md`](./event-bus.md) (the cross-cell **pointer-event bridge** floor, §7.4; the
> personal-data-free control-plane bridge).
>
> **What this doc owns (ADR-11's `[OPEN → P3]` backlog):** the cell topology in detail — **cell sizing**,
> **tenant→cell assignment**, the **isolation spectrum** and how it holds across *every* shared system,
> **multi-cell tenants** + cross-cell collaboration/latency (the deepest unknown, SC-2/SC-3), the **global
> control plane holding zero in-region personal data**, **region-pinning enforced at the data layer**
> (misrouting impossible), **self-host = one cell, same artifacts**, and **CLI/endpoint cell discovery**
> (CA-6). It does **not** re-decide Id, Bus, Storage, or GDPR internals — it sits *above* them and routes.
>
> **Status convention.** *DECIDED* = committed for P4/P5; *FLOOR* = partial answer with a named follow-on;
> *[OPEN → P4/P5/LEGAL]* = handed forward. Every property that can fail names the **drill** that proves it.
> **Altitude:** Phase-3 detailed — concrete data model, routing algorithm, wire shapes, failure modes.
> Illustrative SQL/Rust-shaped snippets are *signatures*, not implementations.

---

## 0. Reading map

- **§1** — purpose, responsibilities, the one-paragraph thesis.
- **§2** — prior art (cell-based architecture, multi-tenant isolation) cited once.
- **§3** — what a cell *is* (the anatomy) + the global control plane.
- **§4** — the data model / schemas (control-plane registry; the per-cell tenant directory).
- **§5** — **cell sizing** (the algorithm + the bin-packing model).
- **§6** — **tenant→cell assignment** (placement, the assignment algorithm, rebalancing/migration).
- **§7** — the **isolation spectrum** and how it holds across *all* shared systems (the matrix).
- **§8** — **region-pinning enforced at the data layer** (misrouting impossible).
- **§9** — **CLI / endpoint cell discovery** (CA-6).
- **§10** — **multi-cell tenants** + cross-cell collaboration (SC-2/SC-3) — the deepest unknown, honest.
- **§11** — **self-host = one cell, same artifacts**.
- **§12** — contracts / APIs this system exposes + consumes (the glue). **Stable.**
- **§13** — scaling/sharding of the control plane itself.
- **§14** — failure modes + the drills owed (quantified).
- **§15** — required changes to foundational systems.
- **§16** — open questions for Phase 4. **§17** — cross-references.

**Floors named up front** (VISION §3 / EI-04 §4): **single-cell tenants are fully designed; multi-cell
tenants are designed-not-built** (§10, follow-on = P4 control-plane + the SC-2/SC-3 resolution). The
**rebalancing/live tenant-migration path is specified-not-built** (§6.4, promotion trigger = a measured
hot cell). **Cell discovery v1 is a control-plane lookup; an edge anycast/GeoDNS optimisation is a named
follow-on** (§9.4).

---

## 1. Purpose, responsibilities, and the thesis

### 1.1 The two non-negotiables this topology reconciles

Two VISION mandates pull in opposite directions (ADR-11 §Context; `gdpr-eu-sovereignty.md §2.2`):
**world-scale from day 1** historically wants a global, proprietary, hyperscaler-managed control plane;
**EU-sovereign + GDPR-by-construction** forbids leaning on one. The **cell** is the structure that
satisfies both without a US hyperscaler's global plane: scale = *add cells* (not a bigger global
service), residency = *the cell's region*, breach blast-radius = *one cell*, self-host = *one cell on the
customer's infra running the same artifacts*. This doc makes that structure concrete.

### 1.2 What Tenancy & Control Plane owns

1. **The cell as the unit of sovereignty + scale + blast-radius** — its anatomy (§3), its **sizing**
   (§5), and the rule that a cell is a *complete region-pinned stack of all subsystems + all shared
   systems on commodity, EU-deployable, self-hostable primitives* (ADR-11.1).
2. **Tenant→cell assignment** (§6): placement at signup, the assignment algorithm (region-first,
   capacity-aware, isolation-tier-aware), and the rebalancing/migration path (floor).
3. **The isolation spectrum** (§7): logical (row-level) → schema/DB-per-tenant → cell-per-tenant, and the
   proof that the chosen tier holds across **every** shared system's partition surface (bus subjects,
   search indices, caches, blob prefixes, agent context, ref partitions, authz tuples).
4. **The global control plane** (§3.3, §4.1): the personal-data-free router/orchestrator that maps
   `tenant → {cell(s), region}`, provisions cells, and owns *nothing* a data subject could be identified
   from. It is itself EU-sovereign and is the **only** globally-replicated component.
5. **Region-pinning at the data layer** (§8): the mechanism that makes misrouting a tenant's personal
   data *impossible*, not merely discouraged — region as a compiled-in shard key, validated at the write
   boundary, with the `tenant-predicate` and a new `residency-pin` lint (E-5).
6. **Cell discovery** (§9): how the CLI, the git wire, the API, and a browser learn *which cell* serves a
   tenant, without the discovery channel itself becoming a personal-data side-channel (CA-6).
7. **Multi-cell tenants** (§10): the honest design + honest deferral of a 10k-person org spanning cells.
8. **Self-host parity** (§11): the same artifacts build a managed cell and an on-prem install.

**What it is NOT.** It is not Id (it routes *to* the cell where Id runs; it never makes an authz
decision). It is not the Bus, Storage, or GDPR engines (it provisions and addresses them; their internals
are their own Phase-3 docs). It holds **no tuples, no events, no blobs, no profiles** — only the
**routing/placement registry** (§4.1), which is structurally PII-free (§3.3).

### 1.3 The one-paragraph thesis

*A **cell** is a bulkhead: a complete, region-pinned, independently-deployable copy of the whole Myelin
stack serving a bounded set of tenants, so a failure, a breach, a noisy tenant, or a residency
requirement is contained to one cell by construction. A tenant is permanently bound to a cell (or, rarely,
a small set of cells in one region) by a **placement record in a personal-data-free global control plane**
whose entire job is to answer "which cell serves this tenant?" and to provision new cells when existing
ones fill. Region is the cell's immutable property and the compiled-in shard key of every store, so "EU
data stays in EU" and "scale this tenant out" are the same mechanism, and routing a tenant's personal data
to the wrong region is a compile-time/admission-time error, not an operational risk. Self-host is just a
one-cell deployment of the identical artifacts.* This is the cell-based / bulkhead architecture (AWS
Builders' Library; Well-Architected REL10-BP04) specialised for EU sovereignty.

---

## 2. Prior art this design stands on (cited once, referenced throughout)

| Concern | Prior art / proven system | Where it lands |
|---|---|---|
| **Cell-based architecture / bulkhead** | AWS Builders' Library *"Workload isolation using shuffle-sharding"*; AWS Well-Architected **REL10-BP04** *"Use bulkhead architectures to limit scope of impact"* (2022); *"The bulkhead principle: cell-based architectures on AWS, end to end"* (AWS Builder Center); Nygard, *Release It!* (2e, 2018) — Bulkhead pattern | §3, §5, §7, §14 |
| **Cell as complete isolated stack; deployment progression per cell** | AWS cell-based architecture guidance; Slack *"cellular architecture"* (2023 eng blog); DynamoDB/Route 53 cell design | §3, §5, §13 |
| **Shuffle sharding (blast-radius reduction without N× cost)** | Colm MacCárthaigh, AWS Builders' Library shuffle-sharding; Route 53 Infima | §6.5 (considered + bounded) |
| **Tenant routing / control-plane vs data-plane split** | AWS SaaS Lens; *"Tenant routing strategies for SaaS applications on AWS"*; AWS SaaS Factory control-plane/app-plane separation | §3.3, §6, §9 |
| **Multi-tenant isolation tiers (silo / pool / bridge)** | AWS SaaS *silo vs pool vs bridge*; *"Designing Multi-tenant SaaS Architecture on AWS"* (2026); per-tenant RLS / schema / DB | §7 |
| **Sharding + routing for relational SaaS** | AWS Database Blog *"Scale your relational database for SaaS: Sharding and routing"*; Citus/Vitess tenant-sharding; PG Row-Level Security | §6, §8 |
| **Region-pinning / data residency as a hard partition** | GDPR Art. 44–49 (transfers); Schrems II (C-311/18); residency-as-shard-key (EI-04 §1) | §8, §10 |
| **Consistent placement / bin-packing** | Bin-packing (first-fit-decreasing); consistent hashing (Karger et al., STOC 1997) for *stable* assignment; jump-consistent-hash (Lamping & Veach, 2014) | §6.2 |
| **Stable per-tenant address; service discovery** | DNS SRV/anycast; HashiCorp Consul / etcd service catalog; envoy/control-plane xDS | §9 |
| **Bounded-staleness cross-region read** | ADR-17 fail-static; Zanzibar zookie bounded-staleness (Pang et al., OSDI 2019) | §10 |
| **Fail-static, not fail-closed, for availability** | Google SRE ch. 22 *"degrade gracefully"*; ADR-17 | §3.3, §14 |

These are the **defaults-to-beat** and **justified adoptions**. The core borrow is **cell-based
architecture** (the bulkhead applied at the whole-stack granularity); the deliberate *deviation* from the
hyperscaler playbook is that our control plane is **personal-data-free and EU-sovereign** (the residency
constraint forecloses the usual global control-plane conveniences — §3.3, §8).

---

## 3. The cell — anatomy — and the global control plane

### 3.1 A cell is a complete region-pinned stack (ADR-11.1)

A **cell** contains, region-pinned, *all five subsystems and all eight shared systems* over commodity,
self-hostable, EU-deployable primitives (ADR-14):

```
┌──────────────── CELL  (cell_id = "eu-central-1a", region = "eu-central") ─────────────────┐
│  Subsystems:  Git │ CI │ Issues │ Knowledge │ Chat                                         │
│  Shared:  Identity(ReBAC tuples,zookies)  Event Bus(JetStream + outbox)  Refs  Search      │
│           Notifications  Agent Fabric(Mock now)  Storage(PG OLTP / S3-compat / log tier)   │
│           GDPR-Audit(DSR orch, KMS, tamper-evident log)  Durable-workflow  OLAP            │
│  Cross-cutting:  per-cell KMS root │ per-tenant DEKs │ residency tag = region (immutable)  │
│  Front door:  stateless gateway (authenticate → inject trusted headers → shed lane)        │
│  Self-host = exactly this box on the customer's infra, same artifacts (§11)                │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```

**Properties that make it a bulkhead** (REL10-BP04; AWS shuffle-sharding):
- **Complete:** a cell can serve a tenant end-to-end with **zero dependency on another cell** for any
  personal-data path (the no-cross-region-query rule, §8; ADR-11 §Consequences).
- **Independently deployable:** each cell is its own deployment progression — a bad release rolls out
  cell-by-cell, so a regression is contained to a wave, not global (Slack/AWS cell deploy practice). The
  control plane orchestrates the **wave order**, never the in-cell logic.
- **Independently observable:** every span/metric/log carries `cell_id` + `tenant` + `region` (the
  substrate trace context, `00-platform-substrate.md §10.1`), so blast-radius scoping is mechanical.
- **Bounded:** a cell has a **sizing envelope** (§5); when it fills, the control plane provisions a new
  cell rather than growing this one without bound (unbounded-anything is the EI-02 §5 cascade).

### 3.2 What is inside a cell vs only in the control plane

| Lives **only inside a cell** (region-pinned, personal-data) | Lives **only in the control plane** (global, PII-free) |
|---|---|
| All `Principal` records, profiles, pseudonym map (Id S1/S2) | The `tenant → {cell_id(s), region}` placement record |
| ReBAC tuples, zookies, caches (Id S3–S7) | The cell **inventory** (cell_id, region, capacity, health, version) |
| Events, outbox, Signals, Triggers (Bus) | The **isolation-tier** assigned to a tenant (logical/schema/cell) |
| Edges, search indices, blobs, agent memory, notifications | Per-tenant **opaque routing token** (no name/email — §3.3) |
| OLTP rows, OLAP rows, audit log, DSR receipts | Cell **capacity & utilisation** counters (aggregate, no per-subject) |
| KMS DEKs, crypto-shred keys | The cell-provisioning workflow state (no tenant content) |

The dividing line is **EI-02 §1 made physical**: the tenant is the unit; personal data is *inside* the
cell that holds the tenant; the control plane holds only the *address* of that cell.

### 3.3 The global control plane (holds ZERO in-region personal data — ADR-11.4)

The control plane is the **one globally-distributed component**, and its design rule is absolute: it
**holds no data a data subject could be identified from** — not a name, not an email, not an IP it
persists, not a tenant *display name* that is itself personal data (a sole-trader's tenant could be
"Alice Müller Consulting"). It holds **opaque tenant ids and cell addresses** only.

**How it avoids holding personal data** (the mechanism, not the aspiration):
- **Opaque `tenant_id`** (a ULID/UUID), minted at signup. The human-facing tenant *name* and the signup
  contact (the admin's email) are **personal data and live inside the assigned cell**, never in the
  control plane. Signup is a two-phase flow (§6.1): the control plane *places* the tenant (region +
  cell), then the **cell** captures the admin identity. The control plane never sees the email.
- **Routing token, not identity.** Discovery (§9) returns `{cell_id, region, cell_endpoint}` keyed by the
  opaque `tenant_id` (or a tenant *slug* that is a non-personal, admin-chosen handle — validated to
  exclude obvious PII, and *changeable*, so it is render-time, ADR-11/EI-02 §7 display-key discipline).
- **Aggregate-only telemetry.** The control plane sees cell *utilisation* (counts, bytes, p99s) — never
  per-subject rows. The X-1 telemetry it exports is cell-level (§13).
- **It is itself EU-sovereign** (ADR-11.4): the control plane runs on EU-controlled infra; even though it
  is "global," its *data* is the PII-free registry, so "global" here means *EU-multi-region for
  availability*, not *worldwide personal-data replication*. There is no Schrems-II transfer because there
  is no personal data to transfer (EI-04 §1; GDPR Art. 44).

**The control plane is a `PersonalDataHolder` of exactly nothing** — which we assert, not assume: the
control-plane schema (§4.1) is run through the **same generated data-map / classification registry**
(ADR-12.6) and the assertion *"control plane has zero `is_personal=true` columns"* is a committed CI gate
(§14, D-CP-1). If a field that is personal data ever lands in the control plane, the build fails.

**Availability discipline (fail-static, ADR-17 / ID-1):** the control plane is a *routing* dependency,
not an *authz* dependency. A control-plane hiccup must **not** take cells down: discovery answers are
**cacheable with bounded staleness** (clients and gateways cache `tenant → cell` — §9.3), and a cell
serves its already-resident tenants with **zero control-plane involvement on the hot path**. The control
plane is only on the *signup / placement / provisioning* path, never the *per-request* path. This is the
single most important blast-radius property: **the control plane is not a single point of cascade because
it is off the request hot path entirely** (contrast EI-02 §10's Id-cascade warning — we structurally
avoid making the global plane a per-request dependency).

---

## 4. Data model / schemas

### 4.1 The control-plane registry (global, PII-free) — `myelin-tenancy` backs this

The control plane owns one small Postgres-class registry (EU-multi-region; §13). **Every column here is
non-personal by construction** (asserted, §3.3 / D-CP-1).

```sql
-- The cell inventory: what cells exist, where, how full, what version.
CREATE TABLE cell (
  cell_id        text PRIMARY KEY,          -- 'eu-central-1a' (region-prefixed, opaque)
  region         text NOT NULL,             -- 'eu-central' — immutable; the residency anchor (ADR-11.2)
  status         cell_status NOT NULL,      -- provisioning | active | draining | sealed | retired
  isolation_kind cell_isolation NOT NULL,   -- shared | dedicated  (pool vs silo at the CELL grain, §7)
  capacity       jsonb NOT NULL,            -- the sizing envelope (§5): {tenants_max, tps_max, gb_max,...}
  utilisation    jsonb NOT NULL,            -- aggregate counters (no per-subject), refreshed from cell telemetry
  version        text NOT NULL,             -- deployed artifact version (deploy-wave ordering, §3.1)
  endpoint       text NOT NULL,             -- the cell's gateway address (discovery, §9)
  created_at     timestamptz NOT NULL,
  sealed_at      timestamptz               -- when it stopped accepting new tenants (full)
);

-- The placement record: which cell(s) serve a tenant. THE routing answer.
CREATE TABLE tenant_placement (
  tenant_id      uuid PRIMARY KEY,          -- OPAQUE; no name/email here (§3.3)
  region         text NOT NULL,             -- the tenant's chosen residency region (immutable, §8)
  home_cell      text NOT NULL REFERENCES cell(cell_id),   -- the authoritative cell (§10)
  isolation_tier iso_tier NOT NULL,         -- logical | schema | db | cell  (§7)
  slug           text UNIQUE,               -- non-personal, changeable handle for discovery (§9); render-time
  status         placement_status NOT NULL, -- active | migrating | suspended | offboarding
  created_at     timestamptz NOT NULL,
  -- multi-cell (FLOOR, §10): the additional cells a multi-cell tenant spans, all in `region`
  member_cells   text[] NOT NULL DEFAULT '{}'  -- ⊆ cells in the same region; empty for single-cell
);
CREATE INDEX tenant_by_slug ON tenant_placement (slug);     -- discovery lookup
-- HARD INVARIANT (enforced by trigger + the residency-pin lint, §8):
--   every cell in {home_cell} ∪ member_cells has cell.region = tenant_placement.region.
```

```sql
-- The provisioning workflow state (no tenant content; just orchestration).
CREATE TABLE cell_provisioning (
  id           uuid PRIMARY KEY,
  region       text NOT NULL,
  target_cell  text,                        -- the cell being stood up
  workflow_ref text NOT NULL,               -- durable-workflow handle (ADR-09)
  state        text NOT NULL,               -- requested | infra-up | migrated | active | failed
  requested_at timestamptz NOT NULL
);
```

**Why this is the whole model.** The control plane is deliberately *thin*: a cell inventory, a placement
table, a provisioning log. Everything else is in a cell. This is the AWS control-plane/data-plane split
(SaaS Lens): the control plane is **small, slow-changing, and PII-free**; the data plane (the cells) is
**large, fast, and personal-data-bearing**.

### 4.2 The per-cell tenant directory (inside each cell; not in the control plane)

Each cell keeps a **local** directory of the tenants it hosts (a cache/projection of the control plane's
placement for *its* tenants only), so the cell's gateway can validate "is this tenant mine?" without a
control-plane round-trip on the hot path:

```sql
-- INSIDE the cell. Tenant-partitioned like everything else (EI-02 §1).
CREATE TABLE local_tenant (
  tenant_id      uuid PRIMARY KEY,
  region         text NOT NULL,             -- MUST equal this cell's region (residency-pin, §8)
  isolation_tier iso_tier NOT NULL,
  status         text NOT NULL,
  -- the tenant DISPLAY NAME + admin contact live in this cell's Id store (Id S1), NOT here and NOT in CP
  synced_at      timestamptz NOT NULL       -- last control-plane reconciliation (bounded-staleness, §9.3)
);
```

The split is the point: **the cell knows who its tenants are** (and holds their personal data); **the
control plane knows where each tenant lives** (and holds no personal data). A gateway rejects a request
for a `tenant_id` not in its `local_tenant` directory as a **misroute** (§8) — it never serves a tenant
that isn't placed in this cell.

---

## 5. Cell sizing (the bin and its envelope)

### 5.1 The sizing question, framed

A cell is a bulkhead, and the bulkhead's size is the core trade-off (AWS shuffle-sharding; *Release It!*):
- **Too big** → the blast radius of one cell failing/leaking is large (many tenants), and the largest
  tenant may not fit one cell (forcing the multi-cell case, §10, earlier than necessary).
- **Too small** → per-cell fixed overhead (a full stack per cell) dominates cost; thousands of tiny cells
  are an operational swarm; cross-cell tenants become common (the hardest case) prematurely.

The sizing envelope is therefore a **measured band**, not a single number, and it is **multi-dimensional**
because the binding constraint differs by tenant mix.

### 5.2 The capacity envelope (the `cell.capacity` vector)

A cell's capacity is a **vector**, and a cell is "full" when **any** dimension crosses its high-water mark
(bin-packing with a multi-dimensional bin — first constraint to bind wins):

| Dimension | Why it bounds a cell | Primary owner of the limit |
|---|---|---|
| `tenants_max` | blast-radius cap (how many tenants share this bulkhead) | this doc (policy) |
| `principals_max` | Id tuple-store + authz QPS ceiling per cell | Id (`identity-and-access.md §14`) |
| `events_tps_max` | JetStream per-cell throughput before the column-store seam (BUS-6) | Bus (`event-bus.md §7`) |
| `oltp_gb_max` | before a single PG shard outgrows itself (ADR-10; measure-before-shard) | Storage |
| `blob_gb_max` | object-tier capacity (CI is the heaviest, TE-13) | Storage / CI |
| `search_docs_max` | index size before query latency degrades | Search |

**The rule (ADR-10 / EI-02 §8 — measure, don't predict):** the *initial* envelope is set conservatively
from load tests; the **binding dimension is discovered by measurement**, and the envelope is tightened per
cell from its own telemetry (the `cell.utilisation` counters, §4.1). We do **not** pre-shard or
pre-size from a predicted curve; premature sizing is its own outage (EI-02 §8).

### 5.3 The sizing bands (the default-to-beat)

Three **cell classes**, so a tiny tenant and a giant tenant don't share a sizing model:

| Class | For | `tenants_max` (default-to-beat) | Isolation | Notes |
|---|---|---|---|---|
| **Pool cell** (`shared`) | the long tail of small/medium tenants | ~hundreds–low-thousands of tenants | logical (RLS) + schema where useful (§7) | the common case; bulkhead = the cell |
| **Bridge cell** | a handful of medium-large tenants | ~tens of tenants | DB-per-tenant within the cell | tenant gets a dedicated DB but shares cell infra |
| **Dedicated cell** (`silo` / cell-per-tenant) | one high-assurance / public-sector / very-large tenant | **1** | cell-per-tenant | strongest isolation + cleanest residency (ADR-11.3) |

The numbers are **defaults-to-beat measured in P5 load tests** (§16); the *structure* (three classes,
multi-dimensional envelope, first-constraint-binds) is DECIDED. A tenant's class is its **isolation tier**
(§7), so sizing and isolation are the **same decision**, recorded once in `tenant_placement.isolation_tier`.

### 5.4 The "largest tenant fits a cell" constraint (the multi-cell trigger)

A single cell must be able to hold the **largest single-cell tenant** at the Dedicated class. When a
tenant's load exceeds even a Dedicated cell's envelope (the 10k-person org of SC-2/SC-3), it becomes a
**multi-cell tenant** (§10) — the named hard case. The sizing envelope's *upper* bound is therefore also
the **multi-cell promotion trigger**: cross it and the tenant spans cells (all within its region, §8).

---

## 6. Tenant→cell assignment (placement)

### 6.1 The signup → placement flow (two phases; PII never touches the control plane)

```
1. Prospective admin hits  signup.myelin (a control-plane edge).
2. They choose a RESIDENCY REGION (eu-central, eu-west, eu-north, …) — the only "personal-data-shaping"
   choice, and it is IMMUTABLE thereafter (§8). No email yet.
3. Control plane runs the ASSIGNMENT ALGORITHM (§6.2) → picks {home_cell, isolation_tier} in that region,
   mints an OPAQUE tenant_id, writes tenant_placement (PII-FREE), returns {tenant_id, cell_endpoint}.
4. The browser/CLI is REDIRECTED to the CELL's gateway. ONLY NOW does the admin authenticate / supply
   email / name — captured by the CELL's Id store (Id S1), inside the region. The control plane never saw it.
5. The cell writes its local_tenant row and reconciles with the control plane (aggregate counters only).
```

This is the control-plane/data-plane separation (AWS SaaS Lens) with a **GDPR twist**: placement (region +
cell) is decided *before* any identity is captured, so the personal data is born inside the cell and never
transits the global plane (EI-04 §1; §3.3).

### 6.2 The assignment algorithm

Placement is **region-first, isolation-tier-second, capacity-third, stability-always**:

```
assign(region, requested_tier) -> {cell_id, isolation_tier}:
  1. FILTER cells to region (HARD — never cross region; §8). If none, PROVISION a cell (§6.3) and wait.
  2. RESOLVE the isolation tier:
        - enterprise/high-assurance/public-sector contract  -> Dedicated (cell-per-tenant): provision a fresh cell.
        - large tenant                                       -> Bridge (DB-per-tenant in a bridge cell).
        - default (long tail)                                -> Pool (shared cell, logical isolation).
  3. AMONG candidate cells of that tier in that region, BIN-PACK:
        pick the cell with the LOWEST max-normalised utilisation across the capacity vector (§5.2)
        that still leaves headroom on EVERY dimension after this tenant's estimated footprint.
        (first-fit-decreasing flavour: place larger tenants first when batch-migrating.)
  4. If no cell has headroom -> mark the fullest 'sealed' and PROVISION a new cell (§6.3); place there.
  5. WRITE tenant_placement{tenant_id, region, home_cell, isolation_tier}; return.
```

**Stability of assignment.** A tenant's placement is **sticky** — it is written once and not recomputed
per request (it is an address, not a hash). We deliberately do **not** use consistent-hashing to *derive*
the cell from the tenant id on the hot path (that would re-place tenants when the cell set changes, moving
personal data — forbidden). Consistent/jump-consistent hashing (Karger 1997; Lamping & Veach 2014) is used
only as an **initial bin-packing heuristic** for even spread, never as the *authoritative* lookup — the
authoritative answer is always the `tenant_placement` row. (This is the key deviation from naive
hash-routing: **residency makes placement a stored fact, not a computed one**.)

### 6.3 Cell provisioning (when a cell fills)

When assignment finds no headroom, the control plane provisions a new cell via a **durable workflow**
(ADR-09): stand up the region-pinned infra → deploy the current artifact version → run the
restore-verify/readiness gate (the new cell must pass §14 D-CP-3 before it accepts traffic) → mark
`active`. Provisioning is **off the tenant's hot path**; a tenant waiting on a fresh Dedicated cell is the
only case where signup blocks on provisioning, and that is an enterprise onboarding, not a self-serve
signup (the Pool path always has a warm cell or seals-and-provisions ahead of demand via a high-water
alarm).

### 6.4 Rebalancing / tenant migration (FLOOR — specified-not-built)

A cell can become hot (one tenant grows; the mix shifts). The **honest position**: **live tenant
migration between cells is designed-not-built in v1.**

- **Why it's hard:** migrating a tenant moves *all* its personal data (Id, tuples, events, blobs, search,
  agent memory) to another cell **in the same region** (never cross-region, §8), atomically enough that no
  request sees a split-brain. It is a cross-seam, cross-system data move with the same consistency
  obligation as the restore drill (ADR-18, STOR-4) — row ↔ blob ↔ index ↔ event-offset must land at one
  consistent point in the destination cell.
- **The v1 floor:** *avoid* migration by **sizing headroom** (§5) and by **placing growth-likely tenants
  in Bridge/Dedicated cells up front** (§6.2). A cell that fills is **sealed** (stops taking *new*
  tenants); its existing tenants stay put. This bounds the problem to "don't overfill," which sizing +
  sealing handle without moving data.
- **The follow-on (named):** an online tenant-migration path, reusing the **reindex-from-source** primitive
  (each derived store rebuilt in the destination by re-emit, `event-bus.md §4.9`) + a **crypto-shred
  cut-over** (the source cell shreds the tenant's keys once the destination is verified). Promotion
  trigger: a **measured** hot cell that sealing cannot relieve. Owner: P4 control-plane + Storage/GDPR.
  The drill it owes (when built): **migrate-loses-nothing + lands-in-region + source-shredded** (§14).

### 6.5 Shuffle-sharding: considered, bounded

Shuffle-sharding (AWS Builders' Library) reduces blast radius below cell granularity by spreading a
tenant's *requests* across a random pair of cells. We **do not adopt it for personal-data paths**: it would
require a tenant's personal data to exist in *two* cells, multiplying the residency surface and the erasure
fan-out for a blast-radius win the cell bulkhead already provides. We note it as the right tool for the
**stateless, PII-free edges** (the discovery layer, §9, and the control-plane API itself) — where spreading
load across instances *is* shuffle-sharded — but the **stateful cell stays a single-home bulkhead per
tenant** (with multi-cell, §10, as the explicit, region-bounded exception for giants only).

---

## 7. The isolation spectrum — and how it holds across ALL shared systems

### 7.1 The spectrum (ADR-11.3; AWS silo/pool/bridge)

Three tiers, recorded in `tenant_placement.isolation_tier`, *increasing* isolation and cost:

```
 logical (row-level)  ──►  schema/DB-per-tenant  ──►  cell-per-tenant
  shared infra,            shared cell infra,          dedicated stack,
  tenant_id + RLS          dedicated DB/schema         strongest isolation +
  (the Pool cell)          (the Bridge cell)           cleanest residency (Dedicated cell)
```

- **Logical (Pool):** shared infra; **`(tenant, region)` first column + Postgres Row-Level Security**
  on every table (EI-02 §1; ADR-11.3). The blast-radius unit is the *cell*; the isolation *mechanism* is
  the tenant predicate + RLS. This is the long-tail default.
- **Schema/DB-per-tenant (Bridge):** the tenant gets a dedicated schema or database *within* a shared
  cell — stronger data isolation, same cell infra. For large tenants or those with a contractual
  isolation need short of a full cell.
- **Cell-per-tenant (Dedicated):** the whole cell is the tenant's. Strongest isolation, cleanest
  residency, the model for public-sector/high-assurance (ADR-11.3).

### 7.2 The hard requirement: isolation must hold across *every* shared system's partition surface

ADR-11.3's load-bearing sentence: *"isolation must hold across all shared systems — bus topics, search
indices, caches, blob prefixes, agent context, reference-graph partitions, authz tuples."* This is where
multi-tenancy is usually wounded (EI-02 §1: "a single missing tenant predicate becomes a cross-tenant data
leak"). The matrix below is the **proof obligation**: for each shared system, what the partition key is at
each isolation tier, and the **lint/drill** that enforces it. Every row's partition key is already
specified in the foundational Phase-3 docs — this table *assembles* them into the single isolation
contract and is the §15 cross-check that none was missed.

| Shared-system surface | Logical (Pool) partition | Schema/DB (Bridge) | Cell-per-tenant (Dedicated) | Enforced by | Source doc |
|---|---|---|---|---|---|
| **OLTP rows** | `(tenant, region)` 1st col + **RLS** | dedicated schema/DB | dedicated DB in dedicated cell | `tenant-predicate` lint (compile-time); RLS policy | `00 §2.11`, ADR-10 |
| **Bus subjects/streams** | `evt.<tenant>.<subsystem>.…` stream per `(tenant, subsystem)` | same, dedicated streams | dedicated JetStream | subject **whitelist** + per-tenant stream provisioning | `event-bus.md §2.2, §7.2` |
| **Outbox** | per-service table, `tenant` col + RLS | dedicated schema | dedicated DB | `tenant-predicate` lint; `UNIQUE(aggregate,seq)` | `event-bus.md §3.2` |
| **Search indices** | per-`(tenant, region)` index/partition | dedicated index | dedicated index in dedicated cell | index is tenant-partitioned; `list_objects` pre-filter | overview §4.2; ADR-03 |
| **Caches (Redis/Valkey)** | cache **key prefix = `(tenant, region, …)`**; never source of truth | namespaced | dedicated instance | STOR-3 (cache never SoT); key-prefix convention | `identity-and-access.md §8.5`; STOR-3 |
| **Blob prefixes (object store)** | content-addressed under **`<tenant>/…` prefix**; per-tenant DEK | dedicated bucket/prefix | dedicated bucket | `BlobStore` trait + per-tenant envelope key (STOR-1) | `00 §2.7`; ADR-12.3 |
| **Agent context / memory** | tenant-scoped per run; per-run token carries tenant | tenant-scoped | dedicated | per-run agent token (tenant-bound, ID-2); memory is a holder | `identity-and-access.md §4`; ADR-08 |
| **Reference-graph partitions** | edges `(tenant, region)`-keyed; backlinks `list_objects`-filtered | tenant-partitioned | dedicated | `tenant-predicate`; `list_objects` filter | overview §3; REF-1 |
| **Authz tuples** | `(tenant, region)` + object-hash; **no cross-tenant tuple** | dedicated namespace | dedicated tuple store | partition key; `public` userset is the *only* cross-tenant edge, gated | `identity-and-access.md §6` |
| **OLAP / audit** | `(tenant, region)`-partitioned columnar / append log | dedicated | dedicated | partition key; holder | overview §8, §9.2 |
| **Notifications** | per-user routing within `(tenant, region)` | dedicated | dedicated | tenant-scoped routing; holder | overview §5 |

**The single rule that makes the matrix true:** **`(tenant, region)` is the first element of every
partition key, cache key, subject, index name, blob prefix, and tuple in every cell, at every isolation
tier** (EI-02 §1; ADR-11.2). The isolation *tier* changes the *physical* separation (shared table → schema
→ DB → cell); it never changes the *logical* partition key, so **a tenant predicate is mandatory even in a
Dedicated cell** (defence in depth: a misconfigured Dedicated cell that somehow held two tenants would
still be partitioned). The `tenant-predicate` lint (`00 §2.11`) makes a tenant-less query *fail to
compile*; the cross-tenant IDOR drill (§14, D-CP-2) proves it at runtime.

### 7.3 Why three tiers, not one

A single tier can't serve both the self-serve long tail (where pooling is the only economical option) and
the public-sector tenant whose contract *requires* a dedicated stack (ADR-11.3). The spectrum lets the
*same codebase* serve both — the tier is a placement decision (`isolation_tier`), and the code path is
identical because the **partition key is identical at every tier**. This is the AWS bridge model (mix silo
and pool behind one code path) made into a per-tenant, recorded decision.

---

## 8. Region-pinning enforced at the data layer (misrouting impossible)

ADR-11.2 demands region binding be **immutable-by-default and enforced at the data layer so misrouting a
tenant's personal data is impossible, not merely discouraged.** Here is the mechanism (four layers,
defence in depth):

1. **Region is an immutable property of the cell and of the tenant.** `cell.region` and
   `tenant_placement.region` are set once and **never updated** (a forward-only invariant; an update is
   rejected by a trigger). A tenant changing region is **not an UPDATE** — it is a *new tenant in the new
   region + a full DSR migration/erasure of the old* (the same machinery as offboarding; rare, deliberate,
   legally reviewed). There is no "move this tenant to another region" button.
2. **The placement invariant (DB trigger + lint):** every cell in `{home_cell} ∪ member_cells` **must**
   have `cell.region = tenant_placement.region`. A placement that violates this fails the trigger (§4.1).
   This makes a *multi-cell* tenant **single-region by construction** (§10) — a tenant's cells are all in
   one region.
3. **The write-boundary residency check (the new lint):** a new architecture lint, **`residency-pin`**
   (sibling to `tenant-predicate`, `00 §2.11`), asserts that **every store a service opens carries the
   cell's `region`, and every write asserts `row.region == cell.region`**. A write whose region ≠ the
   cell's region is a *misroute* and is **rejected at the write boundary** (not just logged). The cell's
   region is injected by the bootstrap harness from config (`00 §3.2`), so a service cannot write outside
   its cell's region even by bug. This is the "compile-time/admission-time, not operational" guarantee.
4. **The gateway misroute rejection:** a request arriving at a cell for a `tenant_id` not in that cell's
   `local_tenant` directory (§4.2) — i.e. routed to the wrong cell — is **rejected** (not proxied
   onward; proxying personal data across cells is exactly what residency forbids). The client re-discovers
   (§9) and retries against the correct cell. A misroute is **audited** (it may be an attack or a stale
   discovery cache).

**The standing rule (ADR-11 §Consequences):** **there is no cross-region query path for personal data.**
Layers 2–4 make a cross-region personal-data access *structurally impossible*: the data isn't there (it's
in the tenant's region's cell), the write boundary refuses to put it there, and the gateway refuses to
serve a tenant it doesn't host. The only cross-cell channel is the **PII-free pointer bridge** (§10), which
carries `subject`/`type`/`correlation_id` — never payload, never PII (`event-bus.md §7.4`).

**`myelin tenant residency verify <tenant>`** (overview §7.5; §9, §12) is the operator-facing proof: it
asserts every store holding the tenant's data reports the tenant's region, with a signed attestation.

---

## 9. CLI / endpoint cell discovery (CA-6)

The client (CLI, git wire, API consumer, browser, agent runtime) must learn **which cell** serves a tenant
— without the discovery channel leaking personal data, and without making the control plane a per-request
dependency (§3.3).

### 9.1 The discovery contract

```
discover(tenant_slug | tenant_id) -> { cell_id, region, cell_endpoint, ttl }     // CP, PII-free
```

- Keyed by the **opaque `tenant_id`** or the **non-personal `slug`** (§3.3) — never an email or a person.
- Returns the **cell endpoint** + a **TTL** for caching (§9.3). It returns *no* personal data and *no*
  authz answer — it is pure routing (the AWS tenant-routing pattern).
- The discovery edge is **stateless, PII-free, and shuffle-shardable** (§6.5) — it is one of the few
  components where spreading load across instances is safe because there is no personal data to localise.

### 9.2 How each client discovers

| Client | Discovery mechanism |
|---|---|
| **CLI** (`myelin`) | first call resolves `tenant_slug → cell_endpoint` via the control-plane discovery API; caches in `~/.myelin/cells.json` with the TTL; thereafter talks **directly to the cell** (no CP on the hot path). `myelin cell discover <tenant>` exposes it. |
| **Browser / web app** | the app is served from the **cell endpoint** after signup redirect (§6.1); the SPA holds the cell base-URL; no per-request CP lookup. |
| **git wire (SSH/HTTPS)** | the remote URL **encodes the cell** (`git@<cell-endpoint>:tenant/repo.git` or `https://<cell-endpoint>/tenant/repo.git`). A clone URL is a discovered, cell-pinned address. A push to a stale endpoint gets a misroute redirect (§8 layer 4) with the current endpoint. |
| **API consumer** | the API base URL is the cell endpoint (handed out at signup / in the dashboard); SDKs cache `tenant → endpoint`. |
| **Agent runtime / inter-service** | agents run **inside** the tenant's cell; no discovery needed (they never cross cells on a personal-data path). The dispatch tier is cell-local (`event-bus.md §7.1`). |

### 9.3 Bounded-staleness discovery cache (the fail-static property of routing)

Discovery answers are **cached with a TTL** at every client and at every cell gateway, so:
- the control plane is **off the hot path** (a CP hiccup doesn't stop already-discovered traffic);
- a tenant's `cell_endpoint` changes rarely (only on the deferred migration path, §6.4), so a long TTL is
  safe; a **misroute redirect** (§8) is the correction signal when a cached endpoint goes stale.

This mirrors ADR-17/ID-1 fail-static for *routing*: serve the cached cell address (bounded staleness),
re-discover on a misroute. The staleness bound here is **availability-only** (a stale *route* is corrected
by the destination cell's misroute rejection; it can never cause a *cross-region* access because the wrong
cell refuses to serve, §8 layer 4).

### 9.4 Floor named

**v1 discovery is a control-plane lookup + client cache** (above). A **GeoDNS / anycast edge** that routes
a bare `app.myelin` to the nearest discovery edge, and latency-based steering for multi-region clients, is
a named **follow-on** (it is a performance optimisation over the same PII-free contract; it does not change
the residency model). Owner: P4 control-plane / infra.

---

## 10. Multi-cell tenants + cross-cell collaboration (SC-2/SC-3) — the deepest unknown, honestly

This is **the deepest open problem in the topology** (ADR-11 §Deferred; SC-2/SC-3), and the foundational
docs already named the seam: Id's **home-cell-authoritative + cross-cell read-through with zookie-bounded
staleness** (`identity-and-access.md §14`) and the Bus's **PII-free pointer-event bridge**
(`event-bus.md §7.4`). This section assembles them into one design and is **honest about what is deferred.**

### 10.1 When a tenant must span cells

A tenant spans cells when it **outgrows even a Dedicated cell's sizing envelope** (§5.4) — the canonical
10,000-person org of SC-2/SC-3. **All its cells are in one region** (the placement invariant, §8 layer 2),
so multi-cell is a *scale* mechanism, **never** a residency or cross-region mechanism. A multi-cell tenant
is the explicit, region-bounded exception to the single-home bulkhead (§6.5).

### 10.2 The model (DESIGNED): home-cell-authoritative + read-through

```
            ┌──────── Tenant "bigco-eu" (region = eu-central) ────────┐
            │  home_cell = eu-central-1a   member_cells = [1b, 1c]    │
            │                                                          │
   identity, billing, the authoritative principal directory  ──► HOME CELL (1a)
   (the single source of truth for "who is in this tenant")           │
            │                                                          │
   a project / space / repo physically lives in ONE member cell ──► its home member cell
   (sharded by workload, e.g. by project, NOT by person)              │
            │                                                          │
   cross-cell collaboration = PII-FREE POINTER BRIDGE + per-viewer local resolution
   (a chat msg in 1b references an issue in 1c: the bridge carries the ArtifactRef
    + correlation_id; 1c resolves the issue per the viewer, authz local to 1c)
            └──────────────────────────────────────────────────────────┘
```

1. **Identity is home-cell-authoritative.** The tenant's principal directory (Id S1/S2) lives in the
   **home cell**; member cells **read-through** a principal's *coarse* grants with **zookie-bounded
   staleness** (`identity-and-access.md §14`). A principal "spanning cells" is resolved by the home cell;
   a member cell caches the coarse answer under the fail-static window (ADR-17). Authz *decisions* about an
   object stay **local to the cell that holds the object** (no cross-cell authz on a personal-data hot
   path, ADR-11 §Consequences).
2. **Workload is sharded by aggregate, not by person.** A *project/space/repo/channel* lives wholly in one
   member cell (its home member cell), so its events, tuples, blobs, and search index are cell-local — the
   bulkhead holds per workload. Sharding by *project* (not by user) keeps a unit of collaboration
   undivided, which is what avoids cross-cell chatter on the common path.
3. **Cross-cell references are PII-free pointer events.** A reference from an artifact in cell 1b to one in
   1c rides the **control-plane pointer bridge** (`event-bus.md §7.4`): only `subject` (ArtifactRef) +
   `type` + `correlation_id` cross — **never payload, never PII, never authz state**. The *target* cell
   resolves the `ArtifactRef` **locally, per viewer** (its own `list_objects` / projection API), so an
   unfurl of a cross-cell issue is permission-filtered in the cell that owns the issue. A viewer who can't
   see it gets a tombstone — the same graceful degradation as same-cell (`identity-and-access.md §5` Chat
   unfurl rule). **No personal data crosses the bridge; resolution is always local.**

### 10.3 Latency vs residency (the accepted trade-off, SC-8)

Cross-cell collaboration adds latency: a cross-cell unfurl is a **second cell's** resolution call. We
**accept** this (ADR-11 §Consequences names latency-vs-residency as a deliberate trade-off): residency
forecloses non-EU replicas, and multi-cell forecloses a single global join. Mitigations on the *designed*
path: (a) shard by project so most collaboration is **intra-cell** (cross-cell is the exception, not the
rule); (b) **cache cross-cell projections** per viewer with bounded staleness (the same fail-static
discipline, invalidated by the pointer event's `*.updated`); (c) the pointer bridge is **async** — a
cross-cell reference appears via the bus, not a synchronous cross-cell call in the write path (ADR-11.5).

### 10.4 What is DEFERRED (the honest floor)

**Multi-cell tenants are designed-not-built in v1.** What is *designed* (above): the home-cell-authoritative
identity model, project-grain workload sharding, and the PII-free pointer bridge for cross-cell refs. What
is **deferred to P4 (control-plane + the SC-2/SC-3 resolution)** and explicitly **not** built or proven in
Phase 3:

- **The cross-cell DSR fan-out** (a multi-cell tenant's erasure/export must fan across *all* its cells —
  `00`/overview §8.6 names this `[OPEN → P3]`; we resolve the *shape* (the DSR orchestrator iterates
  `member_cells` from placement, each cell runs its local fan-out, receipts are aggregated PII-free) but
  the **mechanism is P4** alongside the migration path, §6.4).
- **The pointer-bridge concrete protocol** (bridged-field set, the residency proof that no PII crosses,
  per-viewer cross-cell resolution latency budget) — owned by `event-bus.md §10` item 2 and the
  control-plane P4 work.
- **Cross-cell zookie semantics** (a zookie minted in the home cell read in a member cell) —
  `identity-and-access.md §14` names this `[OPEN → P4/P5]`; the consistency model across cells is the
  hardest sub-problem and is **not** claimed solved.
- **Rebalancing a multi-cell tenant** (adding/removing member cells as it grows) — depends on the
  migration floor (§6.4).

**The v1 reality:** the vast majority of tenants are single-cell; the topology, isolation, residency,
discovery, and self-host stories are **complete for them**. The 10k-org case has a *designed* path and a
*named, deferred* build. This is the EI-04 §4 discipline: ship the floor (single-cell complete; multi-cell
designed), name the follow-on, never let the gap be invisible.

### 10.5 Drills owed by multi-cell (when built)

`cross-cell-ref-no-PII-crosses` (the bridge carries only `subject`/`type`/`correlation_id`);
`multi-cell-DSR-reaches-every-member-cell`; `cross-cell-unfurl-permission-filtered-in-target-cell`;
`multi-cell-stays-single-region` (every member cell shares the tenant's region). Enumerated here; executed
in P5 once built (§14).

---

## 11. Self-host = one cell, same artifacts (ADR-11.1)

A **self-hosted install is exactly one cell** of the *identical artifacts* a managed cell runs (ADR-11.1;
`00 §11`). The forcing function: the same monorepo build (ADR-01) produces the cell image; "self-host" is a
deployment topology, not a code fork.

- **No global control plane needed for self-host.** A single-cell install has a **degenerate control
  plane**: discovery trivially returns "this cell"; placement trivially returns "this cell"; the
  `tenant_placement` registry is a one-row local table. The control-plane *code path* exists but resolves
  to the local cell — so self-host and managed share the same routing code (the AWS "same artifacts"
  parity discipline). A self-host can run **multiple tenants** in its one cell (logical isolation) or be a
  single-tenant install (the cell *is* the tenant).
- **Same residency mechanism.** A self-hosted cell has a `region` (the customer's own datacentre/region);
  the `residency-pin` lint (§8) holds identically — the customer's data stays in the customer's region by
  the same write-boundary check. This is *why* self-host parity is a sovereignty mechanism, not just a
  packaging convenience: it forces **no hidden cloud dependencies** (the monorepo makes it auditable,
  ADR-01 §Consequences) and proves the cell is genuinely self-contained.
- **Same drills.** A self-host cell runs the same restore-verify (ADR-18), cross-tenant IDOR (if
  multi-tenant), and residency-verify drills — the cell is the cell.

**Floor named:** managed-fleet features that are *inherently* multi-cell (cross-cell tenants, fleet-wide
deploy waves) are **N/A for self-host** by definition; a self-host is one cell. This is not a gap — it is
the model. The deferred multi-cell build (§10) is a managed-fleet concern.

---

## 12. Contracts / APIs this system exposes + consumes (the glue — STABLE)

`myelin-tenancy` (ADR-01, Rust) carries `TenantId` / `Region` / residency tags / cell-routing client
types. Field **names AND units** reconciled per X-5 against `00 §2.10`.

### 12.1 Exposed (the control-plane + tenancy contract)

| Contract | Signature (illustrative) | Consumed by | Semantics |
|---|---|---|---|
| **discover** | `discover(tenant_slug \| tenant_id) -> {cell_id, region, cell_endpoint, ttl_seconds}` | CLI, SDKs, gateways | PII-free routing only; cacheable (§9). |
| **place** | `place(region, requested_tier) -> {tenant_id, home_cell, isolation_tier, cell_endpoint}` | signup edge | the assignment algorithm (§6.2); PII-free; immutable region. |
| **placement_of** | `placement_of(tenant_id) -> {region, home_cell, member_cells, isolation_tier, status}` | cell gateways, DSR orchestrator | the routing answer (incl. multi-cell fan-out list). |
| **cell_inventory** | `cells(region?) -> [{cell_id, region, status, utilisation, version}]` | ops, the sizing/rebalance jobs | aggregate-only; no per-subject data. |
| **provision_cell** | `provision_cell(region, tier) -> workflow_ref` | sizing high-water alarm, enterprise onboarding | durable-workflow handle (ADR-09); off the hot path. |
| **residency_verify** | `residency_verify(tenant_id) -> SignedAttestation{every_store_region == tenant.region}` | `myelin tenant residency verify`; auditors | the §8 proof; signed, PII-free. |
| **TenantId / Region / ResidencyTag** | types in `myelin-tenancy` | every service (the partition key) | `(tenant, region)` is the first-class key everywhere (EI-02 §1). |
| **telemetry** | `cell_utilisation`, `placement_count`, `provision_latency`, `misroute_count`, `discovery_cache_hit` | Phase-5 drills (X-1) | cell-level survival signals (§13, §14). |

### 12.2 Consumed (what this system depends on)

- **Durable-workflow engine (ADR-09)** — cell provisioning + (future) tenant migration run as durable
  workflows.
- **Id** — *not on the hot path*; placement happens before identity capture (§6.1). The cell's Id store
  holds the tenant's principals; tenancy only routes to the cell.
- **Storage/KMS** — per-cell KMS root; per-tenant DEKs; crypto-shred is the tenant-decommission +
  (future) migration cut-over primitive (ADR-12.3).
- **The substrate** (`00`) — the bootstrap harness injects `region` + `cell_id` into every service (§8
  layer 3); the three-surface topology fronts each cell; the telemetry baseline carries `cell_id`.
- **The Bus pointer bridge** (`event-bus.md §7.4`) — the multi-cell cross-cell channel (floor).

### 12.3 The new architecture lint this doc adds (E-5)

| Lint | Rule | Citation |
|---|---|---|
| **`residency-pin`** | every store a service opens carries the cell's `region`; every write asserts `row.region == cell.region`; a region-mismatched write fails at the boundary | ADR-11.2, §8 |
| **`control-plane-pii-free`** | no control-plane registry column is classified `is_personal=true` (run through the generated data-map, ADR-12.6) | ADR-11.4, §3.3 |

These join the substrate's `tenant-predicate` / `no-cross-db` / `no-raw-publish` set (`00 §2.11`),
committed to CI (E-4: an uncommitted gate is no gate).

---

## 13. Scaling / sharding of the control plane itself

The control plane is the one *global* component, so its own blast-radius discipline matters:

- **It is small and slow-changing** (§4.1): a cell inventory + placement table + provisioning log. It is
  **not** on the per-request hot path (§3.3), so its QPS is signup/discovery/ops, orders of magnitude below
  a cell's request rate. This is why it is *not* a single point of cascade.
- **It is EU-multi-region for availability, single-source-of-truth for placement.** The placement registry
  is replicated across EU regions (read replicas for discovery; a single writer for placement to keep the
  immutable-region invariant simple). Discovery (read) is served from the nearest replica with bounded
  staleness (§9.3); placement (write) is rare (signup) and tolerates the single-writer latency.
- **Discovery is shuffle-shardable** (§6.5): the PII-free discovery edge scales horizontally; a discovery
  instance failing affects a random subset, not all clients, and clients have cached endpoints (§9.3).
- **The control plane never touches a cell's personal data**, so even a full control-plane outage is a
  **signup/provisioning** outage (degraded onboarding), **not** a data-plane outage — existing tenants keep
  working entirely within their cells (the §3.3 fail-static property; the headline blast-radius win).

### 13.1 Stateful-component register + blast-radius note (X-4)

| Stateful component | Shared-state / sharding plan | Blast radius if it dies |
|---|---|---|
| **Placement registry** (CP) | EU-multi-region; single writer, read replicas | new signups/placement pause; **all cells keep serving** (off hot path) |
| **Cell inventory** (CP) | same registry | sizing/provisioning decisions pause; cells unaffected |
| **Per-cell `local_tenant` directory** | inside each cell, tenant-partitioned | that cell's gateway falls back to CP lookup (bounded staleness); no loss |
| **Discovery cache** (clients + gateways) | per-client/per-gateway, TTL-bounded | stale routes corrected by misroute redirect (§8); never a cross-region access |
| **Provisioning workflows** | durable-workflow store (ADR-09) | in-flight cell provisioning resumes on recovery (durable) |

Everything else (the discovery edge, the signup edge, the assignment logic) is **stateless and
replaceable**. The **cells themselves** are the stateful bulkheads; their internal blast-radius registers
live in each foundational doc (`00 §11.1`, `identity-and-access.md §2`, `event-bus.md §7.6`).

---

## 14. Failure modes + the drills owed (quantified; Phase 5 owns mechanics)

Per the PROVE-IT mindset (EI-01 P3; T-2): each property that can fail names the **quantified drill**.

| # | Property / failure mode | Drill (quantified gate) | Reads (telemetry) | Directive/ADR |
|---|---|---|---|---|
| **D-CP-1** | **Control plane holds personal data** | Run the generated data-map over the CP schema; assert **zero `is_personal=true` columns**; attempt to write a name/email to a CP table → rejected by the `control-plane-pii-free` lint at build. Gate: **0 PII fields; build fails on a PII column**. | data-map inventory | ADR-11.4, §3.3 |
| **D-CP-2** | **Cross-tenant / cross-cell read (IDOR)** | Send a request to a cell for a `tenant_id` it doesn't host (path-tenant spoof + stale discovery); assert **misroute rejection, zero cross-tenant/cross-cell read**, and the `tenant-predicate` lint catches a tenant-less query at compile time. Gate: **0 cross-tenant read; misroute audited**. | `misroute_count` | EI-02 §1, ID-3, §8 |
| **D-CP-3** | **Misrouted personal data (wrong region)** | Attempt a write whose `row.region ≠ cell.region` (simulate a routing bug); assert the **`residency-pin` write-boundary check rejects it**; run `residency_verify(tenant)` → signed attestation every store is in-region. Gate: **0 out-of-region writes; attestation passes**. | residency attestation | ADR-11.2, §8 |
| **D-CP-4** | **Control-plane outage cascades to cells** | Hard-down the control plane; assert **already-placed tenants keep serving** (cells run on cached `local_tenant` + discovery cache), only **signup/provisioning** degrades. Gate: **data-plane availability unaffected; CP-down ⇒ onboarding-only impact**. | discovery cache hit, cell RED | ADR-17, §3.3 |
| **D-CP-5** | **Cell blast-radius escapes its bulkhead** | Inject a fatal fault (or a 30× surge) in one cell; assert **other cells unaffected** (no shared stateful dependency), and a noisy tenant is contained to its cell. Gate: **fault contained to one cell; cross-cell impact = 0**. | per-cell RED/USE | REL10-BP04, EI-02 §10 |
| **D-CP-6** | **New cell provisioned unsafe** | Provision a fresh cell; assert it **passes restore-verify + readiness (`00` D-9) before accepting any tenant**; a cell failing the gate stays `provisioning`, never `active`. Gate: **no traffic to an unverified cell**. | provision_latency, readiness | ADR-18, §6.3 |
| **D-CP-7** | **Tenant migration loses/leaks data** (FLOOR — when built) | Migrate a tenant cell→cell (same region); assert **zero loss across-seam** (row↔blob↔index↔offset, ADR-18), **lands in-region**, and the **source cell is crypto-shredded** post-cutover. Gate: **0 loss; in-region; source unrecoverable**. | migration telemetry | §6.4, STOR-4 |
| **D-CP-8** | **Cross-cell ref leaks PII** (FLOOR — when built) | A cross-cell reference (multi-cell tenant); assert the bridge carries **only `subject`/`type`/`correlation_id`**, the target cell resolves **per-viewer locally**, and an unauthorized viewer gets a tombstone. Gate: **0 PII across the bridge; cross-cell unfurl permission-filtered**. | bridge field audit | §10, `event-bus.md §7.4` |

Each drill emits a **green artifact** when it passes; until then the property is **claimed, not proven**
(T-4). D-CP-7/8 are **floors** — the property is *designed* (§6.4, §10) and the drill is *owed when the
follow-on is built*, named here so the gap is visible (EI-04 §4).

---

## 15. Required changes to foundational systems

This design is built on the foundational Phase-3 contracts and mostly *consumes* them. The changes it
**requires** (stated explicitly per the prompt):

1. **Substrate (`00-platform-substrate.md`) — add the `residency-pin` and `control-plane-pii-free` lints**
   to the architecture-lint table (`00 §2.11`). The substrate already injects `region`+`cell_id` from
   config (`00 §3.2`) and carries `cell_id` in trace context (`00 §10.1`); this doc *adds the two lints*
   that turn those into enforced invariants (§8, §12.3). **Required change: extend `00 §2.11`.**
2. **Substrate harness — assert region at the write boundary.** The bootstrap harness's query layer must
   thread the cell `region` into every store handle so the `residency-pin` check is mechanical (not
   per-service code). **Required change: the harness query builder asserts `row.region == cell.region` on
   write** (a small extension to `00 §3.3`).
3. **Id (`identity-and-access.md`) — none required; this doc consumes §14's** home-cell-authoritative +
   cross-cell read-through seam *as written*. (Multi-cell zookie semantics stay Id's `[OPEN → P4/P5]`.)
4. **Bus (`event-bus.md`) — none required; this doc consumes §7.4's** PII-free pointer-event bridge floor
   *as written*. (The bridged-field protocol stays the Bus's `event-bus.md §10` item 2.)
5. **GDPR/Audit (overview §8) — confirm the DSR orchestrator iterates `member_cells`** from
   `placement_of(tenant_id)` for a multi-cell tenant (the *shape* is decided here, §10.4; the mechanism is
   the DSR orchestrator's P3/P4 detail). **Required confirmation, not a redesign.**

No existing ADR or foundational decision is reversed; these are additive (the E-5 lints) or consumptions
of already-named seams.

---

## 16. Open questions for Phase 4 / Phase 5 / Legal

- **[OPEN → P4 (control plane)]** **Multi-cell tenants — the build.** The cross-cell DSR fan-out mechanism,
  the pointer-bridge concrete protocol (bridged fields + residency proof), cross-cell zookie consistency,
  and multi-cell rebalancing (§10.4). The *design* is decided; the *build* is the deepest deferred item
  (SC-2/SC-3).
- **[OPEN → P4 (control plane + Storage/GDPR)]** **Live tenant migration** (§6.4) — the online cell→cell
  move (same region) reusing reindex-from-source + crypto-shred cut-over. Promotion trigger: a measured hot
  cell sealing cannot relieve.
- **[OPEN → P5, measured]** **The sizing-band numbers** (§5.3): `tenants_max` per class and which capacity
  dimension binds first — set conservatively now, tightened from load-test + per-cell telemetry
  (measure-before-shard, ADR-10). This doc proposes the three-class structure as the default-to-beat.
- **[OPEN → P4 (infra)]** **GeoDNS/anycast discovery edge** (§9.4) — a latency optimisation over the
  PII-free discovery contract; v1 is CP-lookup + client cache.
- **[OPEN → P4 (Search/Storage)]** **Per-tenant index/DB provisioning at the Bridge tier** — the exact
  schema-per-tenant vs DB-per-tenant cut-over and its quota model (§7.1) is per-store detail.
- **[OPEN → LEGAL / DPO]** **Region change as new-tenant-+-DSR** (§8 layer 1) — confirm the legal posture
  that a residency change is a migrate-and-erase, not an in-place UPDATE. **Tenant slug PII screening**
  (§3.3/§9.1) — confirm the rule that excludes obviously-personal slugs. **Cross-cell pointer bridge
  residency proof** (§10) — counsel sign-off that `subject`/`type`/`correlation_id` are not personal data
  for a given tenant before multi-cell ships (ties to GD-6 free-text PII completeness).

---

## 17. Cross-references

- **Spine:** ADR-11 (the cell topology this resolves), ADR-01 (`myelin-tenancy` crate; monorepo
  self-host parity), ADR-03/ADR-13 (no cross-tenant query path; the glue), ADR-10/ADR-12 (storage
  tiering; PersonalDataHolder/crypto-shred — the migration + decommission primitives), ADR-16 (per-cell
  backpressure/blast-radius), ADR-17 (fail-static — applied to *routing*).
- **Foundational Phase-3 docs consumed:** [`00-platform-substrate.md`](./00-platform-substrate.md)
  (harness, three-surface topology, lints, blast-radius register, telemetry);
  [`identity-and-access.md`](./identity-and-access.md) §14 (home-cell-authoritative + cross-cell
  read-through; zookies; `list_objects`); [`event-bus.md`](./event-bus.md) §7.4 (the PII-free
  pointer-event bridge floor; cell-local bus).
- **Directives:** ID-1/ID-3 (fail-static; tenant-from-token), X-2/X-4/X-5 (three-surface; blast-radius
  register; name+unit reconciliation), STOR-1…STOR-4 (blob trait; forward-only; cache-never-SoT; cross-seam
  restore — reused by migration), GD-3 (holder auto-registration), BUS-2 (outbox-only emit).
- **Doctrine:** EI-02 §1 (tenant is the unit; no cross-tenant query path; residency = shard key), §10
  (blast-radius first; fail-static not fail-closed); EI-04 §1 (residency as region-pinning; no cross-region
  query path; crypto-shred substrate).
- **Phase-2 springboard:** [`shared-systems-overview.md`](../02-holistic-architecture/shared-systems-overview.md)
  (§1 Id cell scale, §8.6 multi-cell DSR, §12 the P3 backlog this doc clears for tenancy).
- **Prior art:** AWS Builders' Library (shuffle-sharding; cell-based architecture); AWS Well-Architected
  REL10-BP04 (bulkhead); AWS SaaS Lens (control-plane/data-plane split; silo/pool/bridge isolation); Nygard
  *Release It!* (Bulkhead); Karger et al. (consistent hashing, STOC 1997); Lamping & Veach (jump-consistent
  hash, 2014); GDPR Art. 44–49 + Schrems II (residency); Zanzibar (OSDI 2019, zookie bounded staleness).
