# Phase 5 — Tenancy & Control Plane (refined, canonical)

> Phase: `05-refined-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md)
> (single source of truth, never contradicted). Binding doctrine:
> [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) §1
> (tenant-is-the-unit) / §10 (blast-radius, fail-static),
> [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1 (residency as
> region-pinning, no cross-region query path).
>
> **What this doc is.** The REFINED, canonical "Tenancy & control plane" shared-system architecture that
> Phase 6/7 build on. It **carries forward the Phase-3 design**
> ([`../03-shared-systems-architecture/tenancy-and-control-plane.md`](../03-shared-systems-architecture/tenancy-and-control-plane.md))
> as the base and **applies** the Phase-5 reconciliation
> ([`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md), OQ-I + §10 mirror gate) and this
> system's change requests
> ([`../04-subsystem-architectures/cross-subsystem-change-requests.md`](../04-subsystem-architectures/cross-subsystem-change-requests.md)
> §10). The frozen contract surface is [`contract-index.md`](./contract-index.md) §12 (12.1–12.6) + 10.5.
>
> **No ADR is reversed.** ADR-11 (cell topology) stands unchanged; this is *confirmation + additive
> sharpening*. Where a section is unchanged from Phase 3 it says so and cites the Phase-3 section rather than
> restating it.
>
> **Units (frozen, never re-litigated):** timestamps = RFC-3339 UTC; budgets/costs = integer minor-units;
> TTLs/staleness/timers = **seconds**; resilient-client timeouts = milliseconds;
> `pii_key_ref = kms://<tenant>/<dek-epoch>/<class>`. The `(tenant, region)` partition key and the
> `ArtifactRef` token table (Bus §6.2) are the unchanged names/units authority.

---

## 0. Changes vs Phase 3 (every change, listed)

The Tenancy & Control Plane system is **overwhelmingly CONFIRMED** from Phase 3: the cell-as-bulkhead
thesis, the PII-free control plane, the four-layer region-pinning enforcement, discovery, the isolation
spectrum, self-host parity, and the multi-cell *design* all stand exactly as written. The reconciliation
touched this system in three places only:

| # | Change | Kind | Contract | Source |
|---|---|---|---|---|
| **C-1** | **Repo-granular placement** — `placement_of` gains `placement_of(repo) → cell + group`, region-pinned and **relocatable** (no node-pinning); `discover` is usable by the git wire. | **SHARPEN** | 12.2 | CR §10 (GIT); recon §10 |
| **C-2** | **CI no-global-pool attestation** — `residency_verify` explicitly covers the **CI runner pool + log/artifact/cache region**, so the no-global-pool property is attestable per-tenant. | **SHARPEN** | 12.4 | CR §10 (CI); recon §10 |
| **C-3** | **Cross-cell PII-free pointer bridge frame frozen** — `CrossCellPointer{subject (opaque), type, correlation_id, home_cell}`; resolution **always cell-local** (the home cell renders + permission-checks; only the projection crosses). The named multi-cell floor ISS rollup / KN collab / CHAT cross-org channels ride. | **SHARPEN → frozen** | 12.6 | OQ-I; CR §10 |
| **C-4** | **Outbound push-mirror residency gate (NEW)** — an outbound mirror that targets an extra-EU host for PII-bearing content is **policy-gated at the control plane** (denied by default via the `transfer_allowed` registry). Co-owned with GDPR/Audit (10.5); the control plane is the *placement/residency* half. | **NEW** | 10.5 (Git mirror half) | CR §10 (GIT); recon §10 |

Everything else in this document is **CONFIRMED unchanged** and cited to its Phase-3 section. The two new
lints this doc owns (`residency-pin`, `control-plane-pii-free`) are **CONFIRMED** in the contract index 1.6
lint set and unchanged.

