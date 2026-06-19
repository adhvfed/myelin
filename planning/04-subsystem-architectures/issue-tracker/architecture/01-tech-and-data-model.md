# Issue Tracker — 01 · Technology & Data Model

> See [`00-overview.md`](./00-overview.md) for the framing and the Phase-4→Phase-5 delta table. This doc commits
> the language/DB choice (**carried forward from Phase-4 + confirmed** — reconciliation forced no change) and the
> complete data model: the typed-core + JSONB issue spine, the governance schemes, the `issue_relation` typed
> table, the change-log, cycles/milestones, the rollup aggregate, SLA instances, triggers, key allocation, and
> the import map. Schemas are illustrative Postgres/Rust; the **shape** is the contract. The flexible-field
> storage is *deliberately byte-aligned with Knowledge's `db_row`* — we co-own `myelin-query` (ADR-06) and the
> four shared shapes are now **frozen byte-identical** (contract 13.3).

---

## 1. The language / tools / database choice (carried forward + confirmed)

**Decision (unchanged from Phase-4; reconciliation forced no change — VISION §4, ADR-02): Rust for all
Issue-Tracker services; PostgreSQL-class OLTP as the system of record (one DB per service, the `no-cross-db`
boundary); the shared OLAP read store (ClickHouse-class, Storage §3.4) for analytics via CQRS off the bus; the
shared Search (Tantivy) as the cold/ad-hoc/full-text escalation valve; `BlobStore` (S3-compatible) for
attachments; the frontend a TypeScript/React shell consuming the shared design system and the shared
`myelin-content` Rust core compiled to WASM for the one editor render path.** No divergence from the Rust default
is requested; the justification is positive, not merely default-acceptance.

### 1.1 Service language — Rust (ADR-02 default; positively justified)

| Concern | Choice | Written justification |
|---|---|---|
| Service language | **Rust** | The four load-bearing internal engines are *exactly* Rust's strengths: (1) a **transactional OLTP write path** (validate → check → mutate → outbox-emit in one tx) wants zero-GC predictable latency and strong types; (2) the **workflow state-machine interpreter** + the **scheme algebra** want sum types / exhaustive matching; (3) the **AST→store query compiler** (now also lowering the frozen `SetExpr`, contract 4.3) is compiler-shaped work; (4) **bus consumers** (rollup, SLA, trigger, feeder) are tight idempotent loops. The substrate glue crates (`serve(AppSpec)`, the consumer template, `myelin-client`, `OutboxTx`) are Rust — staying Rust makes every glue contract a *linked type*, not a wire shim (no client/server drift). |
| The shared content + query core | **`myelin-content` + `myelin-query` Rust crates** | Both are frozen shared Rust crates (contracts 13.1/13.3); Issues links them directly — the field-type enum, the `ViewSpec`, the `QueryAst`, and the `order_key` codec are *the same bytes* Knowledge uses, not a re-implementation. The WASM compile target gives the one editor render path client-side (`render(parse(md)) === md`). |
| The one specialised future piece | **Yrs (Rust Yjs)** *iff* the ranking move-CRDT follow-on is promoted | Our single "specialised" candidate (a move-CRDT for concurrent drag-reorder, sketch 06) is **Rust-native** and is the *same* library Knowledge lands for block order — reinforcing the default. v1 ships the CAS floor (no CRDT); the CRDT is a named, measured follow-on ([05 §5](./05-hard-problems.md)). |
| EU-deployable / self-hostable | **Confirmed** | Every component is self-hostable with no US-controlled SaaS dependency: PostgreSQL, an S3-compatible store (MinIO/Ceph/Garage), NATS JetStream (the in-cell bus + firehose), Tantivy (embedded, no JVM), ClickHouse (the shared OLAP store), Yrs (a library). The cell is "one set of artifacts" (ADR-11); self-host = one cell. |
| Glue-contract implementability across any language boundary | **Confirmed (no boundary introduced)** | Because Issues stays in Rust, `project(ref,viewer)`, `replay(scope,since)`, the `OutboxTx::emit` path, the `ToolDef` registrations, the `PersonalDataHolder` impl, and the ReBAC fragment are all linked Rust types. Were a future divergence to occur (none is requested), each contract is also a wire shape with names+units reconciled per the names/units anchor, so the contracts remain implementable across the frozen cross-language harness shim (contract 1.7). |

### 1.2 Database — PostgreSQL-class OLTP as the system of record (ADR-10/ADR-14)

**Decision (carried forward): one PostgreSQL-class database per Issue-Tracker service**, holding the `issue`
spine, the typed relation table, the change-log, the scheme config, cycles/milestones, the rollup aggregate, SLA
instances, triggers, the prefix counters, the import map, the consumer dedup ledgers, and the per-service
`outbox` (the cross-seam anchor).

