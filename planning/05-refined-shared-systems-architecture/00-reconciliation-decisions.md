# Phase 5 — Reconciliation Decisions (the keystone)

> Phase: `05-refined-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md)
> (single source of truth, never contradicted). Binding doctrine:
> [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md),
> [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md).
> Primary input: [`planning/04-subsystem-architectures/cross-subsystem-change-requests.md`](../04-subsystem-architectures/cross-subsystem-change-requests.md)
> (CRs §1–11, CONFLICTS §12 X-1..X-7, OPEN QUESTIONS §13 OQ-A..OQ-L).
> Frozen contracts being refined: [`planning/03-shared-systems-architecture/contract-index.md`](../03-shared-systems-architecture/contract-index.md)
> + the 11 Phase-3 system docs. Spine: [`architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md)
> (ADR-01..ADR-20) + [`02b-doctrine-integration/decision-record.md`](../02b-doctrine-integration/decision-record.md)
> + [`integration-directives.md`](../02b-doctrine-integration/integration-directives.md). Date: 2026-06-19.
>
> Companion deliverable: [`contract-index.md`](./contract-index.md) — the refined, frozen build-to surface
> that incorporates every decision below and **supersedes** the Phase-3 contract index.

---

## 0. How to read this document — and what it does and does not change

This is the **reconciliation lead** output for Phase 5 (VISION §5): get the full overview, fold in the
Phase-4 cross-system requirements, resolve the conflicts so the contracts can freeze, and refine the shared
layer as a whole. **No ADR is reversed** — none was requested (change-requests §14.1). Every decision below
is one of three kinds, and each is tagged:

- **CONFIRM** — the Phase-3 seam is correct as written; Phase 5 ratifies it and (where useful) pins the
  concrete shape the Phase-4 docs assumed but never froze.
- **SHARPEN** — the Phase-3 contract stands, but its *encoding/shape* is now made concrete and final so two
  subsystems cannot drift. The contract surface in the index moves from "→ P4 open" to frozen.
- **NEW** — a genuinely new contract or sub-shape, additive over the spine, named here for the first time.

**Decision discipline (from the doctrine, binding).** Every decision: names its floor where it ships one
(VISION §3, EI-04 §4); grounds in the cited prior art where relevant; obeys the doctrine defaults-to-beat or
writes down why; and is honest about `[OPEN — LEGAL]` items — for those, this document specifies a
**defensible engineering posture** and **flags for counsel/DPO** (we are not counsel).

**Reading order.** §1 resolves the seven CONFLICTS (X-1..X-7). §2 resolves the twelve OPEN QUESTIONS
(OQ-A..OQ-L). §3 folds the eleven grouped change-request sections into the refined contracts. §4 is the
per-system "what you must now do" punch list. The two load-bearing reconciliation anchors from Phase 3
(the canonical `EventEnvelope` field list + units `00 §2.10`, and the `ArtifactRef` token table Bus §6.2)
are **unchanged** and remain the names/units authority; everything here aligns to them (directive X-5).

**Units, restated once (frozen, never re-litigated):** timestamps = RFC-3339 UTC; budgets/costs = integer
minor-units; TTLs/staleness/timers = seconds; resilient-client timeouts = milliseconds;
`pii_key_ref = kms://<tenant>/<dek-epoch>/<class>`, `<class> ∈ {tenant, subject:<id>, blob}`.

---

# Part 1 — The seven CONFLICTS (X-1 .. X-7)

## X-1 / OQ-A — The Git↔CI checks / merge-gate contract (the hardest seam) — **SHARPEN, jointly specified**

**The seam.** Git owns the merge gate and assumes a `check_status` per `(commit_oid, context)`. CI emits a
status keyed `(commit_oid, context)`, last-writer-wins per context, and the merge-queue workflow must wake on
a durable `ci.result` signal. Neither side froze the joint shape; both named it their top open question
(GIT OQ-3, CI OQ-7, CI D-8). This is the single most load-bearing cross-subsystem seam, so it is specified
here in full.

**Decision.** Define **one shared `CheckStatus` fact**, owned by **CI** (the producer), consumed by **Git**
(the gate), carried as an ordinary `EventEnvelope` over the durable bus, and **mirrored into a Git-owned
`check_status` projection table** keyed `(commit_oid, context)` that drives the merge gate. The merge queue
is a durable workflow (`myelin-flow`) that waits on a `ci.result` signal. This rides existing contracts
(2.1 envelope, 5.6 `project`, 9.4 durable signal, 4.3 `list_objects`) — it is a shaping, not a new engine.

### The `CheckStatus` shape (frozen)

```
CheckStatus {
  tenant:        TenantId,            // partition key, from token (EI-02 §1)
  repo:          ArtifactRef,         // myelin://<tenant>/git/repo/<id>
  commit_oid:    GitOid,              // the content-addressed commit the check ran against
  context:       CheckContext,        // the KEY half of (commit_oid, context); see below
  state:         CheckState,          // queued | in_progress | success | failure | error | neutral | cancelled
  required:      bool,                // does this context block the merge gate for this target ref? (Git decides; see policy)
  run:           ArtifactRef,         // myelin://<tenant>/ci/run/<id> — the producing run (for supersession + drill-down)
  run_attempt:   u32,                 // monotonically increasing per (commit_oid, context); higher attempt supersedes lower
  trust_tier:    TrustTier,           // trusted | untrusted_fork  — stamped by CI from the run's provenance
  details_ref:   ArtifactRef,         // myelin://<tenant>/ci/run/<id>#step-<n> — jump-to-failure sub-anchor (see OQ-D)
  summary:       HumanisedRef,        // a (template_key, args) pair humanised by Notif (NOTIF-1), never a raw string
  started_at:    Timestamp,
  completed_at:  Option<Timestamp>,
  cost_settled:  bool,                // reserve/settle bookend closed (11.7) — a check is not "final" until settled
}

CheckContext = { provider: "ci" | "external", name: String }   // e.g. {ci, "build"}, {ci, "test/unit"}, {external, "sonarcloud"}
CheckState   = queued | in_progress | success | failure | error | neutral | cancelled
TrustTier    = trusted | untrusted_fork
```

**Keying + last-writer-wins.** The merge-gate truth is keyed `(commit_oid, context)`. The Git projection
table holds **exactly one current row per key**; an incoming `CheckStatus` **supersedes** the stored row iff
its `run_attempt` is `>=` the stored `run_attempt` (re-run supersession is monotonic on `run_attempt`, not on
wall-clock `completed_at` — clocks are not authority; the attempt counter is). A *lower* `run_attempt`
arriving late is dropped (stale re-delivery), which the bus's at-least-once delivery makes mandatory.

**Event.** CI emits `ci.check.updated` (subsystem `ci`, type `check`, past-tense) via the **outbox only**
(BUS-2), envelope `subject = repo#commit-<oid>/check-<context>` (an `ArtifactRef` sub-anchor per OQ-D),
`aggregate = (repo, commit_oid)` so all checks for one commit are per-aggregate ordered (ADR-04.2). Git's
`check_status` consumer is idempotent on `event_id` and applies the `run_attempt` supersession rule. This is
the canonical "references-not-payloads" path: the event carries the `CheckStatus` struct (small, PII-free),
not log bytes.

**Fork / trust-tier gating (the security-critical half).** A check produced by a run whose
`trust_tier = untrusted_fork` (a PR from a fork, or any run that executed untrusted contributor code) is
recorded **but cannot satisfy a `required` context by itself**. The merge gate treats an
`untrusted_fork` success as **`neutral` for gating purposes** until a trusted principal (a maintainer, via
`check(subject, approve_untrusted_ci, repo)`) **endorses** the run, OR the same context is re-run under
`trust_tier = trusted` (the standard "approve and run" maintainer flow). Rationale: a fork PR must never be
able to turn its own gate green by running attacker-controlled CI config (EI-02 §1 blast-radius; the classic
poisoned-pipeline-execution attack). The trust tier is stamped by **CI** from run provenance and from the
ReBAC ABAC edge CI already declared (`read & !is_untrusted_fork`, CR §1) — Git does not recompute trust, it
*reads* `trust_tier` off the fact.

**The merge gate (Git-owned).** Git evaluates "may this PR merge?" as: all `required` contexts for the
target ref have a **current** row with `state = success` and an **acceptable trust posture** (trusted, or
fork-endorsed). The set of `required` contexts is **Git's** branch-protection policy (a Git-owned config,
not CI's) — CI reports facts, Git decides which facts gate. This keeps the dependency acyclic (EI-02 §3): CI
emits, Git reads; Git never synchronously calls CI to ask "is it green," it reads its own projection.

**The merge-queue durable signal.** A merge queue serialises merges into a busy target ref. It is a durable
workflow (`DurableExecutor::start`, contract 9.1) per target ref. For each queued PR the workflow:
1. computes the speculative merge commit, dispatches the required CI via `SCHEDULE_AND_RUN_JOB` (OQ-F),
2. `wait_for_signal("ci.result", idem_key = <merge_attempt_id>)` — holds **no runtime** while CI runs
   (contract 9.4), wakes hours later if needed,
3. on a `success` signal for all required contexts, performs the merge and emits `git.pr.merged`; on
   `failure`/`error`, dequeues the PR with a humanised reason and continues the queue.

The `ci.result` signal payload is `{ commit_oid, overall: success|failure, contexts: [CheckContext], idem_token }`;
`DurableExecutor::signal` is idempotent on `idem_key` (a double-delivery is one wake — contract 9.1, OQ-F).
**`ci.result` is a CI-derived rollup signal, distinct from the per-context `ci.check.updated` events**: the
events drive the always-visible PR checks UI (via the projection); the single `ci.result` signal drives the
merge-queue workflow's resume. Both are emitted by CI via the outbox.

**What each side must do.** CI: emit `ci.check.updated` per context with `run_attempt`/`trust_tier`; emit the
rollup `ci.result` signal; stamp `details_ref` as a `#step-<n>` sub-anchor. Git: own the `check_status`
projection table + supersession rule + branch-protection `required`-set policy + the fork-endorsement check;
run the merge queue as a durable workflow. Identity: serve the `approve_untrusted_ci` permission as a
namespace relation (X-1 ABAC edge already declared). This is now **SHARPENED → frozen** in the contract index
(new contract 5.9 / the CI↔Git check seam).

