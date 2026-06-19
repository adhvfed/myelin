# Knowledge Platform — 03 · Events, Contracts & Glue

> See [`00-overview.md`](./00-overview.md) for framing. This doc owns the **complete `knowledge.*` event
> taxonomy** (under the Bus §6 grammar), the events Knowledge consumes, and **how Knowledge implements every
> FROZEN glue contract**: `ArtifactRef` + the unified `#sub` grammar + `project(ref, viewer)` + `replay`; the
> envelope via the transactional OUTBOX only; Id `check`/`list_objects` (the `SetExpr` push-down) +
> `CaveatContext` + the ReBAC namespace fragment + the zookie; `PersonalDataHolder` (incl. the erasure posture
> by reference); the `ToolDef` registrations (frozen `requires_approval` defaults); and the content-addressed
> agent-trace holder (AG-7). Knowledge is **not spend-bearing in its own right** — agent runs that write to
> Knowledge pass the Agent Fabric's reserve/settle gate (§7).

---

## 1. The complete `knowledge.*` event taxonomy (contract 2.9 — Knowledge completes its list)

Under the Bus §6 grammar (`<subsystem>.<artifact_type>.<event_name>`, lowercase, singular tokens, past-tense).
The **canonical subsystem token is `knowledge`** (CLI noun alias `doc`). Every event carries the canonical
envelope (the names/units anchor: event_id ULID, type, schema_ver, tenant, region, actor, subject ArtifactRef,
aggregate, correlation/causation/depth, contains_personal_data/data_role/visibility/pii_key_ref,
occurred_at/recorded_at, payload) and is emitted **via the outbox only** (§4).

### 1.1 Page / doc lifecycle

| Event | When | Notes |
|---|---|---|
| `knowledge.page.created` | a page (or sub-page/folder) is created | `subject` = the page ArtifactRef |
| `knowledge.page.updated` | **coalesced** semantic page change (debounced; [02 §7](./02-internals-and-algorithms.md)) | **never per-keystroke**; carries a changed-summary, not raw ops |
| `knowledge.page.moved` | re-parented in the tree | emits `page_parent` typed-edge change (§3) |
| `knowledge.page.archived` / `knowledge.page.restored` | soft-delete / restore | |
| `knowledge.page.deleted` | hard-delete | tombstones inbound edges (§6) |
| `knowledge.page.published` / `knowledge.page.unpublished` | public-publish to web | **security-relevant + audit** (GDPR-flagged) |
| `knowledge.doc.updated` | **pointer** event for live-embed invalidation | rides the durable bus as a *pointer*; the op-stream is on the firehose (scope=doc:<id>, [02 §2](./02-internals-and-algorithms.md)) |

### 1.2 Block-level (higher volume, opt-in)

| Event | Notes |
|---|---|
| `knowledge.block.created` / `knowledge.block.updated` / `knowledge.block.deleted` | **internal/opt-in** due to volume; agents subscribe to coarser `knowledge.page.updated`. Block events drive block-level Search reindex. |

### 1.3 Database / schema / rows

| Event | Notes |
|---|---|
| `knowledge.database.created` | a `db_collection` instance |
| `knowledge.database.schema.changed` | a `FieldType` def added/removed → **triggers the derived-projection feeder** to add/drop a measured-hot-facet index (expand→backfill→contract; the >5% threshold, contract 6.3) |
| `knowledge.view.created` / `knowledge.view.updated` | a saved `ViewSpec` |
| `knowledge.row.created` / `knowledge.row.updated` / `knowledge.row.deleted` | `updated` carries a **changed-property delta** (feeds Search + rollup deltas) |
| `knowledge.row.moved` | board-column / manual-order change (the LexoRank `order_key`) |

### 1.4 References (→ Reference Graph)

| Event | Notes |
|---|---|
| `refs.edge.created` / `refs.edge.removed` | emitted by Knowledge as a **producer** when a `mention`/`artifact_ref`/`embed` node is persisted/removed; `subject` = the **source** artifact, payload `{ target, rel, rel_class }` (contract 5.4). **Not coalesced** — a discrete edge fact. The three inline nodes are the **uniform producers** of `refs.edge.created` across Chat/Issues/Knowledge (X-2). **No standalone edge-write API.** |
| `knowledge.page.parent_set` | the page-tree parent typed-edge event Refs mirrors as a `parent` lifecycle edge (TE-7; §3) |
| `knowledge.relation.created` / `knowledge.relation.removed` | the `db_relation` typed-edge events (TE-7 source of truth = Knowledge; Refs projects) |

