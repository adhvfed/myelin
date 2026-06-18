# Phase 3 — Cross-Artifact Reference Graph (`myelin-refs`)

> Phase: `03-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md).
> Doctrine (binding): [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
> **§7** ("one canonical reference graph") + §1/§8/§10, and
> [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §5.3
> (reindex-from-source as a first-class resilience primitive). Spine: **ADR-13** (the three glue
> contracts; the Reference Graph clause), **ADR-14** (PG-or-graph-index, directional), ADR-03/04/11/12.
> Directives bound: **REF-1, REF-2, REF-3, REF-4**, plus X-1…X-5, BUS-2/BUS-3, STOR-2/STOR-3/STOR-4,
> ID-3, GD-3. Decision-record: §(c) **D11/TE-7** (the hybrid), §(e) "**Postgres + recursive CTEs over a
> dedicated graph DB**".
>
> **Resolves: ADR-13's `TE-7`** — *whether issue hierarchy/relations and knowledge db-relations live as
> edge types in Refs or as subsystem-local materialised structures projected into Refs* — with the
> **HYBRID** (D11): backlinks stay event-sourced projections in Refs; lifecycle/semantic edges
> (`closes`/`blocks`/`depends`/`assigns`) are *also* mirrored to a **typed relation table owned by the
> authoritative subsystem**, and the **typed edge — not the URN string — is the source of truth**.
>
> **What this doc is.** The detailed design of `myelin-refs`: the **URN `ArtifactRef` resolution service**
> (current projection + permission check + update events); the **event-sourced backlink inverse index**
> (rebuilt from `refs.edge.created`/`refs.edge.removed`); the **TE-7 hybrid** typed-edge mirror; the
> **permission-filtered read** path via Id `list_objects`; **hot-artifact backlink scale**;
> **cross-tenant edge visibility gating** (no personal-data side-channel); **sub-artifact (`#block`/`#step`)
> refs + graceful tombstoning on erasure**; and **reindex-from-source**. Postgres + recursive CTEs is the
> default; any graph-DB deviation is justified in writing (there is none).
>
> **What it consumes (does NOT re-invent).** The `EventEnvelope` + `OutboxTx::emit(draft, cause)` + the
> `EventHandler` consumer template from [`event-bus.md`](./event-bus.md) §3/§4/§5; the
> `AuthzClient::{check, list_objects}` + `Consistency`/zookie from [`identity-and-access.md`](./identity-and-access.md)
> §8/§12; the bootstrap harness, three-surface topology, resilient client, fail-static, forward-only
> migrations, and PersonalDataHolder auto-registration from [`00-platform-substrate.md`](./00-platform-substrate.md).
> The canonical `ArtifactRef` subsystem/type **token table is the Bus doc's §6.2** — Refs is its primary
> *consumer and validator*, not a second authority.
>
> **Status convention.** *DECIDED* = committed for P4/P5; *FLOOR* = partial answer + named follow-on;
> *[OPEN → P4/P5]* = handed forward. Every property that can fail names its **drill** (§9).

---

## 0. Reading map

- **§1** — purpose, responsibilities, the one-paragraph thesis, what Refs is NOT.
- **§2** — prior art this design stands on (graph-in-RDBMS, adjacency-list + recursive CTE, Zanzibar, the log).
- **§3** — the data model: the URN, the event-sourced backlink index, the TE-7 typed-edge mirror, sub-artifact refs.
- **§4** — the algorithms: extraction → emit, projection-fan-in, the inverse index build, permission-filtered backlink read, recursive-CTE traversal, tombstoning, reindex-from-source.
- **§5** — the contracts/APIs Refs exposes and consumes (the glue). **Stable.**
- **§6** — scaling/sharding in the cell topology; hot-artifact backlink scale; cross-tenant gating.
- **§7** — failure modes + the drills owed (quantified).
- **§8** — required changes to foundational systems.
- **§9** — open questions for Phase 4.
- **§10** — cited prior art (full).

**Floors named up front** (VISION §3 / EI-04 §4): in-cell graph build is the design here; **cross-cell
backlink fan-out for multi-cell tenants is designed-not-built** (§6.5, follow-on = control-plane / SC-2/SC-3).
The **hot-artifact backlink materialisation tier** (the "viral PR" set-flattening) ships a measured-trigger
floor: read-time CTE first, a Leopard-style flattened reach index only when a hot artifact is *measured* to
exceed the read budget (§6.3). The **typed-table schema** is co-owned with Issues + Knowledge (P4) — Refs
fixes the *contract* and the *mirror protocol*, the owning subsystem fixes the rows (ISS-1).

---

## 1. Purpose, responsibilities, and the one-paragraph thesis

### 1.1 The moat, stated plainly (EI-02 §7)

> *The same graph that lets one action cascade across six subsystems with no human effort is the graph that
> lets a person jump from a failing test to the line of code to the issue to the conversation in four
> keystrokes. Build the graph once; both the machines and the humans win. This is the platform's core moat —
> an integration no bolt-on can copy.* (EI-02 §7.)

`myelin-refs` is the system that makes "everything is addressable, and everything that references anything
is traversable, **live and permission-aware**" true by construction. It is the connective tissue VISION §1
("the myelin sheath") names in concrete form.

### 1.2 Responsibilities

Refs owns, end to end:

1. **The URN library** (`myelin-refs` crate) — parse / format / **resolve** `ArtifactRef`, the *one* library
   services link; **rejects scope-less / ambiguous refs and never guesses scope** (REF-3). The `ArtifactRef`
   *type* lives in `myelin-events` (the envelope embeds it, substrate §2.1/§2.3); the *parse/format/resolve*
   behaviour and the **edge/backlink client** live here.
2. **The `ArtifactRef` resolution service** — resolve a ref to **(a) its current rendered projection** (for
   unfurls/embeds, per viewer), **(b) a permission check** (delegated to Id), **(c) update events** for cache
   invalidation (ADR-13.1). Refs **stores no artifact content**; it calls each subsystem's **projection API**.
3. **The event-sourced backlink inverse index** — "what references this?" — rebuilt **from
   `refs.edge.created`/`refs.edge.removed` events** (= `ref.created`/`ref.removed`), never from owner DBs
   (ADR-13; REF-1/REF-4). Backlinks are **permission-filtered at read time** via Id `list_objects` (REF-1).
4. **The TE-7 typed-edge mirror** — for **lifecycle/semantic edges** (`closes`/`blocks`/`depends`/`assigns`/
   `parent`/`relates`) Refs maintains an event-sourced projection *and* the authoritative subsystem maintains
   the **typed relation table that is the source of truth** (REF-1). Refs is a fast cross-subsystem *reader/
   traverser* of those edges; it is not their authority.
5. **Sub-artifact addressing** (`#block`, `#step`, `#comment`, `#b9`) — the `#sub` fragment grammar, stable IDs
   down to sub-artifact, and **graceful tombstoning** when a target (or sub-target) is erased (ADR-13.1; EI-04 §1).