**Not changed (explicitly):** cell anatomy (§3 P3), the control-plane registry schema (§4 P3), cell sizing
(§5 P3), the assignment algorithm (§6 P3), the isolation matrix (§7 P3), the four-layer region-pinning
mechanism (§8 P3), discovery (§9 P3), the multi-cell *design* and its deferral (§10 P3), self-host parity
(§11 P3), control-plane scaling (§13 P3), the eight drills D-CP-1..8 (§14 P3).

---

## 1. Purpose, responsibilities, thesis (CONFIRMED — Phase 3 §1)

**Unchanged.** Tenancy & Control Plane owns: the **cell** as the unit of sovereignty + scale +
blast-radius; **tenant→cell assignment**; the **isolation spectrum** (logical/schema/db/cell) and the proof
it holds across every shared system; the **global, personal-data-free control plane**; **region-pinning at
the data layer** (misrouting impossible); **cell discovery**; **multi-cell tenants** (designed-not-built);
**self-host parity**. It is **not** Identity, Bus, Storage, or GDPR — it routes *to* the cell where those
run and holds **no tuples, no events, no blobs, no profiles**, only the routing/placement registry. See
Phase-3 §1.1–§1.3 for the full thesis; it is carried forward verbatim.

The one-paragraph thesis (Phase-3 §1.3) is unchanged: *a cell is a bulkhead — a complete, region-pinned,
independently-deployable copy of the whole stack serving a bounded set of tenants; a tenant is permanently
bound to a cell by a placement record in a PII-free global control plane whose only job is "which cell
serves this tenant?"; region is the cell's immutable property and the compiled-in shard key, so "EU data
stays in EU" and "scale this tenant out" are the same mechanism; self-host is one cell of identical
artifacts.*

---

## 2. Prior art (CONFIRMED — Phase 3 §2)

**Unchanged.** Cell-based / bulkhead architecture (AWS Builders' Library shuffle-sharding; AWS
Well-Architected REL10-BP04; Slack cellular architecture 2023; Nygard *Release It!* Bulkhead);
control-plane/data-plane split (AWS SaaS Lens); silo/pool/bridge isolation tiers; residency-as-shard-key
(GDPR Art. 44–49, Schrems II); consistent / jump-consistent hashing (Karger 1997; Lamping & Veach 2014) as
a bin-packing heuristic only; Zanzibar zookie bounded-staleness (OSDI 2019) for cross-cell reads; Google
SRE ch. 22 fail-static. The deliberate deviation from the hyperscaler playbook — a **PII-free,
EU-sovereign** control plane — is unchanged. See Phase-3 §2 table.

---

## 3. The cell + the global control plane (CONFIRMED — Phase 3 §3)

**Unchanged.** A cell is a complete region-pinned stack of all five subsystems + all shared systems on
commodity, EU-deployable, self-hostable primitives (Phase-3 §3.1, the anatomy diagram). The split between
what lives **only inside a cell** (all personal data: principals, tuples, events, edges, indices, blobs,
agent memory, OLTP/OLAP/audit rows, KMS DEKs) and what lives **only in the control plane** (the PII-free
`tenant → {cell_id(s), region}` placement record, cell inventory, isolation tier, opaque routing token,
aggregate utilisation, provisioning state) is unchanged (Phase-3 §3.2).

The control plane **holds ZERO in-region personal data** (Phase-3 §3.3, ADR-11.4): opaque `tenant_id`
only; the human tenant name + admin email are born **inside the assigned cell** (two-phase signup, §6);
aggregate-only telemetry; itself EU-sovereign; off the per-request hot path (the single most important
blast-radius property). The assertion *"control plane has zero `is_personal=true` columns"* is a committed
CI gate (`control-plane-pii-free` lint, D-CP-1). All unchanged.

**Refinement note (C-2, additive):** the per-tenant residency attestation (§4 contracts) now explicitly
includes the **CI runner pool + CI log/artifact/cache region** in the set of stores it covers — see §4.

---

## 4. Contracts exposed & consumed (the glue — STABLE, frozen)