---

## X-2 / OQ-B — The canonical `myelin-content` node taxonomy + ADF→content lossy-map + Chat/Issues subset — **SHARPEN**

**The seam.** `myelin-content` (ADR-05) is led by Knowledge but consumed by Chat (reuses the block/inline
AST) and Issues (needs an ADF→content lossy-node map for import). Risk: a consumer assumes nodes Knowledge
never committed, or the converter is lossier than Issues' import assumes.

**Decision.** Knowledge **owns and freezes the canonical taxonomy below**; it is the complete v1 set. Chat
and Issues declare their **consumed subset** (a strict subset — neither adds a node type). The three
platform-load-bearing **inline reference nodes are identical across all three** (the whole point of ADR-05:
share the AST, not the editor). Inline runs are stored as a **markdown-subset string** (KN-2, D10/D11), so
the inline taxonomy is the *markdown-subset grammar* plus the three structured nodes that survive as objects.

### Canonical block node taxonomy (frozen — `myelin-content` v1)

```
Block =
  | paragraph        { inline }
  | heading          { level: 1..6, inline }
  | bullet_list      { items: [list_item] }
  | ordered_list     { items: [list_item], start: u32 }
  | task_list        { items: [task_item{ checked: bool, inline }] }
  | blockquote       { blocks: [Block] }
  | code_block       { lang: Option<String>, text: String }     // text is raw, NOT markdown-parsed
  | callout          { tone: info|warn|success|danger|note, blocks: [Block] }
  | table            { columns: [col], rows: [[cell{ blocks }]] }
  | divider
  | image            { blob: ArtifactRef, alt: String, caption: Option<inline> }
  | embed            { ref: ArtifactRef, display: inline|card|preview }   // structured node (load-bearing)
  | db_view          { db: ArtifactRef, view: ViewSpec }        // Knowledge-only in v1; a myelin-query view (OQ-C)
  | toggle           { summary: inline, blocks: [Block] }
  | sync_block       { source: ArtifactRef }                     // Knowledge-only; transclusion
```

### Canonical inline grammar (markdown-subset string + structured nodes)

The inline content of a block is a **markdown-subset string** (KN-2). The subset: `**bold**`, `*italic*`,
`` `code` ``, `~~strike~~`, `[text](url)`, and three **structured inline nodes** that are NOT markdown text —
they round-trip as opaque sentinels in the string and resolve to objects:

```
mention(Principal)        // @alice — a principal ref; renders to display name per-viewer (REF-3)
artifact_ref(ArtifactRef) // a typed reference to any artifact; the PRODUCER of refs.edge.created (5.4)
embed(ArtifactRef)        // an inline unfurl/transclusion request
```

The round-trip invariant `render(parse(md)) === md` (D10, the editor gate) holds over this subset; the three
structured nodes are stored structured precisely so reference-extraction stays reliable (EI-04 §2.4) — they
are the producers of `refs.edge.created` (contract 5.4), uniformly across Chat, Issues, and Knowledge.

### ADF → `myelin-content` lossy-node map (frozen — Issues import, CR-9)

Atlassian Document Format (Jira/Confluence) is the dominant import source. The map (lossless unless noted):

| ADF node | → `myelin-content` | Loss |
|---|---|---|
| paragraph, heading, blockquote, codeBlock, rule, bulletList, orderedList, table, mediaSingle(image) | direct equivalent | none |
| taskList / taskItem | task_list / task_item | none |
| panel | callout (tone mapped: info/note/success/warning/error → info/note/success/warn/danger) | none |
| mention | mention(Principal) **if the principal resolves in-tenant**; else a plain-text `@name` run | **lossy**: unresolved external mention degrades to text (named) |
| inlineCard / blockCard (URL) | artifact_ref(ArtifactRef) **if the URL resolves to a Myelin artifact**; else `[text](url)` link | **lossy**: external URL stays a link, not a typed ref |
| emoji | the unicode glyph in the markdown-subset string | none (custom emoji → `:shortcode:` text — lossy) |
| status (Jira lozenge) | inline `code` run with the label | **lossy**: loses colour/lozenge styling |
| date | plain text (ISO date) | **lossy**: loses the interactive date chip |
| mediaGroup / attachments | image blocks + an attachments list on the issue | none |
| expand / nestedExpand | toggle | none |
| extension / bodiedExtension (macros) | a callout(note) carrying the macro's text body + a "[unsupported macro: name]" marker | **lossy by design**: macros are not executed; flagged in the import report |
| layoutSection / layoutColumn | flattened to sequential blocks | **lossy**: loses multi-column layout |

Every lossy conversion is **recorded in the import report** (a per-import Knowledge doc), so the floor is
named, not silent (EI-04 §4). Issues' import assumption is now bounded to exactly this map.

### Consumed subsets (frozen)

- **Chat (CR §X-2):** `paragraph, heading(1..3), bullet_list, ordered_list, task_list, blockquote,
  code_block, callout, table, divider, image` + all three inline nodes. **Excludes** `db_view, sync_block,
  toggle` (no in-message databases/transclusion). Chat messages are small and mostly immutable-after-send
  (ADR-05), so they use the AST with **no collaborative-edit engine** (per-message CAS on edit).
- **Issues (CR-9):** the same block subset as Chat **plus** `db_view` is **not** authored inline but issue
  descriptions/comments use the full block subset; the import map above governs migration fidelity. Issue
  description concurrency is single-author-at-a-time (ADR-05), CAS-guarded.
- **Knowledge:** the **full** taxonomy, with the collaborative-edit engine (KN-1 CAS-floor → CRDT) over it.

**Net:** the taxonomy is frozen and complete; Chat/Issues are strict subsets; the three inline ref nodes are
identical everywhere. **SHARPENED → frozen** (contract index, `myelin-content` row).

---

## X-3 / OQ-C — `myelin-query` primitive + `order_key`/LexoRank encoding parity — **SHARPEN**

**The seam.** Issues owns its AST→store compiler; Knowledge owns flexible-DB execution; both share the
field-type enum, the view-model, the AST grammar, and the `order_key`/LexoRank encoding. Encoding drift
between "a row dragged in a Knowledge db" and "an issue dragged in a backlog" would break a future shared
CRDT/render path. ADR-06/ADR-07 already align them by intent; Phase 5 freezes the bytes.

**Decision.** Freeze the four shared shapes in `myelin-query`. Issues and Knowledge each provide their own
*compiler/executor* (subsystem-owned per ADR-06), but the **definitions below are byte-identical**.

### Field-type enum (frozen)

```
FieldType =
  | text | rich_text(myelin-content) | number{ precision } | checkbox
  | select{ options:[Opt] } | multi_select{ options:[Opt] }
  | date{ has_time: bool } | datetime
  | principal      // user/agent/service ref
  | relation{ target_type: ArtifactType, cardinality: one|many }   // rides Refs cross-artifact, local index intra-collection (TE-7)
  | rollup{ via: FieldId(relation), target: FieldId, fn: RollupFn }   // computed at READ TIME, never stored (KN-3)
  | formula{ expr: FormulaAst, result_type: FieldType }              // computed at READ TIME, never stored
  | url | email | phone | file(ArtifactRef) | created_at | updated_at | created_by | updated_by
Opt = { id: OpaqueId, label: String, color: Token }     // id is stable; label/color are display
```

Personal-data classification (`#[personal_data]`, GDPR 10.2) attaches **per field definition**, so a
`principal`/`email`/`text` field carrying PII is tagged at the schema level — this is how field-level erasure
and the field-level ABAC caveat (X below / CR §1) find their targets.

### View-model (frozen)

```
ViewSpec {
  kind:    table | board | calendar | timeline | gallery | list,
  filter:  QueryAst,                 // the shared AST below; ALWAYS conjoined with list_objects (ADR-07)
  group_by:    Option<FieldId>,
  sort:    [ { field: FieldId, dir: asc|desc } ],   // the LAST resort tiebreak is order_key (below)
  visible: [FieldId],
  order_field: FieldId(order_key),    // the manual drag-order field (fractional index)
}
```

### AST grammar (frozen — shared by saved views, Search compile, EventMatcher, Notif prefs)

```
QueryAst =
  | And([QueryAst]) | Or([QueryAst]) | Not(QueryAst)
  | Cmp { field: FieldPath, op: Op, value: Literal }
  | In  { field: FieldPath, values: [Literal] }
  | Has { field: FieldPath }                  // relation/multi-select membership
  | Text{ query: String, fields: [FieldPath] } // compiles to FT on the search backend
  | Ref { field: FieldPath, target: ArtifactRef }
Op = eq | ne | lt | lte | gt | gte | contains | starts_with | within   // `within` = relative date range
Literal = Str | Num | Bool | Date | Principal | Ref | Null
```

This is the **same `QueryAst` that is the `EventMatcher` predicate core** (contract 3.4): bounded
interpreter, no UDFs/loops/recursion, statically cost-bounded, permission-aware by construction (ADR-07,
not CEL/JSONLogic). One grammar, four compile targets (OLTP, Search, EventMatcher, Notif prefs). The
projection-feeder promotion (CR §4, OQ-C tail): a custom facet filtered often enough (a **measured**
threshold, default-to-beat: a facet appearing in `> 5%` of a collection's view executions over a rolling
window) is promoted from a GIN-indexed JSONB scan to a generated index — measured, never predicted
(EI-02 §8). The threshold is a Search-owned tunable, not a contract constant.

### `order_key` / LexoRank fractional-index encoding (frozen — the drift-killer)

Manual drag-to-reorder uses a **fractional index** (LexoRank-class; the proven Jira ranking scheme):

- **Alphabet / base:** base-62, ordered `0-9 A-Z a-z` (ASCII-ordinal, so byte comparison == rank order).
- **Encoding:** an `order_key` is a non-empty string over the alphabet; ranking is **lexicographic string
  comparison**. Between two keys `a < b`, a new key is the midpoint via digit-wise bisection; when no digit
  fits between, **append** a midpoint digit (the key grows by one char) rather than rebalancing.
