# Phase 6 — Roadmap: Tenancy & Control Plane (`myelin-tenancy`)

> Phase: `06-roadmaps/shared`. The detailed sequenced roadmap for the **tenancy-and-control-plane** shared
> system. Slots into the master sequencing bands M0..M6:
> [`../00-master-sequencing.md`](../00-master-sequencing.md) (§1 ordering thesis Tier 5 "tenant partitioning +
> residency pin"; §2 bands; §3 critical-path/DAG; §4 the gate invariant; §5 name-your-floors — single-cell is
> the named floor, multi-cell the M5 follow-on). Frozen architecture (this roadmap SEQUENCES, it does not
> redesign):
> [`../../05-refined-shared-systems-architecture/tenancy-and-control-plane.md`](../../05-refined-shared-systems-architecture/tenancy-and-control-plane.md)
> (the refined Tenancy architecture, C-1..C-4) + the refined
> [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md)
> §12 (12.1–12.6, the contracts Tenancy owns) + §10.5 (the outbound-mirror gate co-owned with GDPR) + §1/§9/§11
> (the contracts Tenancy consumes). Drills owed:
> [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> §4.2 (CP-D1..CP-D8) + the F2/F3/F7 families + the cross-owner residency instances that ride the control plane
> (STOR-D5, CI-R3, GIT residency/mirror, GA-D8 multi-cell erasure). Doctrine:
> [`../../../external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> (§2 order-by-non-negotiability; §3 prove-it-or-it-isn't-real; §5 the committed gates; §1 name-your-floors,
> code-wins-over-docs) and
> [`../../../external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md) §1 (residency
> as region-pinning, **no cross-region query path** — Tenancy owns the "EU data stays in EU" mechanism). Spine:
> ADR-11 (cell topology), ADR-18 (restore-verify gate, co-owned), ADR-09 (durable workflow for provisioning),
> ADR-12 (GDPR). Date: 2026-06-19.
>
> **The shape of this system, and what that means for sequencing.** Tenancy & Control Plane owns the
> **partition** under everything — `(tenant, region)` is the first-class shard key (12.1) injected by the harness
> into every store handle — and the **routing** to the cell where a tenant's data lives, in a control plane that
> **holds zero personal data**. Three facts dominate the roadmap:
> 1. **The partition + the residency pin land in M1 (Tier 5) and are a hard go/no-go for the entire reactive
>    layer above.** Before any subsystem writes a row, `(tenant, region)` must be the partition key, the
>    `residency-pin` write-boundary check must reject out-of-region writes, the `control-plane-pii-free` lint
>    must hold, and misroute must be rejected (CP-D2/CP-D3). "One tenant's region is immutable, with no
>    cross-region query path" must be true **by construction, not bolted on** (EI-04 §1).
> 2. **Two of the twelve committed lints are Tenancy's and ship in M0** — `residency-pin` and
>    `control-plane-pii-free` — *before* the system they constrain exists, so the partition discipline is
>    structurally enforced from the first write. This is the cheapest ratchet (master §1 Tier 3) and it is
>    Tenancy's, landed early.
> 3. **The control plane is small, slow-changing, PII-free, and off the per-request hot path** — so it is *not*
>    a single point of cascade. A full control-plane outage is a **signup/provisioning outage only**; placed
>    tenants keep serving entirely within their cells (the headline blast-radius win, CP-D4). This is why the
>    control plane can be modest while the *partition discipline* it enforces is load-bearing.
>
> The corollary that orders the work *inside* Tenancy: the **single-cell** topology is complete and proven in
> M1/M4; **multi-cell** (the cross-cell PII-free pointer bridge live, DSR fan-out over `member_cells`, cross-cell
> zookie consistency) is the **named M5 floor follow-on** — designed-not-built in v1, its frame (`CrossCellPointer`)
> frozen now so ISS rollup / KN collab / CHAT cross-org channels ride one PII-free shape. **Live tenant migration
> + repo relocation** is the other named, deferred build (M5, measured-trigger). Tenancy redesigns nothing; it
> *confirms* the frozen Phase-5 architecture and sequences its build.

---

## 0. Where Tenancy & Control Plane lands in the master bands (the one-paragraph map)

Tenancy's **two lints are M0** (`residency-pin`, `control-plane-pii-free`, each with a red+green fixture, wired
into CI loud-never-swallowed) and the `myelin-tenancy` glue-crate skeleton (`TenantId`/`Region`/`ResidencyTag`
types, ADR-01) is frozen in M0 so every consumer compiles against the partition-key contract before the bodies
exist. The **core build is M1** (master Tier 5): the PII-free control-plane registry (cell inventory +
`tenant_placement` + `cell_provisioning`), `discover`/`place`/`placement_of`, the two-phase signup, the
four-layer region-pinning enforcement at the data layer, `residency_verify`, the isolation-tier contract, the
cell-provisioning durable workflow, and the `CrossCellPointer` **frame** (frozen, not yet live). Repo-granular
`placement_of(repo)` (C-1) is **declared** in M1 as the git wire needs it, and goes live with **Git in M3**; the
CI no-global-pool attestation coverage (C-2) is wired when **CI lands in M4**; the outbound-mirror residency gate
(C-4) lands with **Git's mirror config in M3**. **World-scale hardening + the multi-cell floor follow-on + live
migration are M5** (CP-D5 cell bulkhead under 30× surge, CP-D7 migration, CP-D8 cross-cell bridge, GA-D8
multi-cell erasure, the measured sizing-band numbers). Tenancy participates in every M5 whole-system E2E (it is
the partition under all four and the residency attestation in E2E-4) and is dogfooded in M6 (Myelin self-hosts
as exactly one cell).

