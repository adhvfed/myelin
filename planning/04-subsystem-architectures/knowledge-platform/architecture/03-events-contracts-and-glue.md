# Knowledge Platform — 03 · Events, Contracts & Glue

> See [`00-overview.md`](./00-overview.md) for framing. This doc owns the **complete `knowledge.*` event
> taxonomy** (under the Bus §6 grammar), the events Knowledge consumes, and **how Knowledge implements every
> glue contract**: `ArtifactRef` + `project(ref, viewer)` + `replay(scope, since)`; the envelope via the
> transactional OUTBOX only; Id `check`/`list_objects` + the ReBAC namespace fragment; `PersonalDataHolder`
> (locate/export/rectify/restrict/erase + the restriction flag); the `ToolDef` registrations; and the
> content-addressed agent-trace holder (AG-7). Knowledge is **not spend-bearing in its own right** — agent
> runs that write to Knowledge pass the Agent Fabric's reserve/settle gate (§7).

---

## 1. The complete `knowledge.*` event taxonomy (TE-10 completion)

Under the Bus §6 grammar (`<subsystem>.<artifact_type>.<event_name>`, lowercase, singular tokens, past-tense
verbs). The **canonical subsystem token is `knowledge`** (Bus §6.2; CLI noun alias `doc`). Every event
carries the canonical envelope (event_id ULID, type, schema_ver, tenant, region, actor, subject ArtifactRef,
correlation/causation/depth, contains_personal_data, data_role, visibility, pii_key_ref, occurred_at) and is
emitted **via the outbox only** (§4).

### 1.1 Page / doc lifecycle

| Event | When | Notes |
|---|---|---|
| `knowledge.page.created` | a page (or sub-page/folder) is created | `subject` = the page ArtifactRef |
| `knowledge.page.updated` | **coalesced** semantic page change (debounced; [02 §7](./02-internals-and-algorithms.md)) | **never per-keystroke**; carries a changed-summary, not raw ops |
| `knowledge.page.moved` | re-parented in the tree | emits `page_parent` typed-edge change (§3) |
| `knowledge.page.archived` / `knowledge.page.restored` | soft-delete / restore | |
| `knowledge.page.deleted` | hard-delete | tombstones inbound edges (§6) |
| `knowledge.page.published` / `knowledge.page.unpublished` | public-publish to web | **security-relevant + audit** (GDPR-flagged, [05 §6](./05-hard-problems.md)) |
| `knowledge.doc.updated` | **pointer** event for live-embed invalidation (Bus §6.3) | rides the durable bus as a *pointer*; the op-stream is on the firehose ([02 §2](./02-internals-and-algorithms.md)) |

### 1.2 Block-level (higher volume, opt-in)

| Event | Notes |
|---|---|
| `knowledge.block.created` / `knowledge.block.updated` / `knowledge.block.deleted` | **internal/opt-in** due to volume (deep-dive §7.1); agents subscribe to coarser `knowledge.page.updated`. Block events drive block-level Search reindex ([02 §6](./02-internals-and-algorithms.md)). |

### 1.3 Database / schema / rows

| Event | Notes |
|---|---|
| `knowledge.database.created` | a `db_collection` instance |
| `knowledge.database.schema.changed` | a field def added/removed/typed → **triggers the derived-projection feeder** to add/drop a hot-facet index (expand→backfill→contract, [02 §4](./02-internals-and-algorithms.md)) |
| `knowledge.view.created` / `knowledge.view.updated` | a saved view (query-AST projection) |
| `knowledge.row.created` / `knowledge.row.updated` / `knowledge.row.deleted` | `updated` carries a **changed-property delta** (feeds Search + rollup deltas) |
| `knowledge.row.moved` | board-column change (mirrors issue status-change) |

### 1.4 References (→ Reference Graph)

| Event | Notes |
|---|---|
| `refs.edge.created` / `refs.edge.removed` | emitted by Knowledge as a **producer** when a `mention`/`artifact_ref`/`embed` node is persisted/removed; `subject` = the **source** artifact, payload `{ target, rel, rel_class }` (Bus §6.3; Refs §5.1). **Not coalesced** — a discrete edge fact ([02 §7](./02-internals-and-algorithms.md)). |
| `knowledge.page.parent_set` | the page-tree parent typed-edge event Refs mirrors as a `parent` lifecycle edge (TE-7; §3) |
| `knowledge.relation.created` / `knowledge.relation.removed` | the `db_relation` typed-edge events (TE-7 source of truth = Knowledge; Refs projects) |

### 1.5 Comments / mentions (→ Notifications)