- **Initial spacing:** first item `"U"` (mid of the alphabet), appended items step by a fixed gap; bulk
  insert spreads evenly across the range.
- **Jitter:** a new key appends a **2-char random suffix** from the alphabet (the LexoRank "bucket"/jitter)
  so two clients independently inserting "at the same midpoint" produce **distinct** keys — no two concurrent
  drags collide on an identical key (the concurrency-safety reason the jitter exists).
- **Rebalance:** when a key exceeds **48 chars** (measured pathology, not predicted), a background
  rebalance pass re-spaces the collection's keys; rebalance is a `myelin-flow` activity, idempotent,
  emitted via outbox so views resubscribe.
- **Tiebreak:** when two `order_key`s somehow compare equal (should not happen with jitter), the
  deterministic tiebreak is `created_at` then `id` (ULID) — total order guaranteed.

Issues' "drag an issue in a backlog" and Knowledge's "drag a row in a db" produce **byte-identical**
`order_key`s under this scheme. A future shared CRDT/render path can therefore treat the order field
uniformly. **SHARPENED → frozen** (contract index, `myelin-query` row).

---

## X-4 / OQ-D — The unified `#sub` sub-artifact grammar + outdated/tombstone resolution — **SHARPEN**

**The seam.** The `#sub` grammar + tombstone semantics were defined three times for three content shapes:
Git mints content-anchored line ranges (`#L42-L88`, partial/tombstone on content loss); Knowledge mints
block/heading/row anchors stable across edits; Chat mints message/thread anchors. Refs must hold **one**
`#sub` URN grammar + one graceful-degradation rule covering all three (Refs §3.5).

**Decision.** Refs owns one `#sub` grammar and one resolution ladder. Each subsystem mints **stable opaque
sub-ids** of its declared kinds; Refs stores the **full sub-URN** AND the **`#sub`-stripped root** (so a
broken sub-anchor still resolves to the parent artifact). Resolution degrades through a fixed ladder, never
leaks, never silently dangles.

### The unified `#sub` grammar (frozen)

`ArtifactRef = myelin://<tenant>/<subsystem>/<type>/<id>[#<sub>]` where `<sub>` is one of these **kinds**
(the kind prefix makes the grammar self-describing and lets Refs pick the resolver):

```
#sub kinds (frozen vocabulary):
  comment-<opaqueid>     // a comment/review-thread node          (Git PR, Knowledge, Issues)
  thread-<opaqueid>      // a thread root                         (Chat, Git review thread)
  message-<opaqueid>     // a single chat message                 (Chat)
  b<opaqueid>            // a content block                       (Knowledge, Issue description block)
  h<opaqueid>            // a heading anchor                      (Knowledge)
  row-<opaqueid>         // a database row                        (Knowledge db, Issue-as-row)
  field-<opaqueid>       // a field within a row/issue            (Issues, Knowledge db)
  L<start>-L<end>        // a CONTENT-ANCHORED line range         (Git) — see anchoring below
  check-<context>        // a check status on a commit            (CI, X-1)
  step-<n>               // a CI run step (jump-to-failure)        (CI)
```

`<opaqueid>` is a subsystem-minted stable opaque id (NOT a positional index — positions move). The
**stability obligation is each subsystem's** (Refs §3.5): a block id survives edits/moves; a message id is
immutable; a comment id is immutable. Refs validates the grammar and rejects ambiguity (REF-3); it never
guesses scope.

### Git line-ranges — the new specificity (content-anchored, not positional)

`#L42-L88` is **content-anchored**, not a raw line number. Git stores, alongside the range, a
**content fingerprint** (the BLAKE3 hash of the anchored lines + a small context window, plus the blob oid
at mint time). On resolution against a newer blob:

1. **exact**: the blob oid matches → return the exact range (live).
2. **rebased**: blob changed but the fingerprinted lines are found at a shifted position (a 3-way context
   match, the standard diff-anchor technique) → return the shifted range, flagged `moved`.
3. **partial**: some anchored lines survive, some are gone → return the surviving sub-range, flagged
   `outdated` (this is Git's named "outdated-line-range" case).
4. **tombstone**: the anchored content is entirely gone → return a `Tombstone{ root, reason: content_gone }`.

### The one resolution ladder (frozen — covers all three shapes)

`resolve(ref, viewer, mode)` (contract 5.2) for any `#sub`:

```
1. permission: check(viewer, read, root)  → Deny ⇒ Tombstone{reason: denied}     (never leak — EI-02 §1)
2. root resolve: the parent artifact exists?  → No ⇒ Tombstone{reason: root_gone}
3. sub resolve via the owner's project(ref, viewer) sub-anchor resolver:
     LIVE      → Projection (the unfurl/embed)
     MOVED     → Projection + flag `moved`        (Git rebased range; KN block moved)
     OUTDATED  → Projection(partial) + flag `outdated`   (Git partial range; KN edited block)
     GONE      → Tombstone{reason: sub_gone, root}  // root still resolves; sub is dead, embed shows the parent
4. ERASED (any level): Tombstone{reason: erased}   // pseudonym-shred/crypto-shred made it unrenderable
```

A tombstone **always carries the root** so an embed degrades to "this referenced <parent artifact>
(the specific part is no longer available)" rather than vanishing. This is the single graceful-degradation
rule for Git line-ranges, Knowledge block/heading/row anchors, and Chat message/thread anchors.
**SHARPENED → frozen** (contract index, contract 5.7 sub-artifact scheme).

---

## X-5 — *(no Phase-5 conflict; this token was the Phase-3 names-and-units reconciliation directive, already satisfied)*

The change-requests doc numbers its conflicts X-1..X-7. Note that "X-5" in the **integration-directives**
(the names-AND-units reconciliation, directive X-5) is a *different* numbering namespace and is already
honoured (the anchors in §0). The change-requests **X-5** is the *Chat-vs-Knowledge threading* conflict,
resolved under **OQ-L** below (it is the same question). It is called out there to avoid double-resolution.

## X-6 — The unified sandbox: CI owns the runner + the real-kernel escape drill gates all agent execution — **CONFIRM**

**The seam.** The sandbox is CI-owned (ADR-20) but Agent-Fabric-shared; every agent-authoring subsystem
(Issues/Knowledge/Chat) registers gated `ToolDef`s that ultimately execute there. Risk: a subsystem assumes
execution semantics (cost gate, attribution, HITL withhold) the unified runner must guarantee uniformly.

**Decision (CONFIRM, with the uniform guarantees pinned).** ADR-20 stands: **one job spec `kind ∈ {ci,
agent}`, one hardened runner, one real-kernel escape drill that is the single hard go/no-go before any
untrusted customer code runs** (CI *or* agent). `ToolHands::exec` (contract 8.4) **is** the CI runner's
`kind=agent` job; the `no-host-exec` lint forbids any bypass (AG-2). Phase 5 pins the **four uniform
guarantees** every subsystem's tool inherits by construction (so no subsystem has to re-implement them):

1. **Cost gate (universal).** Every execution passes the reserve/settle bookend (contract 11.7, D8/CI-2):
   reserve at dispatch, refuse-on-exhaustion, settle on completion, never interrupt in-flight. CI runs and
   agent runs meter into the **same wallet** (Commercial C-1). A subsystem tool cannot opt out.
2. **Attribution.** The run executes under a **per-run attenuated token** (contract 4.7,
   `mint_run_token`), life == run life, auto-revoked on teardown, re-mintable mid-workflow on resume (S-11).
   Every effect is attributed to the run principal with nested causality (BUS-5).
3. **HITL withhold (plan-then-apply).** Side-effecting mutation never goes through `ToolHands::exec`; it
   goes through `EffectApi::apply` (contract 8.2, routing §5.0), which enforces schema → capability →
   delegation → tenant → budget → **HITL gate** → apply-via-public-endpoint → meter. A gated tool whose name
   is not in the approved set is **withheld** (returns a `Denied` tool error, does not mutate — AG-8); the
   approval card shows the pending action + a live cost estimate. `ToolHands::exec` carries only
   `compute`/`external` untrusted code, never privileged mutation (the routing split is the safety boundary).
4. **Isolation floor + drill.** gVisor-class userspace-kernel **or** microVM; the named hardening profile
   (egress default-deny, read-only root + tmpfs, caps dropped, no-new-privileges, seccomp, digest-pinned
   images fail-closed on un-digested tags, whole-guest kill on teardown, `pids.max` + zero swap, secrets
   resolved inside the boundary, never forwarded via the runtime). The **real-kernel escape drill** (T-5,
   E-9) is the gate.

### Per-subsystem `requires_approval` defaults (frozen jointly with the Fabric)

The `requires_approval` flag on each `ToolDef` (contract 8.1) — the product-call defaults, **gated by
default for any consequential/irreversible action** (ADR-08.6 suggest-by-default; GDPR Art. 22):

| Subsystem | Tool (examples) | `requires_approval` default | Rationale |
|---|---|---|---|
| **CI** | `deploy(env)` to a protected env | **yes** | protected-env deploy is consequential (CI HITL gate) |
| **CI** | `approve_deploy`, `write_secret` | **yes** | secret write + approval are privileged |
| **CI** | `run_pipeline` (non-prod) | no | cheap, reversible, metered |
| **Git** | `git.merge`, `open_pr` | **yes** (merge), no (open_pr) | merge is the consequential gate (AG-8); opening a PR is reversible |
| **Issues** | `forecast`, `triage`, `sla_draft` | no (suggest) | advisory; the human accepts the suggestion |
| **Issues** | `transition(issue, →done)` on an SLA-bound issue | **yes** if the transition has an approver edge (ABAC) | the field/transition ABAC caveat (CR §1) |
| **Knowledge** | `publish`, `edit(confidential_page)` | **yes** | publishing/confidential edits are consequential (approver set) |
| **Knowledge** | `draft`, `comment` | no | reversible |
| **Chat** | `post_message`, `react` | no | reversible, cheap |
| **Chat** | any `EffectApi` tool that mutates another subsystem | inherits **that** subsystem's default | the effect is governed where it lands, not where it's invoked |

**CONFIRMED**; the defaults table is now frozen (contract index, contract 8.1 + 8.4).

## X-7 / OQ-G — ONE platform-wide free-text / immutable-content erasure lawful-basis posture — **NEW policy artifact `[OPEN — LEGAL]`**

**The seam.** The same legal seam is named five times: Git (immutable commit bytes), CI (inline log PII),
Issues (third-party free-text mentions), Knowledge (free-text blocks), Chat (a name typed into another
user's un-erased message body). Risk: five different residual statements instead of one ratified posture.

**Decision.** State **ONE** platform-wide posture; instantiate it per subsystem **by reference**, not by
restating it five times. This is the named "Erasure vs. Immutability reconciliation" deliverable (GD-1, L-2).

### The structural floor (built now, no legal dependency)

For **all** free-text and immutable content, the engineering guarantee is the same and is **fully built**:

1. **Per-subject DEK crypto-shred (the lever).** Free-text/body/op-log/agent-trace columns are encrypted
   with a **per-subject DEK** (contract 11.4, GD-4 granularity rule). A subject's erasure destroys their
   DEK; their content in DBs **and backups and immutable logs** becomes unrecoverable ciphertext. This is the
   primary erasure mechanism for *their own* authored content (their messages, their comments, their blocks).
2. **Pseudonym-map shred (identity erasure).** Author/subject identity in immutable structures is a **stable
   opaque pseudonym** (`<pseudonym>@<tenant>.noreply`, the frozen grammar from CR §1); the person↔pseudonym
   map is the erasable record (contract 4.8, `resolve_pseudonym`/`erase`). Erasing the map means the immutable
   bytes (commit author, event actor) hold only a pseudonym — DSR fan-out **step 1**. This is the answer for
   Git commit-author metadata: commit **pseudonymous-by-default** (GIT-1) so the immutable hash never bakes in
   erasable PII in the first place.
3. **Structural holder coverage.** Every store auto-registers as a `PersonalDataHolder` (contract 1.4, 10.1);
   `restrict` suppresses indexing/agent-use/analytics/notification for a subject pending erasure. "We forgot
   a store" is structurally impossible (the `no-untagged-personal-data` lint + harness auto-registration).

### The residual (the part the floor does NOT erase — for counsel)

The residual is **third-party free-text PII**: a person's name/email **typed by someone else** into that
other person's content (a Chat message body, an issue comment, a doc block, a CI log line, a commit message
written by a different author). This content is encrypted under the **author's** DEK, not the subject's, so
the subject's erasure does not crypto-shred it (shredding the author's DEK would destroy the author's
legitimate content). The same residual exists in **immutable commit message bodies** authored by others.