The honest progression: **first runnable** = early M1 (the degenerate single-cell control plane: `discover`/
`place` return "this cell", `(tenant, region)` injected by the harness, the `residency-pin` lint green — a
service can boot region-pinned and a tenant can be placed); **first useful** = late M1 (the full PII-free
registry + two-phase signup + the four-layer enforcement + `residency_verify` + CP-D2/CP-D3 green, so real
tenant data can be placed with misroute structurally impossible and the silent-data-loss floor STOR-D1 green
*before* that data lands); **production-hardened** = M5 (the 30× cell-bulkhead holds with cross-cell impact 0,
multi-cell is live with the PII-free bridge proven, live migration drilled CP-D7, sizing-band numbers measured,
restore-verify re-confirmed at cell scale).

---

## 1. The contracts Tenancy owns / consumes, mapped to the milestone they land in

From contract-index §12 (owned by Tenancy) + §10.5 (co-owned mirror gate) + §1/§9/§11 (consumed). "Lands" = the
milestone by which the contract must be implemented or callable for the gate that depends on it to be green. A
floor is named inline and tracked in §6.

### 1.1 Owned by Tenancy (contract-index §12 + §10.5 half) — consumed across the platform

| # | Contract | Lands | Notes / floor |
|---|---|---|---|
| 12.1 | `TenantId` / `Region` / `ResidencyTag` — `(tenant, region)` the first-class partition key, injected by the harness; identical at every isolation tier | **M0** (types/crate frozen); **harness injection M1** | the types + the crate are frozen in M0 so every consumer compiles against the partition key; the harness wiring that *threads* `region`+`cell_id` into every store handle lands with the substrate harness (M0/M1 boundary) and is what the `residency-pin` lint checks against. No floor — the shape is the shard key everywhere (EI-02 §1). |
| 12.2 | `discover(slug\|tenant_id) → {cell_id, region, cell_endpoint, ttl_seconds}` **+ repo-granular `placement_of(repo) → {cell_id, group, region, status}`** (region-pinned, relocatable, never node-pinned) | **M1** (tenant-grain `discover`); **repo-grain declared M1, live M3** | tenant-grain discovery is M1 (PII-free routing, client-cached with TTL, fail-static for *routing*). The **repo-granular** `placement_of(repo)` is declared M1 (the contract the git wire compiles against) and goes **live with Git (M3)** — a clone URL encodes the cell; a repo relocates within-region without a URL-identity node pin. Floor: GeoDNS/anycast edge is a named `[OPEN → P4 (infra)]` follow-on; v1 is CP-lookup + client cache. |
| 12.3 | `place(region, requested_tier) → {tenant_id, home_cell, isolation_tier, cell_endpoint}` + `placement_of(tenant_id) → {region, home_cell, member_cells, isolation_tier, status}` — region-first, sticky stored fact, PII-free; `member_cells` for multi-cell fan-out | **M1** | two-phase signup (region chosen first, identity captured **inside** the cell — keeps PII off the control plane). `member_cells` is **single-element in v1** (one home cell); the multi-element fan-out is the **M5 multi-cell floor**. Region is **immutable** (a region change is a new-tenant-+-DSR, never an UPDATE — `[OPEN — LEGAL]` posture, ships regardless). |
| 12.4 | `residency_verify(tenant_id) → SignedAttestation{every_store_region == tenant.region}` — store set **explicitly includes the CI runner pool + CI log/artifact/cache region** (C-2) | **M1** (mechanism + the M1 store set); **CI-store coverage M4** | the mechanism (every store reports its region; the attestation aggregates) is M1 and covers the M1 stores (OLTP, blob, index, KMS). The **CI runner/log/artifact/cache** coverage (C-2) is wired when **CI lands in M4** — proven by CI-R3. The no-global-pool property becomes attestable per-tenant only once CI's stores report in; named as a partial until then. |
| 12.5 | Isolation-tier contract — `logical\|schema\|db\|cell`; the partition key is identical at every tier (Pool/Bridge/Dedicated cell classes) | **M1** (logical/RLS Pool floor); **schema/db/cell tiers M5/on-demand** | v1 floor (named): the **Pool tier** (shared cell, logical/RLS isolation, the long tail) is M1 — it is what `residency-pin` + RLS already enforce. **Bridge** (DB-per-tenant) and **Dedicated** (cell-per-tenant) are provisioned **on demand / M5** (enterprise + public-sector onboarding); the partition-key contract is identical across all three, so the higher tiers are a provisioning concern, not a redesign. |
| 12.6 | Cross-cell PII-free pointer bridge — `CrossCellPointer{subject (opaque), type, correlation_id, home_cell}`; resolution **always cell-local** | **M1** (frame frozen, not live); **live M5** | the **named multi-cell floor**. The four-field frame is **frozen in M1** (so ISS/KN/CHAT consumers compile against it and the `control-plane-pii-free` lint guards it) but the **resolution path goes live in M5**. Resolution is always cell-local: the home cell renders + permission-checks; only the per-viewer projection crosses (never raw rows, never PII). Drill CP-D8 is **owed when multi-cell is built (M5)**. |
| 10.5 (mirror half) | Outbound push-mirror residency gate — `mirror_allowed(tenant_id, mirror_target) → Allow\|Deny{reason}`; an extra-EU PII-bearing target is **deny-by-default**, consulting GDPR's `transfer_allowed` registry | **M3** (with Git's mirror config) | **NEW (C-4)**. The split: the **control plane** decides *"does this target cross the residency boundary?"* (it knows tenant region + target region); **GDPR/Audit** owns the *"is this transfer lawful?"* `transfer_allowed` registry (10.5). The default-deny gate ships regardless; the `[OPEN — LEGAL]` lawful-basis entries are counsel-ratified separately. Lands with Git's mirror feature (M3); its drill rides the CP-D3 family. |
| (lints) | `residency-pin` (every store carries the cell `region`; every write asserts `row.region == cell.region`, region-mismatch fails at the boundary) + `control-plane-pii-free` (no control-plane registry column is `is_personal=true`) | **M0** | the two committed lints this doc owns, each shipped with a **red-fixture** (proves it rejects) + a **green-fixture** (proves it admits), wired into CI loud-never-swallowed (no `\|\| true`). They are the structural floor under every CP drill — a red lint blocks merge (R-2 gate invariant). |
| (telemetry) | `cell_utilisation`, `placement_count`, `provision_latency`, `misroute_count`, `discovery_cache_hit`, `residency-attestation` | **M1** | every CP drill asserts against this signal set; no signal = failed drill (EI-01 §3, observability is part of the pass condition). `misroute_count` is the CP-D2 green artifact; `residency-attestation` is CP-D3's. |