| Event | Notes |
|---|---|
| `knowledge.comment.created` / `knowledge.comment.resolved` | inline comments anchored to a block/range; comment is a sub-artifact (`#comment-12`) |
| `knowledge.mention.created` | a `mention(Principal)` node → Notifications (the inbox) + a `refs.edge.created` |

### 1.6 Permissions / GDPR / audit / cross-cutting

| Event | Notes |
|---|---|
| `knowledge.access.granted` / `knowledge.access.revoked` | page-tree ACL change → **writes ReBAC tuples via Id `write_tuples` (returns a zookie stamped on `page.acl_zookie`)**; security-relevant + audit |
| `knowledge.subject.export.requested` / `.completed` | DSR export lifecycle (the GDPR holder) |
| `knowledge.subject.erasure.requested` / `.completed` | DSR erasure lifecycle |
| `knowledge.*.erased` | the cross-cutting tombstone (Bus §4.8) — emitted when content is erased; live consumers degrade |
| `knowledge.*.snapshot` | the cross-cutting reindex re-emit (Bus §4.9) — produced by `replay` (§3 below) |

`schema_ver` is per-type; envelope evolution is forward-only with registered upcasters (Bus §4.10). Each
event's payload is references-not-payloads (IDs/`ArtifactRef`s), so the common case (`contains_personal_data
= false`) survives erasure untouched (Bus §4.8).

### 1.7 Events Knowledge CONSUMES (the `EventHandler` consumer template, whitelisted subjects — BUS-3)

| Source event | Why Knowledge reacts |
|---|---|
| `identity.member.removed` / `identity.principal.erased` | reassign authorship to the pseudonymous "Deleted user"; recompute view membership/ACL projections; trigger erasure participation (§6) |
| `identity.team.changed` / `identity.permission.*` | recompute the page-tree → tuple projection ([01 §5](./01-tech-and-data-model.md)) |
| `issue.issue.updated` / `issue.issue.closed`, `ci.run.passed` / `ci.run.failed`, `git.commit.pushed`, `chat.message.created` | refresh **embedded live views** + mention previews; update `artifact_ref` field properties; drive agent-maintained **living documents** |
| `refs.edge.removed` / `*.erased` (on a referenced artifact) | **tombstone the reference, degrade rendering gracefully** (no dangling crash; deep-dive §2.6) |
| Scheduled / durable-workflow timers (`myelin-flow`) | "create today's daily-note from template"; "incident opened → create incident doc from runbook template" |
| Agent-fabric delivery (`EffectApi` apply of a Knowledge tool) | apply agent-authored edits **via the collab protocol** with agent attribution ([02 §9](./02-internals-and-algorithms.md)) |

Reactive consumers subscribe to **curated Signals**, not the raw `evt.*` firehose (BUS-4), except the
indexing feeder which is an excepted infra consumer.

---

## 2. The three glue contracts

### 2.1 ArtifactRef (contract 1 — addressing, ADR-13.1)

Every Knowledge artifact is addressable as `myelin://<tenant>/knowledge/<type>/<id>[#sub]` (Bus §6.2 token
table). Types: `page`, `block`, `database`, `row`, `view`. **Sub-artifact `#sub` scheme (the P4 obligation,
Refs §3.5 / substrate §13 Q4 — DECIDED here for Knowledge):**

| Sub-artifact | `#sub` token | Stability guarantee |
|---|---|---|
| A block in a page | `#b<block_id>` (e.g. `#b9`) | **the `block.block_id` is stable across edits/moves** ([01 §2](./01-tech-and-data-model.md)) — so an embed of "block b9 of page 7c2" never dangles when the block is reordered |
| A comment | `#comment-<id>` | stable comment id |
| A row in a db | `#row-<id>` | stable `db_row.row_id` |
| A heading anchor | `#h-<block_id>` | the heading block's stable id (jump-to-section links survive retitling) |

`myelin://acme-eu/knowledge/block/PAGE-7c2#b9` resolves to the page's projection augmented with the
block anchor; if the block was removed but the page survives, `resolve` returns a **partial projection** (the
page + a "this block was removed" marker), not a hard 404 (Refs §3.5 graceful degradation).

### 2.2 `project(ref, viewer)` (contract 2 — the projection API, ADR-13.1 / Refs §5.2)

A **required contract every subsystem implements** — the *only* way Refs/Search/Notif read about a Knowledge
artifact (no cross-DB). Per-viewer, pre-permission-checked:

```rust
fn project(ref_: &ArtifactRef, viewer: &Principal) -> Projection | Tombstone {
    // 1. permission check (defence in depth — Refs also checks)
    if Id.check(viewer, view, ref_, zookie) != Allow { return Tombstone::no_access(); }
    // 2. current rendered projection, per type
    match ref_.type {
        page  => Projection { title, state: published?archived?, icon, render_hint: "page",
                              sub_anchor: ref_.sub },              // for unfurls/embeds
        block => Projection { title: parent_page.title, state, icon, render_hint: "block",
                              sub_anchor: Some(block_id) },        // augmented with the sub-anchor
        database/row/view => Projection { title, state, icon, render_hint: <kind> },
    }
}
```

- Returns `{ title, state, icon, render_hint, sub_anchor? }` (Refs §5.2 frozen shape). A confidential page
  degrades to a **tombstone** for a viewer lacking `read` — never leaks title/state.
- This is what makes a doc embed of "an issue board" and another subsystem's embed of "a Knowledge page" both
  live and permission-correct: each calls the *owner's* `project` (ADR-13).
- The **Display mode** of `project` returns the humanisation projection Notif uses (NOTIF-1): a routable
  `ArtifactRef` + a humanised string, so "alice mentioned you in <Incident runbook>" renders correctly for
  every consumer (Refs §5.2, Notif §3.3).

### 2.3 `replay(scope, since)` (contract 3 — reindex-from-source, Bus §4.9 / §5.6)

Knowledge implements `replay` so Search/Refs/Notif/OLAP rebuild **from source via the live consumer path**
— Knowledge's stores are never read directly for recovery (SEARCH-1/REF-4):

```rust
fn replay(scope: Scope, since: Cursor) -> emits *.snapshot via the OUTBOX through the live bus {
    // scope = a tenant | a space | a page subtree | a database | all
    for each page/row/db in scope since the cursor:
        emit knowledge.page.snapshot   { the full block tree state at version }   // via OutboxTx
        emit knowledge.row.snapshot    { the row props + relations }
        emit refs.edge.snapshot        { each mention/artifact_ref/embed/parent/relation edge }
        // snapshot event_id is DETERMINISTIC from (aggregate, version) → idempotent re-run (Bus §4.9)
}
```

- **Sub-artifact granular** (the Search ask, search §9.2): page snapshots carry block-level granularity so
  Search re-indexes blocks and Refs re-derives sub-artifact edges.
- **One code path**: the same outbox→bus→consumer path as steady state, so cold rebuild and live ingestion
  cannot drift (the reindex-from-cold parity drill, [07](./07-drills-and-open-questions.md)).
- **It is also the TE-7 drift-correction**: if Refs' `lifecycle` projection ever disagrees with Knowledge's
  `db_relation`/`page_parent` typed tables (the authority, REF-1), a scoped `replay` re-emits the typed
  snapshots and Refs reconverges; the typed table always wins.

---

## 3. The TE-7 typed-edge mirror (Knowledge's half, REF-1 / ISS-1 sibling)

Knowledge owns the **typed relation tables that are the source of truth** for its lifecycle/semantic edges
([01 §4.2](./01-tech-and-data-model.md)); Refs holds a rebuildable projection (REF-1). The same transaction
that writes a typed row emits the typed lifecycle event:

- `db_relation` (two-way relation field) → `knowledge.relation.created`/`.removed` → Refs projects a
  `rel_class='lifecycle'` `relates`/`rollup_source` edge.
- `page_parent` (page → sub-page) → `knowledge.page.parent_set` → Refs projects a `parent` lifecycle edge
  (the inverse `child` direction too — Refs fixes the inverse pairing, Refs §3.3).

**Why the typed table is truth, not the URN string** (REF-1): a rollup that aggregates over a relation, and a
page-tree permission inheritance that follows `parent_page`, need **referential integrity + transactional
guards** that only Knowledge's DB gives (the FK in [01 §4.2](./01-tech-and-data-model.md)). Refs is the fast
cross-subsystem *traverser* ("everything blocking this release across all five subsystems") — one Refs query,
not a synchronous fan-out (the head-of-line cost EI-02 §3 forbids). Knowledge fixes the rows; Refs fixes the
`rel` vocabulary + the mirror discipline (Refs §3.3).

---

## 4. The envelope via the transactional OUTBOX only (BUS-2)

Every Knowledge state change emits **only** via `OutboxTx::emit(draft, cause)` in the **same DB transaction**
as the state change (substrate §2.1; BUS-2). There is **no fire-and-forget publish** (the `no-raw-publish`
lint, substrate §2.11). The relay drains the outbox (`FOR UPDATE SKIP LOCKED`), broker-dedups on the ULID
`event_id`, and dead-letters after bounded retries (Bus §4.1).