### 1.5 Comments / mentions (→ Notifications)

| Event | Notes |
|---|---|
| `knowledge.comment.created` / `knowledge.comment.resolved` | inline comments anchored to a block/range; a comment is a sub-artifact (`#comment-<opaqueid>`, X-4) — **same `#comment-`/`#thread-` `#sub` grammar as Chat** (OQ-L: one scheme, two stores) |
| `knowledge.mention.created` | a `mention(Principal)` node → Notifications (the inbox) + a `refs.edge.created` |

### 1.6 Permissions / GDPR / audit / cross-cutting

| Event | Notes |
|---|---|
| `knowledge.access.granted` / `knowledge.access.revoked` | page-tree ACL change → **writes ReBAC tuples via Id `write_tuples` (returns a zookie stamped on `page.acl_zookie`)**; security-relevant + audit |
| `knowledge.subject.export.requested` / `.completed` | DSR export lifecycle (the GDPR holder) |
| `knowledge.subject.erasure.requested` / `.completed` | DSR erasure lifecycle |
| `knowledge.*.erased` | the cross-cutting tombstone (contract 2.7) — emitted when content is erased; live consumers degrade |
| `knowledge.*.snapshot` | the cross-cutting reindex re-emit (contract 2.6) — produced by `replay` (§2.3); also the `resync_required` fallback target for the firehose protocol (OQ-J) |

`schema_ver` is per-type; envelope evolution is forward-only with registered upcasters (contract 2.8). Payloads
are references-not-payloads, so the common case (`contains_personal_data = false`) survives erasure untouched.

### 1.7 Events Knowledge CONSUMES (the `EventHandler` consumer template, whitelisted subjects — never `*`, contract 2.4)

| Source event | Why Knowledge reacts |
|---|---|
| `identity.member.removed` / `identity.principal.erased` | reassign authorship to the pseudonymous "Deleted user" (the `<pseudonym>@<tenant>.noreply` map shred, contract 4.8); recompute view membership/ACL projections; trigger erasure participation (§6) |
| `identity.team.changed` / `identity.permission.*` | recompute the page-tree → tuple projection ([01 §5](./01-tech-and-data-model.md)) |
| `issue.issue.updated` / `issue.issue.closed`, `ci.run.passed` / `ci.run.failed`, `git.commit.pushed`, `chat.message.created` | refresh **embedded live views** + mention previews; update `artifact_ref` field properties; drive agent-maintained **living documents** |
| `refs.edge.removed` / `*.erased` (on a referenced artifact) | **tombstone the reference, degrade rendering gracefully** via the 4-step ladder (§2.1) — no dangling crash |
| Scheduled / durable-workflow timers (`myelin-flow`, contract 9.3) | "create today's daily-note from template"; "incident opened → create incident doc from runbook template" — as `SCHEDULE_AND_RUN_JOB` jobs (§5) |
| Agent-fabric delivery (`EffectApi` apply of a Knowledge tool) | apply agent-authored edits **via the collab protocol** with agent attribution ([02 §9](./02-internals-and-algorithms.md)) |

Reactive consumers subscribe to **curated Signals** (contract 3.1), not the raw `evt.*` firehose, except the
indexing feeder which is an excepted infra consumer.

---

## 2. The glue contracts (`ArtifactRef` + `#sub` + `project` + `replay`)

### 2.1 ArtifactRef + the unified `#sub` grammar (contract 5.1 / 5.7 — Δ4)