### 1.2 Consumed by Tenancy (what the control plane depends on)

| # | Contract | From | Needed by | Why |
|---|---|---|---|---|
| 1.1/1.2/1.8 | `serve(AppSpec)` + three-surface + the telemetry signal set | Substrate (M0) | M1 | the control plane is itself a service booting from the harness; the harness injects `region`+`cell_id` into every store handle and carries `cell_id` in trace context. The control plane is **off the per-request hot path** (fronts each cell, but discovery is cacheable). |
| 1.6 (`residency-pin`, `control-plane-pii-free`) | the lint set (Tenancy authors these, the substrate/CI hosts them) | M0 | the lints are wired into the M0 CI pipeline; Tenancy authors the rule + both fixtures. |
| 9.1 (`DurableExecutor{start}`) | Durable Workflow (`myelin-flow`, M2) | **provisioning M1 floor; full workflow M2** | cell provisioning + (future) tenant migration run as **durable workflows, off the hot path** (ADR-09). Floor: M1 provisioning can run as a scripted/manual procedure gated by restore-verify+readiness (CP-D6) until `myelin-flow` lands in M2; the **durable** provisioning/migration workflow is M2+ (and migration itself is the M5 build). |
| 11.5 (restore-verify) | Storage (M1, co-owned ADR-18) | M1 | a new cell is not given traffic until it **passes restore-verify + readiness** (CP-D6). The control plane gates provisioning on this — it consumes Storage's restore-verify result as the cell-readiness signal. |
| 11.3 (KMS hierarchy, per-cell root → per-tenant KEK) | Storage (M1) | M1 (decommission); M5 (migration cut-over) | crypto-shred is the tenant-decommission primitive (M1) and the **migration cut-over** primitive (M5: source crypto-shredded after the cell→cell move, CP-D7). |
| 10.5 (`transfer_allowed`/`subprocessors` registry) | GDPR/Audit (M1 structural; entries M3+) | M3 | the outbound-mirror gate (C-4) reads this registry. Tenancy owns the *residency judgement*; GDPR owns the *lawful-transfer registry*. The default-deny gate works on an empty registry (deny everything extra-EU); ratified entries are added by counsel. |
| 10.4 (`dsr_submit` iterates `member_cells`) | GDPR/Audit | M1 (single-cell trivial); M5 (fan-out) | the DSR orchestrator iterates `member_cells` over the OQ-I bridge — trivially one cell in v1; the multi-cell fan-out (GA-D8, per-cell receipt set) is the **M5 floor**. |

**The dependency asymmetry (why Tenancy can land so early):** Tenancy consumes almost nothing on the hot path —
the substrate harness (M0), Storage's restore-verify (M1, the cell-readiness gate), and KMS (M1, decommission).
It is *below* Refs/Search/Agent/Notif/GDPR and co-equal with Identity/Storage in M1. The control plane explicitly
**does not depend on Identity on the hot path** (placement happens *before* identity capture — two-phase signup),
which is what keeps PII off the control plane and lets the partition floor land in M1.

---

## 2. The milestones (each mapped to a master-sequencing band)

Each milestone states its **work**, its **entry dependency** (what must be green to start), and its **exit gate**
(the named, quantified drills that must emit a green artifact to call it done). The band boundaries are the
master-sequencing gates; these milestones refine the work *inside* the band and must not contradict the band
ordering or the gate invariant.

### CP-M0 — The two committed lints + the partition-key crate (in M0)

**Band:** M0 (master Tier 3, the committed ratchet). **Thesis:** make the cross-tenant-leak and
control-plane-PII bug-classes impossible to compile *before* the system they constrain exists.

