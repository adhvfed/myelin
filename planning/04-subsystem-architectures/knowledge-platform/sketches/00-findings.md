# Knowledge Platform — Stage-1 Findings (committed direction + handoff to architecture)

> Phase 4, Knowledge, **Stage 1 → Stage 2 handoff**. Canonical: VISION (never contradict), KN-1…KN-4,
> ADR-05/06/07/13, the Phase-3 contract surface, EI-04 §2, EI-05. This is what I learned exploring
> (sketches 01–07) + designing (IA/flows/wireframes), the decisions I now **commit**, and the open
> questions I hand to my own Stage-2 architecture. Dated 2026-06-19.

---

## 1. What I learned (the load-bearing realisations)

1. **The transport is the keystone, not the engine.** KN-1's "build the resume-cursor durable
   transport FIRST" is right because an *op* is opaque bytes — a CAS op, an OT op, and a Yrs CRDT
   update all ride the same per-doc op-log with the same `seq` cursor and the same idempotent apply.
   Build the transport once (the bus's own outbox/`seq`/dedup pattern, reused) and the CAS→CRDT
   promotion is an apply-function swap, not a rewrite. This collapsed three apparently-separate problems
   into one.
2. **The doctrine already pre-resolved most of my "hard problems" as floor-first inversions.** KN-2
   (md-subset inline string), KN-3 (read-time-not-stored rollups), the CAS→CRDT ladder, and the
   AST-for-structure/string-for-inline seam are all *decided in shape* by EI-04 + the decision-record;
   my job was to apply them concretely, not re-open them. The remaining genuine design freedom was
   block-tree storage, permission granularity, and the multi-region-collab case.
3. **Almost every scale/leak concern is already solved by a Phase-3 contract I just consume.**
   Permission-filtered reads (`list_objects`), embed liveness (Refs `resolve` + `project`), big-DB
   structured queries (Search structured index), erasure-over-immutable-data (crypto-shred + per-subject
   DEK), reindex (replay/snapshot). Knowledge adds *very few new scale surfaces* — the new ones are the
   collab op-log volume and the flexible-DB query, both of which have measured-promotion floors.
4. **The editor primitives unblock everything and must ship first.** `render(parse(md)) === md`,
   the offset model, and DOM-surgery (Enter-split/caret-after-split) are independently testable and gate
   no concurrency decision — so they ship + unit-test standalone (KN-4/§8b.2) before the integrated
   editor and before any engine choice bites.
5. **Synced blocks are a trap the `embed(ArtifactRef)` primitive sidesteps.** Transclusion's value
   (show content from elsewhere, live, permission-correct) is already delivered by `embed` via the
   reference graph — without breaking the block tree or opening the private-home/public-site leak.

---

## 2. Committed direction on each hard problem

| Hard problem | Committed direction (Stage 2 builds this) | Floor / promotion trigger |
|---|---|---|
| **CRDT vs OT + granularity (TE-15)** | **CAS floor → Yrs inline-text CRDT + Kleppmann move-CRDT for the block tree**, both over the same op-log transport. OT **rejected** (rich-tree transform-correctness + weak offline). Yrs justified: Rust-native (YATA), convergence-by-construction, server stays the permission/schema/erasure authority above the merge. | Floor: per-block CAS + soft-locks + snapshot/restore (named "does not merge"). Promote on the **first true concurrent-prose conflict** (measured CAS-conflict rate, R5). |
| **Resume-cursor transport FIRST (KN-1)** | **Per-doc append-only op-log in OLTP, server-assigned `seq` = the resume cursor, idempotent apply (op_id + seq dedup), best-effort firehose fan-out, write-through session actor.** Built **first**. Reuses the bus outbox/`seq`/dedup pattern → zero-loss-on-reconnect is *structural*. | The reconnect-loses-zero-ops drill (T-5) is mine and passes by construction. |
| **Block-tree storage (TE-16)** | **Hybrid (Candidate C): per-block rows (adjacency list + fractional order key, stable `block_id`) as the source of truth for tree structure; inline content as the md-subset string (floor), promotable to a per-block Yrs text-CRDT.** Partial/lazy load for huge docs; block-level refs/permissions/erasure; `block_id` = `#sub` ref target, stable across moves. | Single-blob rejected (caps doc size, fights the wedge). |
| **Content model (ADR-05 / KN-2)** | **AST for block structure; inline content = `{ md: markdown-subset string, nodes: [structured inline nodes] }`; `mention`/`artifact_ref`/`embed` are structured nodes anchored by a stable placeholder, never collapsed into the string.** I lead the block + inline taxonomy; Chat/Issues consume the AST+string+nodes, not the engine. | Reference-extraction = node-array walk (reliable); rename/erase never touches stored prose. |
| **Flexible-DB query model (TE-17)** | **JSONB property bag per row (OLTP, transactional, per-tenant, no per-db DDL) + a derived indexable projection — generated columns/expression indexes locally for measured-hot fields; the shared Search structured index (off the bus) for large views.** | Materialised-table-per-db + ClickHouse-as-truth rejected; ClickHouse stays the measured-promotion cross-db analytics home. |
| **Formula/rollup engine (TE-18)** | **Read-time evaluation over the ACL-pre-filtered, paginated working set, using the one safe `myelin-query` evaluator (no UDFs/loops/recursion; cycle-checked at schema-def time). No stored dataflow engine in v1.** | Promote a *specific* measured-too-slow rollup over a large related set → a materialised incremental aggregate maintained off the bus (R5). |
| **Permission granularity (page/row/field)** | **Page/database-level floor** via the `knowledge.*` ReBAC namespace (page-tree inheritance via tuple-to-userset rewrite + override via the `direct_block` exclusion). **Row-level as an opt-in per-database capability in v1**; **field-level as the named ABAC-caveat follow-on**. Pure-pages model (page = doc + folder). | Block-level ACL deferred (block addressable for refs/erasure, not independently ACL'd). |
| **Offline depth** | **Read-anywhere + optimistic light-edit-online; offline edits queued behind CAS.** | Full offline-first co-editing = the CRDT-promotion follow-on (a CRDT is designed for offline merge; CAS is not). |
| **Synced blocks / transclusion** | **Deferred** in favour of `embed(ArtifactRef)` (permission-aware-per-viewer, tombstoning, reference-counted). | Editable-in-place synced blocks = measured-demand follow-on (most-restrictive-of-sites permission, erasure via the edge index). |
| **Embed liveness** | **Live-by-default** via Refs `resolve` → owning subsystem's `project` + subscription to `*.updated`/`*.erased`; three tiers (live on-screen / on-load / Refs cache); embedded-view content is an ACL-pre-filtered paginated query. | No new scale surface; consumes the projection contract, never another subsystem's DB. |
| **GDPR erasure over history/op-logs** | **GD-4 split**: per-subject DEK for free-text/inline/comment bodies (crypto-shred reaches live rows + op-log + history + backups — the "can't delete an op" answer); pseudonym indirection for authorship + structured refs (anonymise, don't delete); per-tenant DEK for bulk structure. | Honest limitation (GD-6): free-prose-about-someone needs tooling+process; structured refs erase reliably. |
| **Agent trace (AG-7)** | **Knowledge accepts a content-addressed agent-trace write → `ArtifactRef`; registers it as an erasable `PersonalDataHolder` (per-subject DEK class); agent is the `kind=agent` author; distinct from the audit log.** | — |

---

## 3. The build order (sequenced by KN-1 + R-1 "what kills you first")

1. **Editor primitives standalone** (serializer / offset model / DOM-surgery; `render(parse(md))===md`
   corpus gate) — KN-4/§8b.2; gate no concurrency choice.
2. **The resume-cursor durable transport** (per-doc op-log, `seq` cursor, idempotent apply, firehose
   fan-out) — KN-1, **first**; the reconnect-loses-zero-ops drill.
3. **Block-tree storage + the content model** (per-block rows + md-subset-string inline + structured
   nodes) — TE-16/ADR-05.
4. **The CAS floor** (per-block CAS + soft-locks + snapshot/restore, named "does not merge") over the
   transport — TE-15 floor.
5. **Permissions** (the `knowledge.*` namespace; collab-authority gates the op-log append on
   `Id.check`; pre-filtered reads) — ADR-03.
6. **Databases** (JSONB property bag + views + read-time formulas/rollups) — TE-17/TE-18/ADR-06.
7. **References/backlinks/embeds + Search integration** (emit `refs.edge.*`; declare `IndexSpec` +
   `project` + `replay`; embed liveness) — Refs/Search.
8. **GDPR holder + the agent-trace write** (locate/export/erase; per-subject DEK; AG-7) — ADR-12/AG-7.
9. **History/restore, sharing/publish, templates, export** — the governance surfaces.
10. **CRDT promotion** (Yrs inline + move-CRDT) — triggered by the first true concurrent-prose conflict.

Floors named in the gap report (E-3): CAS-floor (→ CRDT), read-time-rollup (→ materialised),
page-level-permission (→ row/field), single-cell collab (→ cross-cell), synced-blocks-deferred (→ built),
free-text-PII-detection (→ tooling+process).

---

## 4. Obligations I confirmed I must implement (the Phase-3 build-to surface)

- **The three glue contracts**: `project(ref, viewer) → {title, state, icon, render_hint, sub_anchor?}`
  for my pages/blocks/dbs/rows; `replay(scope, since)` emitting `*.snapshot` (sub-artifact-granular);
  `ArtifactRef` parse/format via the one library; emit every state change via `OutboxTx::emit` only.
- **One Principal authorized by Id**: declare the `knowledge.*` ReBAC namespace fragment
  (`space`/`page`/`database`/`database_row`); never invent object ids; `check`/`list_objects`-pre-filter
  every read; `write_tuples` (zookie) on page-tree/ACL changes.
- **`PersonalDataHolder`**: locate/export/rectify/restrict/erase across blocks/rows/op-log/history/
  mentions/authorship; per-subject + per-tenant DEK classes; `#[personal_data(...)]` tags drive the data
  map.
- **ToolDef registrations**: `knowledge.page.create|append`, `knowledge.row.upsert`,
  `knowledge.page.summarise` (read), `knowledge.search` — with input schema, required caps, effect kind,
  side-effecting + `requires_approval` defaults (consequential edits gated).
- **The typed tables I own**: `page_parent` (page-tree, parent lifecycle edge) + `db_relation` (two-way
  relation field, the TE-7 source of truth; Refs projects the lifecycle edge).
- **The agent-trace write** (AG-7) + register it as an erasable holder.
- **KN-1 first**: own the collab op-stream resume-cursor durable transport; the reconnect drill is mine.
- **No reserve/settle of my own** beyond agent runs: agent runs that author Knowledge content pass the
  reserve/settle gate the Agent Fabric owns; Knowledge does not run its own spend-bearing work outside
  that (scheduled living-doc agents are agent runs → the gate applies). Confirm in Stage 2.

**Language/tools**: **Rust** (ADR-02 default; no divergence requested — and the CRDT promotion is
**Yrs**, Rust-native, which *reinforces* the default). **Postgres-class OLTP** for the block tree
(per-block rows), db rows (JSONB), the op-log, and the typed tables; **S3-compatible object store** for
media + content-addressed snapshots + the agent trace; **firehose transport** (NATS-core-class, the
seam the bus provides) for live op fan-out + presence; **Search (Tantivy)** + **Refs (PG)** consumed,
not rebuilt. No justified divergence from Rust.

---

## 5. Primary screens I designed (the §7.4 catalogue)

Block editor (S1) · Database views table/board/calendar/timeline (S2) · Navigation tree (S3) ·
Backlinks/references pane (S4) · Comments/discussion (S5) · Page history (S6) · Sharing/permissions
dialog (S7) · Templates gallery (S8) · Search/quick-switcher palette + full search (S9) · Agent
affordances + HITL approval card (S10) · Export (S11) · Mobile read+light-edit (S12). Each with
empty/loading/error (+ permission-denied/erased) states and the §8b day-one primitives applied
(portal-always overlays, one editor render path, measured tokens, layout-containment, humanised
strings). See `wireframes.md`.

---

## 6. Open questions handed to my own Stage-2 architecture

1. **The fractional-index rebalancing strategy** (sketch 02): when concurrent inserts exhaust precision,
   how/when to rebalance order keys without a disruptive whole-doc rewrite (interacts with the move-CRDT).
2. **The exact op-log → snapshot compaction cadence + format** (sketch 01/06): the snapshot is the
   `replay` source *and* the history restore point *and* the crypto-shred unit boundary — one format
   serving three masters needs pinning.
3. **The structured-inline-node placeholder encoding** (sketch 02): PUA sentinel vs explicit `{{ref:N}}`
   marker — must survive copy/paste + the `render(parse(md))===md` round-trip *and* be unambiguous for
   reference-extraction. Decide + add to the corpus.
4. **Row-level permission: tuple vs ABAC-caveat** (sketch 04): which mechanism for "see only your team's
   rows" stays off the hot `list_objects` path at scale — and the per-database opt-in UX.
5. **The `list_objects` push-down encoding for big-DB views** (sketch 03): the Search-shared open item
   (`search-and-indexing.md` §10) — how the ACL filter conjoins into the structured DB query at scale.
6. **CAS conflict-rate measurement → CRDT promotion trigger** (sketch 01): the concrete metric + threshold
   that fires the named promotion (R5), and how the per-block migration from CAS to a Yrs doc runs online.
7. **Comments: shared thread component reuse vs KB-native rendering** (sketch 07): the data+anchor is
   KB-native; whether the *rendering* reuses Chat's component is a frontend-reuse call co-owned with Chat.
8. **Cross-cell collab fan-out** (sketch 07): the named floor — the control-plane PII-free pointer-bridge
   detail co-owned with the bus/Refs cross-cell floor + SC-2/SC-3 multi-cell tenancy.
9. **Per-subject DEK granularity vs key-count explosion** (sketch 06): confirm the GD-4 rule keys *only*
   the genuinely-inline-PII classes per-subject (not every block) to avoid millions of keys per tenant.

These are *architecture-shaped*, not exploratory — Stage 2 commits them; Stage 1 has bounded each.

## Cross-references
- Exploration: sketches 01 (collab/transport), 02 (block tree/content), 03 (db/formula), 04
  (permissions), 05 (transclusion/embed liveness), 06 (GDPR/agent trace), 07 (taxonomy/search/refs/
  multi-region).
- Design: `design/information-architecture.md`, `design/user-flows.md`, `design/wireframes.md`.
- Canonical: VISION; KN-1…KN-4, ADR-05/06/07/13, EI-04 §2, EI-05; the Phase-3 contract-index.