Every Knowledge artifact is addressable as `myelin://<tenant>/knowledge/<type>/<id>[#<sub>]`. Types: `page`,
`block`, `database`, `row`, `view`. The `<sub>` kinds are the **frozen unified vocabulary** (contract 5.7,
X-4); Knowledge mints these kinds with **stable opaque ids** (the stability obligation is Knowledge's):

| Sub-artifact | `#sub` kind (frozen) | Stability guarantee |
|---|---|---|
| A block in a page | `b<opaqueid>` (e.g. `#b9`) | **`block.block_id` is stable across edits/moves** — an embed of "block b9 of page 7c2" never dangles when the block is reordered |
| A heading anchor | `h<opaqueid>` (**no hyphen** — Δ4) | the heading block's stable id (jump-to-section links survive retitling) |
| A row in a db | `row-<opaqueid>` | stable `db_row.row_id` |
| A field within a row | `field-<opaqueid>` (**new** — Δ4) | a db field within a row/issue-as-row (Knowledge db) |
| A comment | `comment-<opaqueid>` | immutable comment id (same grammar as Chat — OQ-L) |
| A thread root | `thread-<opaqueid>` | a comment-thread root (shared with Chat) |

**Resolution runs the one frozen 4-step tombstone ladder** (contract 5.2 / 5.7, X-4) — Refs stores the full
sub-URN AND the `#sub`-stripped root, so a broken sub-anchor still resolves to the parent. For a Knowledge
sub-anchor, the owner's `project(ref, viewer)` sub-anchor resolver returns:

```
1. permission: check(viewer, read, root)  → Deny ⇒ Tombstone{reason: denied}     (never leak)
2. root resolve: the page exists?          → No   ⇒ Tombstone{reason: root_gone}
3. sub resolve via Knowledge's sub-anchor resolver:
     LIVE      → the block/row/comment projection
     MOVED     → projection + flag `moved`        (block moved in the tree — the block_id still resolves)
     OUTDATED  → projection(partial) + flag `outdated`   (an edited block whose anchored range shifted)
     GONE      → Tombstone{reason: sub_gone, root}  // the page resolves; the block is dead, embed shows the page
4. ERASED (any level): Tombstone{reason: erased}   // pseudonym-/crypto-shred made it unrenderable
```

A tombstone **always carries the root** so an embed degrades to "this referenced <Incident runbook> (the
specific block is no longer available)" rather than vanishing. (Knowledge does not need Git's content-anchored
line-range fingerprints — its anchors are stable opaque block ids, so the `MOVED` case is a tree move, not a
3-way diff match.)

### 2.2 `project(ref, viewer)` (contract 5.6 — required on every subsystem)

The *only* way Refs/Search/Notif read about a Knowledge artifact (no cross-DB). Per-viewer, pre-permission-
checked:

```rust
fn project(ref_: &ArtifactRef, viewer: &Principal) -> Projection | Tombstone {
    if Id.check(viewer, read, ref_, zookie) != Allow { return Tombstone::no_access(); }   // defence in depth
    match ref_.type {
        page  => Projection { title, state: published?archived?, icon, render_hint: "page",   sub_anchor: ref_.sub },
        block => Projection { title: parent_page.title, state, icon, render_hint: "block",     sub_anchor: Some(block_id) },
        database/row/view => Projection { title, state, icon, render_hint: <kind> },
    }
}
```

- Returns `{ title, state, icon, render_hint, sub_anchor? }` (contract 5.6 frozen shape). A confidential page
  degrades to a **tombstone** for a viewer lacking `read` — never leaks title/state.
- **Display mode** = the humanisation projection Notif uses (contract 7.3 / NOTIF-1): a routable `ArtifactRef`
  + a humanised string, so "alice mentioned you in <Incident runbook>" renders per-viewer for every consumer.
  This feeds the **sole `humanise` templating surface** (OQ-L) — Knowledge registers no second template engine.

### 2.3 `replay(scope, since)` (contract 2.6 — reindex-from-source, sub-artifact-granular)

Knowledge implements `replay` so Search/Refs/Notif/OLAP rebuild **from source via the live consumer path** —
Knowledge's stores are never read directly for recovery:

```rust
fn replay(scope: Scope, since: Cursor) -> emits *.snapshot via the OUTBOX through the live bus {
    // scope = a tenant | a space | a PAGE SUBTREE (block-granular) | a database | all
    for each page/row/db in scope since the cursor:
        emit knowledge.page.snapshot   { the full block-tree state at version }   // BLOCK granularity
        emit knowledge.row.snapshot    { the row props + relations }
        emit refs.edge.snapshot        { each mention/artifact_ref/embed/parent/relation edge }
        // snapshot event_id is DETERMINISTIC from (aggregate, version) → idempotent re-run (contract 2.6)
}
```