- **Causality correct-by-construction** (BUS-5): a reaction (e.g. a living-doc update caused by
  `issue.issue.updated`) calls `emit(draft, cause = Some(incoming))` so `correlation_id` carries,
  `causation_id` = the incoming event, `depth = +1`. The agent loop guards read these (AG-6) — a human or
  agent **cannot typo into a loop**.
- **The aggregate is the doc / row / db** (the ordering partition, Bus §2.2): per-doc ordering is preserved;
  different docs fan out in parallel.
- **Coalescing happens before emit** ([02 §7](./02-internals-and-algorithms.md)): the durable bus gets
  semantic events + the `doc.updated` pointer, never raw ops.

---

## 5. Agent tools + the AG-7 content-addressed trace holder

### 5.1 `ToolDef` registrations (ADR-08.4 / `ToolSurface::register_tool`)

Knowledge registers typed `ToolDef`s into the one catalogue (agent-fabric §6); each declares input JSON
schema, required caps, effect kind, side-effecting flag, `requires_approval`, `exposed_over_mcp`:

| Tool | `effect_kind` | `side_effecting` | `requires_approval` (default) | Notes |
|---|---|---|---|---|
| `knowledge.search` | `read` | false | false | RAG over the knowledge corpus (permission-filtered via Search) |
| `knowledge.page.read` | `read` | false | false | a permission-filtered page projection |
| `knowledge.page.create` | `mutate` | true | false (non-confidential space) | via `EffectApi` → public endpoint → collab apply |
| `knowledge.page.append` | `mutate` | true | false / **true** if the page is published or confidential | append a block subtree |
| `knowledge.page.summarise` | `read` | false | false | read-only; produces a draft, not an edit |
| `knowledge.row.upsert` | `mutate` | true | false / **true** for a PII-bearing database | set a row's props (incl. an `artifact_ref`) |
| `knowledge.page.turn_into_issues` | `mutate` | true | **true** | the flagship "turn action items into issues" (calls Issues' tools) — gated by default for consequential output |

- **Side-effecting tools go through `EffectApi`** (plan-then-apply, agent-fabric §5.2), which calls
  Knowledge's **public endpoint** as the agent principal (same gateway, no carve-out) → the edit is applied
  **through the collab protocol** with "suggested by agent" attribution ([02 §9](./02-internals-and-algorithms.md)).
- **`requires_approval` defaults are a Knowledge product call** (agent-fabric §12 Q1): gated by default for
  irreversible/consequential mutations (publishing, editing a confidential/PII-bearing artifact). The HITL
  card surfaces in **Chat** (agent-fabric §5.3), shows the pending action + a live cost estimate, and
  approval **resumes** the run via a durable signal.
- **Denied = ordinary tool error** (AG-5): an effect outside `agent.policy ∩ delegation ∩ tenant.policy`
  returns `Denied` to the loop — no privileged fallback.

### 5.2 The content-addressed agent-trace holder (AG-7 — the required acceptance)

Knowledge **accepts a content-addressed write of an agent execution trace** (AG-7; agent-fabric §11 ask 1)
and registers it as an erasable holder ([01 §6](./01-tech-and-data-model.md) K8):

```rust
// Knowledge exposes a write path for an agent-authored trace; the trace IS a Knowledge document (reuses myelin-content).
fn write_agent_trace(run_id, content: ContentAddressed<Document>, actor: AgentPrincipal) -> ArtifactRef {
    // content-addressed (BLAKE3, the object tier), immutable; reuses the block model — no new schema (EI-03 §4.4)
    // returns run.trace_ref (an ArtifactRef the agent run records)
    // registered as a PersonalDataHolder (K8): it holds the conversation (system context, tool i/o, surfaced reasoning)
}
```

- **Distinct from the tamper-evident audit log** (agent-fabric §4.5): the audit log is GDPR/Audit's complete
  tamper-evident holder; the trace is the human-readable narrative. **Three distinct holders** (telemetry /
  audit / trace).
- **An erasable holder** (`PersonalDataHolder`, §6): the trace holds personal data → residency-pinned,
  per-subject crypto-shred-capable, erasable. Erasing a subject crypto-shreds their trace content; attribution
  falls back to the opaque pseudonym (the AG-7 erasure drill, agent-fabric D-10).
- **No new schema** — it reuses the block model (EI-03 §4.4); Knowledge accepts the agent as an author.

---

## 6. PersonalDataHolder — locate / export / rectify / restrict / erase (ADR-12.1 / GD-3)

Knowledge is the **hardest GDPR surface** in Myelin (deep-dive §8) and an exhaustive-list holder
(auto-registered by the harness, substrate §3.4). It implements the full contract:

| Op | Behaviour |
|---|---|
| **`locate(subject)`** | find structured personal data reliably: `person` field values, `mention(Principal)` nodes, author/edit attribution (`created_by`/`edited_by`), `db_row` person props, comment authorship, trace authorship; plus **free-text matches** (best-effort, via Search) flagged for review. |
| **`export(subject)`** | the **lossless JSON export** scoped to the subject — all KB content authored by / about the subject (Art. 20 portability; deep-dive §2.10). Knowledge's Export service is the mechanism. |
| **`rectify(subject, change)`** | correct a structured value (a person field, an attribution). |
| **`restrict(subject)`** | **the restriction flag (SUB-X obligation)**: a restricted subject is excluded from **indexing, agent-use (RAG), analytics, and notifications** — Knowledge stops emitting the subject's content to Search/Agents/OLAP/Notif and marks its rows/blocks restricted. (Restriction ≠ erasure; it suspends processing.) |
| **`erase(subject)`** | **purge/crypto-shred/pseudonymise, never hide** (GDPR §3.1). See below. |

### 6.1 The erasure algorithm (the hardest part — reaching immutable history)

The genuinely hard, partially-open problem (deep-dive §8; [05 §6](./05-hard-problems.md)). The committed
mechanism:

1. **Anonymise authorship** — reassign `created_by`/`edited_by` to a pseudonymous "Deleted user" to preserve
   the document's integrity and others' work (deep-dive §8). Because attribution is already the **opaque
   `principal_id`** (never PII — EI-04 §1; Id's pseudonym map is the lever), erasing the *person* needs no
   per-edge mutation in the common case; Id's pseudonym shred makes the id un-resolvable to a human (Id §11).
2. **Crypto-shred free-text PII reachable in immutable history** — free-text blocks and ops holding the
   subject's PII are encrypted under a **per-subject DEK** (GD-4; Storage §5.1). Erasure = **destroy the
   per-subject key** → the ciphertext in the op-log, snapshots, and backups becomes unrecoverable **without
   rewriting the merge-dependent op-log** (deep-dive §8; the crypto-shred-from-immutable-logs technique). You
   cannot delete a CRDT/CAS op (append-only, merge-dependent) — you destroy its key.
3. **Tombstone mentions/backlinks** so they degrade to a neutral placeholder (no dangling crash, deep-dive
   §2.6); Refs flips `tombstoned=true` via the `*.erased` consumer (Refs §4.6).
4. **Purge the search + vector index in lockstep** — emit `knowledge.*.erased`; Search purges + re-indexes,
   **including embeddings** (embeddings of personal data are personal data — Search §4.8). No leak via search.
5. **Published/public pages** — unpublish + CDN/cache purge (a high-risk export, deep-dive §8); lawful-basis
   tracked.
6. Returns a **receipt hash-linked into the audit log** (GDPR §3.1).

**The honest limitation (named floor, GD-6 `[OPEN → LEGAL]`):** full automated free-text PII detection is
**not perfectly solvable** (deep-dive §8). Knowledge is **reliable for structured personal references**
(person props, mentions, attribution) and provides **tooling + a documented process** (search, DSAR export,
flagged-content review) for free-text. This is stated, not over-promised ([05 §6](./05-hard-problems.md)).

The erasure-reaches-every-holder drill ([07](./07-drills-and-open-questions.md)) asserts: structured PII
purged, embeddings purged, per-subject key shredded → 0 recoverable structured PII; free-text covered by
tooling + the residual-limit write-up.

---

## 7. Spend-bearing work (reserve/settle)

Knowledge is **not spend-bearing in its own right** (no model calls, no CI runs originate here). When an
**agent** writes to Knowledge, the run passes the Agent Fabric's universal **reserve/settle** gate
(agent-fabric §5.4; D8) before any tool executes — the gate is the Fabric's bookends, Knowledge's tools are
ordinary effects metered through `EffectApi`. So "no balance → no agent write" is uniformly true, enforced at
the Fabric, not re-implemented in Knowledge. (If a future Knowledge feature embeds spend — e.g. server-side
AI summarisation as a built-in, not an agent run — it would front that work with reserve/settle; v1 has
none.)

Continue to [`04-views-cli-and-api.md`](./04-views-cli-and-api.md).
