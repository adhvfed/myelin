# Knowledge Platform — 06 · Required Shared-System Changes (for Phase 5 reconciliation)

> See [`00-overview.md`](./00-overview.md) for framing. This is the **explicit, itemized list** of what
> Knowledge needs from the shared systems that **isn't already in the Phase-3 contracts** — the Phase-5
> reconciliation input (the agent that rewrites the Phase-4 docs and refines the shared layer). Each item:
> the ask, who owns it, whether it reverses a Phase-3 decision (none do — all are sharpenings or confirmed
> dependencies), and the X-5 names/units reconciliation point. Pre-ship contract reconciliation (T-9 / X-5)
> applies to every item.

---

## 1. From the Event Bus (`myelin-events`)

| # | Ask | Nature | Owner |
|---|---|---|---|
| BUS-CR-1 | **Firehose sizing for the collab op-stream + presence.** The collab op-stream (KN-1) and presence ride `firehose::publish/tail` (event-bus §4.3/§5.5). The bus already provides the seam + the `knowledge.doc.updated` pointer; Knowledge needs the firehose **sized for collab volume** (per-doc op fan-out at hot-document scale) and confirms the durable bus carries only the pointer. | **Confirmation + sizing** of an existing seam (event-bus §4.3) | Bus |
| BUS-CR-2 | **`replay(scope, since)` must support page-subtree + sub-artifact-granular `*.snapshot`.** Knowledge's `replay` ([03 §2.3](./03-events-contracts-and-glue.md)) emits `knowledge.page.snapshot` at **block granularity** (so Search re-indexes blocks and Refs re-derives sub-artifact edges). The bus's re-emit protocol (event-bus §4.9) must carry sub-artifact-granular snapshots. | **Confirmation** (event-bus already provides the protocol; this asks the *granularity* be honoured — the same Search ask, search §9.2) | Bus + every subsystem |
| BUS-CR-3 | **The `knowledge.*` taxonomy registered under the §6 grammar.** The complete list ([03 §1](./03-events-contracts-and-glue.md)) — `knowledge.page.*`, `knowledge.block.*`, `knowledge.database.*`/`view.*`/`row.*`, `knowledge.relation.*`, `knowledge.page.parent_set`, `knowledge.comment.*`/`mention.created`, `knowledge.access.*`, `knowledge.subject.*`, plus the cross-cutting `knowledge.doc.updated` pointer and `knowledge.*.erased`/`*.snapshot`. Validated against the §6 grammar. | **The P4 taxonomy completion** the bus seeded (Bus §10 Q1) | Knowledge (this doc) → Bus validates |

---

## 2. From the Reference Graph (`myelin-refs`)

| # | Ask | Nature | Owner |
|---|---|---|---|
| REF-CR-1 | **The `#sub` block-anchor grammar must be stable across edits/moves.** Knowledge mints `#b<block_id>` / `#h<block_id>` / `#comment-<id>` / `#row-<id>` ([03 §2.1](./03-events-contracts-and-glue.md)); Refs must store the full sub-artifact URN + the `#sub`-stripped `target_root`, and resolve a `#sub` ref to the parent projection + sub-anchor with graceful degradation when the block is removed. | **Confirmation** of Refs §3.5 (the stability is Knowledge's; the grammar is Refs') | Refs + Knowledge |
| REF-CR-2 | **The TE-7 `lifecycle` mirror must carry Knowledge's `parent` + `relates`/`rollup_source` rels.** Knowledge owns `db_relation`/`page_parent` (the source of truth, REF-1); Refs fixes the `rel` vocabulary + the inverse pairing (`parent`↔`child`) and consumes `knowledge.relation.*` / `knowledge.page.parent_set`. | **Confirmation** of the REF-1 hybrid (Refs §3.3); Knowledge fixes the rows | Refs + Knowledge |
| REF-CR-3 | **Two-way db-relation inverse projection is best-effort eventual.** Knowledge maintains the forward edge transactionally (the FK); the inverse projection in Refs lags (eventual, [01 §4.2](./01-tech-and-data-model.md)). Refs' reindex-from-source is the drift-correction. | **Confirmation** (the best-effort bidirectional consistency EI-04 §2 names) | Refs |

---

## 3. From Identity & Access (`myelin-identity`)

