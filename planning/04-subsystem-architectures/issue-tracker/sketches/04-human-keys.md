# Sketch 04 — Human-readable monotonic keys at scale (TE-14)

> Exploration note. Weighs Phase-2 §11 Q6 / deep-dive §6.2: `ENG-1421` — a per-team prefix + monotonic counter
> at world scale. The tension (deep-dive §6.2): **users perceive gaps as bugs, but gaplessness is a
> distributed-contention hotspot.** Leans; commit in `00-findings.md`.

## The requirement

Every issue gets a stable, human-facing key `<PREFIX>-<N>` (`ENG-1421`), prefix per team (configurable), `N`
monotonic per prefix. The key is a **display projection** (REF-3: display keys are render-time, the canonical
ArtifactRef is `myelin://t/issue/issue/<uuid>` — event-bus §6.2 shows `ABC-123` as the *id* segment, so the key
*is* the addressable id here; we reconcile: the **UUID is internal; the human key is the public id used in the
ArtifactRef and CLI**, allocated once at create, immutable thereafter).

Two sub-questions: (1) **gapless or gap-tolerant?** (2) **what allocator** avoids the single-sequence-row
hotspot under high create QPS within a hot team?

## Gapless vs gap-tolerant

- **Gapless** (`1,2,3,…` no holes) requires a single serialised writer per prefix — every create takes a row
  lock on the counter → a hot team (a big import, an incident storm) serialises all creates → contention
  hotspot (deep-dive §6.2). And gaplessness is **not actually a requirement** — it is a *perception* ("did I
  lose issue 1420?"). GitHub/GitLab/Jira all have gaps in practice (deleted issues, failed creates).
- **Gap-tolerant** (`1,2,5,6,…` holes allowed) lets us batch-allocate ranges, removing the per-create hotspot.
  The cost is occasional visible gaps. Mitigation for the *perception*: gaps from **batched allocation** are
  rare and small (a crashed allocator loses at most one unused batch); we never reuse a number (monotonic), so
  a gap reads as "an issue that was created-then-deleted or a failed create," which users already understand.

**Lean: gap-tolerant, monotonic, never-reused.** Match the incumbents' real behaviour; don't pay the
distributed-contention tax for a perception we can manage.

## Candidate allocators

### A — Single sequence row per prefix, locked per create
`UPDATE prefix_counter SET n = n+1 WHERE prefix=? RETURNING n`. Simple, gapless.
- **Against:** the hotspot deep-dive §6.2 names. A hot team's creates serialise on one row. Rejected for the hot
  case (fine for a cold team, but we want one mechanism).

### B — Hi/Lo (batched) allocation per prefix (the classic answer)
Each issue-service worker reserves a **block** of N keys for a prefix in one `UPDATE … RETURNING` (advancing the
counter by N), then hands them out from memory without further DB contact until the block drains. (Hibernate's
HiLo; Marshall/standard allocator pattern.)
- **For:** turns N creates into 1 counter write — the contention drops by N×. A hot team scales by larger blocks.
- **For:** gap-tolerant by construction (a worker that dies with an unused block leaves a gap — exactly the
  rare, small, acceptable gap above).
- **For:** per-prefix, so prefixes are independent — no global hotspot; a busy `ENG` doesn't slow `OPS`.
- **Against:** keys within a hot prefix may be handed out slightly **out of creation order** across workers
  (worker A holds 1000–1099, worker B holds 1100–1199; B may create before A finishes its block) → key order ≠
  strict creation-time order. **Acceptable:** the key is an *identifier*, not a sort order; creation order is
  `created_at`. Users don't expect ENG-1101 to be strictly newer than ENG-1099. (Document it.)
- **Tunable:** block size adapts to a prefix's create rate (small block for cold teams → tiny gaps; large block
  for hot teams → low contention). A measured-promotion knob.

### C — Per-prefix single-writer (route all creates for a prefix to one shard/actor)
A consistent-hash routes every create for prefix P to one owner that holds the counter in memory and persists
periodically.
- **For:** gapless *and* contended-free (the owner serialises in memory, fast).
- **Against:** adds a routing/ownership layer and a failover story (owner dies → who holds P's counter? must
  recover the high-water mark durably anyway) — more moving parts than Hi/Lo for the same outcome. And it
  re-introduces per-prefix serialisation (the owner is a single writer) which Hi/Lo avoids. Over-engineered.

## Multi-cell / residency note

A prefix belongs to a team, a team to a project, a project to a tenant → a prefix lives in **one cell** (ADR-11;
no cross-region). So allocation is cell-local; there is no cross-cell counter coordination. A multi-cell tenant
(SC-2/3) has each team's prefix homed in that team's cell — keys are unique within a prefix, prefixes don't span
cells. (No cross-cell hotspot; the floor is single-cell allocation, which is the whole requirement.)

## Crash-safety / correctness

The durable high-water mark is the `prefix_counter.n` row (advanced by a block at reserve time). A worker crash
*after* reserving but *before* using a block loses that block (a gap) — never reuses a number, never
double-allocates (the reserve `UPDATE … RETURNING` is the atomic step). This is the same crash-tolerance shape
as the rest of the platform (at-least-once + idempotent; here: reserve-then-use, leak-a-block-on-crash).

## Leaning

**Candidate B — Hi/Lo batched allocation per prefix, gap-tolerant, monotonic, never-reused**, with an adaptive
block size (small for cold prefixes to minimise gaps, large for hot prefixes to minimise contention). The UUID
is the internal PK; the human key is allocated once at create and is the public id in the ArtifactRef + CLI +
UI. Gaps are documented as expected and benign. Cell-local allocation (no cross-region coordination).

## Hands forward

- The adaptive block-size policy (start small, grow on measured create rate) — architecture.
- Reconciling "the human key is the ArtifactRef id" with REF-3 "display keys are render-time" — the
  resolution: the **human key is the stable public id** (in the URN), while *short forms* (`#1421` without the
  prefix, in-context) are the render-time display projection. Confirm with Refs in architecture.