6. **Cross-tenant reference visibility gating** — a public-OSS ref from another tenant resolves through a
   narrow, explicitly visibility-gated `public` path (Id's `public` userset), **never a personal-data
   side-channel** (ADR-13 §Deferred; §6.4).
7. **Reindex-from-source** — the backlink index and every Refs projection cache are reconstructible by asking
   owners to **re-emit through the live consumer path** (REF-4); Refs never reads an owner's database.

### 1.3 The one-paragraph thesis

*Refs is a thin, event-sourced projection over the platform's edge facts. An edge exists because a
`refs.edge.created` event exists (the log is authoritative — Kreps); the backlink index is a denormalised
inverse of that log; the **forward** graph for traversals is a Postgres adjacency list walked with **recursive
CTEs** (SQL:1999 `WITH RECURSIVE`) — the proven "graph-in-RDBMS for shallow graphs" pattern the doctrine
mandates (REF-2). Every backlink read is gated by Id's `list_objects` pre-filter, so the answer is
**leak-free by construction**, not by a post-filter. Lifecycle edges that drive behaviour are **also** owned
as typed rows by the subsystem that enforces them, because "issue #88 is blocked" must be a transactional
fact in the Issues database, not a parse of a URN string. Refs never invents an edge, never stores content,
never reads another store, and is fully rebuildable from the event log — which is exactly why it is recoverable,
drift-free, and erasure-correct.*

### 1.4 What Refs is NOT

- **Not the authority for lifecycle edges.** The typed relation table in Issues/Knowledge is the source of
  truth (REF-1/TE-7). Refs holds an event-sourced *projection* for cross-subsystem traversal; on disagreement,
  the typed table wins, and Refs reconciles by reindex-from-source.
- **Not a content store.** It resolves to the owning subsystem's projection API; it never duplicates titles/
  bodies/state except as a **bounded, invalidatable cache** (§4.2) that is itself a `PersonalDataHolder`.
- **Not the permission authority.** Id is. Refs *always* composes `list_objects`/`check`; it never
  re-implements an ACL (ADR-13.3).
- **Not a general-purpose graph database.** It serves the **shallow** graphs this domain produces
  (back-references, a few levels of issue/epic/sprint hierarchy, page trees, dependency chains). A dedicated
  graph DB is rejected by default (REF-2; §2.4) — it buys dual-write sync pain and fragile cross-store
  transactions, the exact hazard ADR-04 warns of.
- **Not a second emit path.** Edge creation is `OutboxTx::emit` of a `refs.edge.*` event from the *producing*
  subsystem (or from Refs' own extraction consumer, §4.1) — there is **no** standalone "write edge" API
  (BUS-2; the outbox is the only sanctioned emit path).

---

## 2. Prior art this design stands on (cited once; full list §10)

| Concern | Prior art / proven system | Where it lands |
|---|---|---|
| **Graph stored as an adjacency list in an RDBMS** | Celko, *Joe Celko's Trees and Hierarchies in SQL for Smarties* (2nd ed., 2012) — adjacency list vs nested set vs closure table trade-offs | §3.4, §4.5 |
| **Recursive graph traversal in SQL** | SQL:1999 / ISO SQL `WITH RECURSIVE` (common table expressions); PostgreSQL docs §7.8 "Recursive Queries"; cycle detection via `UNION` + `CYCLE` clause (SQL:2023) / visited-set | §4.5, §6.3 |
| **Closure table (materialised transitive reach)** for deep/hot subtrees | Karwin, *SQL Antipatterns* (2010) — "Naive Trees" → closure-table remedy; the hot-artifact follow-on | §6.3 |
| **The log as source of truth; derived stores are projections** | Kreps, *The Log* (2013); Kleppmann, *DDIA* (2017) ch. 11 (change capture, derived data) | §1.3, §4.3, §4.7 |
| **Permission-filtered set reads at scale; the leak-free pre-filter** | Zanzibar (Pang et al., USENIX ATC 2019) `check`/`expand`; the **Leopard** set-flattened index (Zanzibar §3.2.1) for hot fan-in | §4.4, §6.3 |
| **Consistency tokens (zookies) to prevent stale re-grant ("new enemy")** | Zanzibar §2.4.4 | §4.4, §4.6 |
| **Content-addressing / URN identity; immutable addressing** | Git object model; RFC 8141 (URN syntax); W3C/REST resource-addressing | §3.1 |
| **Backlinks as a first-class graph (the original "moat")** | Bush, *As We May Think* (1945, the "associative trail"); Nelson, Project Xanadu (bidirectional links); Roam/Obsidian backlink panes | §1.1, §4.4 |
| **Tombstoning / pseudonymisation for erasure over immutable structure** | Kleppmann *DDIA* ch. 5 (tombstones); EI-04 §1 (delete the identity, not the fact) | §4.6 |
| **Reindex-from-source / rebuild-from-log as the only recovery path** | EI-04 §5.3; Kreps *The Log*; the substrate consumer template (§5 of `00-platform-substrate.md`) | §4.7 |

### 2.4 Why Postgres + recursive CTEs, not a graph database (REF-2 / §(e) prior — the written why)

ADR-14 left "PG **or** graph-index" directional; REF-2 and decision-record §(e) **narrow it to PG-by-default,
a graph DB must beat this with a measured reason**. The justification, decisively:

1. **The graphs are shallow and bounded in fan-out *per query*.** A backlink query is a single-hop inverse
   lookup (`WHERE target = ?`). A hierarchy/dependency traversal (epic→story→sub-task, blocked-by chains,
   page trees) is typically **≤ 6–8 levels** and small per node. This is precisely the regime where an
   indexed adjacency list with `WITH RECURSIVE` outperforms the operational tax of a second engine (Celko;
   PostgreSQL recursive-query docs). Neo4j/property-graph engines earn their place on *deep, dense, variable*
   traversals (social graphs, fraud rings) — not here.
2. **A dedicated graph DB is a dual write.** Edges originate as **events** owned by subsystems; a graph DB
   would be a *second* derived store to keep in sync with the log, buying the exact dual-write / cross-store
   transaction fragility ADR-04 and EI-02 §8 warn against. PG-as-projection keeps **one** recovery path
   (reindex-from-source) and **one** transactional story (the outbox).
3. **The hot path is the permission filter, not the traversal.** The expensive operation at world scale is
   "of the N artifacts that reference this, which can the viewer see?" — and that is solved by **Id's
   `list_objects`** (Zanzibar/Leopard), *outside* Refs, regardless of edge store. A graph DB would not help
   the bottleneck.
4. **Operational minimalism (EI-02 §8).** "Every additional data engine is permanent operational cost."
   Refs already depends on PG (projection cache + index) and the bus; adding a graph DB is a residency-pinned,
   crypto-shred-capable, backup-verified, self-hostable *third* engine per cell for marginal traversal gain.

**The named escape hatch (not foreclosed):** the edge store sits behind an internal `EdgeStore` trait
(`upsert_edge` / `remove_edge` / `inbound(target)` / `outbound(source)` / `traverse(root, rel, depth)`), so a
PG→graph-DB swap is a binding change, not a rewrite — **promoted only when a measured query class exceeds the
CTE budget** (§6.3). Until measured, PG + recursive CTEs is the design.

---

## 3. The data model / schema

All tables: `(tenant, region)` first columns / partition prefix, RLS-enforced, **no cross-tenant query path**
(EI-02 §1; ID-3). Every store is residency-pinned, per-tenant envelope-encrypted, crypto-shred-capable, and a
`PersonalDataHolder` auto-registered by the bootstrap harness (substrate §3.4; ADR-12).

### 3.1 The URN `ArtifactRef` (REF-3; grammar consumed, not re-invented)

```
myelin://<tenant>/<subsystem>/<type>/<id>[#<sub>]
        └tenant┘ └ §6.2 of event-bus.md (canonical, singular tokens) ┘ └ sub-artifact ┘
```

The `<subsystem>`/`<type>` token table is **owned by [`event-bus.md`](./event-bus.md) §6.2** (resolves C-3);
Refs is the **validator and primary consumer**. Refs' `parse` enforces:

- **Scope is explicit and total** (REF-3 / EI-02 §7): all of `tenant`, `subsystem`, `type`, `id` are
  required. A scope-less or short-hash ref (`#42`, `@alice`, `~general`, a 7-char commit prefix) is
  **rejected, never guessed** — those are *display keys*, render-time projections only (REF-3; §4.8).
- **`<sub>` grammar** (sub-artifact, §3.5): `#` + a stable, subsystem-minted sub-id —
  `#comment-12`, `#step-3`, `#b9` (a doc block), `#L42-L88` (a code line range). The sub-id scheme **per
  subsystem is `[OPEN → P4]`** (substrate §13 Q4); Refs fixes only that it is a stable opaque token after `#`,
  and that a ref with a `#sub` resolves to the **parent's** projection augmented with the sub-anchor.
- **Residency invariant**: the `tenant` segment pins the cell/region; a ref is resolved **in the cell that
  holds the target** (ADR-11; §6.5). A cross-tenant ref is gated (§6.4), never silently dereferenced
  cross-region.

The URN is a **value type** (immutable, content of the address). It is the platform's "associative trail"
primitive (Bush 1945): the bytes that make any two artifacts linkable.

### 3.2 The edge fact (event-sourced; the inverse index source — REF-1)

Edges are **derived from events**; this table is the **materialised projection** of the edge log, not a second
source of truth. It is rebuildable by replay (§4.7).

```sql
-- The backlink inverse index. PROJECTION of refs.edge.created/removed (ADR-13; REF-1/REF-4).
CREATE TABLE edge (
  tenant        uuid        NOT NULL,
  region        text        NOT NULL,
  edge_id       text        NOT NULL,            -- deterministic id (§4.1) → idempotent rebuild
  source        text        NOT NULL,            -- ArtifactRef (the referencing artifact / sub-artifact)
  target        text        NOT NULL,            -- ArtifactRef (the referenced artifact / sub-artifact)
  target_root   text        NOT NULL,            -- target with #sub stripped (the parent) — the hot index key
  rel           edge_rel    NOT NULL,            -- 'mentions' | 'embeds' | 'links' | lifecycle rels (§3.3)
  rel_class     rel_class   NOT NULL,            -- 'reference' (mention/embed/link) | 'lifecycle' (typed-mirror)
  origin_event  text        NOT NULL,            -- the event_id that created this edge (provenance, audit)
  origin_actor  text        NOT NULL,            -- pseudonymous Principal ArtifactRef (erasure-safe; EI-04 §1)
  created_at    timestamptz NOT NULL,
  zookie        text,                            -- consistency token at edge-write time (§4.4)
  tombstoned    boolean     NOT NULL DEFAULT false, -- target erased/deleted → render placeholder (§4.6)
  PRIMARY KEY (tenant, edge_id),
  -- a (source, target, rel) is unique within a tenant: the same mention twice is one edge (idempotent)
  UNIQUE (tenant, source, target, rel)
);

-- The backlink hot path: "what references this artifact (or any of its sub-artifacts)?"
CREATE INDEX edge_inbound  ON edge (tenant, target_root) WHERE NOT tombstoned;
CREATE INDEX edge_outbound ON edge (tenant, source);
CREATE INDEX edge_by_rel   ON edge (tenant, target_root, rel) WHERE rel_class = 'lifecycle';
```

**Why `target_root` is a stored, separate column.** The dominant query is "backlinks of artifact X" where X is
a *parent* (a PR, an issue, a page) and the inbound edges may point at its **sub-artifacts** (a PR comment, a
doc block). Indexing on `target_root` (the `#sub`-stripped URN) makes "all backlinks to this artifact and its
children" a **single index range scan**, not a `LIKE 'myelin://…/88%'` (which would be a slow, unsafe prefix
match). This is the adjacency-list "index the column you actually query" discipline (Celko).

**`rel_class` is the TE-7 seam.** `reference` edges (mention/embed/link) are **Refs-authoritative projections**
of the event log — pure backlinks. `lifecycle` edges (the typed-mirror, §3.3) are projections whose **source
of truth is the subsystem's typed table**; Refs' row is a fast cross-subsystem read-replica reconciled by
reindex. The column lets the read path and the erasure path treat the two correctly.

### 3.3 The TE-7 typed-edge mirror (REF-1 / D11 — the hybrid, resolved)

**The decision (resolves TE-7).** Lifecycle/semantic edges — the ones that *drive behaviour* — are
**dual-homed**:

- **Source of truth = a typed relation table owned by the authoritative subsystem** (Issues owns
  `issue_relation`; Knowledge owns `db_relation` / `page_parent`). The typed edge is a **transactional row** in
  the system that *enforces* it: "issue #88 is `blocked_by` #91" must be a committed fact in the Issues DB,
  because Issues is what refuses to let #88 transition to Done while #91 is open (ISS-1 stateful Trigger). The
  **URN string is not the source of truth** (REF-1) — a typed `blocked_by(88, 91)` row is, with referential
  integrity, transition guards, and the relation field type (ADR-06) behind it.
- **Projection into Refs** — the same write emits `issue.relation.created` (a typed lifecycle event) which Refs
  consumes (§4.1) and projects as a `lifecycle`-class `edge` row, so the **cross-subsystem traversal**
  ("everything blocking this CI run, across Issues + Git + the failing test") is one Refs query, not a fan-out
  to five subsystems.

```sql
-- ILLUSTRATIVE — OWNED BY ISSUES (P4), shown here so the mirror contract is concrete (ISS-1).
-- This is the SOURCE OF TRUTH for lifecycle edges; Refs mirrors it via events.
CREATE TABLE issue_relation (
  tenant        uuid NOT NULL,
  region        text NOT NULL,
  relation_id   uuid NOT NULL,
  src_issue     uuid NOT NULL,                   -- internal id (Issues' own key)
  dst_ref       text NOT NULL,                   -- ArtifactRef of the other end (may be cross-subsystem)
  rel           issue_rel NOT NULL,              -- 'blocks' | 'blocked_by' | 'closes' | 'depends_on' | 'parent' | 'relates'
  created_by    uuid NOT NULL,
  created_at    timestamptz NOT NULL,
  PRIMARY KEY (tenant, relation_id),
  UNIQUE (tenant, src_issue, dst_ref, rel),
  FOREIGN KEY (tenant, src_issue) REFERENCES issue(tenant, id)   -- referential integrity Refs can't give
);
```

**The lifecycle relation set (DECIDED for the mirror contract):**
`closes` / `blocks` / `blocked_by` / `depends_on` / `parent` (epic→story, page→sub-page) / `assigns` /
`relates`. The *enumeration per subsystem* and the *typed table columns* are the P4 deliverable (Issues +
Knowledge own them, ISS-1); Refs fixes the **`rel` vocabulary**, the **`rel_class='lifecycle'` mirror
discipline**, and the **inverse pairing** (`blocks`↔`blocked_by`, `parent`↔`child`) it materialises so a
single typed event yields both directions in the index.

**Why hybrid, not one or the other** (the both-halves-wanted resolution, D11):
- *Refs-only* (URN-as-truth) fails: lifecycle behaviour needs **referential integrity + transactional
  guards + the relation field type** that only the owning subsystem's DB gives. A "blocked" state derived from
  a backlink projection can lag the log and let a blocked issue close — a correctness bug.
- *Subsystem-only* (no Refs mirror) fails: cross-subsystem traversal ("show me everything blocking this
  release across all five subsystems") would require a synchronous fan-out to every subsystem on every read —
  the head-of-line, latency, and coupling cost EI-02 §3 forbids.
- **Hybrid** gives both: the owner enforces; Refs traverses. The typed edge is truth; the Refs edge is a fast,
  rebuildable read model. On any drift, **reindex-from-source** (REF-4, §4.7) makes Refs reconverge to the
  typed table.

### 3.4 The adjacency structure walked by recursive CTEs (§4.5)

The `edge` table **is** the adjacency list. A traversal (hierarchy, dependency chain, impact analysis) is a
`WITH RECURSIVE` walk over `edge` filtered by `rel`/`rel_class`, with a **visited-set cycle guard** and a
**depth ceiling** (default 16, configurable per rel). No separate "graph structure" table exists; the index
`edge_by_rel (tenant, target_root, rel)` and `edge_outbound (tenant, source)` make both walk directions
indexed. This is the textbook adjacency-list-+-recursive-query design (Celko; PostgreSQL §7.8).

### 3.5 Sub-artifact refs (`#block` / `#step` / `#comment`) — the granularity contract (ADR-13.1)

Every subsystem **exposes stable, resolvable IDs down to sub-artifact granularity** (ADR-13 contract 1): a PR
comment (`git/pr/88#comment-12`), a doc block (`knowledge/block/PAGE-7c2#b9`), a CI step (`ci/run/4412#step-3`),
a code line range (`git/commit/<sha>#L42-L88`). Refs stores the **full sub-artifact URN** in `edge.source`/
`edge.target` (so "this chat message embeds *block b9 of page 7c2*" is exact) **and** the `#sub`-stripped
`target_root` (so backlinks roll up to the parent). Resolution of a `#sub` ref calls the parent's projection
API with the sub-anchor; if the sub-artifact no longer exists but the parent does, Refs returns a **partial /
relocated projection** (the parent + a "this block was removed" marker), not a hard 404 — graceful degradation
(§4.6). The exact `#sub` minting scheme per subsystem is `[OPEN → P4]` (substrate §13 Q4).

### 3.6 The projection cache (bounded, invalidatable, a holder)

Refs caches a **bounded** projection (current title, state, icon, viewer-independent render hint) per
`ArtifactRef`, keyed `(tenant, ref)`, with a TTL and **event-driven invalidation** (an `*.updated`/`*.erased`
event on the subject busts it; §4.2). The cache is a `PersonalDataHolder` (it may hold a title containing a
name) and is **never a source of truth** (STOR-3): on miss or erasure it re-resolves via the projection API.
This is the Redis/Valkey-class store of S-Refs (§6.6) — ephemeral, residency-pinned, crypto-shred-capable.

### 3.7 Stateful-component register (X-4)

| # | Component | Engine | Holds | Shard key | Blast radius | Crypto-shred unit |
|---|---|---|---|---|---|---|
| R1 | **`edge` projection (backlink index)** | Postgres-class | derived edge rows (the inverse index + adjacency list) | `(tenant, region)` + `target_root` hash | one tenant; **derived — rebuildable from the log** (REF-4) | per-tenant DEK (rows are pseudonymous refs, EI-04 §1) |
| R2 | **Projection cache** | Redis/Valkey-class (NEVER truth — STOR-3) | bounded current-projection snapshots per ref | `(tenant, region, ref)` | one cell; cache miss → re-resolve | ephemeral; TTL-bounded |
| R3 | **`edge_seq` / consumer dedup** | Postgres-class | the consumer template's idempotency ledger (substrate §5) | `(tenant, consumer)` | re-process is idempotent → no loss | inherits R1 |
| R4 | **Hot-reach flattened index (FLOOR)** | Postgres-class (or Leopard-style) | materialised transitive reach for *measured-hot* artifacts only (§6.3) | `(tenant, region)` + artifact | derived — rebuildable from R1 | inherits R1 |

The **typed relation tables are NOT Refs' components** — they belong to Issues/Knowledge (their register).
Everything else in Refs (the resolution gateway, the extraction consumer, the CTE traverser) is **stateless
and horizontally replaceable**, recoverable by reconnecting to the log + reindex-from-source (§4.7).

---

## 4. The algorithms

### 4.1 Edge extraction → emit (the producer side; BUS-2)

Edges are born from two producers, both emitting `refs.edge.created` via the **outbox only** (no standalone
edge-write API):

1. **Content-node extraction (reference edges).** When a subsystem persists content containing the
   platform-load-bearing inline nodes `mention(Principal)`, `artifact_ref(ArtifactRef)`, `embed(ArtifactRef)`
   (`myelin-content`, ADR-05 — these are *structured nodes*, not parsed-from-raw-text, which is why extraction
   is reliable and **not a regex over prose**), the **same transaction** that writes the content emits one
   `refs.edge.created` per structured ref node (`rel ∈ {mentions, links, embeds}`, `rel_class='reference'`).
   This is ADR-05's "`mention`/`artifact_ref` nodes are the producers of `ref.created`" made concrete. A
   message that *embeds* `block b9 of page 7c2` emits an edge `source=chat/message/M-991`,
   `target=knowledge/block/PAGE-7c2#b9`.
2. **Typed-relation writes (lifecycle edges).** When Issues/Knowledge writes a typed relation row (§3.3), the
   **same transaction** emits a typed lifecycle event (`issue.relation.created`, `knowledge.page.parent_set`,
   …). Refs' extraction consumer maps it to a `lifecycle`-class edge (both directions of the inverse pair).

**`edge_id` is deterministic** — `edge_id = hash(tenant, source, target, rel)` — so a re-delivered or
replayed event upserts the *same* row (`ON CONFLICT (tenant, source, target, rel) DO NOTHING/UPDATE`):
**idempotent rebuild** is free (the substrate consumer template's `event_id` dedup plus this deterministic key).
Removal emits `refs.edge.removed` (content node deleted, or relation row deleted) → soft-delete or
hard-delete the projection row.

**Causality.** Every `refs.edge.*` event is emitted with `OutboxTx::emit(draft, cause = Some(content_event))`
so `correlation_id` (root) carries, `causation_id` = the content/relation event, `depth = +1` (BUS-5). This is
what lets the loop guard treat **only a structured `artifact_ref` node** as a re-trigger source (AG-6's
reference gate: "only a structured picker-produced reference can re-trigger, never raw typed text") — wired to
ADR-05's "only `artifact_ref` nodes emit `ref.created`".

### 4.2 Resolution: ref → (projection, permission, update-events) (ADR-13.1)

`resolve(ref, viewer, mode) → Projection` is the live, per-viewer unfurl/embed path. The algorithm:

1. **Parse + validate** the URN (reject ambiguity; §3.1).
2. **Permission check** — `Id.check(viewer, view, ref)` (substrate §2.2). If denied → return a **tombstone
   projection** (`{ kind: "no-access", display: "a restricted <type>" }`) — *never* leak title/state. This is
   why an unfurl of a confidential issue degrades to a placeholder for a viewer lacking `issue.view`
   (identity-and-access §5 Chat clause): **Refs is the chokepoint that makes unfurls non-leaking**.
3. **Projection** — on cache hit (R2, fresh) return it; on miss call the **owning subsystem's projection API**
   (`<subsystem>.project(ref, viewer) → {title, state, icon, render_hint}`) through the **resilient client**
   (substrate §6: timeout/breaker/bulkhead), cache the result (TTL + invalidation), return it. Refs **never
   reads the owner's DB** (ADR-01) — only its projection contract.
4. **Update events** — the caller (Chat unfurl, PR context pane) subscribes to `*.updated`/`*.erased` on the
   subject so the rendered ref stays **live** (ADR-13.1c); a busted cache re-resolves on next read.

**Per-viewer correctness.** The permission check (step 2) is per-viewer; the projection cache (step 3) is
**viewer-independent content** keyed by ref, gated by the per-viewer check — so the cache is shared (one entry
per ref, not per (ref, viewer)) without leaking, because **no content is returned until the per-viewer check
passes**. (Caveated/conditional grants resolve via `check` with context, identity-and-access §9.)

### 4.3 The inverse-index build (consumer side; the substrate consumer template)

Refs runs **two consumers** built from the substrate `EventHandler` template (substrate §5; BUS-3):

- **`refs-edge-builder`** — whitelists `evt.*.*.refs.edge.>` and the typed lifecycle subjects
  (`evt.*.*.issue.relation.>`, `evt.*.*.knowledge.page.>` parent events). It is one of the **excepted
  firehose-class infra consumers** (BUS-4) that legitimately need broad event coverage — reviewed explicitly.
  On `created`: upsert the `edge` row (deterministic `edge_id`, idempotent). On `removed`: soft/hard-delete.
  On `*.erased` tombstone (§4.6): mark `tombstoned=true` for edges whose `target_root` or `target` is the
  erased subject. **Ack-after-apply**, idempotent on `event_id`, terminate-malformed (the four D7 gotchas are
  the template's, not re-implemented here).
- **`refs-projection-invalidator`** — whitelists `evt.*.*.*.updated` and `*.erased` for subjects Refs has
  cached; busts R2 cache entries. Bounded prefetch; per-tenant in-flight caps.

Because the index is **purely a function of the event log**, a cold rebuild (§4.7) and steady-state ingestion
are the **same code path** — they cannot drift (EI-04 §5.3; the reindex-from-cold parity drill, §7 D-4).

### 4.4 Permission-filtered backlink read (REF-1 — the leak-free path)

`backlinks(ref, viewer, opts) → [Edge]` is the "what references this?" query. **Leak-free by construction**
(not by post-filter), via Id's `list_objects`:

```
backlinks(target, viewer):
  1. target_root := strip_sub(target)
  2. filter := Id.list_objects(viewer, perm=view, type=<inferred or all>, at=Consistency)   -- the pre-filter
  3. SELECT source, rel, rel_class, origin_actor, tombstoned, zookie
       FROM edge
      WHERE tenant = :viewer.tenant            -- NO cross-tenant path (ID-3)
        AND target_root = :target_root
        AND NOT tombstoned
        AND  <filter applied to `source`>       -- ids-mode: source IN (reachable set)
                                                -- filter-mode: a pushed-down predicate Id compiled (§8.2 Id)
      ORDER BY created_at DESC
      LIMIT :page;                              -- always paginated (hot-artifact safety, §6.3)
```

- **The filter is Id's, applied to the *source* of each inbound edge**: you see a backlink **iff you can see
  the artifact that made the reference** (ADR-03 §Consequences; REF-1). A confidential issue that references a
  public doc does **not** reveal itself in the doc's backlinks to a viewer who can't see the issue.
- **Two return modes** mirror Id `list_objects` (identity-and-access §8.2): **ids-mode** (`source IN
  (enumerated reachable set)`) for small/bounded sets; **filter-mode** (a zookie-stamped compiled predicate
  Refs composes into the SQL `WHERE`) for large tenants. The exact push-down encoding is Id's `Filter` shape;
  Refs is a **consumer** of it (the contract is frozen in identity-and-access §12; the encoding is `[OPEN →
  P4]`).
- **Zookie carried** (identity-and-access §8.4): the backlink read passes the `Consistency`/zookie so a
  **just-revoked** grant cannot be read stale ("new enemy" problem, Zanzibar §2.4.4). Security-sensitive reads
  (a freshly-reclassified-confidential artifact) always carry a zookie → they **bypass Id's fail-static cache**
  and are evaluated at ≥ the named snapshot (identity-and-access §10).
- **N+1 is forbidden.** Refs **never** loops `check(viewer, source_i)` per inbound edge — that is the leak-prone,
  slow anti-pattern. It composes **one** `list_objects` filter. This is the "single most load-bearing
  inter-system contract" (identity-and-access §8.2).

### 4.5 Recursive-CTE traversal (hierarchy / dependency / impact)

For multi-hop questions — "the epic tree of this story", "everything `blocked_by` this, transitively", "every
artifact impacted if this page is erased" — Refs walks the adjacency list with a bounded, cycle-safe recursive
CTE:

```sql
WITH RECURSIVE walk(ref, rel, depth, path) AS (
    SELECT :root, NULL::edge_rel, 0, ARRAY[:root]
  UNION ALL
    SELECT e.target, e.rel, w.depth + 1, w.path || e.target
      FROM edge e
      JOIN walk w ON e.tenant = :tenant
                 AND e.source_root = w.ref            -- index edge_outbound
     WHERE e.rel = ANY(:rels)                          -- only the requested lifecycle rels
       AND e.rel_class = 'lifecycle'
       AND NOT e.tombstoned
       AND w.depth < :ceiling                          -- DEPTH CEILING (default 16)
       AND NOT (e.target = ANY(w.path))                -- VISITED-SET cycle guard (no infinite loop)
)
SELECT * FROM walk WHERE depth > 0;
```

- **Cycle safety** is structural: the `path` array (or SQL:2023 `CYCLE` clause) prevents revisiting a node;
  the **depth ceiling** bounds the walk even on a pathological graph (Celko; PostgreSQL §7.8). A dependency
  cycle (A blocks B blocks A) is detected and surfaced as a **diagnostic**, not an infinite loop.
- **Permission filtering on traversals**: the result set is post-filtered by **one** `list_objects` over the
  collected node set (not per-hop) — and a hop into an artifact the viewer cannot see **prunes that branch**
  (you cannot traverse *through* an artifact you can't read), preventing the traversal itself from being a
  side-channel. For the rare deep-hot traversal, the materialised reach index (R4, §6.3) replaces the live CTE.
- **Bounded cost** (X-3): every traversal is depth-capped, row-capped, and runs under a statement timeout; a
  request that would exceed the budget returns a **partial result + a "truncated" marker**, never an unbounded
  scan. The CTE is the safe-by-default path; R4 is the measured-hot escalation.

### 4.6 Tombstoning on erasure / deletion (ADR-13.1; EI-04 §1)

The graph must survive erasure **without leaking and without dangling**. Two distinct cases:

- **Artifact deleted** (a comment removed, a page deleted): the owner emits `*.deleted`; the edge-builder marks
  inbound edges `tombstoned=true`. A backlink to it renders a **"(deleted)" placeholder**; a `resolve` returns
  a tombstone projection. The **edge fact survives** (audit/causality integrity — EI-04 §1 "delete the fact?
  no — tombstone the target") but renders inert.
- **Subject erased** (GDPR Art. 17 — a person erased): the **person is pseudonymous in the edge** already
  (`origin_actor` is the opaque `principal_id`, never PII — EI-04 §1; identity-and-access §3). So erasing the
  *person* needs **no edge mutation** in the common case: the edge points at a stable opaque id, and Id's
  pseudonym-map shred (identity-and-access §11) makes the id un-resolvable to a human. Where an edge's *target*
  is itself erased content, the `*.erased` tombstone (event-bus §4.8) flips `tombstoned=true` and resolution
  degrades to a placeholder.

**Refs as a `PersonalDataHolder`** (ADR-12.1; GD-3): the bootstrap harness auto-registers `edge` (R1) + the
projection cache (R2). Refs implements:
- `locate(subject)` → edges where the subject is `origin_actor` (its references) or appears in a cached
  projection; tombstone status.
- `erase(subject)` → **purge the projection cache** entries containing the subject's PII (R2) and **rely on
  Id's pseudonym shred** for `origin_actor` (the edge keeps the opaque id; the human becomes unresolvable).
  Edges whose *target content* is erased are tombstoned via the `*.erased` consumer. Refs **never holds the
  PII itself** for the references-not-payloads case — so its erasure surface is small and structural.
- `export(subject)` → the subject's outbound references (resolved via owners), for DSR portability.

This is the EI-04 §1 split applied to the graph: **the graph's *structure* (the fact that a reference existed)
is immutable and audit-bearing; the *identity* behind it is erasable** — delete the identity, not the fact.

### 4.7 Reindex-from-source (REF-4 — first-class resilience, the only recovery path)

The `edge` index and projection cache are **fully reconstructible by asking owners to re-emit through the live
consumer path** (REF-4; EI-04 §5.3; event-bus §4.9). Refs **never reads an owner's database** for recovery.

```
reindex(scope):                                   -- scope = a tenant, a subsystem, an artifact subtree, or all
  events::reindex(scope)                          -- the bus's re-emit protocol (event-bus §5.6)
    → each owning subsystem.replay(scope, since): emits *.snapshot events (content nodes + typed relations)
      through the SAME outbox→bus path
    → refs-edge-builder ingests them idempotently (deterministic edge_id + event_id dedup)
    → the rebuilt `edge` index byte-matches the live index (the parity drill, §7 D-4)
```

- **One code path**: steady-state ingestion (§4.3) and cold rebuild are *identical*, so they cannot drift
  (the doctrine's core reindex argument).
- **The drift-correction mechanism for TE-7**: if the Refs `lifecycle` projection ever disagrees with a
  subsystem's typed table (the authority), a scoped `reindex` re-emits the typed snapshots and Refs reconverges.
  The typed table always wins (REF-1).
- **New-consumer bootstrap is the same path** (a brand-new Refs index is `reindex(since=0)`), as is the
  schema-upcaster backfill (event-bus §4.10).

### 4.8 Display keys are render-time only (REF-3 / EI-02 §7)

`#42`, `@alice`, `~general`, a 7-char commit prefix are **never stored** in an edge or resolved as a scope —
they are **derived at render time** from the canonical URN by the **alias/display projection** (event-bus
§6.2's documented alias map is the render-time projection; identity-and-access supplies `@handle`). Refs stores
the canonical URN; the UI/CLI projects the friendly key per viewer per locale. This keeps the stored graph
unambiguous and erasure-stable (an `@alice` that became `@anon-after-erasure` is a render concern, not a
re-write of stored edges).

---

## 5. Contracts / APIs exposed and consumed (the glue — STABLE)

Field names + units reconciled per X-5 against the substrate envelope (§2.10) and Id's `list_objects` shape.
`myelin-refs` (Rust, ADR-01/02) carries the types + the edge/backlink client; cross-language services consume
the same surface over the internal RPC (ADR-02).

### 5.1 Exposed (what other systems link)

| Contract | Signature (illustrative) | Consumed by | Semantics |
|---|---|---|---|
| **parse / format** | `parse(&str) → Result<ArtifactRef>` ; `format(&ArtifactRef) → String` | every service (the one URN library) | rejects ambiguity; never guesses scope (REF-3). |
| **resolve** | `resolve(ref, viewer, mode) → Projection \| Tombstone` | Chat unfurl, PR context pane, Knowledge embeds, Notif | live, per-viewer; denied → tombstone, never leak (§4.2). |
| **edges (outbound)** | `edges(ref, viewer) → [Edge]` | UI "references" pane, agents | what *this* artifact points at; permission-filtered. |
| **backlinks (inbound)** | `backlinks(ref, viewer, page) → [Edge]` | "what references this", impact view | the leak-free inverse index via `list_objects` (REF-1; §4.4). |
| **traverse** | `traverse(root, rels, depth, viewer) → [Path]` | hierarchy/dependency/impact | bounded recursive-CTE walk; cycle-safe (§4.5). |
| **reindex** | `reindex(scope)` | ops, GDPR re-erasure, new consumers | reindex-from-source (REF-4; §4.7). |
| **PersonalDataHolder** | `locate/export/erase(subject)` | DSR orchestrator | §4.6; auto-registered (GD-3). |
| **telemetry** | `backlink_read_latency`, `resolve_cache_hit_ratio`, `index_lag`, `hot_artifact_fanout`, `tombstone_count`, `reindex_parity` | Phase-5 drills (X-1) | the survival signals the drills read (§7). |

**The two emitted events (the edge contract)** — emitted by producers via the outbox, *not* a Refs write API:
`refs.edge.created` / `refs.edge.removed` (= `ref.created`/`ref.removed`), envelope `subject` = the **source**
artifact, payload `{ target, rel, rel_class }`. (Event-bus §6.3 seed taxonomy.)

### 5.2 Consumed (what Refs depends on — the inbound contracts)

| Dependency | Contract consumed | From |
|---|---|---|
| **Permission filter** | `AuthzClient::{check, list_objects}` + `Consistency`/zookie | Identity (`identity-and-access.md` §8/§12) |
| **Edge facts** | the `EventEnvelope` + `refs.edge.*` / typed-lifecycle events; the `EventHandler` consumer template | Event Bus (`event-bus.md` §3/§4/§5) |
| **Re-emit** | `events::reindex(scope)` + each owner's `replay(scope, since)` | Event Bus + every subsystem |
| **Projection content** | each subsystem's **`project(ref, viewer) → Projection`** API (the ADR-13 contract every subsystem implements) | every subsystem (P4) |
| **Typed-edge truth** | the typed relation tables (read *only* via their events, never their DB) | Issues + Knowledge (P4; REF-1/ISS-1) |
| **Substrate** | `serve(AppSpec)`, resilient client, fail-static, forward-only migrations, holder auto-registration | `00-platform-substrate.md` |

**The `project` API is a required contract on every subsystem** (ADR-13.1): `project(ref, viewer) →
{ title, state, icon, render_hint, sub_anchor? }` — current state, per viewer, **already permission-checked by
the subsystem** (defence in depth; Refs also checks). This is the contract Refs calls on a cache miss; it is
**the only way Refs ever reads about another subsystem's artifact** (no cross-DB).

---

## 6. Scaling / sharding in the cell topology (ADR-11)

### 6.1 In-cell, tenant-partitioned, async off the bus (ADR-11.5)

Refs is **cell-local**; the heavy work (edge-index build, projection resolution, traversal) is **async off the
bus**, never synchronous in a subsystem's write path (ADR-11.5; the write only emits the edge event into its
own outbox). The `edge` index is `(tenant, region)`-partitioned; **there is no cross-tenant edge and no
cross-tenant query path** (EI-02 §1; ID-3). The tenant is the blast-radius/fairness unit: per-tenant in-flight
caps on the builder consumer (substrate §5) keep one tenant's edge storm (a giant import) from starving
another.

### 6.2 Measure before you shard (EI-02 §8; ADR-10)

The first scaling moves, in order, **before** any sharding (premature sharding is its own outage):
1. **Read replica** for the hot backlink-read path (mirrors Id's ID-4 "the authn hot path is the first
   replica"): backlink reads are read-mostly and stale-tolerant for non-zookied reads, so a replica absorbs the
   read fan-out cheaply.
2. **Bounded pools + statement timeouts + pagination** (X-3): every backlink/traverse is paginated and
   time-bounded (§4.4/§4.5).
3. **The projection cache (R2)** absorbs unfurl/embed read storms (a popular doc embedded in 500 messages
   resolves from cache).

Sharding the `edge` table is deferred until a **measured** hot tenant outgrows a single PG-backed shard; the
shard key is already `(tenant, region)` + `target_root` hash, so the future shard is a re-home, not a redesign.

### 6.3 Hot-artifact backlink scale (the "viral PR" / "referenced-by-10,000" case) — FLOOR + follow-on

The adversarial case: a single artifact (the default-branch tip, a foundational design doc, a release issue)
is referenced by **tens of thousands** of others, and a backlink read must stay fast *and* permission-filtered.

- **Floor (built): read-time CTE + `list_objects` filter + pagination + replica.** `edge_inbound (tenant,
  target_root)` makes the inbound scan an index range; the `list_objects` filter prunes to the viewer's set;
  **pagination caps the per-request cost** (you never materialise 10,000 backlinks — you page them). For the
  vast majority of artifacts this is sufficient and is the design.
- **Follow-on (measured-trigger): a Leopard-style flattened reach index (R4).** When an artifact is
  **measured** to exceed the per-read budget (hot-fanout telemetry, §5.1), Refs materialises a
  **per-(viewer-class, hot-artifact) flattened inbound set** — the same incremental set-flattening Zanzibar's
  **Leopard** index (§3.2.1) uses for hot ACL fan-in, and the **closure-table** remedy Karwin names for hot
  trees. It is **derived** (rebuildable from R1), **incrementally maintained** from `refs.edge.*` events, and
  **gated by the same `list_objects` filter** (it pre-flattens the *graph* reach, not the *permission* — Id
  still owns the permission set). **Promotion trigger = measured hot-fanout exceeding the read budget** (R5
  "named promotion triggers, not vague v2"); until then, the CTE path is the design (REF-2's anti-premature
  posture).

This mirrors exactly how Id handles hot tuple fan-in (identity-and-access §6/§8.5): don't widen the base table;
add a derived, rebuildable, incrementally-maintained flattened index **only when measured**.

### 6.4 Cross-tenant edge visibility gating (no personal-data side-channel — ADR-13 §Deferred)

A **public OSS repo** in tenant A referenced from tenant B is the cross-tenant case. The rules:

- **No cross-tenant edge row, ever** (EI-02 §1). An edge lives in exactly one tenant's `edge` table.
  A reference *from* B *to* A's public artifact is an edge in **B's** tenant (`source` in B, `target` a
  `public`-scoped URN in A).
- **Resolution of a cross-tenant `target` goes through Id's narrow `public` userset** (identity-and-access §6):
  Refs calls `Id.check(viewer, view, target)` which succeeds **only** via the explicit `public` relation —
  **never a cross-tenant tuple read/join**. If the target is not `public`, resolution returns a tombstone.
- **Backlinks do NOT cross tenants by default.** A's public repo does **not** expose "tenant B references you"
  to A's viewers, because that would leak *B's* existence/activity — a personal-data and competitive
  side-channel. Cross-tenant **inbound** visibility is an **explicit, opt-in, product/legal-gated** capability
  (`[OPEN → P4]`, ties to identity-and-access §15 and `gdpr-eu-sovereignty.md §3.1`). The **mechanism** (a
  `public` userset, never a cross-tenant join) is **DECIDED here**; the **policy** (when an inbound public ref
  is shown) is the P4/legal call.
- **Residency-preserving**: a cross-tenant resolve happens **in the cell that holds the target** (ADR-11), and
  carries **no PII** across cells — only the `subject`/`type`/`correlation_id` pointer (event-bus §7.4
  cross-cell bridge). This forecloses the "public ref becomes a PII exfiltration path" failure
  (`gdpr-eu-sovereignty.md §3.1`).

### 6.5 Cross-cell backlink fan-out for multi-cell tenants (FLOOR — designed-not-built)

A 10,000-person org spanning cells (SC-2/SC-3) can have an artifact in cell-1 referenced from cell-2.
**Single-cell graph build is the design here; cross-cell fan-out is a named floor.** The seam: the **control
plane** (personal-data-free, ADR-11.4) carries a **minimal pointer-edge bridge** (`source`/`target`/`rel`/
`correlation_id` — **never payload or PII**, residency-preserving); each cell resolves its end of a cross-cell
edge **locally per viewer** via the projection API. Follow-on owner: **P4 control-plane + multi-cell tenancy
resolution (SC-2/SC-3)**, co-designed with the bus's identical cross-cell pointer bridge (event-bus §7.4). The
contracts (§5) are **cell-agnostic**, so this extends without a rewrite.

### 6.6 Stateful-component register (X-4) — see §3.7

`edge` index (R1) + dedup ledger (R3) are the systems of record for the *projection* (rebuildable from the
log); the projection cache (R2) and hot-reach index (R4) are derived/ephemeral. The **typed relation tables
(the real source of truth) belong to Issues/Knowledge**. Everything else in Refs is stateless and replaceable.

---

## 7. Failure modes + the drills owed (PROVE-IT; Phase 5 executes, this is the obligation register)

Per T-2/T-5: each property that can fail names the **quantified drill** that proves it (a green artifact ⇒
"proven"; until then "claimed").

| # | Property / failure mode | Drill (quantified gate) | Reads (telemetry §5.1) | Directive/ADR |
|---|---|---|---|---|
| D-1 | **Backlink leak** (a viewer sees a reference from an artifact they can't read) | **zero-escape leak drill**: a confidential issue / overridden page / private channel that references a public artifact must **not** appear in that artifact's backlinks/traverse for an unauthorized viewer — incl. under zookie staleness and in filter-mode. Gate: **0 unauthorized backlinks**. | `backlink_read_latency`, filter-mode hit | REF-1, ADR-03, identity-and-access D4 |
| D-2 | **Cross-tenant edge / IDOR** | **cross-tenant IDOR**: attempt to read edges across tenants via path-tenant spoofing or a crafted cross-tenant URN; assert **zero cross-tenant edge readable**, `tenant`-predicate enforced. Gate: **0 cross-tenant read**. | — | EI-02 §1, ID-3 |
| D-3 | **Hot-artifact backlink falls over** | drive a "referenced-by-50,000" artifact under concurrent permission-filtered reads; assert **paginated reads stay within the latency budget**, the hot-fanout telemetry fires, and (post-promotion) R4 serves it. Gate: **p99 within budget at target fan-in**. | `hot_artifact_fanout`, latency | §6.3, X-3 |
| D-4 | **Index drift / unrecoverable graph** | **reindex-from-cold parity**: wipe the `edge` index; `reindex(scope)`; assert the rebuilt index **byte-matches** the live index, and a **TE-7 drift** (Refs lifecycle projection ≠ typed table) **reconverges** to the typed table. Gate: **cold == live; typed-table wins**. | `reindex_parity`, `index_lag` | REF-4, EI-04 §5.3, T-5 |
| D-5 | **Erasure leaves a dangling/leaking edge** | **erasure-reaches-the-graph**: erase a subject and erase a referenced artifact; assert references become tombstones (no dangling resolve), the person is unresolvable (pseudonym shred), and **no PII recoverable** in `edge`/cache/backups. Gate: **0 recoverable PII; tombstones present; no 500 on resolve**. | `tombstone_count` | ADR-12, EI-04 §1, GD-3 |
| D-6 | **Stale re-grant via backlinks** ("new enemy") | **zookie consistency drill**: revoke a viewer's access to an artifact, immediately re-read backlinks with the post-revoke zookie; assert the now-hidden reference is **not** returned stale (bypasses Id fail-static). Gate: **no stale allow**. | — | identity-and-access §8.4 / D7 |
| D-7 | **Edge loss / no-ghost** (dual-write) | crash a producer between the content/relation commit and the relay publish; assert the edge event is still delivered (outbox) and **never** an edge without its content. Gate: **0 ghost, 0 lost edge**. | `index_lag` | BUS-2, ADR-04.3 |
| D-8 | **Traversal cycle / unbounded walk** | construct a dependency cycle (A blocks B blocks A) and a 1000-deep chain; assert the CTE **terminates** (visited-set + depth ceiling), surfaces the cycle as a diagnostic, and respects the statement timeout. Gate: **bounded, no hang; cycle reported**. | traverse latency | §4.5, X-3 |
| D-9 | **Sub-artifact tombstone** | delete a doc block / PR comment that others embed; assert embeds degrade to a **partial/relocated projection** (parent + "removed" marker), not a hard 404. Gate: **graceful degrade, 0 dangling embed**. | `tombstone_count` | ADR-13.1, §3.5/§4.6 |
| D-10 | **30× agent-surge on the graph** | 30× agent reference-creation + backlink-read surge on one tenant; assert the human read lane holds (protected lane), the agent lane sheds (429+Retry-After honoured), **other tenants unaffected**. Gate: **human-lane latency within budget; cross-tenant unaffected**. | per-tenant in-flight, shed counters | ADR-16, T-5 |

Each drill asserts against the §5.1 telemetry (observability is part of the pass condition — T-1). The
substrate exposes the failure-injection seams (scoped, reversible dependency break — T-3).

---

## 8. Required changes to foundational systems

The design slots into the foundational contracts as written; the **changes required are small and explicit**:

1. **Identity (`identity-and-access.md`) — `list_objects` filter-mode applied to an *edge's source*.** Refs'
   leak-free backlink read (§4.4) needs to apply Id's compiled `Filter` to the **`source` column of inbound
   edges**, not only to a top-level object list. Id's `Filter{set_expr, zookie}` shape already supports this
   (it is a pushed-down predicate over a candidate id set), so this is a **usage confirmation, not a new
   contract** — but it must be **named in the X-5 reconciliation**: the `Filter` must be composable into a
   `WHERE source IN (…)` / pushed-down predicate by a *consumer* (Refs), exactly as Search composes it. *Action:
   Id confirms the `Filter` is consumer-composable over an arbitrary id column (it is); no signature change.*

2. **Event Bus (`event-bus.md`) — `refs.edge.*` and typed-lifecycle subjects are firehose-class for the
   edge-builder.** The `refs-edge-builder` is one of the **excepted infra consumers** (BUS-4) that subscribes
   broadly (`evt.*.*.refs.edge.>` + `evt.*.*.issue.relation.>` + `evt.*.*.knowledge.page.>`). This is already
   sanctioned (the bus names "infra indexers/refs-builders" as the firehose exception); *Action: the bus's
   reviewed-firehose-consumer list must explicitly include `refs-edge-builder` (it is named in event-bus §4.2/
   §5.3 already — confirm).* No new transport.

3. **Every subsystem (P4) — the `project(ref, viewer)` projection API and `replay(scope, since)`.** ADR-13
   already mandates the projection API on every subsystem; this doc **fixes its shape** (§5.2:
   `{title, state, icon, render_hint, sub_anchor?}`, per-viewer, pre-permission-checked) and requires
   `replay` for reindex-from-source. *Action: Issues + Knowledge + Git + CI + Chat implement `project` and
   `replay` (P4 deliverable); the contract is frozen here.*

4. **Issues + Knowledge (P4) — own the TE-7 typed relation tables and emit typed lifecycle events** (REF-1/
   ISS-1). The typed table is the source of truth; the typed event (`issue.relation.created`, …) is the mirror
   feed. *Action: Issues owns `issue_relation` (ISS-1, ADR-06 relation field type); Knowledge owns
   `db_relation`/`page_parent`. Schema is theirs; the `rel` vocabulary + mirror discipline + inverse pairing are
   fixed here (§3.3).*

No change is required to the substrate, the envelope, the outbox, the consumer template, fail-static, or the
migration discipline — Refs is a **conforming consumer** of all of them.

---

## 9. Open questions for Phase 4 / Phase 5

- **[OPEN → P4 Issues/Knowledge]** The **full typed-relation schemas + per-subsystem `rel` enumerations** (the
  TE-7 tables). Refs fixes the `rel` vocabulary, `rel_class` discipline, and inverse pairing; the owning
  subsystems fix the rows, the transition guards, and the relation field type (ADR-06; ISS-1).
- **[OPEN → P4]** The **`#sub` minting scheme per subsystem** (a doc block id, a PR comment id, a CI step id, a
  code line range) — must be **stable across edits** (a block that moves keeps its id) so embeds don't dangle.
  Refs fixes the grammar (`#<opaque>`); the stability guarantee is each subsystem's (substrate §13 Q4).
- **[OPEN → P4 Id]** The **exact `list_objects` filter push-down encoding** for the backlink read (filter-mode):
  enumerated-ids vs a compiled predicate Refs composes into SQL. The *contract* (zookie-stamped `Filter`) is
  frozen (identity-and-access §12); the encoding is Search's + Refs' joint call.
- **[OPEN → P4/control-plane]** **Cross-cell backlink fan-out** for multi-cell tenants (§6.5) — the deepest
  unknown, co-owned with the bus's cross-cell pointer bridge (event-bus §7.4) and SC-2/SC-3.
- **[OPEN → P4/Legal]** **Cross-tenant *inbound* reference visibility policy** (§6.4) — *when* a public-OSS
  inbound ref is shown to the target tenant (the mechanism — `public` userset, no cross-tenant join — is
  DECIDED; the policy is product/legal, ties to identity-and-access §15 and `gdpr-eu-sovereignty.md §3.1`).
- **[OPEN → P5]** All **drill thresholds** (the hot-fanout read budget that triggers R4 promotion; the traverse
  depth ceiling; the surge multiplier; the index-lag alarm). This doc proposes depth ceiling = 16 and the
  measured-read-budget promotion trigger as defaults-to-beat.
- **[FLOOR → P4/P6]** The **Leopard-style hot-reach index (R4)** is specified-not-built; promotion trigger =
  measured hot-fanout exceeding the read budget (R5). Tracked in the gap report (E-3).

---

## 10. Cited prior art (full)

- **Graph-in-RDBMS / adjacency list / recursive CTE.** Joe Celko, *Joe Celko's Trees and Hierarchies in SQL
  for Smarties*, 2nd ed. (Morgan Kaufmann, 2012) — adjacency list vs nested set vs closure table; the
  shallow-graph regime. Bill Karwin, *SQL Antipatterns* (Pragmatic Bookshelf, 2010), ch. "Naive Trees" — the
  closure-table remedy for hot/deep trees (the §6.3 follow-on). ISO/IEC SQL:1999 `WITH RECURSIVE` (common table
  expressions); SQL:2023 `CYCLE` clause for cycle detection. PostgreSQL Documentation §7.8 "Recursive Queries"
  (the canonical `WITH RECURSIVE` + visited-set pattern).
- **The log as source of truth; derived stores as projections; reindex-from-source.** Jay Kreps, *The Log:
  What every software engineer should know about real-time data's unifying abstraction* (2013) — the
  log-as-truth thesis behind event-sourced edges. Martin Kleppmann, *Designing Data-Intensive Applications*
  (O'Reilly, 2017), ch. 11 (derived data, change capture) + ch. 5 (tombstones). EI-04 §5.3 — reindex-from-source
  as a first-class resilience primitive.
- **Permission-filtered set reads; hot fan-in; consistency tokens.** Pang et al., *Zanzibar: Google's
  Consistent, Global Authorization System*, USENIX ATC 2019 — `check`/`expand`, the **Leopard** set-flattened
  index (§3.2.1) for hot fan-in (the §6.3 R4 model), and **zookies** (§2.4.4) for the "new enemy" / stale
  re-grant problem (§4.4/§4.6, D-6). SpiceDB / OpenFGA as the EU-self-hostable implementations Refs consumes via
  Id.
- **Addressing / content-addressing / the associative trail.** Git object model (content addressing); RFC 8141
  (URN syntax) for the `myelin://…` scheme discipline. Vannevar Bush, *As We May Think* (The Atlantic, 1945) —
  the "associative trail"; Ted Nelson / Project Xanadu — bidirectional links (the original backlink moat);
  Roam/Obsidian backlink panes as the modern UX precedent for the "what references this" surface.
- **Doctrine.** EI-02 §7 (one canonical reference graph; backlinks are event-sourced projections; lifecycle
  edges mirrored to a typed table owned by the authoritative service — the source-of-truth rule), §1
  (tenant-first / no cross-tenant path), §8 (PG + recursive CTEs over a graph DB; measure before sharding),
  §10 (blast-radius / fail-static). EI-04 §1 (erasure vs immutability — delete the identity, not the fact),
  §5.3 (reindex-from-source). Decision-record §(c) D11/TE-7 (the hybrid), §(e) (PG + CTEs prior); directives
  REF-1…REF-4.

---

## 11. Cross-references

- [`VISION.md`](../../VISION.md) — §1 the reference graph as the connective tissue / core moat; world-scale;
  GDPR-by-construction; agent-native.
- [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) §7 (the
  binding doctrine for this system), §1/§8/§10.
- [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1 (erasure vs
  immutability), §5.3 (reindex-from-source; "search and the reference graph are easy to under-budget").
- [`00-platform-substrate.md`](./00-platform-substrate.md) — envelope, outbox, consumer template, resilient
  client, fail-static, forward-only migrations, holder auto-registration (all consumed, not re-invented).
- [`identity-and-access.md`](./identity-and-access.md) — `check`/`list_objects`/zookie (§8/§12); the `public`
  userset (§6); pseudonym shred (§11).
- [`event-bus.md`](./event-bus.md) — the envelope (§3.1), the outbox + relay (§4.1), the consumer template
  (§4.2), the `ArtifactRef` token table (§6.2), reindex-from-source (§4.9), cross-cell pointer bridge (§7.4).
- Spine: ADR-13 (the three glue contracts; the Reference Graph clause; TE-7), ADR-14 (PG-or-graph-index,
  directional → narrowed to PG by REF-2), ADR-03 (`list_objects`), ADR-04 (events authoritative), ADR-05
  (`mention`/`artifact_ref`/`embed` nodes produce edges), ADR-06 (relation field type → typed tables), ADR-11
  (cells), ADR-12 (PersonalDataHolder).
- Directives: REF-1 (event-sourced backlinks + typed-edge mirror), REF-2 (PG + CTEs), REF-3 (URNs not display
  keys), REF-4 (reindex-from-source); ISS-1 (Issues owns the typed tables); X-1…X-5; ID-3; GD-3.
- **Seeds Phase 4:** Issues + Knowledge own the typed relation tables (§8.4); every subsystem implements
  `project` + `replay` (§5.2); the `#sub` scheme + cross-tenant policy + cross-cell fan-out are the §9 open
  items.
```