| # | Ask | Nature | Owner |
|---|---|---|---|
| ID-CR-1 | **The Knowledge ReBAC namespace fragment: page-tree inheritance with overrides + row-level + field-level caveat.** The `space`/`page`/`database_row` definitions ([01 §5](./01-tech-and-data-model.md)) — `page.read = parent_page->read + direct_reader - direct_block`, a `row_reader` relation, and a field-level **ABAC caveat off the hot `list_objects` path**. Id §5 sketched the Knowledge clause; this is the full fragment. | **The P4 namespace declaration** Id's engine compiles (Id §5; the role-bundle catalogue is Id §15 `[OPEN → P4]`) | Knowledge declares → Id owns the engine |
| ID-CR-2 | **`list_objects` `Filter` must be composable over an arbitrary id column for `database_row` at scale.** Knowledge's database views pre-filter via `list_objects(viewer, read, 'database_row')` conjoined into the JSONB query ([02 §4](./02-internals-and-algorithms.md)); the `Filter{set_expr}` must be facet-expressible/push-down, not opaque-id-only (the same S-10 ask Search/Refs made). | **Usage confirmation** of Id §8.2 `Filter` (S-10) | Id |
| ID-CR-3 | **A zookie is returned from `write_tuples` and stamped on `page.acl_zookie`.** A page ACL change (`knowledge.access.*`) writes tuples via Id `write_tuples` and stamps the returned zookie so subsequent collab/read authz cannot read a just-revoked grant stale (the "new enemy" problem, Id §8.4). | **Confirmation** of Id §6/§8.4 (the zookie flow) | Id + Knowledge |

---

## 4. From Storage / GDPR (`BlobStore` + KMS + `myelin-gdpr`)

| # | Ask | Nature | Owner |
|---|---|---|---|
| STOR-CR-1 | **A per-subject DEK for free-text/op-log columns (GD-4) — the crypto-shred lever for history erasure.** Free-text blocks, PII-bearing ops, and the agent trace are encrypted under a **per-subject DEK** so an individual's erasure crypto-shreds exactly their content reachable in the immutable op-log + snapshots + backups ([03 §6](./03-events-contracts-and-glue.md)). | **Confirmation** of GD-4 (Storage §5.1 already names free-text/profile/agent-memory as the per-subject-DEK class) | Storage/GDPR |
| STOR-CR-2 | **Content-addressed snapshot/media blobs in the object tier, crypto-shred on erase.** CRDT snapshots + media are content-addressed (BLAKE3) blobs; erasure of an immutable blob is crypto-shred (destroy the key), not `delete` (STOR-1, Storage §3.2). | **Confirmation** of STOR-1 | Storage |
| STOR-CR-3 | **The cross-seam restore-consistency point must cover OLTP rows ↔ snapshot blobs ↔ op-log ↔ search index ↔ event offsets.** A `doc_op`/`block` row pointing at a missing snapshot blob is silent corruption (STOR-4). The restore-verify drill (ADR-18) must assert Knowledge's row↔blob↔offset consistency. | **Confirmation** of STOR-4 (the event-offset cross-seam cursor) | Storage |
| GDPR-CR-1 | **The named Knowledge free-text-erasure residual write-up (GD-6), co-owned with Legal/DPO.** Full automated free-text PII detection is not perfectly solvable; the residual limit (structured reliable + free-text tooling + documented process) is a named co-owned write-up, not a checkbox ([05 §6](./05-hard-problems.md)). | **A named deliverable** (GD-6 `[OPEN → LEGAL]`) | Knowledge + Legal/DPO |

---

## 5. From Search (`myelin-search`)

| # | Ask | Nature | Owner |
|---|---|---|---|
| SEARCH-CR-1 | **Both block-level and page-level index documents, multilingual, vector-in-v1.** Knowledge declares two `IndexSpec`s (page + significant-block) for jump-to-block ([02 §6](./02-internals-and-algorithms.md)); semantic/vector in v1 for agent RAG; per-language analyzers (EU). | **Confirmation** of Search §3.2/§4.7 (Search accommodates either granularity) | Search |
| SEARCH-CR-2 | **Structured-field queries over flexible JSONB db fields, ACL-aware.** Search's structured/field index must serve filters over Knowledge's `myelin-query` field defs (db row properties) — the same field-defs Issues uses (ADR-06). | **Confirmation** of Search §2.2 (structured shape) | Search |
| SEARCH-CR-3 | **Embeddings purged with their source on `knowledge.*.erased`.** Embeddings of Knowledge content are personal data; erasure purges vectors in lockstep (Search §4.8). | **Confirmation** of Search §4.8 | Search |

