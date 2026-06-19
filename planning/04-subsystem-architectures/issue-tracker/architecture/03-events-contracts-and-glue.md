# Issue Tracker — 03 · Events, Contracts & Glue

> See [`00-overview.md`](./00-overview.md) for the role and [`01`](./01-tech-and-data-model.md)/[`02`](./02-internals-and-algorithms.md)
> for the model and algorithms. This doc is the **build-to glue surface**, implemented against the **frozen**
> reconciled contracts: the complete `issue.*` event taxonomy Issues owns (under the Bus §6 grammar), the events
> it consumes (incl. the frozen `CheckStatus` / `ci.check.updated`), and the implementation of **every** glue
> contract — `ArtifactRef` + the unified `#sub` grammar, `project(ref,viewer)`, `replay(scope,since)`, the
> envelope via the OUTBOX, Identity `check` (+ `CaveatContext`) / `list_objects` (the `SetExpr` push-down) + the
> ReBAC namespace fragment, `PersonalDataHolder` (+ the ONE erasure posture by reference), the `ToolDef`
> registrations (+ the frozen `requires_approval` defaults), reserve/settle, and the stateful Trigger. Names +
> units align to the frozen `EventEnvelope` field list + the `ArtifactRef` token table (the reconciliation
> anchors, contract index §14).

---

## 1. The complete `issue.*` event taxonomy (Issues owns; extends the Bus §6.2 seed)

Under the Bus §6 grammar (`<subsystem>.<artifact_type>.<event_name>` — lowercase, singular tokens, past-tense
verbs). The seed froze the `issue` subsystem's representative tokens; Issues **owns its complete list** (a P4
deliverable, contract 2.9) and the **`initiative` type token is now a registered token** (contract 2.9 / index §14):

| Artifact type | Events owned |
|---|---|
| **`issue`** | `created` · `updated` (carries field deltas; the rollup/sync/feeder input) · `transitioned` (from, to, **category**) · `closed` · `reopened` · `deleted` (soft) · `restored` · `assigned` · `priority_changed` · `type_changed` · `parent_changed` · `archived` |
| **`relation`** | `created` · `removed` (the TE-7 typed-edge event Refs mirrors; one event yields both projection directions — contract 5.5) |
| **`field`** | `defined` · `updated` · `removed` (field-scheme changes; the `#field-<opaqueid>` sub-artifact) |
| **`comment`** | `created` · `updated` · `deleted` (the `#comment-<opaqueid>` sub-artifact; body is `myelin-content`) |
| **`rollup`** | `recomputed` (the derived-aggregate change; feeds roadmap + the forecast agent; `input_hash`-suppressed) |
| **`cycle`** | `started` · `completed` · `issue_added` · `issue_removed` (the time axis; burndown/OLAP) |
| **`milestone`** | `released` (versions/releases) |
| **`sla`** | `started` · `paused` · `resumed` · `at_risk` · `breached` · `met` (compliance feed → OLAP) |
| **`approval`** | `requested` · `granted` · `rejected` · `timed_out` (the HITL gate surface; humanised via contract 7.3) |
| **`initiative`** | `health_changed` (the forecast/drift agent crosses an at-risk threshold → roadmap "date-at-risk") |
| **triage (on `issue`)** | `triaged` · `duplicate_suspected` · `labelled_by_agent` (agent-assist provenance; always attributed) |
| **cross-cutting** | `issue.*.erased` (the tombstone) · `issue.*.snapshot` (reindex-from-source) |

**Units (the frozen names/units anchor):** timestamps RFC-3339 UTC; SLA targets/`stale_after`/durations in
**seconds**; estimates/story-points as **numeric** in the payload (not minor-units — they are not money); the
actor/subject are `ArtifactRef`s (references-not-payloads); `contains_personal_data`/`data_role`/`pii_key_ref`
set on any event whose payload could carry free-text PII. Every event carries the nested
`correlation_id`/`causation_id`/`depth` (derived correct-by-construction by `OutboxTx::emit`).