**Work:**
- The `myelin-tenancy` glue crate skeleton: `TenantId` / `Region` / `ResidencyTag` types (12.1), the
  `(tenant, region)` partition-key shape frozen as the names/units anchor every store aligns to (ADR-01 — a
  change here breaks every consumer's build *now*, never silently in prod).
- The **`residency-pin` lint** (1.6): every store a service opens carries the cell's `region`; every write
  asserts `row.region == cell.region`; a region-mismatched write fails at the boundary. Shipped with a
  **red-fixture** (an out-of-region write that must fail the build) + a **green-fixture** (an in-region write
  that must admit).
- The **`control-plane-pii-free` lint** (1.6): no control-plane registry column is classified `is_personal=true`
  (run through the generated data-map). Shipped with a red-fixture (a `name`/`email` column on a CP table that
  must fail the build) + a green-fixture.
- Both wired into CI **loud, never swallowed** (no `|| true`, no silent filter — EI-01 §5).

**Entry dependency:** the M0 workspace skeleton + the contract-coverage scanner + the data-map derive
(`#[personal_data]`, GDPR 10.2) that `control-plane-pii-free` reads.

**Exit gate (contributes to M0→M1):**
- **CP-D1** floor leg — the two lints **green with both fixtures**: `control-plane-pii-free` red on a PII column,
  `residency-pin` red on an out-of-region write; both green on the clean fixture. (The full CP-D1/D3 *runtime*
  drills land in CP-M1; the lint legs are M0.)
- The lints are part of the "all twelve lints green with both fixtures" M0→M1 go/no-go (master §4).

### CP-M1 — The PII-free control plane + the partition + the four-layer residency pin (the core, in M1)

**Band:** M1 (master Tier 5, tenant partitioning + residency pin). **Thesis:** stand up the partition and the
routing so that "EU data stays in EU, with no cross-region query path" and "no cross-tenant read" are true **by
construction** — *before* any subsystem writes real tenant data, and only after the silent-data-loss floor
(STOR-D1) is green.

**Work:**
- **The PII-free control-plane registry** (5.1 arch): three small tables — `cell` (inventory: `cell_id`,
  immutable `region`, `status`, `isolation_kind`, capacity vector, `utilisation`, `version`, `endpoint`),
  `tenant_placement` (`tenant_id` opaque PK, immutable `region`, `home_cell`, `isolation_tier`, non-personal
  `slug`, `status`, `member_cells[]`), `cell_provisioning` (orchestration log) — plus the per-cell
  `local_tenant` directory. The **HARD INVARIANT** (DB trigger + `residency-pin` lint): every cell in
  `{home_cell} ∪ member_cells` has `cell.region == tenant_placement.region` — **multi-cell is single-region by
  construction.**
- **`discover` + `place` + `placement_of`** (12.2/12.3): PII-free routing, off the hot path; two-phase signup
  (region first, identity captured inside the cell); region-first/isolation-tier-second/capacity-third/
  stability-always assignment; placement a **sticky stored fact, never a hot-path hash**. The harness threads
  `region`+`cell_id` into every store handle (the `residency-pin` runtime check).
- **The four-layer region-pinning enforcement** (5.3 arch): (1) region immutable on cell + tenant; (2) the
  placement invariant trigger; (3) the `residency-pin` write-boundary check rejects `row.region ≠ cell.region`;
  (4) the gateway **rejects** (does not proxy) a request for a `tenant_id` it doesn't host. **No cross-region
  query path for personal data.**
- **`residency_verify`** (12.4): every M1 store (OLTP, blob, index, KMS) reports the tenant's region; the
  attestation aggregates into a `SignedAttestation`. (CI-store coverage is wired in CP-M4.)
- **The isolation-tier contract** (12.5) — the **Pool tier** (logical/RLS) as the v1 floor; Bridge/Dedicated
  declared but provisioned on demand.
- **The `CrossCellPointer` frame** (12.6) — frozen as types (so consumers compile + the `control-plane-pii-free`
  lint guards it), resolution path **not yet live** (M5).
- **Cell-provisioning gating** (CP-D6): a new cell gets no traffic until it passes restore-verify + readiness.
  Floor: provisioning runs as a scripted procedure in M1 (the **durable** workflow is M2 once `myelin-flow`
  exists); the *gating* (restore-verify+readiness before `active`) is M1.
- **Self-host parity** (10 arch): a self-hosted install is **one cell of identical artifacts**; the control plane
  is degenerate (discovery/placement return "this cell"; the registry is a one-row local table) but the **same
  code path** runs; the same `residency-pin` lint holds; the same drills run.

**Entry dependency:** M0 green (the two lints; the `myelin-tenancy` crate; the harness `region`/`cell_id`
injection; the data-map derive). **Co-lands with Identity + Storage in M1.** Storage's **STOR-D1 (restore-verify,
the silent-data-loss floor) must be green before real tenant data is placed** — CP-M1's "place real data" leg
does not start over a red STOR-D1 (master §1 Tier 1).

**Exit gate (contributes to M1→M2 — the partition floor):**
- **CP-D1** (full) — data-map over the control-plane schema → **0 `is_personal=true` columns**; writing a
  name/email fails the build. *(CI.)*
- **CP-D2** — a request to a cell for a `tenant_id` it doesn't host → **misroute rejected, 0 cross-tenant/
  cross-cell read, audited** (`misroute_count`, audit entry). *(CI.)* — this is the master M1→M2 go/no-go pair.
- **CP-D3** — a write where `row.region ≠ cell.region` → **`residency-pin` rejects; `residency_verify`
  attestation passes** (`residency-attestation`). *(CI.)* — the master M1→M2 go/no-go pair.
- **CP-D4** — hard-down the control plane → **already-placed tenants keep serving; only signup/provisioning
  degrades** (`serving-uptime`, degrade scope). *(SCHED.)* — the blast-radius win, fail-static (F7).
- **CP-D6** — provision a fresh cell → **passes restore-verify + readiness before accepting any tenant**; a
  failing cell stays `provisioning`. *(SCHED.)*