### The ratified engineering posture (defensible; FLAG FOR COUNSEL)

`[OPEN — LEGAL]` — this is a defensible engineering posture pending DPO/counsel ratification (L-2); we are
not counsel:

- **Primary basis:** structured PII and self-authored free-text erase **reliably** via per-subject DEK shred
  + pseudonym-map shred. This covers the overwhelming majority and is the GDPR-compliant default.
- **Residual posture:** third-party free-text mentions and immutable-byte content authored by others are
  handled under a **documented lawful-basis limit** — best-effort on-request redaction (a targeted
  `rectify`/tombstone of the specific span where the subject identifies it), plus the standing structural
  guarantee that the residual is **never indexed, never agent-readable, never in analytics for a restricted
  subject** (the `restrict` suppression). For git history specifically, the documented options are
  (a) the pseudonymous-by-default floor (covers author identity), and (b) a **history-rewrite erasure path**
  (audited, tamper-evident, rate-limited tenant op with fork/mirror/clone-cache invalidation fan-out — the
  Git erasure-admin tool, CR §9) for the rare case where a body must be expunged — **with the understood,
  disruptive consequence of changed hashes** (EI-04 §1).
- **What counsel must ratify (one statement, not five):** the lawful basis and documented limit for
  residual third-party/immutable free-text PII; the Art. 17 reach into immutable git bytes; the
  history-rewrite-vs-documented-limit choice; the audit-log retention carve-out (GD-5); and the
  worklog-sensitivity classification (OQ-H). The DPO ratifies; the structural floor ships regardless.

**This is ONE posture, instantiated per subsystem by reference.** No subsystem doc restates it; each says
"the residual is handled per the platform posture in 00-reconciliation §X-7." **NEW policy artifact**;
flagged for counsel/DPO. (Contract index: a new contract 10.9 "erasure posture for free-text/immutable
content.")

---

# Part 2 — The twelve OPEN QUESTIONS (OQ-A .. OQ-L)

> OQ-A is X-1, OQ-B is X-2, OQ-C is X-3, OQ-D is X-4, OQ-G is X-7 — resolved above; cross-referenced here so
> the index is complete. OQ-E, F, H, I, J, K, L are resolved below.

## OQ-A — *resolved as X-1 above* (Git↔CI check seam). The single most load-bearing seam.
## OQ-B — *resolved as X-2 above* (`myelin-content` taxonomy + ADF map + subsets).
## OQ-C — *resolved as X-3 above* (`myelin-query` + `order_key` parity + promotion threshold).
## OQ-D — *resolved as X-4 above* (unified `#sub` grammar + tombstone ladder).

## OQ-E / S-10 — The `list_objects` `Filter{set_expr, zookie}` push-down — **SHARPEN (the most-repeated, several-times-blocking ask)**

**The ask.** All five subsystems need the leak-free, ACL-pre-filtered list/board/search scan; three name it
*blocking* (ISS CR-1, + GIT/CI/KN/CHAT leak-free scans). The one shape must conjoin the ACL filter into the
native query — no N+1 `check` per row, no post-filter — over an **arbitrary subsystem id column**: git
repo/PR ids, CI `run_id`, issue `issue.id`, KN `database_row`, chat `message`/`channel` ids. This is the
single most repeated request; it is defined concretely here.

**Decision.** `list_objects` (contract 4.3) returns **either** a materialised id set **or** a `Filter` that
the consumer **composes into its own query as a SQL-pushdownable predicate over the consumer's id column**.
The `Filter` is **not** an opaque blob — it is a structured, consumer-composable `set_expr` plus the zookie
that bounds its consistency. Freeze the shape:

```
ListObjectsResult =
  | Ids        { ids: Vec<ObjectId>, zookie: Zookie }     // small result sets: materialise (default under a cardinality cap)
  | Filter     { set_expr: SetExpr, zookie: Zookie }      // large/unbounded: push down

SetExpr =                                  // a tenant-scoped, monotone set algebra over the object-id space
  | All                                    // subject can see every object of this type in the tenant (e.g. admin)
  | None                                   // subject can see nothing (deny) — the consumer adds `WHERE false`
  | Ids(Vec<ObjectId>)                     // an explicit allow-set (when small enough to inline)
  | NotIds(Vec<ObjectId>)                  // an explicit deny-set over an otherwise-visible space
  | InRelation { relation: RelName, via_column: ColRef }   // "objects where this id is the object of <relation> for the subject"
  | Union([SetExpr]) | Intersect([SetExpr]) | Difference(SetExpr, SetExpr)
  | TupleSet { index: AuthzIndexRef }      // a server-materialised tuple set the consumer JOINs against (the big-result path)

ColRef = { table: "<consumer table>", column: "<the id column>" }   // names the consumer's OWN id column
```

### How it pushes down (the no-N+1, no-post-filter mechanism)

The consumer calls `list_objects(subject, read, type, zookie?)`. For a large space, Identity returns
`Filter { set_expr, zookie }`. The consumer's `myelin-query` compiler (the same one that compiles saved-view
ASTs) **lowers `set_expr` into a SQL predicate over `via_column` / `ColRef`** and ANDs it into the board/
list/search query. Concretely, the three lowering forms:

- **`Ids` / `NotIds`** → `WHERE <id_col> IN (...)` / `NOT IN (...)` (inlined when under the cap).
- **`InRelation { relation, via_column }`** and **`TupleSet { index }`** → a **JOIN against a
  per-tenant, residency-pinned authz tuple index** that Identity maintains (a materialised
  `(subject, relation, object_id)` projection of the ReBAC tuples, kept fresh via the bus). The consumer
  emits `... JOIN authz_visible av ON av.object_id = <consumer table>.<id column> AND av.subject = $1 AND
  av.relation = $2`. This is the SpiceDB/Zanzibar "reverse index / `LookupResources`" pattern realised as a
  **co-located JOIN target** so the consumer's own query planner does the conjoin — **one query, no N+1, no
  post-filter** (the SC-1 leak-and-slowness fix; ADR-03 mandate that Search/Refs pre-filter not post-filter).
- **`Union/Intersect/Difference`** → the boolean composition of the above, compiled to `AND`/`OR`/`EXCEPT`.

The authz tuple index is **per-tenant** (EI-02 §1: no cross-tenant query path) and is itself a
`PersonalDataHolder` (tuples reference subjects). It is the dedicated read replica the doctrine names as the
likely first scaling need (ID-4). For HYOK/`can_derive_plaintext_index()=false` cases the index still works
(it indexes *tuples*, not content).

### Why this exact shape serves all five id columns

Each subsystem names its own `via_column` and JOINs against the same `authz_visible` index keyed by *that*
object type — git `repo`/`pr`, CI `run`, issue `issue`, KN `database_row`, chat `channel`/`message`:

| Consumer | `type` | `via_column` (the consumer's own id column) | the conjoin |
|---|---|---|---|
| Git | `pr` / `repo` | `pr.id` / `repo.id` | board/list of PRs/repos the viewer may read |
| CI | `run` | `run.id` | the runs list, ACL-filtered |
| Issues (blocking) | `issue` | `issue.id` | the board/backlog scan — the Tier-3 escalation valve compiles the board query to Search with the **same** `Filter` conjoined |
| Knowledge | `database_row` | `db_row.id` | a db view, row-level ACL pushed down (plus the field-level ABAC caveat at `check`-time, off this hot path) |
| Chat | `channel` / `message` | `channel.id` / `message.id` | the ambient channel list; `list_subjects` (not `list_objects`) serves the read-fanout side |

**Consistency.** The returned `zookie` bounds staleness; a security-sensitive scan passes the zookie so the
read does not use the fail-static cache (contract 4.10). Read-your-writes: a just-revoked grant
(`write_tuples` returned a newer zookie) is reflected because the JOIN reads the tuple index at-or-after the
zookie's revision (the index carries a revision watermark; a scan requiring a fresher revision waits or
falls back to `check`).

**Field/transition ABAC caveat (CR §1, the off-hot-path half).** Row/object visibility is the `list_objects`
push-down above. **Field-level** hiding (issue field.view, KN column hiding) and **transition** approver
checks are an **ABAC caveat evaluated at `check`-time** on the already-filtered, already-fetched rows — never
on the hot `list_objects` path (it would defeat the conjoin). The caveat context shape:

```
CaveatContext { object: ArtifactRef, field: Option<FieldId>, transition: Option<TransitionId>, attrs: Map<String, Literal> }
check(subject, view_field, object, zookie?, caveat: CaveatContext) → Allow | Deny | Conditional
```

So `list_objects` returns the visible rows cheaply; `check` with a `CaveatContext` then redacts individual
fields / gates individual transitions on those rows. **SHARPENED → frozen** (contract index, contract 4.3
gets the `SetExpr` shape; 4.2 gets the `CaveatContext`). This closes the platform's most-repeated ask.

## OQ-F — `SCHEDULE_AND_RUN_JOB` long-parked-activity-completed-by-signal + per-effect `idem_key` for batch HITL — **NEW pattern over Workflow §4.4**

**The ask.** CI needs the seam between its scheduler and the engine: a `SCHEDULE_AND_RUN_JOB` activity whose
completion is a **signal** arriving hours later, idempotent on an `idem_token`. Chat needs
`DurableExecutor::signal` per-effect `idem_key` for batch/partial approval (a double-click is one approval; a
partial approval is well-defined).

**Decision.** Define the **long-parked-activity-completed-by-signal** pattern as a first-class
`myelin-flow` idiom (it rides contracts 9.1/9.4, no new engine):

### `SCHEDULE_AND_RUN_JOB` (CI scheduler ↔ engine seam)

```
// inside a workflow definition (the merge queue, a pipeline, an agent run):
let job = ctx.activity(SCHEDULE_AND_RUN_JOB, JobSpec{ kind: ci|agent, ..., idem_token })?;   // returns immediately: job dispatched
ctx.wait_for_signal("job.done", idem_key = job.idem_token)?;                                   // parks: holds NO runtime (9.4)
// ... woken hours later by `signal(run, "job.done", {result}, idem_key=job.idem_token)` ...
```

The activity **dispatches** the job (reserve at dispatch — 11.7) and returns; it does **not** block on
completion. Completion arrives as a **durable signal** keyed by the `idem_token`, so the workflow holds no
runtime while a multi-hour CI job runs. The signal is **idempotent on `idem_token`** — the runner can deliver
"done" twice (at-least-once) and the workflow wakes once. This is the concrete pattern behind the
merge-queue's `ci.result` wait (X-1) and any long CI stage. The `idem_token` is minted by the workflow at
dispatch and stamped on the job, so the producer (runner) and consumer (workflow) agree on the key without
coordination.

### Per-effect `idem_key` for batch / partial HITL approval (Chat)

A batch approval card may gate **multiple effects** (e.g. "approve these 3 proposed merges"). The signal
idempotency key is **per-effect**:

```
idem_key = card_id                    // single-effect card: one approval, double-click is idempotent
idem_key = card_id ":" effect_idx     // multi-effect card: each effect approved independently and idempotently
```

A **partial approval** (approve effects 0 and 2, decline 1) sends three signals
`{card_id:0 = approve}`, `{card_id:1 = decline}`, `{card_id:2 = approve}`; each is idempotent on its own key,
each maps to exactly one `EffectApi::apply` (a declined effect is **withheld** — AG-8, returns a `Denied`
tool error, never mutates). A double-click on "approve all" re-sends the same keys → no double-apply. This
makes "a double-click is one approval" and "a partial approval is well-defined" both true by construction.
**NEW pattern**; frozen (contract index, contract 9.1/9.4 gain the `SCHEDULE_AND_RUN_JOB` idiom + per-effect
`idem_key` rule).

## OQ-G — *resolved as X-7 above* (one erasure posture, `[OPEN — LEGAL]`).

## OQ-H — Worklog / productivity-field sensitivity (works-council / labour-law) + build-data-as-LLM-training + CD-as-PaaS — **NEW legal classification `[OPEN — LEGAL]`**

**The ask.** Are worklog/productivity/estimate fields special-category or works-council-consultable in EU
jurisdictions (GD-13)? Plus build-data-as-LLM-training lawful basis (AG-8) and CD-as-PaaS product scope
(PR-5), both flagged-not-foreclosed.

**Decision (engineering posture; FLAG FOR COUNSEL).** `[OPEN — LEGAL]` — we specify the defensible
engineering posture and flag; we are not counsel:

- **Worklog/productivity/estimate fields** are tagged `#[personal_data(category = behavioural, role =
  tenant-content, basis = TBD-LEGAL, retention = tenant-policy)]` with a **restricted `data_role`** by
  default. Engineering posture: treat them as **potentially works-council-consultable / elevated-sensitivity**
  in EU jurisdictions — meaning (a) they are **excluded from cross-individual analytics and agent-use for a
  restricted subject** by default (the `restrict` suppression already covers this), (b) per-individual
  productivity rollups are **off by default** and gated behind an explicit tenant admin enablement that the
  posture flags as requiring works-council consultation in applicable jurisdictions, and (c) they carry the
  same per-subject DEK crypto-shred as other free-text PII. **Counsel must ratify** whether these are
  special-category (Art. 9) or merely elevated, and the works-council consultation trigger per jurisdiction.
- **Build-data-as-LLM-training (AG-8):** **foreclosed by default** pending lawful basis — tenant build data
  is `role = tenant-content` (processor), and training a model on it is a new purpose requiring its own basis.
  Engineering posture: no platform code path feeds tenant content to model training; the future real-LLM
  adapter is a region-aware, EU-hostable sub-processor (ADR-12.8, AG-9) and training-on-tenant-data is a
  **separately-ratified opt-in**, not a default. Flag for counsel.
- **CD-as-PaaS scope (PR-5):** a **product/commercial** scope question, not an engineering blocker; the CI
  sandbox + reserve/settle + residency primitives already support it. Flagged to Commercial, not foreclosed.

**NEW legal classification**; flagged. (Contract index: folded into 10.2 classify tags + the 10.9 posture.)

## OQ-I — The cross-cell PII-free pointer bridge shape — **CONFIRM (the named multi-cell floor)**

**The ask.** The seam ISS cross-cell portfolio rollup, KN cross-cell collab, and CHAT cross-org channels all
ride: a bridge carrying only `subject`/`type`/`correlation_id`, per-viewer resolution always cell-local.

**Decision (CONFIRM, shape pinned).** The cross-cell pointer bridge (contract 12.6, Tenancy §10, Bus §7.4)
is confirmed as the **named multi-cell floor** — single-home-cell is v1; cross-cell is designed-not-built.
The bridge frame is frozen PII-free:

```
CrossCellPointer {
  subject:        OpaqueSubjectId,    // an opaque id — NEVER a name/email/body (control-plane-pii-free lint)
  type:           ArtifactType,       // what kind of thing is pointed at (issue/page/channel/...)
  correlation_id: CorrelationId,      // ties it to the originating causal chain (BUS-5)
  home_cell:      CellId,             // where it lives; resolution happens THERE
}
```

**Resolution is always cell-local.** A viewer in cell A wanting to render a pointer to an artifact homed in
cell B does **not** fetch B's data into A. Instead: A's gateway, holding the viewer's identity, asks **cell
B** to `resolve(ref, viewer, mode)` (contract 5.2) **in B**, permission-checked **in B** against B's tuples,
and returns only the **already-rendered, already-permission-filtered projection** (or a tombstone) — never
raw rows, never PII that should stay in B (EI-02 §1; ADR-11 no-cross-region-PII). The control plane carries
only the pointer; the **resolution is a per-viewer cell-local projection fetch**. This is exactly the
PII-free bridge: ISS portfolio rollup aggregates *projections* (counts, titles the viewer may see), KN
cross-cell collab and CHAT cross-org channels resolve membership/content **in the home cell**. The DSR
orchestrator iterates `member_cells` (contract 10.4) over the same bridge. **CONFIRMED**; floor named.

## OQ-J — Resume-cursor / resync protocol + per-view subscription scope discipline over the firehose — **SHARPEN (co-designed once)**

**The ask.** Co-design the resume-cursor discipline once for: a huge board (ISS), a hot doc (KN KD-8), a hot
channel (CHAT). The firehose (contract 3.5) is the shared transport; a dropped connection must lose nothing,
and a per-view subscription must be scope-bounded so one client cannot subscribe to the whole tenant's
firehose.

**Decision (SHARPEN — one protocol, frozen).** Define the **resume-cursor subscription** over the firehose
once; all three surfaces use it. This is the doctrine's "build the durable resume-cursor transport FIRST,
the CRDT slots into it" (EI-04 §2.2, KN-1):

```
subscribe(stream, scope, cursor?) → SubStream      // stream e.g. fan.<tenant>.<channel>; scope BOUNDS what frames arrive
SubStream yields Frame { seq: u64, ... }            // seq is per-(stream,scope) monotonic
resume(stream, scope, last_seq) → backfill from (last_seq, now] then live    // the gap is replayed, never lost
```