This is the refined, final contract surface, matching [`contract-index.md`](./contract-index.md) §12 +
10.5. Field **names AND units** reconciled per the names/units anchor (`00 §2.10`).

### 4.1 Exposed contracts (control-plane + tenancy)

| Contract | Signature (frozen) | Consumed by | Status | Semantics |
|---|---|---|---|---|
| **`TenantId` / `Region` / `ResidencyTag`** (12.1) | types in `myelin-tenancy`; `(tenant, region)` is the first-class partition key, injected by the harness | every service | **CONFIRMED** | the shard key everywhere (EI-02 §1); identical at every isolation tier |
| **`discover`** (12.2) | `discover(tenant_slug \| tenant_id) → {cell_id, region, cell_endpoint, ttl_seconds}` | CLI, SDKs, gateways, **git wire** | **SHARPENED** (git-wire use, C-1) | PII-free routing only; cacheable (§7); no authz answer |
| **`place`** (12.3) | `place(region, requested_tier) → {tenant_id, home_cell, isolation_tier, cell_endpoint}` | signup edge | **CONFIRMED** | the assignment algorithm (§6); PII-free; **region immutable** |
| **`placement_of`** (12.3) | `placement_of(tenant_id) → {region, home_cell, member_cells, isolation_tier, status}` **+ `placement_of(repo) → {cell_id, group, region, status}`** | cell gateways, DSR orchestrator, **git wire / Git subsystem** | **SHARPENED** (repo-granular, C-1) | the routing answer; `member_cells` for multi-cell fan-out; **repos region-pinned + relocatable, never node-pinned** |
| **`cell_inventory`** | `cells(region?) → [{cell_id, region, status, utilisation, version}]` | ops, sizing/rebalance jobs | **CONFIRMED** | aggregate-only; no per-subject data |
| **`provision_cell`** | `provision_cell(region, tier) → workflow_ref` | sizing high-water alarm, enterprise onboarding | **CONFIRMED** | durable-workflow handle (ADR-09); off the hot path |
| **`residency_verify`** (12.4) | `residency_verify(tenant_id) → SignedAttestation{ every_store_region == tenant.region }` — **the store set explicitly includes the CI runner pool + CI log / artifact / cache region** | `myelin tenant residency verify`; auditors | **SHARPENED** (CI no-global-pool, C-2) | the §5 proof; signed, PII-free; the no-global-pool property is attestable |
| **`CrossCellPointer`** (12.6) | the PII-free cross-cell bridge frame (§6 below) | multi-cell event/ref/search/inbox/workflow/DSR fan-out | **SHARPENED → frozen** (OQ-I, C-3) | resolution always cell-local; only the projection crosses |
| **telemetry** | `cell_utilisation`, `placement_count`, `provision_latency`, `misroute_count`, `discovery_cache_hit` | Phase-5 drills | **CONFIRMED** | cell-level survival signals (§14) |

### 4.2 Consumed contracts (what Tenancy depends on)

**Unchanged from Phase-3 §12.2**, with one addition (C-4):

- **Durable-workflow engine** (`myelin-flow` 9.1) — cell provisioning + (future) tenant migration run as
  durable workflows.
- **Identity** — *not on the hot path*; placement happens before identity capture (§6). The cell's Id store
  holds the tenant's principals; tenancy only routes.
- **Storage / KMS** (11.3) — per-cell KMS root; per-tenant DEKs; crypto-shred is the tenant-decommission +
  (future) migration cut-over primitive.
- **The substrate** (`00`) — the bootstrap harness injects `region` + `cell_id` into every service and
  carries `cell_id` in trace context; the three-surface topology fronts each cell.
- **The Bus pointer bridge** (`event-bus.md §7.4`) — the multi-cell cross-cell channel (floor), now framed
  by `CrossCellPointer` (§6).