- **Sub-artifact-granular** (contract 2.6: "KN page-subtree at block granularity"): page snapshots carry
  block-level granularity so Search re-indexes blocks and Refs re-derives sub-artifact edges. It is also the
  **`resync_required` fallback** the firehose protocol falls back to (OQ-J).
- **One code path**: the same outbox→bus→consumer path as steady state, so cold rebuild and live ingestion
  cannot drift (KD-6).
- **The TE-7 drift-correction**: if Refs' projection disagrees with Knowledge's `db_relation`/`page_parent`
  typed tables (the authority), a scoped `replay` re-emits the typed snapshots and Refs reconverges; the typed
  table always wins.

---

## 3. The TE-7 typed-edge mirror + the FROZEN `list_objects`/`check` glue (contracts 5.5, 4.2, 4.3)

### 3.1 The TE-7 typed-edge mirror (contract 5.5, REF-1)

Knowledge owns the typed relation tables that are the **source of truth** for its lifecycle/semantic edges
([01 §4.3](./01-tech-and-data-model.md)); Refs holds a rebuildable projection. The same transaction that writes
a typed row emits the typed lifecycle event:

- `db_relation` → `knowledge.relation.created`/`.removed` → Refs projects a `rel_class='lifecycle'`
  `relates`/`rollup_source` edge.
- `page_parent` → `knowledge.page.parent_set` → Refs projects a `parent` lifecycle edge (and the inverse
  `child` — Refs fixes the inverse pairing, contract 5.5).

The typed table is truth (not the URN string) because a rollup aggregating over a relation, and page-tree
permission inheritance following `parent_page`, need **referential integrity + transactional guards** only
Knowledge's DB gives (the FK). Refs is the fast cross-subsystem *traverser*.

### 3.2 `list_objects` + `SetExpr` + `CaveatContext` (contracts 4.3, 4.2 — the frozen pre-filter)

Knowledge calls `list_objects(viewer, read, 'database_row'|'page', zookie)` and conjoins the returned
`Filter{set_expr, zookie}` into every list/board/view/search query via the **frozen `SetExpr` lowering over
its own id column** ([02 §4.1](./02-internals-and-algorithms.md)). Row-level visibility lowers to
`InRelation { relation: row_reader, via_column: db_row.id }` (a JOIN against the per-tenant authz reverse
index); field-level hiding is the `CaveatContext{object, field, attrs}` caveat at `check`-time on the already-
filtered rows, **off the hot path** ([01 §5.1](./01-tech-and-data-model.md)). No N+1, no post-filter.

### 3.3 The zookie new-enemy guard (contract 4.6 / 4.10)

A page ACL change (`knowledge.access.*`) writes tuples via `write_tuples([Δtuple]) → zookie` and stamps the
returned zookie on `page.acl_zookie`. Subsequent collab/read authz pass that zookie so a just-revoked grant
cannot be read stale (the "new enemy" problem); the authz reverse index honours the zookie revision watermark.

---

## 4. The envelope via the transactional OUTBOX only (contract 2.2)

Every Knowledge state change emits **only** via `OutboxTx::emit(draft, cause)` in the **same DB transaction**
as the state change. There is **no fire-and-forget publish** (the `no-raw-publish` lint). The relay drains the
outbox (`FOR UPDATE SKIP LOCKED`), broker-dedups on the ULID `event_id`, dead-letters after bounded retries.

- **Causality correct-by-construction** (BUS-5): a reaction (e.g. a living-doc update caused by
  `issue.issue.updated`) calls `emit(draft, cause = Some(incoming))` so `correlation_id` carries,
  `causation_id` = the incoming event, `depth = +1`. The agent loop guards read these (AG-6) — a human or agent
  cannot typo into a loop.
- **The aggregate is the doc / row / db** (the ordering partition, contract 2.3): per-doc ordering is
  preserved; different docs fan out in parallel.
- **Coalescing happens before emit** ([02 §7](./02-internals-and-algorithms.md)): the durable bus gets semantic
  events + the `doc.updated` pointer, never raw ops.

---

## 5. Agent tools + the AG-7 content-addressed trace holder

