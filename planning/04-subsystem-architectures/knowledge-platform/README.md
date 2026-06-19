# Knowledge Platform (Notion-class) — Subsystem Architecture Index

> Phase: `04-subsystem-architectures/knowledge-platform`. Subsystem #4 of five. **Phase 5-B rewrite** — the
> subsystem architecture re-written from scratch against the RECONCILED shared layer
> ([`05/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md), the FROZEN
> build-to surface; [`05/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md),
> the rationale). Canonical brief: [`VISION.md`](../../../VISION.md). Lead architect: Knowledge Platform
> subsystem. Date: 2026-06-19.
>
> This index points to the **final detailed architecture** under [`architecture/`](./architecture/). The
> **design record is PRESERVED, not rewritten**: the exploration sketches ([`sketches/`](./sketches/) 01–07 +
> [`00-findings.md`](./sketches/00-findings.md)) and the design sketches ([`design/`](./design/): IA,
> user-flows, and per-screen wireframes with empty/loading/error states). The Phase-4 design was sound and
> largely ratified; the Phase-5-B rewrite conforms it to the now-frozen contract shapes — see
> [`architecture/00-overview.md` §0](./architecture/00-overview.md) for the exact "Changes vs the Phase-4 first
> pass" list.

---

## What this subsystem is

Myelin's **Notion-class workspace**: rich-text content, block-based pages, folders, and databases
(table/board/calendar/timeline views), for an organisation to host its durable, human- and agent-authored
knowledge inside the platform. Its differentiated role is to be **not a silo** but the rich, referenceable,
agent-readable substrate the rest of the platform points at and writes into — the heaviest producer and
consumer of the reference graph (the moat, EI-02 §7). Knowledge **leads + freezes** the canonical
`myelin-content` taxonomy (X-2) that Chat and Issues consume, and **co-owns** `myelin-query` + `order_key`
(X-3) with Issues.

## The architecture documents (read in order)