- **GDPR/Audit `transfer_allowed` registry** (10.5) — **NEW dependency (C-4)**: the control plane consults
  the `subprocessors` / `transfer_allowed` registry to **gate an outbound push-mirror** whose target is an
  extra-EU host (§7.4 below). The control plane owns the *placement/residency* judgement ("does this target
  cross the residency boundary?"); GDPR/Audit owns the *lawful-transfer* registry.

### 4.3 The two lints this doc owns (CONFIRMED — Phase 3 §12.3, in contract-index 1.6)

| Lint | Rule | Status |
|---|---|---|
| **`residency-pin`** | every store a service opens carries the cell's `region`; every write asserts `row.region == cell.region`; a region-mismatched write fails at the boundary | CONFIRMED |
| **`control-plane-pii-free`** | no control-plane registry column is classified `is_personal=true` (run through the generated data-map) | CONFIRMED |

Both are unchanged and committed to CI (an uncommitted gate is no gate).

---

## 5. Data model + region-pinning (CONFIRMED — Phase 3 §4, §8) — with repo-granular placement (C-1)

### 5.1 The control-plane registry (CONFIRMED — Phase 3 §4.1)

**Unchanged.** Three small PII-free tables: `cell` (inventory: `cell_id`, immutable `region`, `status`,
`isolation_kind`, `capacity` vector, `utilisation`, `version`, `endpoint`), `tenant_placement`
(`tenant_id` opaque PK, immutable `region`, `home_cell`, `isolation_tier`, changeable non-personal `slug`,
`status`, `member_cells[]`), and `cell_provisioning` (orchestration log). The HARD INVARIANT —
**every cell in `{home_cell} ∪ member_cells` has `cell.region = tenant_placement.region`** (trigger +
`residency-pin` lint) — is unchanged. The per-cell `local_tenant` directory (Phase-3 §4.2) is unchanged.

### 5.2 Repo-granular placement (SHARPENED — C-1)

`placement_of` is extended to answer at **repo granularity** for the git wire (CR §10, GIT):

```
placement_of(repo: ArtifactRef) → { cell_id, group, region, status }
```

- A repo's `cell_id` is its tenant's `home_cell` (single-cell) or the **member cell** that homes the
  repo's workload (multi-cell, sharded by aggregate not by person — Phase-3 §10.2). `group` is the
  repo-storage group within the cell (the object-backed pack tier's placement, Storage 11.2).
- **Region-pinned + relocatable, never node-pinned.** A repo's placement is a *stored fact* (like tenant
  placement, Phase-3 §6.2) — it can be **relocated** within its region (the deferred migration path, §6.4
  P3) without a hash recompute, but it is **never** derived from a node hash on the hot path (that would
  move data on cell-set changes — forbidden). This is the property the git wire needs: a clone URL encodes
  the cell, and a repo can move cells (same region) without its URL identity being a node pin.
- The git wire (`git@<cell-endpoint>:tenant/repo.git`) discovers the cell via `discover` / `placement_of`
  and gets a **misroute redirect** (§7) to the current endpoint if relocated — exactly the discovery
  fail-static property (Phase-3 §9.3), now extended to repo grain.

### 5.3 Region-pinning enforced at the data layer (CONFIRMED — Phase 3 §8)

**Unchanged.** Four layers of defence-in-depth make misrouting personal data *structurally impossible*:
(1) region is an **immutable property** of cell and tenant — a region change is a new-tenant-+-DSR, never an
UPDATE; (2) the placement invariant (DB trigger + lint) keeps every member cell in the tenant's region —
**multi-cell is single-region by construction**; (3) the `residency-pin` write-boundary check rejects any
`row.region ≠ cell.region` write at the boundary, with the cell's region injected by the harness; (4) the
gateway rejects (does not proxy) a request for a `tenant_id` it doesn't host. **There is no cross-region
query path for personal data.** The only cross-cell channel is the PII-free pointer bridge (§6). All
unchanged — see Phase-3 §8 for the full mechanism.

### 5.4 CI no-global-pool attestation (SHARPENED — C-2)

`residency_verify(tenant_id)` (Phase-3 §8, §12.1) is unchanged in shape but its **store set is now
explicitly enumerated to cover the CI surfaces** the change request named (CR §10, CI): the **CI runner
pool**, the **CI log tier** (T3 content-addressed segments, Storage 11.8), the **CI artifact store**, and
the **CI cache namespaces** (including the trust-tier/branch-scoped namespaces, Storage 11.2) all report
the tenant's region into the signed attestation. This makes the EU-sovereign pitch's *no-global-CI-pool*
claim **attestable per tenant** — a CI runner that executed a tenant's job in the wrong region would fail
`residency_verify`. The mechanism is unchanged (every store reports its region; the attestation aggregates
them); only the coverage is pinned.

---

## 6. The cross-cell PII-free pointer bridge (SHARPENED → frozen — OQ-I, C-3)

This is the **named multi-cell floor** (Phase-3 §10): single-home-cell is v1; cross-cell is
**designed-not-built**. The reconciliation (OQ-I) freezes the **bridge frame** so ISS cross-cell portfolio
rollup, KN cross-cell collab, and CHAT cross-org channels all ride one PII-free shape.

### 6.1 The frame (frozen, PII-free)

```
CrossCellPointer {
  subject:        OpaqueSubjectId,    // an opaque id — NEVER a name/email/body (control-plane-pii-free lint)
  type:           ArtifactType,       // what kind of thing is pointed at (issue/page/channel/...)
  correlation_id: CorrelationId,      // ties it to the originating causal chain (BUS-5)
  home_cell:      CellId,             // where it lives; resolution happens THERE
}
```

The bridge carries **only** these four fields — never payload, never PII, never authz state. This is the
Phase-3 §8/§10 PII-free pointer-event bridge (`event-bus.md §7.4`) with its field set now frozen. The
`subject` is an opaque `ArtifactRef`-class id, not a person.

### 6.2 Resolution is always cell-local (the load-bearing rule)

A viewer in cell A wanting to render a pointer to an artifact homed in cell B does **not** fetch B's data
into A. Instead:

1. A's gateway, holding the viewer's identity, asks **cell B** to `resolve(ref, viewer, mode)` (Refs 5.2)
   **in B**;
2. permission-checked **in B** against B's own tuples (`check` / `list_objects`, Id 4.2/4.3);
3. B returns only the **already-rendered, already-permission-filtered projection** (or a tombstone) —
   never raw rows, never PII that should stay in B.

So the control plane carries only the pointer; the **resolution is a per-viewer cell-local projection
fetch**. ISS portfolio rollup aggregates *projections* (counts, titles the viewer may see); KN cross-cell
collab and CHAT cross-org channels resolve membership/content **in the home cell**; an unauthorised viewer
gets a tombstone (the same graceful degradation as same-cell). The DSR orchestrator iterates `member_cells`
(GDPR 10.4) over this same bridge.

### 6.3 What is designed vs deferred (the honest floor — CONFIRMED, Phase 3 §10.4)

**Unchanged.** *Designed:* home-cell-authoritative identity (Id home-cell-authoritative + cross-cell
read-through with zookie-bounded staleness), project-grain workload sharding (a project/space/repo/channel
lives wholly in one member cell), and this PII-free pointer bridge. *Deferred to a later build (named, not
invisible):* the cross-cell DSR fan-out **mechanism**, the per-viewer cross-cell resolution latency budget,
**cross-cell zookie consistency** (the hardest sub-problem — a zookie minted in the home cell read in a
member cell), and multi-cell rebalancing. The v1 reality: the vast majority of tenants are single-cell and
their topology/isolation/residency/discovery/self-host stories are **complete**; the 10k-org case has a
designed path and a named, deferred build.

---

## 7. Algorithms (CONFIRMED — Phase 3 §5, §6, §9) + the outbound-mirror gate (C-4)

### 7.1 Cell sizing (CONFIRMED — Phase 3 §5)

**Unchanged.** The capacity envelope is a **multi-dimensional vector** (tenants/principals/events-tps/
oltp-gb/blob-gb/search-docs); a cell is full when **any** dimension crosses its high-water mark
(first-constraint-binds bin-packing). The binding dimension is **discovered by measurement**, never
predicted (ADR-10 / EI-02 §8). Three cell classes — **Pool** (shared, logical/RLS, the long tail),
**Bridge** (DB-per-tenant within a shared cell), **Dedicated** (cell-per-tenant, public-sector/high-
assurance) — map 1:1 to the isolation tier. The sizing-band *numbers* remain an `[OPEN → P5, measured]`
default-to-beat (§8 below). See Phase-3 §5.

### 7.2 Tenant→cell assignment (CONFIRMED — Phase 3 §6)

**Unchanged.** Two-phase signup keeps PII off the control plane (region chosen first; identity captured
inside the cell). Assignment is **region-first, isolation-tier-second, capacity-third, stability-always**;
placement is a **sticky stored fact**, never a hot-path hash. Cell provisioning is a durable workflow,
off the hot path, gated by restore-verify/readiness before a cell goes `active`. Live tenant migration is
**designed-not-built** (Phase-3 §6.4) — the v1 floor is *avoid migration by sizing headroom + sealing*; the
follow-on reuses reindex-from-source + crypto-shred cut-over, triggered by a **measured** hot cell.
Shuffle-sharding is adopted only for the stateless PII-free edges, never for personal-data paths
(Phase-3 §6.5).

### 7.3 Cell discovery (CONFIRMED — Phase 3 §9) + git-wire use (C-1)

**Unchanged in mechanism.** `discover` returns `{cell_id, region, cell_endpoint, ttl_seconds}` keyed by the
opaque `tenant_id` or non-personal `slug`; clients cache with the TTL (bounded-staleness fail-static for
*routing*); a misroute redirect is the correction signal. The git wire (SSH/HTTPS) **encodes the cell** in
the remote URL and re-discovers on a misroute redirect — now resolving at **repo granularity** via
`placement_of(repo)` (C-1) so a relocated repo's clone/push is corrected to the current cell-endpoint. The
GeoDNS/anycast edge remains a named `[OPEN → P4 (infra)]` follow-on. See Phase-3 §9.

### 7.4 The outbound push-mirror residency gate (NEW — C-4)

An **outbound push-mirror** (a Git mirror config that pushes a repo to a foreign host) is a **residency
boundary crossing** for PII-bearing content (commit author identity, message bodies). It is therefore
**policy-gated at the control plane** (CR §10, GIT):

```
mirror_allowed(tenant_id, mirror_target) → Allow | Deny{reason}
```

- A mirror target that resolves to an **extra-EU host** for PII-bearing content is **denied by default**.
  The gate consults the GDPR/Audit `transfer_allowed` / `subprocessors` registry (10.5): an outbound
  transfer is permitted only if the registry records a lawful basis (an explicit, legally-reviewed
  `transfer_allowed` entry for that target).
- The split of ownership: the **control plane** decides *"does this target cross the residency boundary?"*
  (it knows the tenant's region and the target's region); **GDPR/Audit** owns the *"is this transfer
  lawful?"* registry (10.5). A mirror push whose target crosses the boundary without a `transfer_allowed`
  entry is refused — not logged-and-allowed.
- This rides the same residency discipline as the four-layer write-boundary enforcement (§5.3): a
  PII-bearing byte does not leave the region absent an explicit, registered lawful basis.

This is the only **NEW** contract this system gains in Phase 5; it is additive over the existing
`transfer_allowed` registry and the residency-pin discipline.

---

## 8. Scaling of the control plane (CONFIRMED — Phase 3 §13)

**Unchanged.** The control plane is **small, slow-changing, PII-free, and off the per-request hot path**,
so it is **not a single point of cascade**: its QPS is signup/discovery/ops, orders of magnitude below a
cell's request rate. It is EU-multi-region for availability (read replicas for discovery, single writer for
placement to keep the immutable-region invariant simple); discovery is shuffle-shardable. A **full
control-plane outage is a signup/provisioning outage only** — existing tenants keep working entirely within
their cells (the headline blast-radius win). The stateful-component register (placement registry / cell
inventory / per-cell `local_tenant` / discovery cache / provisioning workflows) and their blast radii are
unchanged — see Phase-3 §13.1.