### 1.1 Events Issues consumes (the cross-subsystem reflexes)

Via the substrate consumer template (contract 2.4 — `subjects()` whitelist, never `*`; idempotent on `event_id`;
bounded prefetch; lag metric):

| Consumed event | Reflex (the cross-sub consumer; flows C1–C4) |
|---|---|
| `git.branch.created` (refs an issue) | create the issue↔branch ref edge (the content producer emits `refs.edge.created`); workflow-permitting auto-transition → *In Progress* |
| `git.pr.opened` / `git.pr.merged` | link PR↔issue (`closes` typed edge); on merge, transition → *Done* if the guard is satisfied |
| **`ci.check.updated`** (the frozen `CheckStatus`, contract 5.9) | feed the "can't mark Done while CI red" guard: the linked PR's commit has a **current** `CheckStatus{state, trust_tier, ...}`; Issues reads it via `project(PR_ref)` at transition time — never recomputes trust (Δ10) |
| `chat.message.created` ("create issue") | `issue.create` with the chat message as a `relates` ref edge |
| `identity.member.added` / deactivated / **erased** | reassign/anonymise: the actor becomes the frozen pseudonym `<pseudonym>@<tenant>.noreply` across history (the erasure lever, §7) |
| `issue.updated` (own) | drive the **rollup**, **SLA pause/resume**, **trigger resolution**, and the **projection feeder** consumers |

---

## 2. ArtifactRef + the unified `#sub` scheme (contracts 5.1, 5.7)

`ArtifactRef = myelin://<tenant>/issue/<type>/<id>[#<sub>]`. Issues uses the canonical `issue` subsystem token
(singular — index §14). The `<type>` is `issue|epic|sprint|field|comment|relation|initiative` (the seed + the
registered `initiative`). The **`<id>` is the stored canonical human key `<PROJECTKEY>-<seqno>`** (`ENG-1421`) —
the frozen REF-3 reconciliation (Δ3, contract 5.1). Examples:
- `myelin://acme-eu/issue/issue/ENG-1421` — an issue.
- `myelin://acme-eu/issue/epic/ENG-1390` — an epic (a ranked issue-family type, same id space).
- `myelin://acme-eu/issue/issue/ENG-1421#comment-7f3a` — a comment sub-artifact.