| # | Document | What it covers |
|---|---|---|
| 00 | [`architecture/00-overview.md`](./architecture/00-overview.md) | **§0 the reconciliation deltas (Changes vs Phase-4)**; role & responsibilities; owns-vs-delegates; the floors named up front; the component map; the build-order law. |
| 01 | [`architecture/01-tech-and-data-model.md`](./architecture/01-tech-and-data-model.md) | The language/DB choice + written justification (Rust + Postgres + S3 + Yrs, carried forward); the full schema over the **frozen `myelin-content` taxonomy** + the **frozen `myelin-query` shapes** + the **frozen LexoRank `order_key`**; the page-tree ReBAC fragment; the typed tables; the stateful-component register. |
| 02 | [`architecture/02-internals-and-algorithms.md`](./architecture/02-internals-and-algorithms.md) | The resume-cursor transport over the **frozen firehose `subscribe/resume/scope` protocol** (KN-1, built FIRST); the CAS floor → Yrs CRDT ladder + the online migration; the flexible-DB query with the **frozen `SetExpr` push-down**; the read-time formula/rollup engine; **the one editor render path** (KN-4). |
| 03 | [`architecture/03-events-contracts-and-glue.md`](./architecture/03-events-contracts-and-glue.md) | The complete `knowledge.*` taxonomy + consumed events; **how Knowledge implements every frozen glue contract** (`ArtifactRef`+the unified `#sub` grammar+tombstone ladder; `project`/`replay`; the outbox; `check`/`list_objects`+`SetExpr`+`CaveatContext`+the ReBAC fragment+zookie; `PersonalDataHolder`+the erasure posture by reference; `ToolDef`s with frozen `requires_approval` defaults); the TE-7 mirror; the AG-7 holder. |
| 04 | [`architecture/04-views-cli-and-api.md`](./architecture/04-views-cli-and-api.md) | The views/primary screens (ref `design/`, with empty/loading/error states); the CLI surface; the API / agent-tool surface. |
| 05 | [`architecture/05-hard-problems.md`](./architecture/05-hard-problems.md) | Each hard problem resolved with **cited prior art** + named floor: CRDT-vs-OT (TE-15), block-tree storage (TE-16), flexible-DB query (TE-17), formula/rollup (TE-18), permission granularity, GDPR erasure, synced blocks, offline depth, multi-region collab, comment threading. |
| 06 | [`architecture/06-reconciliation-compliance.md`](./architecture/06-reconciliation-compliance.md) | **How Knowledge now IMPLEMENTS the frozen reconciled contracts** (the firehose protocol, `myelin-content`, `myelin-query`+`order_key`, the `SetExpr` Filter, `CaveatContext`, the `#sub` grammar, the erasure posture, `requires_approval`, the sole `humanise` surface) + the residual Phase-6 requests. |
| 07 | [`architecture/07-drills-and-open-questions.md`](./architecture/07-drills-and-open-questions.md) | The **quantified drills owed** (KD-1 reconnect-loses-zero-ops is Knowledge's headline) + open questions for Phase 6 + the gap-report seed. |

## The headline decisions (one-line each)

- **Language/DB (carried forward, no reconciliation change):** Rust services; PostgreSQL-class OLTP as the
  system of record; S3-compatible object store for media + CRDT snapshots; Yrs (Rust Yjs) as the eventual
  CRDT; Tantivy via shared Search; the editor as a Rust `myelin-content` core compiled to WASM. EU-deployable /
  self-hostable. ([01 §1](./architecture/01-tech-and-data-model.md))
- **Collaboration (KN-1):** resume-cursor durable transport built FIRST over the **frozen
  `firehose::subscribe/resume(scope=doc:<id>)` protocol** → per-block CAS floor (no merge) → Yrs CRDT,
  promoted on the first true concurrent-edit conflict (online `engine_promote` migration). KD-1 is Knowledge's.
- **Content (KN-2/KN-4):** the **frozen `myelin-content` taxonomy** (Knowledge leads); inline = markdown-subset
  string with `mention`/`artifact_ref`/`embed` as structured nodes (`U+FFFC`-anchored); one editor render path
  with `render(parse(md)) === md` as a hard gate.
- **Databases (KN-3):** JSONB property bag + derived projection over the **frozen `myelin-query` shapes**;
  formulas/rollups **read-time, never stored** (by contract); the **frozen `SetExpr` push-down** conjoined.
- **Permissions:** page-tree inherit-with-overrides ReBAC fragment; row-level via `InRelation`; field-level via
  the **frozen `CaveatContext`** off the hot path.
- **GDPR:** per-subject crypto-shred + pseudonym-map shred reach the immutable op-log (structural floor, fully
  built); the residual is the **ONE platform erasure posture (10.9)** by reference, not restated.

## Owns vs delegates (the frozen-contract handoff)

**Owns / leads / co-owns:** the `myelin-content` taxonomy (X-2, Knowledge leads + freezes) + the ADF lossy-map;
`myelin-query` + `order_key` parity (X-3, co-owns with Issues); the `knowledge.*` taxonomy + the page-tree
inherit-with-overrides ReBAC fragment; the collab resume-cursor durable transport (KN-1, built first over the
frozen firehose protocol); the `db_relation`/`page_parent` typed tables; the block tree + concurrency engine
(TE-15); the `#sub` block/heading/row/field anchor grammar (X-4, the stability obligation); read-time rollups;
the one editor render path; the AG-7 agent-trace holder. **Delegates:** identity/ACL/row-field-ABAC (Id),
events+firehose-seam (Bus), refs (Refs), search (Search), notifications+humanise (Notif), agents (Agent
Fabric), storage/KMS (Storage), DSR/erasure (GDPR/Audit), durable automations (Workflow) — all via the frozen
glue contracts, reading no other store.

## Cross-references

- The frozen surface: [`05/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md)
  + [`05/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md)
  (X-1..X-7, OQ-A..OQ-L; the Part-4 per-system punch list).
- Phase-1: [`01-research/subsystem-deep-dives/knowledge-platform.md`](../../01-research/subsystem-deep-dives/knowledge-platform.md)
- Phase-2: [`02-holistic-architecture/subsystems/knowledge-platform.md`](../../02-holistic-architecture/subsystems/knowledge-platform.md)
  + [`design-language.md`](../../02-holistic-architecture/design-language.md) §7.4/§8b
- Phase-2b: [`integration-directives.md`](../../02b-doctrine-integration/integration-directives.md) (KN-1…KN-4)
- Doctrine: [`EI-04`](../../../external-insights/04-hard-problems.md) §2,
  [`EI-05`](../../../external-insights/05-ux-and-design.md) §2.