---

## 9. Failure modes + drills (CONFIRMED — Phase 3 §14)

**Unchanged.** Eight drills, each a quantified gate emitting a green artifact:

| # | Property | Gate (unchanged) |
|---|---|---|
| **D-CP-1** | Control plane holds personal data | 0 PII fields; build fails on a PII column (`control-plane-pii-free`) |
| **D-CP-2** | Cross-tenant / cross-cell read (IDOR) | 0 cross-tenant read; misroute rejected + audited; tenant-less query fails to compile |
| **D-CP-3** | Misrouted personal data (wrong region) | 0 out-of-region writes (`residency-pin`); `residency_verify` attestation passes |
| **D-CP-4** | Control-plane outage cascades to cells | data-plane availability unaffected; CP-down ⇒ onboarding-only impact |
| **D-CP-5** | Cell blast-radius escapes its bulkhead | fault contained to one cell; cross-cell impact = 0 |
| **D-CP-6** | New cell provisioned unsafe | no traffic to an unverified cell (passes restore-verify + readiness first) |
| **D-CP-7** (FLOOR) | Tenant migration loses/leaks data | 0 loss across-seam; lands in-region; source crypto-shredded — drill owed *when migration is built* |
| **D-CP-8** (FLOOR) | Cross-cell ref leaks PII | 0 PII across the bridge; cross-cell unfurl permission-filtered in the target cell — drill owed *when multi-cell is built* |

