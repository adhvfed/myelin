# Knowledge Platform (Notion-class) — Subsystem Architecture Index

> Phase: `04-subsystem-architectures/knowledge-platform`. Subsystem #4 of five. Canonical brief:
> [`VISION.md`](../../../VISION.md). Lead architect: Knowledge Platform subsystem. Date: 2026-06-19.
>
> This index points to the **final detailed architecture** under [`architecture/`](./architecture/). It
> builds directly on the Phase-1 deep-dive, the Phase-2 high-level architecture, the Phase-2b doctrine
> directives (KN-1…KN-4), the Phase-3 contracts, **and the Stage-1 outputs** — the exploration sketches
> ([`sketches/`](./sketches/) 01–07 + [`00-findings.md`](./sketches/00-findings.md), the committed direction)
> and the design sketches ([`design/`](./design/): IA, user-flows, and per-screen wireframes with
> empty/loading/error states). The nine open questions `00-findings.md` §6 handed to Stage-2 are committed in
> [`architecture/08-committed-resolutions.md`](./architecture/08-committed-resolutions.md). See each
> architecture doc's header for the per-doc citations.

---

## What this subsystem is

Myelin's **Notion-class workspace**: rich-text content, block-based pages, folders, and databases
(table/board/calendar/timeline views), for an organisation to host its durable, human- and agent-authored
knowledge inside the platform. Its differentiated role is to be **not a silo** but the rich, referenceable,
agent-readable substrate the rest of the platform points at and writes into — the heaviest producer and
consumer of the reference graph (the moat, EI-02 §7).

## The architecture documents (read in order)