**Why Postgres, not a document store or per-tenant DDL** (the *written why* TE-17 demands, mirroring Knowledge's
identical resolution):

- **The flexible-field problem is solved by JSONB + a derived projection — not per-tenant DDL.** A real SQL
  column per custom field means **DDL-per-tenant at world scale** (sparse, migration-heavy, multi-tenancy-hostile).
  EAV (a row per (issue, field, value)) is the canonical SQL antipattern — N self-joins for an N-field filter
  (Karwin, *SQL Antipatterns*, 2010). The proven answer is a **typed core (always-present, indexed columns) + a
  JSONB property-bag tail (custom fields, zero DDL) + a derived indexable projection (GIN + per-hot-facet
  generated indexes maintained off the bus)** — *byte-for-byte the shape Knowledge committed for `db_row`*.
- **Postgres serves the containment spine and the relation graph natively.** `parent_id` adjacency + recursive
  CTEs (`WITH RECURSIVE`, SQL:1999) handle the hierarchy/rollup walk (Celko, *Trees and Hierarchies in SQL*,
  2012; Refs §4.5). The board scan is an index range over `(tenant, project, state_category, rank)`.
- **The transactional outbox co-commits.** The state change + the `outbox` row + the audit append all commit in
  one Postgres transaction (no dual-write hazard — EI-02 §4). A document store would lose this, lose recursive-CTE
  walks, and add a second residency-pinned, crypto-shred-capable, backup-verified engine per cell (EI-02 §8:
  "every additional data engine is permanent operational cost"). **Rejected by default.**
- **Distributed-SQL (CockroachDB/Yugabyte) is the named, measured follow-on, not the floor.** PG-sharded-by-tenant
  is the floor; distributed-SQL only if a single tenant's shard is *measured* to outgrow PG (EI-02 §8).

Analytics (CFD, cycle-time, velocity over years) never touch the OLTP `issue` table — they hit the **OLAP read
store** (CQRS, Storage §3.4, contract 11.6) fed by the clean `issue.*` event stream, **honouring the restriction
flag** (no analytics for a restricted subject). Attachments live in `BlobStore` (content-addressed, BLAKE3,
residency-pinned); the OLTP row holds the pointer + metadata, not the bytes.

### 1.3 Frontend, content, query primitive, timers (frozen-shape ties)

| Concern | Choice | Justification / frozen-contract tie |
|---|---|---|
| Frontend shell | **TypeScript + React-class** consuming the shared design-system package | The one shell (design-language §5.1); Issues owns one rail entry + secondary nav + main view + the right context pane (IA §1). |
| Editor / renderer | **One Rust `myelin-content` core compiled to WASM** (Knowledge leads, ADR-05) | One editor render path for issue body + comments. Issues consumes the **frozen block subset** (contract 13.1, Δ5): paragraph/heading/lists/task_list/blockquote/code_block/callout/table/divider/image + the three inline ref nodes (`mention`/`artifact_ref`/`embed`); it **excludes** `db_view`/`sync_block`/`toggle` from inline authoring. `mention`/`artifact_ref` are structured nodes kept OUT of the markdown-subset string (KN-2) so reference-extraction is reliable; they are the producers of `refs.edge.created` (contract 5.4). |
| Structured-collection primitive | **`myelin-query`** shared crate (ADR-06/07) — co-owned with Knowledge, **frozen byte-identical** (contract 13.3) | Field-type enum + `ViewSpec` + `QueryAst` + `order_key` shared, byte-identical. Knowledge leads the definitions; Issues owns its **AST→store compiler + cost-bounding** ("share the schema language and the view model, not the query planner"). |
| Content concurrency | issue body = **single-author CAS** (ADR-05) | NOT the Knowledge CRDT — coarse-grained, server-arbitrated (sketch 08); board concurrency is server-arbitrated CAS. No Issues CRDT in v1. |
| Durable timers / waits | **`myelin-flow`** (`DurableExecutor`, the SC-11 wheel, contract 9.3) | SLA timers, the Trigger `stale_after`, snooze re-surfacing, HITL approval-card resume all ride the shared wheel — Issues does **not** reinvent durable waits. |
| Analytics read store | **shared OLAP** (ClickHouse-class, contract 11.6) | CQRS read model fed by the bus; reindex-from-source only; a `PersonalDataHolder`; restriction-flag-honouring. |
| Search | **shared Search** (Tantivy, contracts 6.1/6.3) | the cold/ad-hoc/full-text escalation valve; ACL-pre-filtered via the OQ-E `Filter`; Issues declares its `IndexSpec` and `project`s text. |