- **Resume cursor.** Every frame carries a per-`(stream, scope)` monotonic `seq`. On reconnect the client
  sends its `last_seq`; the transport **backfills `(last_seq, now]`** from a bounded firehose retention
  window, then resumes live. A reconnect **loses zero ops** (the T-5 "reconnect-loses-zero-ops" drill is the
  pass condition). If `last_seq` is older than the retention window, the client gets a **`resync_required`**
  signal and falls back to a full `*.snapshot` replay (Bus §4.9, sub-artifact-granular) — the cold-rebuild
  path, named, not silent.
- **Per-view scope bounding (the head-of-line + cost discipline).** A subscription's `scope` is a
  **bounded selector**, never `*`: a board subscribes to `scope = board:<id>` (the issues in *that* board's
  current filter), a doc to `scope = doc:<id>` (its block subtree), a channel to `scope = channel:<id>`.
  The transport rejects an unbounded/over-broad scope (the whitelist-not-`*` rule, BUS-3, generalised to the
  firehose). A huge board paginates its scope (the visible window + a margin), so a 50k-row board does not
  stream 50k live frames to one client. This is the per-view subscription scope discipline: **the client
  declares the bounded slice it is looking at; the firehose delivers only that slice's frames + presence.**
- **Backpressure.** Per-connection in-flight frame caps; over-cap sheds in the firehose's own bounded queue
  (EI-02 §5); a slow consumer is dropped to a `resync_required` rather than buffering unboundedly. The
  per-surface shed budgets are OQ-K.

**SHARPENED → frozen** (contract index, contract 3.5 firehose gains the `subscribe/resume/scope` protocol).
This is co-designed once and used by ISS/KN/CHAT identically.

## OQ-K — Per-surface shed budgets + protected-human-lane reservation sizes — **CONFIRM + per-surface budget table (floor)**

**The ask.** Per-surface shed budgets (in-flight caps + protected-human-lane reservation) tuned to each
storm profile: CI-surge, collab op-stream, connection-storm, agent-mention-storm. The 30×-agent-surge /
connection-storm drills assert against these.

**Decision (CONFIRM the discipline; name the v1 budgets as a FLOOR).** ADR-16 (backpressure + protected
human lane + shed order speculative → batch/CI → agent → human-last) stands. Phase 5 names the **per-surface
v1 budget floor** — these are **named floors**, tuned by the drills (T-5), not claimed-final:

| Surface | Storm profile | In-flight cap (per tenant) | Protected-human-lane reservation | Shed order applied |
|---|---|---|---|---|
| CI dispatch | CI-surge (30× agent) | bounded run-queue per tenant; runners pull-bounded | n/a (CI is batch lane) | batch/CI shed before agent? — **no**: CI and agent share the wallet; shed speculative → batch/CI → agent → human-last |
| Collab op-stream (KN) | hot-doc edit/read storm | per-doc op in-flight cap; read-fanout bounded | a reserved fraction for the **active editors** vs passive viewers | viewers shed before editors; agents shed before humans |
| Connection tier (CHAT) | connection-storm | per-tenant connection cap + per-connection frame cap | reserved connection slots for interactive humans | speculative/presence shed first; message delivery last |
| Agent-mention (CHAT/all) | agent-mention-storm | per-tenant agent-run in-flight cap (reserve/settle refuses over-cap) | humans never queue behind agent runs (lane) | agent lane sheds with `429 + Retry-After`; the agent runtime honours it (ADR-16.3) |

The concrete numbers are each subsystem's **P4 budget call** (CR §11), asserted by the drills; the **floor**
is "every one of these is bounded, has a reserved human lane, and applies the shed order" — an unbounded one
is the cascade (EI-02 §5). **CONFIRMED**; budgets named as floors. (Contract index: 1.11 shed order + a
per-surface budget note.)

## OQ-L (incl. X-5) — Threading / comment-primitive consolidation + shared templating — **CONFIRM (v1 separate; named follow-on)**

**The ask.** Knowledge v1 ships KB-native comment threads but flags "reuse the Chat threading primitive?"
(KN KQ-2, the X-5 conflict); Chat owns threads-first conversation. Risk: two threading/comment primitives
diverge before consolidation. Plus shared templating (KN KQ-3, with ISS/CI).

**Decision (CONFIRM the floor; name the consolidation follow-on).** **v1 ships two separate threading
implementations**, but **over one shared sub-artifact + content + ref scheme**, so consolidation later is a
merge, not a rewrite:

- Both Chat threads and Knowledge/Issues comment threads use the **same `#thread-`/`#comment-` `#sub`
  grammar** (X-4/OQ-D) and the **same `myelin-content` AST** (X-2/OQ-B) and emit **the same
  `refs.edge.created` events** (contract 5.4). So a thread is addressable, referenceable, and renderable
  identically regardless of which subsystem hosts it.
- **v1 floor:** Chat owns conversation-threads (real-time, presence, the connection tier); Knowledge/Issues
  own document-anchored comment threads (anchored to a block/line/field via `#sub`). They are **separate
  stores** because their concurrency/transport profiles differ (Chat: firehose live tier; Knowledge: comment
  on a CAS-guarded block). This is the named floor.
- **The consolidation follow-on (named, not "someday"):** when document-anchored comments need real-time
  multi-party presence (the trigger), promote them onto the **Chat threading primitive + the firehose
  resume-cursor transport** (OQ-J). Because they already share `#sub` + content + refs, the promotion swaps
  the store/transport, not the data model. Tracked in the gap report (E-3) as "KB-native comments floor →
  Chat-threading consolidation."
- **Shared templating (KN KQ-3):** the **`humanise` / ICU-MessageFormat template registry** (contract 7.3,
  NOTIF-1) is the **one** templating surface — backend-humanised, `ArtifactRef`-paired, inherited by every
  consumer and every agent-authored message. Knowledge living-doc templates, Issues SLA strings, and CI
  status summaries all register into it; there is no second template engine. **CONFIRMED.**