| # | Document | What it covers |
|---|---|---|
| 00 | [`architecture/00-overview.md`](./architecture/00-overview.md) | Role & responsibilities; owns-vs-delegates; the floors named up front; the component map; the build-order law (transport → editor primitives → CAS floor → DB → CRDT). |
| 01 | [`architecture/01-tech-and-data-model.md`](./architecture/01-tech-and-data-model.md) | **The language/DB choice + written justification** (Rust + Postgres + S3 + Yrs); the full schema (block tree, op-log + snapshots, flexible DB, page-tree ACL projection, typed `db_relation`/`page_parent` tables); the X-4 stateful-component register. |
| 02 | [`architecture/02-internals-and-algorithms.md`](./architecture/02-internals-and-algorithms.md) | The resume-cursor durable transport (KN-1, built FIRST); the CAS floor → Yrs CRDT ladder; the flexible-DB query model; the read-time formula/rollup engine; event coalescing; **the one editor render path** (KN-4). |
| 03 | [`architecture/03-events-contracts-and-glue.md`](./architecture/03-events-contracts-and-glue.md) | The complete `knowledge.*` event taxonomy + consumed events; **every glue contract** (`ArtifactRef`/`project`/`replay`; the outbox; Id `check`/`list_objects` + the ReBAC namespace fragment; `PersonalDataHolder` incl. the restriction flag; `ToolDef`s); the TE-7 typed-edge mirror; the AG-7 agent-trace holder. |
| 04 | [`architecture/04-views-cli-and-api.md`](./architecture/04-views-cli-and-api.md) | The views/primary screens (with empty/loading/error states); the CLI surface; the API / agent-tool surface. |
| 05 | [`architecture/05-hard-problems.md`](./architecture/05-hard-problems.md) | Each hard problem resolved with **cited prior art** + named floor: CRDT-vs-OT (TE-15), block-tree storage (TE-16), flexible-DB query (TE-17), formula/rollup (TE-18), permission granularity, GDPR erasure, synced blocks, offline depth, multi-region collab. |
| 06 | [`architecture/06-shared-system-change-requests.md`](./architecture/06-shared-system-change-requests.md) | The itemized **required shared-system changes** for Phase-5 reconciliation (Bus/Refs/Id/Storage/GDPR/Search/Agents/Workflow/Notif/substrate). |
| 07 | [`architecture/07-drills-and-open-questions.md`](./architecture/07-drills-and-open-questions.md) | The **quantified drills owed** (KD-1 reconnect-loses-zero-ops is Knowledge's headline) + open questions for Phase 5 + the gap-report seed. |
| 08 | [`architecture/08-committed-resolutions.md`](./architecture/08-committed-resolutions.md) | The **nine Stage-1 open questions, now committed** (CR-A…CR-I): fractional-index rebalancing, snapshot cadence/format, inline-node placeholder (`U+FFFC`), row-vs-field permission mechanism, `list_objects` push-down, CAS→CRDT online migration, comments component, cross-cell bridge, per-subject DEK granularity. |

## The headline decisions (one-line each)

- **Language/DB:** Rust services; PostgreSQL-class OLTP as the system of record; S3-compatible object store
  for media + CRDT snapshots; Yrs (Rust Yjs) as the eventual CRDT; Tantivy via shared Search; the editor as a
  Rust `myelin-content` core compiled to WASM. No Rust divergence requested. EU-deployable / self-hostable
  confirmed. ([01 §1](./architecture/01-tech-and-data-model.md))
- **Collaboration (KN-1):** resume-cursor durable transport built FIRST → per-block CAS floor (no merge) →
  Yrs CRDT, promoted on the first true concurrent-edit conflict. The reconnect-loses-zero-ops drill is
  Knowledge's. ([02 §2-3](./architecture/02-internals-and-algorithms.md), [05 §1](./architecture/05-hard-problems.md))
- **Content (KN-2/KN-4):** block tree = adjacency list + fractional ordering key; inline = markdown-subset
  string with `mention`/`artifact_ref`/`embed` as structured nodes; one editor render path with
  `render(parse(md)) === md` as a hard gate.
- **Databases (KN-3):** JSONB property bag + derived projection; formulas/rollups **read-time, never stored**.
- **Permissions:** page-tree inheritance with overrides compiled to ReBAC tuples; row-level + field-level
  (ABAC caveat off the hot path).
- **GDPR:** per-subject crypto-shred reaches the immutable op-log; structured PII reliable, free-text =
  tooling + documented residual (GD-6, co-owned with Legal).

## Owns vs delegates (the Phase-3 handoff)

**Owns:** the `knowledge.*` taxonomy + the page-tree-inheritance-with-overrides ReBAC namespace; the collab
op-stream resume-cursor durable transport (KN-1, built first); the `db_relation`/`page_parent` typed tables;
the block tree + concurrency engine (CRDT-vs-OT, TE-15); leads the shared content/block-model taxonomy
(ADR-05; Chat/Issues consume); co-owns the field-definition/view primitive (ADR-06, with Issues); accepts a
content-addressed agent-trace write (AG-7). **Delegates:** identity/ACL (Id), events (Bus), refs (Refs),
search (Search), notifications (Notif), agents (Agent Fabric), storage/KMS (Storage), DSR/erasure
(GDPR/Audit), durable automations (Workflow) — all via the glue contracts, reading no other store.

## Stage-1 outputs (the committed direction this architecture builds on)

- Exploration sketches: [`sketches/00-findings.md`](./sketches/00-findings.md) (the committed direction +
  the build order + the nine handed-forward questions) and sketches 01 (collab/transport), 02 (block
  tree/content), 03 (db/formula), 04 (permissions), 05 (transclusion/embed liveness), 06 (GDPR/agent
  trace), 07 (taxonomy/search/refs/multi-region).
- Design sketches: [`design/information-architecture.md`](./design/information-architecture.md),
  [`design/user-flows.md`](./design/user-flows.md), [`design/wireframes.md`](./design/wireframes.md)
  (S1–S12, each with empty/loading/error + permission-denied/erased states, §8b primitives applied).

## Cross-references

- Phase-1: [`01-research/subsystem-deep-dives/knowledge-platform.md`](../../01-research/subsystem-deep-dives/knowledge-platform.md)
- Phase-2: [`02-holistic-architecture/subsystems/knowledge-platform.md`](../../02-holistic-architecture/subsystems/knowledge-platform.md)
  + [`design-language.md`](../../02-holistic-architecture/design-language.md) §7.4/§8b
- Phase-2b: [`integration-directives.md`](../../02b-doctrine-integration/integration-directives.md) (KN-1…KN-4)
  + [`decision-record.md`](../../02b-doctrine-integration/decision-record.md) §(c) D10/D11
- Phase-3: [`contract-index.md`](../../03-shared-systems-architecture/contract-index.md) +
  [`README.md`](../../03-shared-systems-architecture/README.md) §5 (the handoff) + the foundational docs.
- Doctrine: [`EI-04`](../../../external-insights/04-hard-problems.md) §2,
  [`EI-05`](../../../external-insights/05-ux-and-design.md) §2.