---

## 2. The issue spine — typed core + JSONB tail + derived projection (sketch 03; TE-17)

The central table. **Typed-core columns carry the hot path** (board/list/report — Linear-fast for the 90%); the
**JSONB tail carries the long-tail custom fields** (zero DDL); a **derived projection** (GIN + measured-hot
generated indexes off the bus) serves heavy custom-field filtering. This is the resolution of TE-17 / the
JQL-performance trap ([05 §3](./05-hard-problems.md)).

```sql
CREATE TYPE state_cat AS ENUM ('unstarted','started','completed','cancelled');  -- the FIXED category set (sketch 02 invariant)

CREATE TABLE issue (
  tenant         uuid        NOT NULL,
  region         text        NOT NULL,                  -- residency-pinned (ADR-11); == cell.region (residency-pin lint)
  id             uuid        NOT NULL,                   -- internal PK (UUIDv7: time-ordered, index-friendly)
  key            text        NOT NULL,                   -- HUMAN KEY 'ENG-1421' = the STORED CANONICAL <id> in the ArtifactRef (frozen <PROJECTKEY>-<seqno>, contract 5.1; §7)
  prefix         text        NOT NULL,                   -- the PROJECTKEY / team prefix (the Hi/Lo allocation key, §7)

  -- ── TYPED CORE (always present, indexed — the board/report hot path) ──
  type_id        uuid        NOT NULL,                   -- ranked type (sketch 01) → type_scheme row; carries hierarchy rank
  type_rank      smallint    NOT NULL,                   -- denormalised rank for fast roadmap/board partitioning (sub-task=0…initiative=3)
  state          text        NOT NULL,                   -- the named state (workflow-scheme defined; renamable)
  state_category state_cat   NOT NULL,                   -- the FIXED category (the cross-project reporting invariant)
  priority       smallint    NOT NULL DEFAULT 0,         -- typed core
  assignee       uuid,                                   -- pseudonymous principal_id (erasure-safe; EI-04 §1)
  reporter       uuid        NOT NULL,                   -- pseudonymous principal_id
  parent_id      uuid,                                   -- containment spine cache (issue_relation 'parent' is the truth, §4)
  project_id     uuid        NOT NULL,                   -- the identity 'project' authz scope (Id §5) — NOT re-invented
  cycle_id       uuid,                                   -- current cycle membership cache (cycle_membership is the truth, §5)
  rank           text        NOT NULL,                   -- the FROZEN order_key LexoRank string (contract 13.3, §5 of doc 02)
  title          text        NOT NULL,                   -- rendered title; contains_personal_data possible
  body_block     uuid,                                   -- the description's myelin-content block subtree root (ADR-05); #b<id> sub-anchors (contract 5.7)

  -- ── FLEXIBLE TAIL (zero-DDL custom fields) ──
  props          jsonb       NOT NULL DEFAULT '{}',      -- THE PROPERTY BAG: { field_id → value } (TE-17 source of truth)
  props_nodes    jsonb       NOT NULL DEFAULT '[]',      -- STRUCTURED ref/mention values kept OUT of free-text (refs.edge producer)

  -- ── LIFECYCLE / GDPR ──
  created_at         timestamptz NOT NULL,
  updated_at         timestamptz NOT NULL,
  state_changed_at   timestamptz NOT NULL,               -- the cycle-time clock basis
  archived           boolean NOT NULL DEFAULT false,
  deleted_at         timestamptz,                        -- soft-delete (reversibility window); NULL = live
  contains_personal_data boolean NOT NULL DEFAULT false, -- routes GDPR (title/props free-text may hold PII)
  data_role          text NOT NULL DEFAULT 'tenant-content',  -- the GDPR data_role (envelope-aligned)
  restricted         boolean NOT NULL DEFAULT false,     -- a subject's restrict flag touches rows referencing them (contract 10.1)
  pii_key_ref        text,                               -- per-subject DEK ref if free-text holds PII (GD-4) = kms://<tenant>/<epoch>/subject:<id>

  version        bigint      NOT NULL,                   -- CAS optimistic-concurrency token (board/field edits)
  PRIMARY KEY (tenant, id),
  UNIQUE (tenant, key)                                   -- the human key is unique per tenant (prefix makes it unique)
);

-- THE HOT BOARD/LIST SCAN = an index range (sketch 03):
CREATE INDEX issue_board   ON issue (tenant, project_id, state_category, rank) WHERE deleted_at IS NULL;
CREATE INDEX issue_roadmap ON issue (tenant, project_id, type_rank, rank)      WHERE type_rank >= 2 AND deleted_at IS NULL;
CREATE INDEX issue_assignee ON issue (tenant, assignee, state_category)        WHERE deleted_at IS NULL;
CREATE INDEX issue_parent  ON issue (tenant, parent_id);
CREATE INDEX issue_cycle   ON issue (tenant, cycle_id, state_category)         WHERE cycle_id IS NOT NULL;
-- CUSTOM-FIELD filters: the GIN index is the default; per-hot-facet generated indexes are provisioned off the bus:
CREATE INDEX issue_props_gin ON issue USING gin (props jsonb_path_ops);
--   e.g. CREATE INDEX issue_sev ON issue ((props->>'severity')) WHERE type_id = :bug;  (maintained by the projection feeder, §3 of doc 02)
```