**Refinement note:** D-CP-3's attestation now explicitly covers the CI runner/log/artifact/cache stores
(C-2); D-CP-8's "0 PII across the bridge" gate is now asserted against the frozen `CrossCellPointer` frame
(C-3, the four-field bridge). A **NEW drill obligation (C-4)** rides D-CP-3's family: *an outbound mirror to
an extra-EU host without a `transfer_allowed` entry is denied* — assert 0 unauthorised cross-residency
mirror pushes. The drill *mechanics* are unchanged; the coverage is sharpened.

---

## 10. Self-host parity (CONFIRMED — Phase 3 §11)

**Unchanged.** A self-hosted install is **exactly one cell of identical artifacts** (ADR-11.1, same
monorepo build). The control plane is **degenerate** (discovery/placement trivially return "this cell"; the
registry is a one-row local table) but the **same code path** runs. The same `residency-pin` lint holds —
the customer's data stays in the customer's region by the same write-boundary check. The same drills run.
Managed-fleet-only features (cross-cell tenants, fleet deploy waves) are **N/A for self-host by definition**
— not a gap, the model. See Phase-3 §11.

---

## 11. Required changes to foundational systems (CONFIRMED — Phase 3 §15) + C-4

**Unchanged (Phase-3 §15):** extend the substrate lint table with `residency-pin` + `control-plane-pii-
free`; the harness threads the cell `region` into every store handle for a mechanical write-boundary check;
Id and Bus require *no* change (this doc consumes their home-cell-authoritative + pointer-bridge seams as
written); GDPR confirms the DSR orchestrator iterates `member_cells`.