---

## 6. From the Agent Fabric (`myelin-agent`) + Durable Workflow (`myelin-flow`)

| # | Ask | Nature | Owner |
|---|---|---|---|
| AG-CR-1 | **Knowledge accepts a content-addressed agent-trace write (AG-7) and registers it as an erasable holder.** The `write_agent_trace` path ([03 §5.2](./03-events-contracts-and-glue.md)) — reuses the block model, no new schema, a `PersonalDataHolder`. | **The required change the Agent Fabric names** (agent-fabric §11 ask 1) — Knowledge's deliverable, seam fixed by the Fabric | Knowledge (this doc) |
| AG-CR-2 | **Knowledge `ToolDef` `requires_approval` defaults + the approver set.** Which Knowledge mutations are HITL-gated by default (consequential/irreversible: publish, edit confidential/PII-bearing) and the `list_subjects` approver set per tool ([03 §5.1](./03-events-contracts-and-glue.md)). | **The P4 product call** the Fabric defers (agent-fabric §12 Q1) | Knowledge + Agent Fabric |
| FLOW-CR-1 | **Durable timers + signals for scheduled/living-doc automations + HITL resume.** Daily-notes, living-doc maintenance ride `DurableExecutor::start` + the timer wheel (Workflow §9.3); the HITL approval-card resume is a durable signal (Workflow §9.4). | **Confirmation** of Workflow §9 | Workflow |

---

## 7. From the Notification system (`myelin-notif`)

| # | Ask | Nature | Owner |
|---|---|---|---|
| NOTIF-CR-1 | **Knowledge declares its `watcher` relation + `define_notif_rule` for mentions/comments/shares/watched-page changes.** Mentions, comment replies, shares, and changes to watched pages/databases feed the **one inbox** (C-9); "watched pages" is the read-fanout watcher set (Notif §8.3). | **The P4 watcher declaration** every subsystem owes (Notif §8.3, NOTIF-rule) | Knowledge declares → Notif |
| NOTIF-CR-2 | **Backend humanisation of `knowledge.*` strings paired with a routable `ArtifactRef`.** "alice mentioned you in <Incident runbook>" humanises at the source via Knowledge's `project` Display mode + Notif templating (NOTIF-1) — never a frontend string map. | **Confirmation** of NOTIF-1 (Knowledge's `project` Display mode, [03 §2.2](./03-events-contracts-and-glue.md)) | Notif + Knowledge |

---

## 8. From the substrate / cross-cutting

| # | Ask | Nature | Owner |
|---|---|---|---|
| SUB-CR-1 | **Knowledge's hot-table flags for the `forward-only-migration` lint.** `block`, `db_row`, `doc_op` are high-write-volume → expand→backfill→contract, no blocking `ALTER` (substrate §9, OQ 2). | **The P4 hot-table flag** every subsystem owes (substrate §13 OQ2) | Knowledge (this doc) |
| SUB-CR-2 | **Knowledge's per-surface shed budgets** (substrate §7, OQ3) — the collab op-stream and hot-document read storms have a distinct load profile from Issues/CI; the per-tenant in-flight caps + human-lane reservation are Knowledge's load-profile call. | **The P4 shed-budget call** (substrate §13 OQ3) | Knowledge |
| SUB-CR-3 | **The WASM compilation target for `myelin-content`.** The one editor render path reuses the Rust `myelin-content` core compiled to WASM client-side (DL §8.1) so `render(parse(md)) === md` holds on identical code — the build toolchain must support a WASM target for the content crate. | **A build-system confirmation** (DL §8.1) | Knowledge + the frontend platform |

**None of these reverses a Phase-3 decision.** Every item is a confirmation of an already-named seam, a P4
declaration the Phase-3 docs explicitly deferred to the subsystem, or a named co-owned write-up (GD-6).

Continue to [`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md).