**Design notes:**
- **`type_rank` is denormalised** onto the row so the board (`type_rank ≤ 1`) and the roadmap (`type_rank ≥ 2`)
  are index-range scans — the structural core of board↔roadmap co-equality ([05 §1](./05-hard-problems.md)). Kept
  in sync when `type_id` changes (rare; via the workflow path).
- **`state` (name) and `state_category` (fixed enum) are both stored.** The name is what the user sees and
  renames freely; the category is what *every* cross-project board/report/burndown reads (the one mandatory
  governance invariant — sketch 02). Assigning a new workflow scheme never migrates the row.
- **`parent_id` / `cycle_id` are caches of the relation truth.** The authoritative containment edge is
  `issue_relation(parent)`; the authoritative cycle membership is `cycle_membership`; the row carries a
  denormalised copy for the hot read, kept consistent in the same transaction.
- **`rank` is the frozen `order_key`** (contract 13.3, Δ7): base-62 `0-9A-Za-z`, lexicographic compare, midpoint
  bisection, 2-char jitter, 48-char rebalance trigger, `created_at`+ULID tiebreak — **byte-identical** with a
  Knowledge `db_row` drag, so a future shared CRDT/render path treats the field uniformly.
- **`body_block` points at a `myelin-content` block subtree** (ADR-05); description blocks are addressable via
  the frozen `#b<opaqueid>` sub-anchor (contract 5.7). Inline `mention`/`artifact_ref` produce
  `refs.edge.created` via the content producer path (no separate edge-write API — contract 5.4).