**The unified `#sub` scheme** (frozen vocabulary, contract 5.7 — each subsystem mints **stable opaque** sub-ids,
stable across edits so embeds don't dangle; Δ4). Issues mints these kinds:
- `comment-<opaqueid>` — a comment/review-thread node (immutable opaque id; never reused).
- `b<opaqueid>` — a content block within the issue **description** (`myelin-content` block; stable across edits).
- `field-<opaqueid>` — a field value (the stable `field_id` UUID, not the display name).
- `row-<opaqueid>` — the issue rendered **as a database row** (issue-as-row in a `db_view`).

`parse`/`format` are the shared `myelin-refs` library (we never write a second URN parser). Refs stores the
**full sub-URN AND the `#sub`-stripped root**, so a broken sub-anchor still resolves to the parent issue. The
**one 4-step tombstone ladder** (contract 5.7) governs resolution: permission → root → sub-resolve
{live/moved/outdated/gone} → erased; a tombstone always carries the root (the embed degrades to "this referenced
ENG-1421 (the specific part is no longer available)"). Display keys (`#1421`, no prefix, in-context) are
render-time only (REF-3).

---

## 3. `project(ref, viewer)` — the projection API (contract 5.6; ADR-13.1)

**The only way another subsystem reads about an Issues artifact (no cross-DB).** Per-viewer,
pre-permission-checked. Consumed by Refs `resolve` (the context-pane unfurl, the PR pane, chat unfurls), Search
(the text projection), and Notif (humanisation). The `Display` mode of `resolve` returns this same projection for
the Notif humanisation surface (contracts 5.2/7.3).

```rust
fn project(reference: ArtifactRef, viewer: Principal) -> Projection | Tombstone {
    let issue = load(reference.id);     // <id> = the canonical ENG-1421 key
    // PRE-PERMISSION-CHECKED: a confidential issue the viewer can't read returns a Tombstone (never leaks the title):
    if Id.check(viewer, "view", issue.ref(), zookie).is_deny() { return Tombstone{ reason: denied, root: issue.root_ref() }; }
    Projection {
        title:       issue.title,                         // free-text — erasure-safe (tombstones if erased)
        state:       issue.state,                         // the NAMED state (humanised)
        category:    issue.state_category,                // the FIXED category (cross-sub "is it done?")
        icon:        type_icon(issue.type_id),
        render_hint: "issue",                             // Refs picks the chip/embed render
        sub_anchor:  resolve_sub(reference.sub),          // #comment-<id>/#b<id>/#field-<id>/#row-<id> via the frozen ladder (5.7)
    }
}
```

This is what makes the **PR context pane** (flow C1.5) show `ENG-1421` inline — Git calls `resolve`, Refs calls
Issues' `project`; **Git never reads the Issues DB** (ADR-13.1). For a **cross-cell** pointer (OQ-I), resolution
is **cell-local**: the viewer's gateway asks the home cell to `project` there, permission-checked there, and only
the rendered projection (or a tombstone) crosses — never raw rows (contract 12.6).

---

## 4. `replay(scope, since)` — reindex-from-source (contract 2.6; the only recovery path)

When Search / Refs / OLAP / Notif rebuild, Issues' `replay(scope, since)` re-emits `*.snapshot` events through the
**live consumer path** — never reads another system; derived stores rebuild drift-free (steady-state and recovery
share one code path).

```rust
fn replay(scope: Scope, since: Cursor) -> impl Stream<Item = Draft> {
    // scope = tenant | project | a single issue | a sub-artifact; SUB-ARTIFACT-GRANULAR snapshots (contract 2.6)
    for issue in scan(scope, since) {
        yield issue.snapshot();                  // issue.issue.snapshot — the full current state (references-not-payloads)
        for rel in relations_of(issue) { yield rel.snapshot(); }       // issue.relation.snapshot (Refs rebuilds the edge projection)
        for c   in comments_of(issue)  { yield c.snapshot(); }         // issue.comment.snapshot (sub-artifact granular)
        if let Some(r) = rollup_of(issue) { yield r.snapshot(); }      // rollup is DERIVED but snapshot-emittable for OLAP convenience
    }
}
```

- **Sub-artifact granularity:** a snapshot can be scoped to a single comment or field so Search/Refs reindex one
  changed sub-artifact, not the whole tenant.
- **Imported data rebuilds the same way** — import emits the normal `issue.*` events (one indexing path), so
  reindex-from-source works on imported data for free (sketch 09; flow C4).
- **The rollup aggregate is rebuilt by replay** (it is derived; the edge truth is `issue_relation`) — the
  reindex-from-source rollup/edge parity drill proves it (D8).

---

## 5. The envelope via the OUTBOX (contracts 2.1–2.3)

`OutboxTx::emit(draft, cause)` is the **only** sanctioned emit path — in the same transaction as the state change
(no dual-write hazard; no `publish_now`). Causality is derived correct-by-construction (the nested
`correlation_id`/`causation_id`/`depth`). The `outbox` table is the per-service cross-seam anchor
(`(event_id UNIQUE, aggregate, seq, subject, envelope)`, `UNIQUE(aggregate, seq)` ordering — the **issue is the
aggregate**, so per-issue ordering is preserved at production QPS, the D-9 drill). The relay drains it
(`FOR UPDATE SKIP LOCKED`). The **audit append is a bus consumer** of the same events (audit-via-outbox, not a
second write path — contract 10.6). The `no-raw-publish` lint enforces this at build.

Every state-changing handler in [02 §2/§5/§6](./02-internals-and-algorithms.md) (create, transition, reorder,
relation-write, rollup, SLA) ends in an `OutboxTx::emit` inside the transaction.

---

## 6. Identity — `check` (+ `CaveatContext`) / `list_objects` (the `SetExpr`) + the ReBAC fragment (contracts 4.2/4.3/4.9)

### 6.1 The namespace fragment (Issues declares; Id owns the engine — contract 4.9, frozen)

Issues declares its ReBAC namespace fragment; Identity owns the engine and never invents object ids. The frozen
fragment (the Issues `issue` namespace + field/transition caveats + the `watcher` relation):

```
definition issue {
  relation parent_project:    project
  relation assignee:          user | team#member
  relation watcher:           user                       // the Notif read-fanout relation (contract 4.9 / 4.4)
  relation confidential:      user | team#member         // marks the issue confidential (the exclusion driver)
  relation confidential_grant: user | team#member        // explicit grant for confidential issues
  permission view       = (parent_project->read - confidential) + confidential_grant
  permission comment    = view
  permission transition = assignee + parent_project->write
  permission manage      = parent_project->write
}
definition issue_field {                                 // field-level visibility (a sub-object; ABAC caveat at the edge)
  relation parent_issue: issue
  permission view_field = parent_issue->view             // + the frozen CaveatContext at check-time (§6.2), off the hot list_objects path
}
definition issue_transition {                            // transition-level visibility (governed transitions)
  relation parent_issue: issue
  permission perform_transition = parent_issue->transition   // + the frozen CaveatContext (approver-role)
}
```

- **The `- confidential` exclusion** is Zanzibar's set-difference userset — a confidential issue **disappears from
  a normal project-reader's `list_objects` by construction**, not by a post-filter (the no-leak guarantee, D3).
- **`watcher`** is the read-fanout relation: Notif resolves watchers via `list_subjects(issue, watcher)` (contract
  4.4, performant at density via the same authz reverse index) for the unbounded ambient set; write-fanout handles
  the bounded assignee/mention set.

### 6.2 The `SetExpr` push-down and the `CaveatContext` (the two OQ-E halves; Δ1/Δ2)

- **Row visibility = the `list_objects` `SetExpr` push-down** (contract 4.3, the granted blocking CR-1). Every
  read (board/list/view/search/backlink) conjoins `list_objects(viewer, 'view', 'issue', zookie?)` → either
  `Ids{ids, zookie}` (materialised under the cardinality cap) or `Filter{set_expr, zookie}`. The planner lowers
  `set_expr` over `ColRef{ table:"issue", column:"id" }` into a SQL predicate / JOIN against the **per-tenant
  authz reverse index** — **one query, no N+1, no post-filter** ([02 §3](./02-internals-and-algorithms.md)). The
  `zookie` bounds staleness; a security-sensitive scan passes it so the JOIN reads the index at-or-after the
  zookie's revision (contract 4.10).
- **Field/transition hiding = the frozen `CaveatContext`** (contract 4.2, the granted CR-2), evaluated at
  `check`-time on the already-filtered, already-fetched rows — **never** on the hot `list_objects` path:

  ```
  CaveatContext { object: issue.ref(), field: Some(field_id)|None, transition: Some(t.id)|None, attrs: { "severity": 3, ... } }
  check(viewer, view_field|perform_transition, issue.ref(), zookie?, caveat) → Allow | Deny | Conditional
  ```

  So `list_objects` returns the visible rows cheaply; `check` with the `CaveatContext` then redacts individual
  fields ("hide the salary column"; "field visible iff `issue.severity < X`") or gates individual transitions
  (approver-role) on those rows.

### 6.3 Tuple writes

Tuple writes (assigning, watching, confidential-grant) go through `write_tuples([Δ], precondition?)` returning the
zookie to stamp on the object (emitted via outbox — contract 4.6). A just-revoked grant cannot read stale on the
next board/search because the scan reads the reverse index at-or-after that zookie's revision (the new-enemy
guard, contract 4.10).

---

## 7. `PersonalDataHolder` + the ONE erasure posture by reference (contracts 10.1, 10.9)

Issues implements `PersonalDataHolder` for every store the harness opens (auto-registered with the DSR
orchestrator — contract 1.4). Personal data in Issues: `assignee`/`reporter`/`created_by` (pseudonymous principal
ids), free-text `title`/`props`/comment bodies/change-deltas, attachment filenames, and the worklog/productivity
fields (the frozen behavioural tags, [01 §6.1](./01-tech-and-data-model.md)). Every personal-data field carries
`#[personal_data(category, role, basis, retention, erasure, subject_locator)]` (the `no-untagged-personal-data`
lint fails the build otherwise — contract 10.2).

| Op | Implementation |
|---|---|
| **locate(subject)** | scan for the subject as assignee/reporter/author/mentionee/watcher across `issue`, `issue_change_log`, comments, triggers, the import people-map |
| **export(subject)** | the canonical interchange format (the same format import consumes — sketch 09): their issues, comments, change-log entries, attachments manifest |
| **rectify(subject)** | update the pseudonymous mapping / free-text via the normal edit path (emits events; reindexes) |
| **restrict(subject)** | set the **restriction flag** → no indexing / no agent-use / no analytics / no notification for the restricted subject (contract 10.1); the planner, the projection feeder, **and the OLAP feed** check it (contract 11.6) |
| **erase(subject)** | (1) Id `erase` shreds the pseudonym map → "Former user 8a2f" across all history without rewriting issues others own; (2) crypto-shred the **per-subject DEK** for their free-text PII (title/props/comments/deltas — GD-4, contract 11.4); (3) crypto-shred attachment blobs (BlobStore key destroy); (4) emit `issue.*.erased` tombstones (the live consumers tombstone Search/Refs/OLAP/Notif). Returns a receipt hash-linked into the audit log. |

**The free-text residual is handled per the ONE platform posture, by reference (Δ13).** Third-party free-text PII
(a person's name typed into another person's issue body/comment) is encrypted under the **author's** DEK, so the
subject's erasure does not crypto-shred it. Per the frozen platform posture (contract 10.9, recon §X-7): the
structural floor — per-subject DEK crypto-shred (self-authored) + pseudonym-map shred (identity) + `restrict`
suppression — **ships now**; the residual is under the **documented lawful-basis limit** (best-effort `rectify`/
tombstone of the specific span where the subject identifies it, plus the standing guarantee that the residual is
never indexed / never agent-readable / never in analytics for a restricted subject). Issues **does not restate a
separate residual**; it points at the platform posture. `[OPEN — LEGAL]`: counsel/DPO ratify the residual basis
(one statement, not five). **Post-restore re-erasure** (GD-14) runs against the erasure ledger so a restore can't
resurrect erased data (contract 10.8).

---

## 8. `ToolDef` registrations — the one catalogue humans + agents share (contract 8.1; frozen defaults)

Issues registers its actions into the `ToolSurface` as permissioned `ToolDef`s — the **same catalogue** the
command palette (S19) and agents use (no privileged back-channel; UI=CLI=agent parity). Each declares
`required_caps`, `effect_kind`, `side_effecting`, `requires_approval`, `exposed_over_mcp`. The
`requires_approval` defaults are now **frozen jointly with the Fabric** (contract 8.1, X-6; Δ16):

| ToolDef | `required_caps` | `side_effecting` | `requires_approval` (frozen default) |
|---|---|---|---|
| `issue.create` | `issue.create` on project | yes | no |
| `issue.update` (fields) | `issue.update` | yes | no (a *governed* field → caveat-gated) |
| `issue.transition` | `issue.transition` | yes | **yes if the transition has an approver edge** (the field/transition caveat — X-6 row) |
| `issue.comment` | `issue.comment` | yes | no |
| `issue.link` (relation) | `issue.update` | yes | no |
| `issue.estimate` | `issue.update` | yes | no |
| `issue.reorder` (rank) | `issue.update` | yes | no (same CAS path as a human — [02 §5](./02-internals-and-algorithms.md)) |
| `issue.assign` | `issue.transition` | yes | no |
| `issue.close` | `issue.transition` | yes | **yes if confidential or governed** |
| `forecast` / `triage` / `sla_draft` (agent tools) | compute / `issue.update` | no (forecast) / yes | **no — suggest by default** (X-6 row: advisory; the human accepts the suggestion) |

All side-effecting tools apply via `EffectApi::apply` (schema → capability → delegation → tenant → budget → **HITL
gate** → apply via the **public endpoint**, no carve-out → meter — contract 8.2). A `Denied` is an ordinary tool
error; a withheld gated tool does not mutate (AG-8). The **forecast agent** registers `forecast` (compute-only,
reads OLAP; the at-risk threshold is config). `run --dry-run` returns the proposed effects without applying
(plan-then-apply testability — contract 8.7; the triage agent's S9 suggestions). The four uniform sandbox
guarantees (cost gate, per-run-token attribution, HITL withhold, isolation floor+drill) are inherited by
construction from the unified runner (contract 8.4, X-6) — Issues re-implements none of them.

---

## 9. Reserve/settle — spend-bearing agent work (contract 11.7)

Where Issues runs spend-bearing work — the **triage agent**, the **forecast agent**, the **SLA-draft agent**, any
agent invoked via an automation/trigger — the run is a **durable workflow** (contract 9.5) with the reserve/settle
gate as its bookends: `reserve` at dispatch (no balance → no start), `settle` on completion (never interrupt
in-flight). Metering is integer minor-units; CI runs and agent runs meter into the **same wallet** (Commercial
C-1). The HITL approval card surfaces a **live cost estimate** before a human approves a gated effect (flow B2).
Issues does not own the wallet — it consumes the gate.

---

## 10. The stateful Trigger — the Issues-side ownership (contract 3.3, frozen `QueryAst` condition)

Issues **owns the Issues-side Trigger UX and semantics** (the armable conditions + the armed/resolved/stale
surface); it **consumes** the bus `arm_trigger`/`disarm_trigger` primitive, the `myelin-flow` `stale_after`
durable timer (contract 9.3), and the one Notif inbox for `on_resolve`. The condition is the **frozen
`myelin-query` `QueryAst`** over projection state (Δ8, contract 3.3/3.4) — the granted CR-5.

- **The armable-condition catalogue** — each is a frozen `QueryAst` over `issue.*` events and `issue_relation`
  projection state (`Has`/`Ref`/`In` predicates express the relational condition; no per-subsystem CEL):
  - **"Remind me when unblocked"** (the flagship) — `condition: Has(blocked_by) = false after all blocked_by
    edges resolve` (reads `issue_relation`, [01 §4](./01-tech-and-data-model.md)).
  - **"Ping me when this leaves triage / state X"** — `condition: Cmp(state_category, ne, X)`.
  - **"Notify me when assigned to me."** — `condition: Ref(assignee, me)`.
  - **"Tell me when SLA at risk."** — driven by `sla.at_risk`.
  - **"Tell me when this initiative goes at-risk"** — driven by `initiative.health_changed`.
- **Lifecycle:** `armed → {resolved | stale | disarmed}`, fires **once per arming** (contract 3.3). The last
  blocker closing → `issue.relation`/`issue.transitioned` → the bus resolves the trigger → **one** inbox item
  (humanised via contract 7.3, with the routable ArtifactRef). After `stale_after` (default 30d, a `myelin-flow`
  durable timer) with no resolution → a "still blocked after 30d — escalate?" nudge → the trigger goes stale.
  **No silent forever-armed promises.**
- **Why it is the make-or-break agent-adjacent UX:** instead of an agent *doing* something, the platform *watches
  on your behalf* and re-surfaces precisely when relevant — calm-by-default, zero polling, durable across
  restarts/days. "My Work that comes to you." The **Trigger-fires-once-after-restart** drill proves the durability
  (D7).

Continue to [`04-views-cli-and-api.md`](./04-views-cli-and-api.md).