**CONFIRMED**; floor + named follow-on. (Contract index: no new contract; the `#sub`/content/refs/humanise
contracts already carry it — the note is added that threading is "two stores, one scheme, consolidation
named.")

---

# Part 3 — Folding the eleven grouped change-request sections into the refined contracts

> For each CR section (change-requests §1–11): how the de-duplicated asks fold into the **refined** contract
> shapes. Most are **CONFIRM** (the Phase-3 seam was right); the genuinely-new shapes are marked **NEW**, the
> sharpened encodings **SHARPEN**. The per-system punch list is §4.

## §1 Identity & Access
- **`list_objects` push-down** → **SHARPEN**, the `SetExpr` shape (OQ-E). Frozen.
- **Per-subsystem ReBAC namespace fragments** → **CONFIRM** (contract 4.9). Each subsystem declares its
  fragment; Identity owns the engine and never invents object ids. The frozen per-subsystem fragments:
  Git (ref-glob-scoped relations + CODEOWNERS-as-relations + `approve_untrusted_ci`); CI (`ci_project /
  environment / secret / run` + the `read & !is_untrusted_fork` ABAC edge); Issues (`issue` namespace +
  field/transition ABAC caveats); Knowledge (page-tree inheritance-with-overrides + row-level + field-level
  caveat); Chat (`channel.read = member + parent_project->read`). Each declares a `watcher` relation per
  watchable type (Notif read-fanout).
- **Field/transition ABAC caveat at `check`-time, off the hot path** → **SHARPEN**, the `CaveatContext`
  shape (OQ-E). Frozen.
- **`resolve_pseudonym`/`erase` + pseudonym grammar** → **CONFIRM + NEW grammar pin**: the pseudonym grammar
  is frozen `<pseudonym>@<tenant>.noreply` (CR §1, X-7). Git commits pseudonymous-by-default (GIT-1).
- **Machine-identity resolution (SSH/deploy-key/PAT/per-job token → Principal)** → **CONFIRM** (contract 4.1,
  S-11) **+ NEW self-hosted-scope specificity**: a self-hosted runner token is scoped to **one tenant's
  `SelfHosted` jobs** (cannot mint cross-tenant); a deploy key is a repo-scoped machine principal; a per-job
  token is attenuated and re-mintable mid-workflow on resume (S-11).
- **Zookie from `write_tuples`, stamped on the object** → **CONFIRM** (contract 4.6, the new-enemy guard):
  `page.acl_zookie`, Chat membership writes. A just-revoked grant cannot read stale on the next collab/read.
- **`mint_run_token` mid-workflow on resume** → **CONFIRM** (contract 4.7).
- **`list_subjects(channel, watcher)` performant at 50k-member density** → **CONFIRM** (contract 4.4): the
  read-fanout half of the fanout boundary; served by the same authz tuple index as OQ-E (the reverse index).

## §2 Event Bus (incl. firehose)
- **`<subsystem>.*` taxonomy registered under §6 grammar** → **CONFIRM** (contract 2.9). Each subsystem owns
  + completes its dotted-name list; Bus validates. New: `ci.check.updated`, `ci.result` (X-1).
- **New type token `initiative`** → **NEW token** (sanctioned §6.2 extension): a ranked `issue`-family type.
- **Per-aggregate ordering at production QPS** → **CONFIRM** (contract 2.3, the D-9 drill): per-**ref** (Git
  push QPS), per-**conversation** (Chat total order). The outbox order == the state-change order
  (`UNIQUE(aggregate, seq)`).
- **Firehose sized for heaviest producers + `tail(stream, range)`** → **CONFIRM + sizing** (contract 3.5):
  `ci.log.appended` (heaviest), KN collab op-stream + presence, Chat live delivery + presence + agent partials.
- **`replay(scope, since)` sub-artifact-granular `*.snapshot`** → **CONFIRM** (contract 2.6): CI one-run
  scope, KN page-subtree at block granularity, all (cold rebuild + post-restore re-erasure).
- **`EventMatcher` cost-bound suffices; relational/projection-state conditions** → **CONFIRM/RECONCILE**
  (contract 3.4): the `QueryAst` (OQ-C) **is** the matcher core — it expresses CI's `on: pull_request`/
  `issue.transitioned` filters and Issues' `arm_trigger` over `issue_relation` projection state ("all
  `blocked_by` resolved"). No per-subsystem CEL; the `Has`/`Ref`/`In` predicates over projection state cover
  the relational condition.
- **Firehose presence/typing/read-state subject grammar + TTLs + durable-vs-firehose split** → **CONFIRM**
  (contract 3.5, the OQ-J protocol): `fan.<tenant>.<channel>`, presence/typing/partial subjects ride NATS
  core as an agreed seam.

## §3 Reference Graph
- **`#sub` grammar + outdated/tombstone semantics** → **SHARPEN**, the unified grammar + ladder (X-4/OQ-D).
- **TE-7 typed-edge mirror carries each subsystem's rel vocabulary** → **CONFIRM** (contract 5.5, REF-1):
  Issues `issue_relation` (parent/blocks/closes/...) and Knowledge `db_relation`/`page_parent`
  (parent/relates/rollup_source) are the source of truth; Refs holds the rebuildable projection + fixes the
  inverse pairing.
- **Edge production from commit trailers / PR links / two-way db relations; best-effort eventual inverse**
  → **CONFIRM** (contract 5.4): `Closes ISSUE-412`, `Co-authored-by` → `refs.edge.created`; reindex-from-source
  corrects drift (REF-4).
- **Reconcile "the human key IS the ArtifactRef id" with REF-3** → **RECONCILE → frozen** (the section-3
  blocking item). Decision: the **canonical `<id>` segment is the stable mintable key** the subsystem owns
  (Issues: `ENG-1421` — the project-prefix + monotonic number **is** the `<id>` in
  `myelin://<tenant>/issue/issue/ENG-1421`); the **render-time display projection** `#1421` is derived by the
  UI (REF-3: display keys are render-time, never stored as the link). No contradiction: the human-readable
  *canonical key* is the stored id; the short *display form* (`#1421`, dropping the project prefix in-context)
  is the render-time projection. The ArtifactRef id grammar for Issues is therefore `<PROJECTKEY>-<seqno>`,
  agreed before keys are minted. (TE-14 human-readable monotonic key, resolved.)

## §4 Search
- **Code-/content-/struct-shaped `IndexSpec`, incremental, ACL-aware via `list_objects`** → **CONFIRM**
  (contract 6.3): Git code projection (path/symbols/literals/commit-message + trigram, camel/snake tokenizer);
  Issues Tier-3 escalation (board query over budget → Search query, ACL-pre-filtered via the **same** OQ-E
  `Filter` — blocking for Tier-3, now unblocked); KN block-level + page-level, multilingual, **vector-in-v1**
  for RAG, struct queries over JSONB. The `search-requires-acl-filter` lint holds (every query conjoins
  `list_objects`).
- **Embeddings purged with source on `*.erased`** → **CONFIRM** (contract 10.1 / 11.3): KN content vectors,
  Chat message vectors; HYOK `can_derive_plaintext_index()=false` **structurally** skips indexing.
- **Projection-feeder frequency signal → measured promotion** → **CONFIRM + feeder signal** (OQ-C threshold):
  a facet filtered often → generated index; GIN serves until measured promotion.
- **Consume CI-produced SCIP/LSIF for "find usages"** → **NEW (future; named)**: semantic code search as a
  later index input (GF-3 follow-on, jointly Git+CI). Named in the gap report.

## §5 Notifications
- **"My Work"/"Activity/Mentions" = `list_inbox(principal, filter)` over the ONE inbox; shared read-state**
  → **CONFIRM** (contract 7.1, C-9): never a second store; Issues assigned/blocked/needs-approval/overdue and
  Chat Activity/Mentions are reason/subject **filters**. Mark-once-consistent-everywhere.
- **`define_notif_rule` set + `humanise` templates each subsystem registers** → **CONFIRM** (contract 7.6/7.3,
  NOTIF-1): Issues SLA at-risk/unblocked/approval-requested, KN mentions/comments/shares/watched-page changes,
  Chat mentioned/replied/thread_watched/approval_requested + agent-message strings. Backend humanisation
  paired with a routable `ArtifactRef`, never a frontend string map.
- **`watcher` relation declaration + read-fanout** → **CONFIRM** (contract 4.9 + Notif §8.3): all declare
  their watcher set; served by the authz tuple index (OQ-E reverse index).
- **Escalation-chain config shape (`oncall_now`/`page`)** → **CO-DESIGN → frozen** (contract 7.5): Issues
  passes the chain-definition shape; an SLA breach starts a durable escalation workflow (`page` → `oncall_now`
  → escalate-after-timer, all on the `myelin-flow` timer wheel).

## §6 Agent Fabric
- **`ToolHands::exec` = CI runner's `kind=agent` job; the escape drill gates both** → **CONFIRM** (X-6,
  contract 8.4): untrusted execution built + drilled once, metered into the same wallet.
- **Subsystem `ToolDef` set + `requires_approval` defaults → one `ToolSurface`, MCP-exposable** → **CONFIRM**
  (contract 8.1, the frozen defaults table in X-6).
- **`agent_needs_human` HITL gate enforced; agent legibility metadata** → **CONFIRM** (contract 8.2, AG-8 +
  ADR-08): an agent cannot bypass required human approval on `git.merge`/`open_pr`; `is_agent`/`agent_run`
  rendered distinctly (AI-Act labelling).
- **Content-addressed agent-trace write (AG-7) + erasable holder** → **CONFIRM** (contract 8.8): Knowledge
  reuses the block model; registers it as an erasable `PersonalDataHolder`.
- **Explicit-first dispatch** → **CONFIRM** (CHAT-1, AG-6): a mention notifies, does not auto-spawn a costed
  run; reserve/settle gates even the explicit run. (Implicit auto-dispatch is L-3, counsel-gated.)

## §7 Durable Workflow
- **`SCHEDULE_AND_RUN_JOB` + `ci.result:<job_id>` signal handshake** → **NEW pattern** (OQ-F). Frozen.
- **`DurableExecutor::signal` idempotency + per-effect `idem_key` for batch/partial approval** → **CONFIRM +
  small joint decision → frozen** (OQ-F, the `card_id:<effect_idx>` rule).
- **Multi-day HITL via `wait_for_signal` holding no runtime** → **CONFIRM** (contract 9.4): the bridge across
  CI protected-env deploys, Chat approval card, KN approval-card resume, Issues escalation.
- **Reserve/settle as workflow bookends (refuse-on-exhaustion, never interrupt in flight)** → **CONFIRM**
  (contract 11.7, CI-2): the one metering path; CI + Issues/KN/Chat (via `EffectApi` reserves).
- **Maintenance ops as resumable activities; SLA timer re-arm on pause/resume** → **CONFIRM** (contract
  9.2/9.3): Git GC/repack/bundle/history-rewrite as resumable activities; Issues' cheap disarm/re-arm of the
  precomputed `fire_at` without polluting the wheel with calendar logic (the minute-bucket wheel).
- **Durable timers + signals for scheduled/living-doc automations** → **CONFIRM** (contract 9.3): KN
  daily-notes, living-doc maintenance ride the timer wheel.