**Added by C-4:** GDPR/Audit's `transfer_allowed` registry (10.5) gains the **outbound-mirror gate**
consumer — the control plane calls `mirror_allowed` which reads `transfer_allowed`. This is a *consumption*
of the existing registry plus the residency judgement; the registry shape is GDPR's (10.5), the
residency-boundary decision is the control plane's (§7.4). No redesign.

---

## 12. Cited prior art (CONFIRMED — Phase 3 §2, §17)

Unchanged — see §2 and Phase-3 §17. Core borrow: cell-based / bulkhead architecture (AWS); deliberate
deviation: PII-free EU-sovereign control plane (residency forecloses the global control-plane conveniences).

---

## 13. Open questions for Phase 6

The Tenancy & Control Plane system carries forward Phase-3's open items unchanged (none was closed by the
reconciliation; none was added beyond C-4's residency gate, which is decided). For Phase 6:

- **[OPEN → P6 build (control plane)] Multi-cell tenants — the build.** The *design* is decided (§6,
  Phase-3 §10) and the `CrossCellPointer` frame is **frozen** (C-3). The **build** is the deepest deferred
  item (SC-2/SC-3): the cross-cell DSR fan-out mechanism, the per-viewer cross-cell resolution latency
  budget, **cross-cell zookie consistency** (the hardest sub-problem, named in Id's open set), and
  multi-cell rebalancing. Drills D-CP-7/D-CP-8 are owed when built.
- **[OPEN → P6 build (control plane + Storage/GDPR)] Live tenant migration** (Phase-3 §6.4) — and, by
  extension, **repo relocation** (C-1): the online cell→cell move (same region) reusing reindex-from-source
  + crypto-shred cut-over. The repo-granular placement (C-1) makes repos *relocatable by design*, but the
  relocation *mechanism* is this same deferred migration build. Promotion trigger: a measured hot cell that
  sealing cannot relieve.
- **[OPEN → P6, measured] The sizing-band numbers** (§7.1, Phase-3 §5.3) — `tenants_max` per class and
  which capacity dimension binds first: set conservatively, tightened from load-test + per-cell telemetry
  (measure-before-shard).
- **[OPEN → P6 (infra)] GeoDNS/anycast discovery edge** (§7.3) — a latency optimisation over the PII-free
  discovery contract; v1 is CP-lookup + client cache.
- **[OPEN → P6 (Search/Storage)] Bridge-tier per-tenant index/DB provisioning** — the schema-per-tenant vs
  DB-per-tenant cut-over and quota model (per-store detail).
- **[OPEN — LEGAL / DPO]** (structural floor ships regardless; flagged to counsel, *we are not counsel*):
  (a) **region change = new-tenant-+-DSR**, not an in-place UPDATE — confirm the legal posture; (b) **tenant
  slug PII screening** — confirm the rule excluding obviously-personal slugs; (c) **cross-cell pointer
  bridge residency proof** — counsel sign-off that `subject`/`type`/`correlation_id` are not personal data
  for a given tenant before multi-cell ships (ties to the X-7 free-text-PII posture); (d) **outbound-mirror
  lawful basis** (C-4) — counsel ratifies the `transfer_allowed` entries that permit an extra-EU mirror
  (Schrems II / Art. 44–49); the default-deny gate ships regardless.

---

## 14. Cross-references

- Refined surface: [`contract-index.md`](./contract-index.md) §12 (12.1–12.6) + 10.5.
- Reconciliation: [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) §OQ-I (cross-cell
  bridge), §10 (residency `placement_of`/`residency_verify`/mirror gate).
- Phase-3 base (carried forward): [`../03-shared-systems-architecture/tenancy-and-control-plane.md`](../03-shared-systems-architecture/tenancy-and-control-plane.md).
- Change requests: [`../04-subsystem-architectures/cross-subsystem-change-requests.md`](../04-subsystem-architectures/cross-subsystem-change-requests.md) §10.
- Spine: ADR-11 (cell topology), ADR-01/03/10/12/13/16/17. Doctrine: EI-02 §1 / §10; EI-04 §1.