### 5.1 `ToolDef` registrations (contract 8.1 — the FROZEN `requires_approval` defaults)

Knowledge registers typed `ToolDef`s into the one catalogue (MCP-exposable); each declares input schema,
required caps, effect kind, side-effecting, `requires_approval`, `exposed_over_mcp`. The **`requires_approval`
defaults are now frozen jointly with the Fabric** (X-6, contract 8.1: "KN publish/confidential = yes;
draft/comment = no"):

| Tool | `effect_kind` | `side_effecting` | `requires_approval` (frozen default) | Notes |
|---|---|---|---|---|
| `knowledge.search` | `read` | false | no | RAG over the corpus, permission-filtered (the `Filter` conjoin) |
| `knowledge.page.read` / `knowledge.page.summarise` | `read` | false | no | a permission-filtered projection / a draft, not an edit |
| `knowledge.page.create` / `knowledge.page.append` / `knowledge.comment` / `knowledge.draft` | `mutate` | true | **no** (reversible draft/comment) | via `EffectApi` → public endpoint → collab apply |
| `knowledge.row.upsert` | `mutate` | true | no / **yes** for a PII-bearing database | set a row's props (incl. an `artifact_ref`) |
| `knowledge.page.publish` / `edit(confidential_page)` | `mutate` | true | **yes** (frozen: publishing/confidential edits are consequential) | the approver set = `list_subjects(object, manage)` |
| `knowledge.page.turn_into_issues` | `mutate` | true | **yes** | the flagship "turn action items into issues" (cross-subsystem effect — **inherits Issues' default** where it lands, X-6) |

- **Side-effecting tools go through `EffectApi::apply`** (contract 8.2, plan-then-apply): the edit is applied
  through the collab protocol with "suggested by agent" attribution; the **four uniform guarantees** (X-6,
  [02 §9](./02-internals-and-algorithms.md)) hold by construction.
- **HITL withhold** (contract 8.2 / AG-8): a gated tool not in the approved set returns `Denied` and **does not
  mutate**; the approval card surfaces in **Chat** (with a live cost estimate) and **resumes the run via a
  durable signal** — the per-effect `idem_key` rule (`card_id` single, `card_id:<effect_idx>` for a batch/
  partial approval, contract 9.1/9.4, OQ-F) makes "a double-click is one approval" and "a partial approval is
  well-defined" both true.
- **Denied = ordinary tool error** (an effect outside `agent.policy ∩ delegation ∩ tenant.policy` returns
  `Denied` — no privileged fallback).

### 5.2 The content-addressed agent-trace holder (AG-7 — contract 8.8)

Knowledge **accepts a content-addressed write of an agent execution trace** and registers it as an erasable
holder (K8):

```rust
fn write_agent_trace(run_id, content: ContentAddressed<Document>, actor: AgentPrincipal) -> ArtifactRef {
    // content-addressed (BLAKE3, the object tier), immutable; REUSES the block model — no new schema (contract 8.8)
    // returns run.trace_ref (an ArtifactRef the agent run records)
    // registered as a PersonalDataHolder (K8): the conversation (system context, tool i/o, surfaced reasoning)
}
```

- **Distinct from the tamper-evident audit log** (contract 10.6): the audit log is GDPR/Audit's tamper-evident
  holder; the trace is the human-readable narrative. Three distinct holders (telemetry / audit / trace).
- **An erasable holder**: residency-pinned, per-subject crypto-shred-capable, erasable. Erasing a subject
  crypto-shreds their trace content; attribution falls back to the opaque pseudonym (KD-12).

---

## 6. PersonalDataHolder — locate / export / rectify / restrict / erase (contract 10.1)

Knowledge is the **hardest GDPR surface** in Myelin and an exhaustive-list holder (auto-registered by the
harness, contract 1.4). It implements the full contract:

| Op | Behaviour |
|---|---|
| **`locate(subject)`** | structured personal data reliably: `principal` field values, `mention(Principal)` nodes, author/edit attribution (`created_by`/`edited_by`), `db_row` person props, comment authorship, trace authorship; plus **free-text matches** (best-effort, via Search) flagged for review. |
| **`export(subject)`** | the **lossless JSON export** scoped to the subject (Art. 20 portability). Knowledge's Export service is the mechanism. |
| **`rectify(subject, change)`** | correct a structured value (a person field, an attribution); also the **best-effort `rectify`/tombstone of a specific free-text span** where the subject identifies it (the residual posture, §8 below). |
| **`restrict(subject)`** | the restriction flag: a restricted subject is excluded from **indexing, agent-use (RAG), analytics, and notifications** — Knowledge stops emitting the subject's content to Search/Agents/OLAP/Notif and marks its rows/blocks restricted. (The restriction flag flows into OLAP per contract 11.6.) |
| **`erase(subject)`** | **purge/crypto-shred/pseudonymise, never hide** — see §6.1. |

### 6.1 The erasure algorithm — the structural floor + the platform posture (contract 10.9, X-7 — Δ8)

The structural floor is **fully built** and is the primary mechanism (contract 10.9 §"structural floor"):

1. **Pseudonym-map shred (identity erasure).** Attribution is the **opaque `principal_id`**, never PII; the
   person↔pseudonym map (grammar `<pseudonym>@<tenant>.noreply`, contract 4.8) is the erasable record. Erasing
   the map makes the id un-resolvable to a human; Knowledge needs no per-edge mutation in the common case (DSR
   step 1).
2. **Per-subject DEK crypto-shred (self-authored content).** Free-text blocks and ops holding the subject's PII
   are encrypted under a **per-subject DEK** (`<class> = subject:<id>`, contract 11.4). Erasure destroys the
   key → the ciphertext in the op-log, snapshots, **and backups** becomes unrecoverable **without rewriting the
   merge-dependent op-log** (you cannot delete a CAS/CRDT op; you destroy its key). **One DEK per (subject,
   tenant)**, applied selectively only to PII-bearing classes (CR-I) — so the tenant key count is O(subjects
   with inline PII), not O(blocks).
3. **Structural holder coverage + tombstoning.** Mentions/backlinks tombstone to a neutral placeholder (Refs
   flips `tombstoned=true` via the `*.erased` consumer); the Search + vector index purges in lockstep
   (embeddings of personal data are personal data); published/public pages unpublish + CDN/cache purge. The
   `restrict` suppression covers the pending-erasure window. Returns a receipt hash-linked into the audit log.

**The residual (handled per the platform posture, contract 10.9 — instantiated by reference, NOT restated):**
third-party free-text PII (a person's name typed by *someone else* into that other person's content) is
encrypted under the *author's* DEK, not the subject's, so the subject's erasure does not crypto-shred it. Per
the **ONE platform-wide posture (X-7)**: this residual is handled under a documented lawful-basis limit —
best-effort `rectify`/tombstone of the specific span where the subject identifies it, plus the standing
structural guarantee that the residual is **never indexed, never agent-readable, never in analytics for a
restricted subject** (the `restrict` suppression). `[OPEN — LEGAL]`: counsel/DPO ratify the residual basis in
**one statement** (10.9), not a Knowledge-specific write-up. The structural floor ships regardless. See
[05 §6](./05-hard-problems.md) and [06 §8](./06-reconciliation-compliance.md).

**Drill (KD-4):** erase a subject; assert structured PII purged/pseudonymised, free-text under the per-subject
DEK crypto-shredded (key destroyed → unrecoverable in op-log/snapshots/backups), embeddings purged, backlinks
tombstoned → **0 recoverable structured PII incl. vectors**; the residual covered by the platform posture.

---

## 7. Spend-bearing work (reserve/settle)

Knowledge is **not spend-bearing in its own right** (no model calls, no CI runs originate here). When an
**agent** writes to Knowledge, the run passes the Agent Fabric's universal **reserve/settle** gate (contract
11.7) before any tool executes — the gate is the Fabric's bookends, Knowledge's tools are ordinary effects
metered through `EffectApi`. So "no balance → no agent write" is uniformly true, enforced at the Fabric. A
scheduled living-doc automation is an **agent run** dispatched via `SCHEDULE_AND_RUN_JOB` (contract 9.2, OQ-F),
so the same gate applies (reserve at dispatch). v1 embeds no Knowledge-native spend.

Continue to [`04-views-cli-and-api.md`](./04-views-cli-and-api.md).