- **Erasure-safety:** `assignee`/`reporter`/`created_by` are *pseudonymous principal ids* (EI-04 §1) — erasing a
  person shreds the pseudonym map (Id `erase`, frozen grammar `<pseudonym>@<tenant>.noreply`, contract 4.8),
  turning them into "Former user 8a2f" everywhere without rewriting issues others own. Free-text PII (title/props)
  is encrypted under a **per-subject DEK** (GD-4, contract 11.4). The residual (third-party free-text typed into
  another person's issue) is handled per the **ONE platform erasure posture** (contract 10.9, recon §X-7) — not
  restated here (Δ13; see [06](./06-reconciliation-compliance.md)).

---

## 3. Governance schemes — interpreted config, never baked (sketch 02; PR-3)

Governance is **layered scheme objects assigned per (type × team/project)**, *interpreted* at runtime — so
turning governance on is a config write, never a data migration. "Linear-simple = no schemes assigned; Jira-
powerful = more schemes; one product, no fork" ([05 §2](./05-hard-problems.md)).

```sql
CREATE TABLE scheme (
  tenant     uuid NOT NULL, region text NOT NULL,
  scheme_id  uuid NOT NULL,
  kind       scheme_kind NOT NULL,    -- 'workflow' | 'field' | 'permission' | 'sla' | 'type'
  name       text NOT NULL,
  body       jsonb NOT NULL,          -- the kind-specific definition
  version    bigint NOT NULL,
  PRIMARY KEY (tenant, scheme_id)
);

CREATE TABLE scheme_assignment (
  tenant     uuid NOT NULL,
  scheme_id  uuid NOT NULL,
  kind       scheme_kind NOT NULL,
  type_id    uuid,                    -- NULL = any type
  project_id uuid,                    -- NULL = any project (org-default)
  team_id    uuid,                    -- NULL = any team
  PRIMARY KEY (tenant, kind, COALESCE(type_id,'…'), COALESCE(project_id,'…'), COALESCE(team_id,'…'))
);
```

### 3.1 The five scheme kinds (the `body` shapes)

| Scheme | `body` shape (illustrative) | Default (Linear-simple) | When opted in |
|---|---|---|---|
| **workflow** | `{ states:[{name, category}], transitions:[{from,to,guard:QueryAst,post_actions:[]}] }` | one 3-state default `Todo→In Progress→Done` + `Cancelled`, each mapped to a category | custom states, transition guards (CI-gate, approval), required-fields-on-transition |
| **field** | `{ fields:[{field_id, type:FieldType, scope, required_on:[transition], validation:QueryAst}] }` | none required; ad-hoc fields allowed | typed/validated/required fields; the `type` is the **frozen `FieldType` enum** (contract 13.3) |
| **permission** | `{ field_overlays:[{field_id, caveat:CaveatContext-template}], transition_overlays:[…], confidential_default:bool }` | the default ReBAC inheritance (no overlays) | field-level / transition-level / confidential overlays (the frozen `CaveatContext`, §9 of doc 03) |
| **sla** | `{ applies_to:QueryAst, metric, target:business_duration, calendar_id, pause_conditions:QueryAst[], escalation_chain }` | none | support/ITSM orgs (§6, sketch 07); `escalation_chain` = the frozen Notif 7.5 shape |
| **type** | `{ types:[{type_id, name, rank, may_parent_ranks:[]}] }` | the default ranked set | custom types, custom ranks, DAG portfolios |

**The two design rules that give the "no data migration" guarantee** (sketch 02):
1. **Schemes are interpreted, not compiled into the row.** An issue stores `state` (a name) + `category`, not
   "which workflow am I." Assigning a new workflow to its (type, project) is a config write; the existing
   `category` lands the issue in the new workflow's matching-category state set.
2. **Adding a flexible field is a config write (a `field` scheme edit), never DDL** (JSONB tail; §2).

The **scheme-resolution precedence algebra** (resolved in [02 §1](./02-internals-and-algorithms.md)) is a
deterministic, cached `(type × project × team)` lookup. Guards/validations are the **frozen `myelin-query`
`QueryAst`** (= the `EventMatcher` core, contract 3.4) — bounded, no UDFs/loops/recursion, statically
cost-bounded, permission-aware by construction (ADR-07; not CEL/JSONLogic) — so there is no Jira-Groovy footgun.

---

## 4. The typed relation table — `issue_relation` (TE-7 source of truth; contract 5.5)

Issues **owns `issue_relation` as the source of truth** for lifecycle edges (the frozen Refs §3.3 / contract 5.5).
We write the **forward** edge transactionally + emit **one** typed event; **Refs materialises both directions** in
its rebuildable projection and **fixes the inverse pairing**. We do *not* store both directions (avoids dual-write
drift).

```sql
CREATE TYPE issue_rel AS ENUM ('parent','blocks','blocked_by','closes','depends_on','relates');

CREATE TABLE issue_relation (
  tenant      uuid NOT NULL, region text NOT NULL,
  relation_id uuid NOT NULL,
  src_issue   uuid NOT NULL,          -- internal id; FK referential integrity Refs cannot give
  dst_ref     text NOT NULL,          -- ArtifactRef of the other end (may be cross-subsystem: a PR, a doc, a chat thread)
  rel         issue_rel NOT NULL,
  created_by  uuid NOT NULL, created_at timestamptz NOT NULL,
  PRIMARY KEY (tenant, relation_id),
  UNIQUE (tenant, src_issue, dst_ref, rel),
  FOREIGN KEY (tenant, src_issue) REFERENCES issue(tenant, id) ON DELETE CASCADE
);
CREATE INDEX issue_rel_src ON issue_relation (tenant, src_issue, rel);
CREATE INDEX issue_rel_dst ON issue_relation (tenant, dst_ref, rel);   -- "what blocks me" = the guard read (sketch 05A)
```

**Owned decisions** (sketch 05A):
- **`parent` is a tree** (single parent per issue; rank-monotonic by default — an epic may parent stories, not
  vice versa; sketch 01). `depends_on` / `relates` / `blocks` form a **DAG** — these are where cycles can appear
  (A blocks B blocks A) → cycle detection lives in the rollup/traversal walk (visited-set + depth ceiling 16,
  matching Refs §4.5 / contract 5.3), surfaced as a *diagnostic* ("⚠ dependency cycle"), never a hang.
- **Inverse pairing is maintained by Refs, not dual-written here** (contract 5.5). `blocks(A,B)` ⇒ Refs projects
  `blocked_by(B,A)`. The transition guard "can't close while `blocked_by` an open issue" reads `issue_rel_dst`
  for the `blocks` edges *pointing at* this issue — no projection round-trip needed for the hot guard.
- **Cross-subsystem ends** (`closes` a PR, `relates` a doc) put an `ArtifactRef` in `dst_ref`; the FK constrains
  only the `src_issue` end. The far end's integrity is the projection's concern (best-effort eventual inverse —
  EI-04 §2; contract 5.4).
- **The stateful Trigger reads this table** via the frozen `QueryAst` ("becomes unblocked" = all `blocked_by`
  edges resolve, §10 of doc 03; contract 3.3).

---

## 5. The change-log, cycles, milestones

```sql
-- Every state change recorded (the audit + GDPR + activity-feed basis — sketch 02 invariant; baked-in).
CREATE TABLE issue_change_log (
  tenant     uuid NOT NULL, region text NOT NULL,
  issue_id   uuid NOT NULL,
  seq        bigint NOT NULL,                -- per-issue monotonic; the issue is the aggregate (ordering)
  actor      uuid NOT NULL,                  -- pseudonymous principal (human OR agent — same shape)
  actor_kind text NOT NULL,                  -- 'human' | 'agent' | 'service' (attribution; design-language §6.4)
  on_behalf_of uuid,                         -- delegation chain (agent acting for a human)
  change     jsonb NOT NULL,                 -- the field deltas { field → {from,to} } (references-not-payloads where possible)
  pii_key_ref text,                          -- per-subject DEK if a delta carries free-text PII (GD-4)
  at         timestamptz NOT NULL,
  PRIMARY KEY (tenant, issue_id, seq)
);

-- TIME AXIS — a cycle is NOT an issue (no workflow state/assignee); membership is a relation, not containment.
CREATE TABLE cycle (
  tenant uuid NOT NULL, region text NOT NULL, cycle_id uuid NOT NULL,
  project_id uuid NOT NULL, name text NOT NULL,
  starts_at timestamptz NOT NULL, ends_at timestamptz NOT NULL,
  capacity numeric,                          -- committed-estimate vs capacity bar (flow A3)
  state text NOT NULL,                       -- 'planned' | 'active' | 'completed'
  PRIMARY KEY (tenant, cycle_id)
);
CREATE TABLE cycle_membership (
  tenant uuid NOT NULL, cycle_id uuid NOT NULL, issue_id uuid NOT NULL,
  added_at timestamptz NOT NULL, carried_over_from uuid,   -- carry-over provenance (flow A3)
  PRIMARY KEY (tenant, cycle_id, issue_id)
);

CREATE TABLE milestone (                       -- versions/releases (import target; sketch 09)
  tenant uuid NOT NULL, region text NOT NULL, milestone_id uuid NOT NULL,
  project_id uuid NOT NULL, name text NOT NULL, target_date date, released_at timestamptz,
  PRIMARY KEY (tenant, milestone_id)
);
```

The change-log is the basis for the activity feed (humanised at render via the ONE templating surface — contract
7.3), the audit attribution Issues contributes to the tamper-evident log (contract 10.6; Issues contributes
attribution, not the log itself), and the OLAP cycle-time/CFD computation (the `state_category` transitions feed
the read store).

---

## 6. The rollup aggregate + SLA + forecast (sketch 05B, 07)

```sql
-- DERIVED materialised aggregate per ancestor (rebuildable by replay; the edge truth stays in issue_relation).
CREATE TABLE rollup (
  tenant uuid NOT NULL, region text NOT NULL,
  issue_id uuid NOT NULL,                     -- the ancestor (epic/initiative)
  done_count int NOT NULL, total_count int NOT NULL,
  sum_estimate numeric NOT NULL, done_estimate numeric NOT NULL,
  earliest_start date, latest_due date,
  blocked_count int NOT NULL,                 -- contributing-blocked surface (roadmap "date-at-risk", flow A4)
  forecast jsonb,                             -- the agent-written forecast field (floor: linear; follow-on: Monte-Carlo)
  input_hash text NOT NULL,                   -- no-op suppression: a recompute with the same input_hash emits no event (AG-6)
  computed_at timestamptz NOT NULL,
  PRIMARY KEY (tenant, issue_id)
);

-- SLA INSTANCE — the per-issue live SLA, with the precomputed wall-clock deadline handed to the SC-11 timer.
CREATE TABLE sla_instance (
  tenant uuid NOT NULL, region text NOT NULL,
  sla_id uuid NOT NULL, issue_id uuid NOT NULL,
  policy_scheme_id uuid NOT NULL,             -- the sla scheme (§3)
  metric text NOT NULL,                       -- 'time_to_first_response' | 'time_to_resolution' | custom
  state text NOT NULL,                        -- 'running' | 'paused' | 'met' | 'breached'
  started_at timestamptz NOT NULL,
  remaining_business_secs bigint NOT NULL,    -- seconds (units anchor); decremented at pause (sketch 07 Candidate A)
  fire_at timestamptz,                        -- the PRECOMPUTED wall-clock deadline → the SC-11 timer fire_at (contract 9.3)
  at_risk_fire_at timestamptz,                -- the 80% timer (a second SC-11 timer)
  timer_handle text,                          -- the myelin-flow timer id (cheap disarm/re-arm on pause/resume, contract 9.3)
  PRIMARY KEY (tenant, sla_id)
);
CREATE INDEX sla_by_issue ON sla_instance (tenant, issue_id) WHERE state IN ('running','paused');

CREATE TABLE business_calendar (              -- a reusable calendar config (sketch 07)
  tenant uuid NOT NULL, calendar_id uuid NOT NULL,
  tz text NOT NULL,                           -- IANA tz (DST-correct via the tz database)
  working_hours jsonb NOT NULL,               -- [{dow, start, end}] per weekday
  holidays jsonb NOT NULL,                    -- [date] (+ optional RRULE for recurring holidays)
  PRIMARY KEY (tenant, calendar_id)
);
```

The SLA logic engine (policy + business-calendar arithmetic + pause/resume + escalation) is **built over** the
`myelin-flow` timer/signal/workflow substrate; the *timers* are the SC-11 wheel (we precompute `fire_at`, never
poll, never pollute the wheel). The arithmetic algorithm is [02 §6](./02-internals-and-algorithms.md). On
breach, the **escalation chain** is the frozen Notif `7.5` shape (`page → oncall_now → escalate-after-timer` on
the wheel; Δ11). Breach/met events feed OLAP for compliance reporting. Forecasting is an **agent-powered
swappable strategy** reading OLAP (floor: linear `remaining ÷ velocity`; follow-on: a Monte-Carlo agent —
[05 §7](./05-hard-problems.md)).

### 6.1 Worklog / productivity / estimate field sensitivity (frozen tags, Δ12)

`estimate`, `story_points`, time-tracking/worklog, and any per-individual productivity field carry the frozen
classification (contract 10.2, OQ-H):

```
#[personal_data(category = behavioural, role = tenant-content, basis = TBD-LEGAL, retention = tenant-policy)]
```

with a **restricted `data_role` by default**, meaning: excluded from cross-individual analytics + agent-use for a
restricted subject (the `restrict` suppression, contract 10.1); per-individual productivity rollups are **off by
default** behind an explicit tenant-admin enablement the posture flags as requiring works-council consultation in
applicable EU jurisdictions; same per-subject DEK crypto-shred as other free-text PII. This is `[OPEN — LEGAL]`
(counsel ratifies special-category vs elevated; the structural floor ships regardless).

---

## 7. Human-key allocation — Hi/Lo per prefix (sketch 04; TE-14 / the frozen REF-3 reconciliation)

```sql
CREATE TABLE prefix_counter (
  tenant uuid NOT NULL, prefix text NOT NULL,   -- the PROJECTKEY; belongs to a team (the project/team owns it)
  high_water bigint NOT NULL,                   -- the durable high-water mark (advanced by a block at reserve)
  block_size int NOT NULL DEFAULT 50,           -- adaptive: small for cold prefixes (tiny gaps), large for hot (low contention)
  PRIMARY KEY (tenant, prefix)
);
```

**Allocation = Hi/Lo batched per prefix** (Hibernate's HiLo; the classic allocator): a worker reserves a block of
`block_size` keys in one `UPDATE … RETURNING` (advancing `high_water`), then hands them out from memory until the
block drains — N creates into 1 counter write. **Gap-tolerant, monotonic, never-reused** (matching
GitHub/GitLab/Jira's real behaviour — gaps already exist; gaplessness is a *perception*, not a requirement, and
its single-writer cost is the contention hotspot we refuse to pay — sketch 04). A crashed worker leaks at most
one block (a small, benign, documented gap). Per-prefix isolation means a busy `ENG` doesn't slow `OPS`.
Allocation is **cell-local** (a prefix lives in one cell — no cross-region coordination).

**The frozen REF-3 reconciliation (Δ3, contract 5.1).** The **stored canonical `<id>` segment** in the
ArtifactRef is the full `<PROJECTKEY>-<seqno>` key (e.g. `ENG-1421`) — `myelin://<tenant>/issue/issue/ENG-1421`.
The short display form `#1421` (dropping the project prefix in-context) is the **render-time display projection**,
derived by the UI; it is **never stored as the link**. The id grammar is agreed before any key is minted, so the
URN never has to be rewritten.

---

## 8. The trigger, import map, and the stateful-component register

```sql
-- The Issues-side stateful Trigger record (the bus owns arm/disarm; we own the armable-condition catalogue, §10).
CREATE TABLE issue_trigger (
  tenant uuid NOT NULL, region text NOT NULL,
  trigger_id uuid NOT NULL,
  owner uuid NOT NULL,                          -- the person to notify (pseudonymous)
  arms_subject text NOT NULL,                   -- the issue ArtifactRef being watched
  condition jsonb NOT NULL,                     -- the FROZEN QueryAst (contract 3.3/3.4): e.g. "all blocked_by resolved"
  on_resolve jsonb NOT NULL,                    -- the Notif inbox item template (humanised at resolve via contract 7.3)
  stale_after_secs bigint NOT NULL,             -- seconds → a myelin-flow durable timer (contract 9.3)
  state text NOT NULL,                          -- 'armed' | 'resolved' | 'stale' | 'disarmed'
  timer_handle text,
  armed_at timestamptz NOT NULL,
  PRIMARY KEY (tenant, trigger_id)
);

-- The IMPORT MAP — the load-bearing artifact for idempotency/resume/rollback/re-sync + export round-trip (sketch 09).
CREATE TABLE import_map (
  tenant uuid NOT NULL, region text NOT NULL,
  import_job uuid NOT NULL,
  source text NOT NULL,                         -- 'jira' | 'linear' | 'github' | 'csv' | 'canonical'
  source_id text NOT NULL,                      -- the source's id (idempotency key)
  myelin_kind text NOT NULL,                    -- 'issue' | 'cycle' | 'milestone' | 'relation' | 'user'
  myelin_id uuid,                               -- the created Myelin id (NULL until pass-1 creates it)
  status text NOT NULL,                         -- 'pending' | 'created' | 'wired' | 'lossy' | 'dropped'
  loss_note text,                               -- the named lossy/dropped reason (per the FROZEN ADF map, contract 13.2; never silent)
  PRIMARY KEY (tenant, import_job, source, source_id)
);
```

### 8.1 The stateful-component register (shard key + blast radius + crypto-shred unit)

Every stateful component is named with a shard key + blast radius + crypto-shred unit; everything else is
stateless/replaceable.

| # | Component | Engine | Holds | Shard key | Blast radius if it dies | Crypto-shred unit |
|---|---|---|---|---|---|---|
| I1 | **`issue` + `issue_relation` + `issue_change_log` + schemes + `outbox`** | Postgres-class | the spine + typed edges (source of truth) + change-log + config + the outbox | `(tenant, region)` + project_id | one tenant's issues; recoverable (outbox drains; reindex rebuilds derived) | per-tenant DEK; **per-subject DEK for free-text columns** (title/props/change deltas — GD-4) |
| I2 | **`rollup`** | Postgres-class | the derived ancestor aggregate | `(tenant, region)` + issue_id | derived — rebuildable from I1 by replay | inherits I1 |
| I3 | **`sla_instance` + `business_calendar`** | Postgres-class | live SLA state; calendars | `(tenant, region)` + issue_id | timers re-derive from policy+state; the timer durability is `myelin-flow`'s (contract 9.3) | inherits I1 |
| I4 | **`prefix_counter`** | Postgres-class | the Hi/Lo high-water mark | `(tenant, prefix)` | a crash leaks one block (a gap); never double-allocates | inherits I1 |
| I5 | **`issue_trigger`** | Postgres-class | the armable-condition records | `(tenant, region)` + owner | a trigger fires late until recovery; the `stale_after` is durable (contract 9.3) | per-subject (owner) |
| I6 | **`import_map`** | Postgres-class | the source↔Myelin id map | `(tenant, region)` + import_job | resume re-reads it (idempotent); no duplicate creates | per-tenant; source-PII per-subject |
| I7 | **Derived projection indexes (feeder)** | Postgres (GIN/generated cols) | the hot-custom-facet projection | `(tenant, region)` + type_id | derived — rebuildable from I1 | inherits I1 |
| I8 | **Consumer dedup ledgers** | Postgres-class | the consumer template's idempotency (contract 2.5) | `(tenant, consumer)` | re-process is idempotent → no loss | inherits I1 |

The **`issue` ReBAC tuples are NOT Issues' component** — they live in Id's tuple store (and feed the per-tenant
authz reverse index the `SetExpr` push-down JOINs against, contract 4.3); Issues only *projects* into them via
`write_tuples` (§9 of [03](./03-events-contracts-and-glue.md)). **Hot tables flagged for the
`forward-only-migration` lint** (contract 1.5): `issue`, `issue_relation`, `issue_change_log` — schema changes
use expand→backfill→contract, never a blocking `ALTER`.

Continue to [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md).