- **STOR-D5 (cross-owner, Tenancy's residency leg)** — read/replicate a tenant's data outside its region →
  **impossible** (region in the partition key; `residency-pin` rejects out-of-region writes); **0 cross-region
  PII egress**. *(SCHED.)*
- **Gate-invariant note:** this band does **not** start its "real data" leg over a red **STOR-D1** (the
  silent-data-loss floor, Storage-owned, master §1 Tier 1).

### CP-M3 — Repo-granular placement goes live + the outbound-mirror gate (in M3, with Git)

**Band:** M3 (the producer subsystems — Git). **Thesis:** the git wire needs to route at **repo** granularity
and the mirror feature needs the residency gate; both were *declared* in M1 and go **live with Git**.

**Work:**
- **`placement_of(repo)`** (12.2, C-1) goes **live**: a repo's `cell_id` is its tenant's `home_cell` (single-cell)
  / the member cell that homes the repo's workload (multi-cell, M5); `group` is the repo-storage group within the
  cell (Storage 11.2 object-backed pack tier). **Region-pinned + relocatable, never node-pinned** — a clone URL
  encodes the cell; a repo can move cells (same region) without its URL identity being a node pin. The git wire
  (`git@<cell-endpoint>:tenant/repo.git`) discovers the cell and gets a **misroute redirect** to the current
  endpoint if relocated (the discovery fail-static property at repo grain).
- **The outbound push-mirror residency gate** (10.5 half, C-4): `mirror_allowed(tenant_id, mirror_target)` —
  an extra-EU PII-bearing mirror target is **deny-by-default**, consulting GDPR's `transfer_allowed` registry. A
  mirror push whose target crosses the boundary without a `transfer_allowed` entry is **refused, not
  logged-and-allowed**.

**Entry dependency:** CP-M1 green + Git's data model (M3) + GDPR's `transfer_allowed` registry (the structural
half, M1; ratified entries on demand).

**Exit gate (contributes to M3→M4):**
- **GIT residency leg of CP-D2/CP-D3** — cross-tenant repo access via repo-grain misroute → 0; a relocated repo's
  clone/push is corrected to the current cell-endpoint. *(CI/SCHED, rides GIT-D8.)*
- **The C-4 mirror drill (rides CP-D3's family)** — an outbound mirror to an extra-EU host **without** a
  `transfer_allowed` entry is **denied**; assert **0 unauthorised cross-residency mirror pushes**. *(SCHED.)*

### CP-M4 — CI no-global-pool attestation coverage (in M4, with CI)

**Band:** M4 (the consumer subsystems — CI). **Thesis:** the EU-sovereign pitch's *no-global-CI-pool* claim
becomes **attestable per tenant** once CI's stores report their region into `residency_verify`.

**Work:**
- **`residency_verify` store-set extension** (12.4, C-2): the **CI runner pool**, the **CI log tier** (T3
  content-addressed segments, Storage 11.8), the **CI artifact store**, and the **CI cache namespaces** (incl. the
  trust-tier/branch-scoped namespaces, Storage 11.2) all report the tenant's region into the signed attestation.
  A CI runner that executed a tenant's job in the wrong region would **fail `residency_verify`**.
- **Residency-pinned runners** (CI-R3): an EU-resident tenant's run is claimed **only by an in-region runner**;
  logs/artifacts/caches never leave the region (within-EU CDN); `residency-pin` passes on every write.

**Entry dependency:** CP-M1 green + CI (M4) with its runner pool + log/artifact/cache stores reporting region.

**Exit gate (contributes to M4→M5):**
- **CI-R3 (cross-owner, Tenancy's attestation leg)** — an EU-resident tenant's run → in-region runner only;
  logs/artifacts/caches never leave region; **`residency_verify` attests; `residency-pin` passes on every
  write**. *(SCHED.)* The no-global-pool property is now attestable per-tenant.

### CP-M5 — World-scale hardening + the multi-cell floor follow-on + live migration (in M5)

**Band:** M5 (world-scale hardening + the floor follow-ons + the E2E wedge). **Thesis:** with all five subsystems
on one substrate and the single-cell correctness drills green, prove the **cell-as-bulkhead** under world-scale
load, ship the named multi-cell + migration follow-ons, and set the **measured** sizing numbers.

**Work — world-scale hardening:**
- **The cell bulkhead under 30× surge** (CP-D5): a fatal fault / 30× surge in one cell leaves **other cells
  unaffected**; a noisy tenant is **contained to its cell** (the F6 surge family, cross-cell impact 0).
- **The measured sizing-band numbers** (7.1 arch, `[OPEN → P6, measured]`): `tenants_max` per cell class +
  **which capacity dimension binds first** — set conservatively, tightened from load-test + per-cell telemetry
  (measure-before-shard, ADR-10).
- **Restore-verify at cell scale** re-confirmed (STOR-D2 at cell scale under world-scale load — RPO/RTO hold).

**Work — the multi-cell floor follow-on (the named M5 follow-on, §5):**
- **The cross-cell PII-free pointer bridge goes LIVE** (12.6): resolution **always cell-local** — A's gateway asks
  cell **B** to `resolve(ref, viewer, mode)` *in B*; permission-checked *in B* against B's tuples; B returns only
  the already-rendered, already-permission-filtered projection (or a tombstone). ISS cross-cell portfolio rollup
  aggregates projections; KN cross-cell collab + CHAT cross-org channels resolve membership/content in the home
  cell.
- **The deferred sub-problems built**: the cross-cell DSR fan-out **mechanism**, the per-viewer cross-cell
  resolution latency budget, **cross-cell zookie consistency** (the hardest sub-problem — a zookie minted in the
  home cell read in a member cell, named in Id's open set), and multi-cell rebalancing.

**Work — live tenant migration + repo relocation (the other named M5 build):**
- **The online cell→cell move (same region)** reusing **reindex-from-source + crypto-shred cut-over**. The
  v1 floor was *avoid migration by sizing headroom + sealing*; the M5 follow-on is the real move, **triggered by
  a measured hot cell** that sealing cannot relieve. Repo relocation (C-1) is this same mechanism at repo grain.

**Entry dependency:** M4 green (all five subsystems exist; the single-cell correctness drills green; the
`CrossCellPointer` frame + the bridge consumers in ISS/KN/CHAT exist to be lit up). **The FLOOR drills CP-D7,
CP-D8, GA-D8 are now owed** (master §5 honesty register).

**Exit gate (contributes to M5→M6 / world-scale readiness):**
- **CP-D5** — fatal fault / 30× surge in one cell → **other cells unaffected; noisy tenant contained**
  (`cross-cell impact 0`). *(SCHED.)*
- **CP-D7 (FLOOR, now owed)** — migrate a tenant cell→cell (same region) → **0 loss across-seam, lands in-region,
  source crypto-shredded** (`migration receipt; 0 loss`). *(SCHED.)*
- **CP-D8 (FLOOR, now owed)** — a cross-cell ref (multi-cell) → the bridge carries **only**
  `subject`/`type`/`correlation_id`; the target resolves per-viewer; an unauthorised viewer → **tombstone**
  (`PII-free bridge proof`). *(SCHED.)*
- **GA-D8 (cross-owner FLOOR, Tenancy's multi-cell leg)** — multi-cell erasure: the fan-out iterates all
  `member_cells ∪ home_cell`; a complete merged receipt set; **0 cells missed**. *(SCHED.)*
- **STOR-D2 at cell scale** re-confirmed (RPO ≤ 5 min / RTO ≤ 1h-tenant / 4h-cell under world-scale load).
- **Tenancy is the partition under all four E2E scenarios** (E2E-1..E2E-4) and the residency attestation in
  E2E-4 (DSAR fan-out) — those scenarios must be green for the band.

### CP-M6 — Dogfooding: Myelin self-hosts as exactly one cell (in M6)

**Band:** M6 (dogfooding). **Thesis:** the self-host path is proven by the builders running it: the Myelin
platform's own deployment **is exactly one cell of identical artifacts** (the degenerate control plane), so the
same `residency-pin` lint + the same drills run on the platform's own data.

**Work:** the Myelin team's own tenant is placed on the platform; the degenerate single-cell control plane serves
it; the self-hosting CI graph runs the `residency-pin` + `control-plane-pii-free` lints as Myelin CI jobs on every
Myelin commit. **The team's data is real tenant data** — so this band does not start over a red restore-verify or
DSAR fan-out (master §1 Tier 1 + Tier 6).

**Exit gate:** the self-host cell serves the team with `residency_verify` green; the lints green on the platform's
own commits; no later-band CP gate red (the gate invariant holds end-to-end).

---

## 3. The floor-then-full progression (name each floor + its follow-on)

The discipline (VISION §3, EI-04 §4): **name the floor and name the follow-on.** Every Tenancy floor, with its
ship-band, its follow-on, and the **trigger** that promotes it.

| Floor (shipped) | Band | The full answer (follow-on) | Band | The trigger |
|---|---|---|---|---|
| **Single-cell** (one home cell per tenant; `member_cells` single-element; resolution always same-cell) | **M1** | **Multi-cell** — the `CrossCellPointer` bridge **live**; DSR iterates `member_cells`; cross-cell zookie consistency; multi-cell rebalancing | **M5** | cross-cell rollup/collab/cross-org demand (OQ-I); the 10k-org case. FLOOR drills CP-D8 + GA-D8 owed. |
| **Avoid migration by sizing headroom + sealing** (a tenant never moves cells in v1) | **M1** | **Live tenant migration** (cell→cell, same region) via reindex-from-source + crypto-shred cut-over; repo relocation (C-1) at repo grain | **M5 / on-demand** | a **measured** hot cell that sealing cannot relieve (ADR-10, measure-before-shard). FLOOR drill CP-D7 owed. |
| **Pool isolation tier** (shared cell, logical/RLS — the long tail) | **M1** | **Bridge** (DB-per-tenant) + **Dedicated** (cell-per-tenant) tiers provisioned on demand | **M5 / on-demand** | enterprise / public-sector / high-assurance onboarding. The partition-key contract is identical across tiers — a provisioning concern, not a redesign. |
| **Scripted/manual cell provisioning** (gated by restore-verify + readiness) | **M1** | **Durable-workflow provisioning** (ADR-09, off the hot path) | **M2** | `myelin-flow` lands (M2). The *gating* is M1; only the *durability* of the procedure waits on the workflow engine. |
| **CP-lookup + client cache discovery** (PII-free, TTL-bounded, fail-static for routing) | **M1** | **GeoDNS/anycast discovery edge** (a latency optimisation over the same PII-free contract) | **post-M5 (infra)** | `[OPEN → P4 (infra)]` — a latency optimisation, not a correctness gate. |
| **`residency_verify` over the M1 stores** (OLTP/blob/index/KMS) | **M1** | **CI runner/log/artifact/cache coverage** (C-2) — the no-global-pool property attestable per-tenant | **M4** | CI lands (M4) and its stores report region. Named as a partial until then (CI-R3). |
| **Default-deny outbound-mirror gate** (extra-EU PII-bearing target refused) | **M3** | **Counsel-ratified `transfer_allowed` entries** that permit a specific extra-EU mirror | **parallel (legal)** | `[OPEN — LEGAL]` (C-4, Schrems II / Art. 44–49). The default-deny gate ships regardless; the residual is one ratified statement per target. |
| **Region = immutable; a region change is new-tenant-+-DSR** (structural posture) | **M1** | **Counsel/DPO ratification** of the legal posture (region change, slug PII screening, bridge residency proof) | **parallel (legal)** | `[OPEN — LEGAL]` — the structural floor ships regardless; the residual is a counsel sign-off, not an engineering gate. |

**The honest-floor rule binds all of these:** each floor is tracked in the gap report with its claimed/proven
status and its linked follow-on; the gap being *invisible* is the only failure (EI-04 §4). The `CrossCellPointer`
frame is deliberately **frozen in M1** so the floor's promotion (CP-D8/GA-D8) is drilled against a stable shape,
not a moving target.

---

## 4. The world-scale / hard-problem work, scheduled explicitly

The two hard problems this system owns (EI-04 §1 residency; the cell-as-bulkhead at world scale), sequenced with
what ships as a floor named:

1. **Residency as region-pinning, with no cross-region query path (EI-04 §1).** This is **not deferred** — it is
   the M1 core. The full mechanism (four-layer enforcement + immutable region + `residency-pin` lint +
   gateway-rejects-misroute) lands in M1 and is drilled (CP-D3, STOR-D5). What is *floored*: the **legal posture**
   around region-change-as-DSR and slug PII screening (`[OPEN — LEGAL]`, ships regardless of ratification); the
   **CI-store attestation coverage** (M4, named partial until CI lands); the **outbound-mirror lawful-basis
   entries** (M3 default-deny gate ships; entries ratified in parallel).

2. **The cell as the world-scale unit of bulkhead + scale (ADR-11).** The **single-cell** topology is complete and
   world-scale-proven in M1/M5 (one cell scales a bounded set of tenants; the sizing envelope is a measured
   multi-dimensional vector). The **multi-cell** build — the genuinely deferred, hardest work — is **M5**:
   - The **cross-cell PII-free pointer bridge** (frame frozen M1, live M5).
   - **Cross-cell zookie consistency** — the hardest sub-problem (a zookie minted in the home cell, read in a
     member cell), named in Id's open set and owed when multi-cell ships.
   - The **cross-cell DSR fan-out mechanism** + the **per-viewer cross-cell resolution latency budget** +
     **multi-cell rebalancing**.
   - **Live tenant migration + repo relocation** — the measured-trigger online cell→cell move.

3. **The measured numbers (Q32/Q33 defaults-to-beat, EI-02 §8).** Every world-scale number is **measured in M5/M6,
   not predicted**: the cell sizing-band `tenants_max` per class + which capacity dimension binds first; the 30×
   surge cross-cell-impact budget (CP-D5); the discovery cache TTL + fail-static window; the migration latency +
   downtime budget (CP-D7). Phase 6 proposes a conservative default; the drill measures and sets the final value.

**The scheduled SCHED drills (the expensive, world-scale legs, run nightly/weekly):** CP-D4 (CP-outage
blast-radius), CP-D5 (cell bulkhead under surge), CP-D6 (provision-safe), CP-D7 (migration FLOOR), CP-D8
(cross-cell bridge FLOOR), STOR-D5 (residency egress), CI-R3 (CI no-global-pool), GA-D8 (multi-cell erasure
FLOOR), STOR-D2 at cell scale. The **CI-cheap** legs (run every change): CP-D1 (the two lints + 0 PII columns),
CP-D2 (misroute 0), CP-D3 (residency-pin rejects + attestation passes).

---

## 5. The dependency DAG for Tenancy (upstream / downstream)

**Upstream — what must exist first (the critical inputs):**
- **M0 substrate** — the harness (`serve(AppSpec)`, the `region`/`cell_id` injection, the telemetry signal set),
  the data-map derive (`control-plane-pii-free` reads it), the CI pipeline that hosts the two lints. *Tenancy's
  M0 lints are themselves a substrate-band deliverable.*
- **Storage M1 — restore-verify (11.5, ADR-18)** — the **cell-readiness gate**: no cell goes `active` without it
  (CP-D6); and the **silent-data-loss floor (STOR-D1)** that must be green before CP-M1 places real data.
- **Storage M1 — KMS hierarchy (11.3)** — crypto-shred for tenant decommission (M1) + migration cut-over (M5).
- **GDPR M1 — `transfer_allowed` registry (10.5) + `dsr_submit` `member_cells` iteration (10.4)** — the
  mirror-gate registry (M3) + the multi-cell DSR fan-out (M5).
- **Workflow M2 — `DurableExecutor` (9.1)** — durable provisioning (M2) + the migration workflow (M5). *(Floor:
  M1 provisioning is scripted, gated by restore-verify.)*
- **NOT Identity on the hot path** — placement precedes identity capture (two-phase signup); this asymmetry is
  what keeps PII off the control plane and lets the partition land in M1.

**Downstream — what Tenancy unblocks (highest fan-out first):**
- **Every store, every service** — `(tenant, region)` (12.1) is the partition key injected by the harness; the
  `residency-pin` lint constrains every write. Nothing writes a row until this floor is green.
- **Git (M3)** — `placement_of(repo)` (the git wire routes at repo grain) + the outbound-mirror gate.
- **CI (M4)** — `residency_verify` CI-store coverage + residency-pinned runners (CI-R3).
- **ISS / KN / CHAT (M5)** — the `CrossCellPointer` bridge (rollup / collab / cross-org channels) once multi-cell
  is live.
- **GDPR DSR (M1 trivial, M5 fan-out)** — iterates `member_cells` over the bridge.
- **The acyclicity rule** (`no-cross-sync-cycle`, M0): Tenancy is a **leaf-ward routing/placement service** —
  consumers call `discover`/`placement_of`/`residency_verify`; the control plane calls *down* into Storage's
  restore-verify and GDPR's registry, never *up* into a hot-path consumer. No cross-subsystem sync cycle.

---

## 6. The gap report rows Tenancy owns (the honest register, dated)

Tracked durably so the next worker sees the real state (EI-04 §4; code-wins-over-docs, EI-01 §1). As of
2026-06-19 these are **claimed/planned** (no green artifact yet — this is a roadmap, not an implementation):

| Item | Status | Band it becomes proven | The drill that proves it |
|---|---|---|---|
| The two lints (`residency-pin`, `control-plane-pii-free`) with both fixtures | planned | M0 | CP-D1 lint legs |
| PII-free registry; 0 `is_personal=true` columns | planned | M1 | CP-D1 |
| Misroute rejected, 0 cross-tenant/cross-cell read | planned | M1 | CP-D2 |
| `residency-pin` rejects out-of-region write; attestation passes; 0 cross-region egress | planned | M1 | CP-D3 + STOR-D5 |
| CP-outage = signup-only impact (blast-radius / fail-static) | planned | M1 | CP-D4 |
| Provision-safe (restore-verify + readiness before traffic) | planned | M1 | CP-D6 |
| `placement_of(repo)` live; relocatable not node-pinned | planned | M3 | GIT residency leg |
| Outbound-mirror deny-by-default (extra-EU) | planned | M3 | C-4 mirror drill (CP-D3 family) |
| CI no-global-pool attestation coverage | planned (named partial in M1) | M4 | CI-R3 |
| Cell bulkhead under 30× surge, cross-cell impact 0 | planned | M5 | CP-D5 |
| **Multi-cell live** (`CrossCellPointer` bridge) — **FLOOR** | designed-not-built | M5 | CP-D8 + GA-D8 |
| **Live tenant migration / repo relocation** — **FLOOR** | designed-not-built | M5 | CP-D7 |
| Bridge/Dedicated isolation tiers | floor (Pool only in v1) | M5 / on-demand | (isolation-tier provisioning) |
| Sizing-band numbers (measured) | open (default-to-beat) | M5/M6 | CP-D5 + cell telemetry |
| `[OPEN — LEGAL]` residuals (region-change-as-DSR; slug PII; bridge residency proof; mirror lawful basis) | structural floor ships; residual flagged to counsel/DPO | parallel (legal) | not an engineering gate |

---

## 7. The honest progression (first runnable / first useful / production-hardened)

- **First runnable (early M1):** the **degenerate single-cell control plane** — `discover`/`place` return "this
  cell", `(tenant, region)` injected by the harness, the `residency-pin` lint green. A service boots region-pinned
  and a tenant can be placed. This is also exactly the **self-host shape** (one cell, one-row registry, same code
  path). *Not yet useful for real data — misroute enforcement + restore-verify gating are not yet drilled.*
- **First useful (late M1):** the **full PII-free registry + two-phase signup + the four-layer residency
  enforcement + `residency_verify` + CP-D1/D2/D3 green**, over a green **STOR-D1** (the silent-data-loss floor).
  Now real tenant data can be placed with **misroute structurally impossible** and **no cross-region query path** —
  a real subsystem can write rows knowing the partition + residency floor holds. Single-cell is complete; the
  topology/isolation/residency/discovery/self-host stories are **done** for the vast majority of tenants.
- **Production-hardened (M5):** the **cell bulkhead holds under 30× surge** (CP-D5, cross-cell impact 0);
  **multi-cell is live** with the PII-free bridge proven (CP-D8) and the multi-cell DSR fan-out complete (GA-D8);
  **live migration is drilled** (CP-D7); the **sizing-band numbers are measured** (not predicted); **restore-verify
  re-confirmed at cell scale** (STOR-D2). The 10k-org case now has a built-and-drilled path, not a designed-only
  one — and M6 dogfoods the whole thing as the team's own self-hosting cell.

---

## 8. Digest

**Milestones (each mapped to a master band):**
- **CP-M0 (M0)** — the two committed lints (`residency-pin`, `control-plane-pii-free`, red+green fixtures) + the
  `myelin-tenancy` partition-key crate.
- **CP-M1 (M1, the core)** — the PII-free control-plane registry + `discover`/`place`/`placement_of` + two-phase
  signup + the four-layer region-pinning + `residency_verify` + the Pool isolation tier + the frozen (not-live)
  `CrossCellPointer` frame + provision-gating + self-host parity.
- **CP-M3 (M3, with Git)** — `placement_of(repo)` goes live (repo-granular, relocatable, not node-pinned) + the
  outbound-mirror residency gate (deny-by-default).
- **CP-M4 (M4, with CI)** — `residency_verify` extended to the CI runner/log/artifact/cache stores (the
  no-global-pool property attestable per-tenant); residency-pinned runners.
- **CP-M5 (M5)** — cell bulkhead under 30× surge + the multi-cell floor follow-on (bridge live, cross-cell zookie
  consistency, DSR fan-out, rebalancing) + live tenant migration / repo relocation + measured sizing numbers +
  restore-verify at cell scale.
- **CP-M6 (M6)** — Myelin self-hosts as exactly one cell; the lints run as Myelin CI on its own commits.

**Floors + follow-ons:**
- Single-cell **(M1)** → multi-cell live **(M5)**; trigger: cross-cell demand. FLOOR drills CP-D8 + GA-D8.
- Avoid-migration-by-sizing **(M1)** → live tenant migration / repo relocation **(M5)**; trigger: a *measured* hot
  cell. FLOOR drill CP-D7.
- Pool tier **(M1)** → Bridge + Dedicated tiers **(on demand)**; trigger: enterprise/public-sector onboarding.
- Scripted provisioning **(M1)** → durable-workflow provisioning **(M2)**; trigger: `myelin-flow` lands.
- CP-lookup+cache discovery **(M1)** → GeoDNS/anycast edge **(post-M5, infra)**; a latency optimisation.
- `residency_verify` over M1 stores **(M1)** → CI-store coverage **(M4)**; trigger: CI lands.
- Default-deny mirror gate **(M3)** → counsel-ratified `transfer_allowed` entries **(parallel, legal)**.

**Critical upstream dependencies:** the **M0 substrate harness** (`region`/`cell_id` injection + the lints' CI
host + the data-map derive); **Storage's restore-verify (STOR-D1, M1)** — the silent-data-loss floor that must be
green before real tenant data is placed, and the cell-readiness gate (CP-D6); **Storage's KMS hierarchy (M1)** for
crypto-shred (decommission + migration cut-over); **GDPR's `transfer_allowed` registry (M1) + `member_cells` DSR
iteration**; **Workflow's `DurableExecutor` (M2)** for durable provisioning/migration. **Not Identity on the hot
path** — placement precedes identity capture (two-phase signup), which is the asymmetry that keeps the control
plane PII-free and lets the partition floor land in M1. The exit-gate go/no-go for the master M1→M2 boundary that
Tenancy owns: **CP-D2 (misroute 0) + CP-D3 (residency-pin rejects + attestation passes)**, both gated behind a
green **STOR-D1**.