## §8 Storage (BlobStore / KMS / log tier / OLAP)
- **Per-subject DEK for free-text/op-log/body columns (GD-4)** → **CONFIRM + NEW per-subject granularity for
  CI logs** (contract 11.4): Git PR/review/comment bodies + reflogs/bitmaps/pack backups; CI free-text PII in
  log segments (GD-6 floor — now per-subject, not per-tenant, where a subject's PII is isolable); Issues row
  free-text + change-log; KN free-text blocks + PII-bearing ops + agent trace; Chat message bodies/drafts
  (the canonical GD-4 case). An individual's erasure crypto-shreds exactly their reachable content incl.
  backups.
- **Object-backed pack/delta over `BlobStore` + smart-transport read path** → **CONFIRM the seam** (contract
  11.2, STOR-5): impl is the Git P4 deliverable (TE-24); the v1 data model keeps repos relocatable, never
  node-pinned.
- **Within-EU CDN-distributable clone/bundle blob class** → **NEW**: a named blob class + within-EU CDN
  posture (hot-repo/clone-storm acceleration), residency-respecting (no extra-EU edge for PII; clone bundles
  are content-addressed, the tenant's region pins them).
- **T3 log tier sealing firehose frames into T2 content-addressed segments + OLTP `(job,step,byte-range)`
  index** → **CONFIRM + CI specificity** (contract 11.x / Storage §3.3): append-mostly, per-tenant-DEK,
  byte-range index keyed by `(job, step)`; the jump-to-failure `details_ref` (X-1) resolves through this index.
- **Trust-tier/branch-scoped cache namespaces in `BlobStore`** → **NEW**: a scope-key convention over
  per-tenant `BlobStore` — an `UntrustedFork` write cannot reach the trusted cache scope (the poisoned-cache
  defence, ties to X-1 trust tiers).
- **Content-addressed snapshot/media blobs, crypto-shred on erase; restore-consistency cross-seam; Scylla
  hot-tier promotion seam** → **CONFIRM** (STOR-1/STOR-4, ADR-18): KN CRDT snapshots + media (BLAKE3),
  row↔blob↔offset consistency; Chat `MessageStore` Scylla promotion residency-pinned + crypto-shred per cell.
  The restore-verify drill asserts cross-seam consistency (T-5).
- **OLAP read store accepts the subsystem's event stream + honours the restriction flag** → **CONFIRM +
  restriction-flag propagation** (contract 11.6): Issues `issue.*`/`sla.*`/`cycle.*` for CFD/cycle-time/
  velocity; **no analytics for a restricted subject** (the `restrict` suppression flows into OLAP — a
  compliance gate). The worklog-sensitivity classification (OQ-H) governs which fields are analytics-eligible.

## §9 GDPR / Audit + Legal `[OPEN — LEGAL]`
- **Harness auto-registration of every store as erasable `PersonalDataHolder`; `restrict` suppresses
  indexing/agent-use/analytics/notif** → **CONFIRM** (contract 1.4 / 10.1): "we forgot a store" structurally
  impossible.
- **The free-text/immutable-content erasure residual — one ratified lawful-basis posture** → **NEW policy
  artifact** (X-7/OQ-G), `[OPEN — LEGAL]`. ONE posture, instantiated per subsystem by reference. Flagged.
- **History-rewrite as audited, tamper-evident, rate-limited tenant op with fork/mirror/clone-cache
  invalidation fan-out** → **NEW** (the Git erasure-admin tool): an audited op (contract 10.6 hash-chain) +
  the invalidation surface (fan-out to forks/mirrors/clone-cache, ties to the trust-scoped cache namespaces).
- **Worklog/productivity/estimate field sensitivity** → **NEW legal classification** (OQ-H), `[OPEN —
  LEGAL]`. Tagged restricted by default; flagged.
- **Build-data-as-LLM-training basis; CD-as-PaaS scope** → **NEW `[OPEN — LEGAL]`** (OQ-H): foreclosed/
  flagged by default.

## §10 Tenancy / Control plane (residency, multi-cell)
- **`residency_verify(tenant)` covers stores; `placement_of`/`discover` usable by the front door; repos
  relocatable** → **CONFIRM + repo-granular specificity** (contract 12.2/12.4): `placement_of(repo)` → cell +
  group, region-pinned, relocatable; `discover` for the git wire; CI runner pool + log/artifact/cache region;
  the no-global-pool property attestable.
- **Cross-cell PII-free pointer bridge** → **CONFIRM** (OQ-I, contract 12.6): the named multi-cell floor.
- **Outbound push-mirror to a foreign host = residency boundary crossing → policy-gated at the control
  plane** → **NEW**: a residency policy gate on outbound mirror (a mirror config that targets an extra-EU
  host for PII-bearing content is denied by default; the `transfer_allowed` registry, contract 10.5, gates it).

## §11 Substrate / shared crates / cross-cutting
- **Cross-language harness shim frozen** → **CONFIRM** (contract 1.7): the three-surface topology,
  liveness≠readiness, no fire-and-forget emit, `PersonalDataHolder`, resilient-client, shed order,
  forward-only migrations — frozen as the contract a non-Rust subsystem (the Chat BEAM/TE-21 connection tier)
  must satisfy. A no-op if Chat stays Rust; the hatch is real either way.
- **Hot-table flags for the forward-only-migration lint** → **CONFIRM** (contract 1.5): every subsystem
  declares its hot tables (KN `block`/`db_row`/`doc_op`, and all high-write subsystems).
- **Per-surface shed budgets** → **CONFIRM** (OQ-K, the budget-floor table).
- **WASM compile target for `myelin-content`** → **CONFIRM** (build-system): the one editor render path
  reuses the Rust core client-side; `render(parse(md)) === md` on identical code (D10 round-trip gate).
- **`myelin-query` primitive parity + ADF→`myelin-content` converter fidelity** → **SHARPEN** (X-3/OQ-C +
  X-2/OQ-B): the field-type enum / view-model / AST / `order_key` are frozen byte-identical; the ADF lossy-map
  is frozen.

---

# Part 4 — Per-system punch list (what each system/subsystem must now do)

> The single source of "what changed for me." Each item references the contract index entry it implements.

### Identity & Access (`myelin-identity`)
- Implement the `SetExpr` push-down (4.3) + maintain the **per-tenant authz tuple reverse index** (the JOIN
  target) kept fresh off the bus; the `via_column` lowering contract for all five subsystems.
- Implement the `CaveatContext` field/transition ABAC at `check`-time (4.2), kept off the `list_objects` path.
- Compile each subsystem's ReBAC namespace fragment (4.9), incl. Git `approve_untrusted_ci`, CI
  `!is_untrusted_fork`, the per-watchable-type `watcher` relation.
- Pin the pseudonym grammar `<pseudonym>@<tenant>.noreply` (4.8); self-hosted runner token scope (4.7).
- Fail-static bounded-staleness cache (4.11) ≤ revocation SLA; DPO ratifies the bound (L-1).

### Event Bus (`myelin-events`)
- Register `ci.check.updated`, `ci.result`, `initiative` and complete each taxonomy (2.9).
- Firehose `subscribe/resume/scope` resume-cursor protocol (3.5, OQ-J); per-`(stream,scope)` `seq`;
  `resync_required` fallback to `*.snapshot`.
- Confirm `QueryAst` is the sole `EventMatcher` core (3.4); no per-subsystem CEL.

### Reference Graph (`myelin-refs`)
- Implement the unified `#sub` grammar + the 4-step tombstone ladder (5.7, OQ-D); store full sub-URN +
  stripped root.
- Freeze the Issues ArtifactRef id grammar `<PROJECTKEY>-<seqno>` as the stored canonical key; `#1421` is
  render-time (5.1, REF-3 reconciliation).
- TE-7 typed-edge mirror inverse pairing (5.5).

### Search (`myelin-search`)
- Conjoin the OQ-E `Filter` in every query (6.1) — the Tier-3 Issues escalation now unblocked.
- Measured projection-feeder promotion threshold (6.3, OQ-C); purge embeddings on `*.erased` (10.1).
- Named follow-on: SCIP/LSIF "find usages" index input (gap report).

### Notifications (`myelin-notif`)
- `list_inbox` filters = the ONE inbox (7.1); register subsystem `define_notif_rule` + `humanise` templates
  (7.6/7.3) — the single templating surface (OQ-L).
- Escalation chain shape (7.5) on the timer wheel.

### Agent Fabric (`myelin-agent`)
- `ToolHands::exec` = CI `kind=agent` job (8.4); the four uniform guarantees (X-6); the frozen
  `requires_approval` defaults table (8.1).
- AG-7 trace as erasable holder (8.8); explicit-first dispatch (CHAT-1).

### Durable Workflow (`myelin-flow`)
- `SCHEDULE_AND_RUN_JOB` long-park-completed-by-signal idiom (9.1/9.4, OQ-F); per-effect `idem_key`
  (`card_id:<effect_idx>`).
- Merge-queue workflow + `ci.result` wait (X-1); resumable maintenance activities + cheap timer re-arm (9.3).

### Storage
- Per-subject DEK for all free-text/body/op-log incl. **CI log segments** (11.4); the `(job,step,byte-range)`
  index (Storage §3.3); trust-tier/branch-scoped cache namespaces (NEW); within-EU CDN clone/bundle class
  (NEW); restriction-flag into OLAP (11.6).

### GDPR / Audit
- The ONE free-text/immutable erasure posture (10.9, X-7) instantiated by reference; history-rewrite audited
  op + invalidation fan-out (NEW); worklog sensitivity tags (10.2, OQ-H). All `[OPEN — LEGAL]` items flagged
  to counsel/DPO (L-1..L-4).

### Tenancy / Control plane
- Repo-granular `placement_of` + relocatable repos (12.2); the cross-cell PII-free pointer bridge frame
  (12.6, OQ-I); the outbound-mirror residency gate (NEW, 10.5 `transfer_allowed`).

### Subsystems (P4 rewrites in 5-B must build to)
- **Git:** `check_status` projection + supersession + branch-protection `required`-set + fork-endorsement
  (X-1); pseudonymous commits (GIT-1); content-anchored line-range fingerprints (OQ-D); object-backing seam.
- **CI:** emit `ci.check.updated` + `ci.result` + `trust_tier` + `run_attempt` (X-1); own the unified runner
  + escape drill (X-6); `SCHEDULE_AND_RUN_JOB` (OQ-F); the T3 log tier (Storage).
- **Issues:** consume the OQ-E `Filter` (board, blocking — now unblocked); `<PROJECTKEY>-<seqno>` keys; the
  `myelin-query` parity (X-3); ADF import map (X-2); typed `issue_relation` table (TE-7); SLA escalation
  workflow; worklog sensitivity tags.
- **Knowledge:** own the `myelin-content` taxonomy + lossy-map (X-2); CAS-floor → CRDT over the resume-cursor
  transport (OQ-J, KN-1); read-time rollups (KN-3); the WASM editor round-trip; AG-7 trace holder.
- **Chat:** the firehose resume-cursor protocol (OQ-J); per-effect `idem_key` approval cards (OQ-F);
  consumed `myelin-content` subset (X-2); explicit-first dispatch (CHAT-1); the cross-org channel cross-cell
  floor (OQ-I); the connection-tier language decision (TE-21) under the frozen cross-language harness (1.7).

---

## 5. Open items explicitly carried (honesty register)

- **`[OPEN — LEGAL]`** (flagged to counsel/DPO, structural floor ships regardless): the ONE free-text/
  immutable-content erasure posture (X-7/OQ-G, L-2); worklog/productivity special-category classification
  (OQ-H, GD-13); build-data-as-LLM-training basis (OQ-H, AG-8); Art. 17 reach into immutable git bytes;
  audit-log retention carve-out (GD-5); the fail-static staleness bound ratification (L-1).
- **Named floors (gap report E-3):** KB-native comments → Chat-threading consolidation (OQ-L); single-home-
  cell → multi-cell cross-cell bridge (OQ-I); CAS-floor → CRDT (KN-1); node-backed git → object-backed git
  (STOR-5); read-time rollups → materialised-when-measured (KN-3); SCIP/LSIF "find usages" (future Search
  input); the per-surface shed budgets are v1 floors tuned by drills (OQ-K).
- **Measured-not-predicted thresholds:** the projection-feeder promotion threshold (OQ-C), the `order_key`
  48-char rebalance trigger (X-3), the column-store promotion (BUS-6) — all promoted on measurement, not
  prediction (EI-02 §8).

## 6. Cross-references
- [`contract-index.md`](./contract-index.md) — the refined frozen surface incorporating every decision here.
- [`../03-shared-systems-architecture/contract-index.md`](../03-shared-systems-architecture/contract-index.md)
  — the Phase-3 surface this refines (superseded).
- [`../04-subsystem-architectures/cross-subsystem-change-requests.md`](../04-subsystem-architectures/cross-subsystem-change-requests.md)
  — the primary input (CRs §1–11, X-1..X-7, OQ-A..OQ-L).
- Spine: [`../02-holistic-architecture/architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md)
  (ADR-01..ADR-20); [`../02b-doctrine-integration/integration-directives.md`](../02b-doctrine-integration/integration-directives.md).
- Doctrine: [`../../external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md),
  [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md).
