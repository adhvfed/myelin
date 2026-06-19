# Issue Tracker — 03 · Events, Contracts & Glue

> See [`00-overview.md`](./00-overview.md) for the role and [`01`](./01-tech-and-data-model.md)/[`02`](./02-internals-and-algorithms.md)
> for the model and algorithms. This doc is the **build-to glue surface**: the complete `issue.*` event taxonomy
> Issues owns (under the Bus §6 grammar), the events it consumes, and the implementation of **every** glue
> contract — `ArtifactRef`, `project(ref,viewer)`, `replay(scope,since)`, the envelope via the OUTBOX, Identity
> `check`/`list_objects` + the ReBAC namespace fragment, `PersonalDataHolder`, the `ToolDef` registrations, and
> reserve/settle on spend-bearing work. Reconciles names+units with X-5 against the canonical envelope field
> list (`00 §2.10`) and the token table (Bus §6.2).

---

## 1. The complete `issue.*` event taxonomy (Issues owns; extends the Bus §6.2 seed)

Under the Bus §6 grammar (`<subsystem>.<artifact_type>.<event_name>` — lowercase, singular tokens, past-tense
verbs). The Bus §6.2 seed froze the `issue` subsystem's representative tokens as `issue, epic, sprint, field,
comment, relation`; the seed events as `issue.issue.created|updated|transitioned|closed`, `issue.relation.created`.
Issues **owns its complete list** (a P4 deliverable) and **adds the `initiative` type token** (a ranked
issue-family type — sketch 01; Bus §6.2 extension):

| Artifact type | Events owned |
|---|---|
| **`issue`** | `created` · `updated` (carries field deltas; the rollup/sync/feeder input) · `transitioned` (from, to, **category**) · `closed` · `reopened` · `deleted` (soft) · `restored` · `assigned` · `priority_changed` · `type_changed` · `parent_changed` · `archived` |
| **`relation`** | `created` · `removed` (the TE-7 typed-edge event Refs mirrors; one event yields both projection directions) |
| **`field`** | `defined` · `updated` · `removed` (field-scheme changes; the `#field-…` sub-artifact) |
| **`comment`** | `created` · `updated` · `deleted` (the `#comment-N` sub-artifact; body is `myelin-content`) |
| **`rollup`** | `recomputed` (the derived-aggregate change; feeds roadmap + the forecast agent; `input_hash`-suppressed) |
| **`cycle`** | `started` · `completed` · `issue_added` · `issue_removed` (the time axis; burndown/OLAP) |
| **`milestone`** | `released` (versions/releases) |
| **`sla`** | `started` · `paused` · `resumed` · `at_risk` · `breached` · `met` (compliance feed → OLAP) |
| **`approval`** | `requested` · `granted` · `rejected` · `timed_out` (the HITL gate surface; humanised) |
| **`initiative`** | `health_changed` (the forecast/drift agent crosses an at-risk threshold → roadmap "date-at-risk") |
| **triage (on `issue`)** | `triaged` · `duplicate_suspected` · `labelled_by_agent` (agent-assist provenance; always attributed) |
| **cross-cutting** | `issue.*.erased` (the tombstone) · `issue.*.snapshot` (reindex-from-source) |

**Units (X-5 reconciliation):** timestamps RFC-3339 UTC; SLA targets/`stale_after`/durations in **seconds**;
estimates/story-points as **numeric** in the payload (not minor-units — they are not money); the actor/subject
are `ArtifactRef`s (references-not-payloads); `contains_personal_data`/`data_role`/`pii_key_ref` set on any
event whose payload could carry free-text PII (a title delta, a comment). Every event carries the nested
`correlation_id`/`causation_id`/`depth` (derived correct-by-construction by `OutboxTx::emit`).

### 1.1 Events Issues consumes (the cross-subsystem reflexes)

Via the substrate consumer template (`subjects()` whitelist, never `*`; idempotent on `event_id`; bounded
prefetch; lag metric):

| Consumed event | Reflex (the cross-sub consumer; flows C1–C4) |
|---|---|
| `git.branch.created` (refs an issue) | create the issue↔branch ref edge (the content producer emits `refs.edge.created`); workflow-permitting auto-transition → *In Progress* |
| `git.pr.opened` / `git.pr.merged` | link PR↔issue (`closes` typed edge); on merge, transition → *Done* if the guard is satisfied |
| `ci.run.passed` / `ci.run.failed` | feed the "can't mark Done while CI red" guard (read at transition time via `project`) |
| `chat.message.created` ("create issue") | `issue.create` with the chat message as a `relates` ref edge |
| `identity.member.added` / deactivated / **erased** | reassign/anonymise: the actor becomes a pseudonym across history (the erasure lever, §6) |
| `issue.updated` (own) | drive the **rollup**, **SLA pause/resume**, **trigger resolution**, and the **projection feeder** consumers |

---

## 2. ArtifactRef + the sub-artifact `#sub` scheme (contracts 5.1, 5.7)

`ArtifactRef = myelin://<tenant>/issue/<type>/<id>[#sub]`. Issues uses the canonical `issue` subsystem token
(singular — Bus §6.2). The `<type>` is `issue|epic|sprint|field|comment|relation|initiative` (the seed +
`initiative`). The `<id>` is the **human key** (`ENG-1421`) — the stable public id (§7 of doc 01; the REF-3
reconciliation in CR-3). Examples:
- `myelin://acme-eu/issue/issue/ENG-1421` — an issue.
- `myelin://acme-eu/issue/epic/ENG-1390` — an epic (a ranked issue-family type, same id space).
- `myelin://acme-eu/issue/issue/ENG-1421#comment-12` — a comment sub-artifact.

**The stable `#sub` scheme** (contract 5.7 — each subsystem mints stable opaque sub-ids, stable across edits so
embeds don't dangle):
- `#comment-<N>` — a comment (monotonic per issue; never reused).
- `#field-<field_id>` — a field value (stable across renames; the field_id is the UUID, not the display name).
- `#sub-<key>` — a sub-issue rendered inline (the child's own key).
- `#rel-<relation_id>` — a relation (so a backlink can point at a specific edge).

`parse`/`format` are the shared `myelin-refs` library (we never write a second URN parser). Display keys
(`#1421` without the prefix, in-context) are render-time only (REF-3).

---

## 3. `project(ref, viewer)` — the projection API (contract 5.6; ADR-13.1)

**The only way another subsystem reads about an Issues artifact (no cross-DB).** Per-viewer, pre-permission-
checked. Consumed by Refs `resolve` (the context-pane unfurl, the PR pane, chat unfurls), Search (the text
projection), and Notif (humanisation).

```rust
fn project(ref: ArtifactRef, viewer: Principal) -> Projection | Tombstone {
    let issue = load(ref.id);
    // PRE-PERMISSION-CHECKED: a confidential issue the viewer can't read returns a Tombstone (never leaks the title):
    if Id.check(viewer, "view", issue.ref()).is_deny() { return Tombstone; }
    Projection {
        title:       issue.title,                         // free-text — erasure-safe (tombstones if erased)
        state:       issue.state,                         // the NAMED state (humanised)
        category:    issue.state_category,                // the FIXED category (cross-sub "is it done?")
        icon:        type_icon(issue.type_id),
        render_hint: "issue",                             // Refs picks the chip/embed render
        sub_anchor:  ref.sub,                             // resolve #comment-12 to that comment's snippet
        // a confidential issue's references never leak its title to an unauthorised viewer (deep-dive §8.4)
    }
}
```

This is what makes the **PR context pane** (flow C1.5) show `ENG-1421` inline — Git calls `resolve`, Refs calls
Issues' `project` — **Git never reads the Issues DB** (ADR-13.1). The `Display` mode of `resolve` returns this
same projection for Notif humanisation (contract 5.2/7.3).

---

## 4. `replay(scope, since)` — reindex-from-source (contract 2.6; the only recovery path)

When Search / Refs / OLAP / Notif rebuild, Issues' `replay(scope, since)` re-emits `*.snapshot` events through
the **live consumer path** — never read from another system; the derived stores rebuild drift-free (steady-state
and recovery share one code path, so they cannot drift — Phase-3 README §1 invariant 6).

```rust
fn replay(scope: Scope, since: Cursor) -> impl Stream<Item = Draft> {
    // scope = tenant | project | a single issue | a sub-artifact; supports SUB-ARTIFACT-GRANULAR snapshots (contract 2.6)
    for issue in scan(scope, since) {
        yield issue.snapshot();                  // issue.issue.snapshot — the full current state (references-not-payloads)
        for rel in relations_of(issue) { yield rel.snapshot(); }       // issue.relation.snapshot (Refs rebuilds the edge projection)
        for c   in comments_of(issue)  { yield c.snapshot(); }         // issue.comment.snapshot (sub-artifact granular)
        if let Some(r) = rollup_of(issue) { yield r.snapshot(); }      // rollup is DERIVED but snapshot-emittable for OLAP convenience
    }
}
```

- **Sub-artifact granularity** (contract 2.6 / 6.4): a snapshot can be scoped to a single comment or field so
  Search/Refs can reindex one changed sub-artifact, not the whole tenant.
- **Imported data rebuilds the same way** — import emits the normal `issue.*` events (one indexing path), so
  reindex-from-source works on imported data for free (sketch 09; flow C4).
- **The rollup aggregate is rebuilt by replay** (it is derived; the edge truth is `issue_relation`) — the
  reindex-from-source rollup/edge parity drill proves it ([07](./07-drills-and-open-questions.md)).

---

## 5. The envelope via the OUTBOX (contracts 2.1–2.3)

`OutboxTx::emit(draft, cause)` is the **only** sanctioned emit path — in the same transaction as the state
change, so the event and the data commit atomically (no dual-write hazard; no `publish_now`). Causality is
derived correct-by-construction (the nested `correlation_id`/`causation_id`/`depth`). The `outbox` table is the
per-service cross-seam anchor (`(event_id UNIQUE, aggregate, seq, subject, envelope)`, `UNIQUE(aggregate, seq)`
ordering — the **issue is the aggregate**, so per-issue ordering is preserved). The relay drains it
(`FOR UPDATE SKIP LOCKED`). The **audit append is a bus consumer** of the same events (audit-via-outbox, not a
second write path). The `no-raw-publish` lint enforces this at build.

Every state-changing handler in §2 of [02](./02-internals-and-algorithms.md) (create, transition, reorder,
relation-write, rollup, SLA) ends in an `OutboxTx::emit` inside the transaction.

---

## 6. Identity — `check`/`list_objects` + the `issue` ReBAC namespace fragment (contracts 4.2/4.3/4.9)

### 6.1 The namespace fragment (Issues declares; Id owns the engine)

Issues declares its ReBAC namespace fragment (the Id §5 seed, extended with the watchable `watcher` relation and
the field/transition sub-objects). The seed (Id §5) is the baseline; Issues' P4 fragment:

```
definition issue {
  relation parent_project:    project
  relation assignee:          user | team#member
  relation watcher:           user                       // the Notif read-fanout relation (contract 4.9 / Notif §8.3)
  relation confidential:      user | team#member         // marks the issue confidential (the exclusion driver)
  relation confidential_grant: user | team#member        // explicit grant for confidential issues
  permission view       = (parent_project->read - confidential) + confidential_grant
  permission comment    = view
  permission transition = assignee + parent_project->write
  permission manage      = parent_project->write
}
definition issue_field {                                 // field-level visibility (a sub-object; ABAC at the edge)
  relation parent_issue: issue
  permission view = parent_issue->view                   // + an ABAC caveat (e.g. "visible iff issue.severity < X") at check-time (§9, off the hot list_objects path)
}
definition issue_transition {                            // transition-level visibility (governed transitions)
  relation parent_issue: issue
  permission perform = parent_issue->transition          // + ABAC overlay (approver-role)
}
```

- **The `- confidential` exclusion** is Zanzibar's set-difference userset — a confidential issue **disappears
  from a normal project-reader's `list_objects` by construction**, not by a post-filter (Id §5; the no-leak
  guarantee). This is the deep-dive §8.4 / reference-graph §6.4 confidential-no-leak guarantee surfaced as the
  context-pane UX (IA §4).
- **Field/transition ABAC** ("hide the salary column"; "field visible only if `issue.severity < X`") is a
  **caveat at the edge** (Id §9), evaluated at `check` time with context — kept **off** the hot `list_objects`
  pre-filter so the bulk pre-filter stays fast.
- **`watcher`** is the read-fanout relation: Notif resolves watchers via `list_subjects(issue, watcher)` for the
  unbounded ambient set (write-fanout handles the bounded assignee/mention set).

### 6.2 Usage

- **Every write** runs `Id.check(actor, perm, object, zookie?)` before mutating (fail-closed on uncertainty;
  security-sensitive transitions carry a zookie — Id §8.4/§10).
- **Every read** (board/list/view/search/backlink) conjoins `list_objects(viewer, 'view', 'issue')` as the
  leak-free pre-filter **before** scoring/scanning (the planner, [02 §3](./02-internals-and-algorithms.md)). No
  path bypasses it. The `Filter{set_expr, zookie}` must be consumer-composable over the `issue.id` column
  (push-down — contract S-10; CR-1 in [06](./06-shared-system-change-requests.md)).
- **Tuple writes** (assigning, watching, confidential-grant) go through `write_tuples([Δ], precondition?)`
  returning the zookie to stamp (emitted via outbox — contract 4.6).

---

## 7. `PersonalDataHolder` — locate/export/rectify/restrict/erase + the restriction flag (contract 10.1)

Issues implements `PersonalDataHolder` for every store the harness opens (auto-registered with the DSR
orchestrator — substrate §3.4). Personal data in Issues: `assignee`/`reporter`/`created_by` (pseudonymous
principal ids), free-text `title`/`props`/comment bodies/change-deltas, attachment filenames. Every personal-
data field carries `#[personal_data(category, role, basis, retention, erasure, subject_locator)]` (the
`no-untagged-personal-data` lint fails the build otherwise — GDPR §2.1).

| Op | Implementation |
|---|---|
| **locate(subject)** | scan for the subject as assignee/reporter/author/mentionee/watcher across `issue`, `issue_change_log`, comments, triggers, the import people-map |
| **export(subject)** | the canonical interchange format (§ the same format import consumes — sketch 09) — their issues, comments, change-log entries, attachments manifest |
| **rectify(subject)** | update the pseudonymous mapping / free-text via the normal edit path (emits events; reindexes) |
| **restrict(subject)** | set the **restriction flag** → no indexing / no agent-use / no analytics / no notification for the restricted subject (the contract obligation — Phase-3 README §5); the planner and the projection feeder check it |
| **erase(subject)** | (1) Id `erase` shreds the pseudonym map → "Former user 8a2f" across all history without rewriting issues others own; (2) crypto-shred the **per-subject DEK** for free-text PII (title/props/comments/deltas — GD-4); (3) crypto-shred attachment blobs (BlobStore key destroy); (4) emit `issue.*.erased` tombstones (the live consumers tombstone Search/Refs/OLAP/Notif). Returns a receipt hash-linked into the audit log. |

**The free-text PII residual (GD-6, [OPEN — LEGAL]):** PII someone *typed into another person's issue body* is
the hard residual — anonymise-actor + redaction-tombstone + agent-assisted scan + crypto-shred-attachment, with
the **residual risk documented honestly** (not claimed solved). This is the named floor (sketch findings; CR-7
to GDPR/Legal in [06](./06-shared-system-change-requests.md)). **Post-restore re-erasure** (GD-14) runs against
the erasure ledger so a restore can't resurrect erased data.

---

## 8. `ToolDef` registrations — the one catalogue humans + agents share (contract 8.1)

Issues registers its actions into the `ToolSurface` as permissioned `ToolDef`s — the **same catalogue** the
command palette (S19) and agents use (no privileged back-channel; UI=CLI=agent parity, ADR-08). Each declares
`required_caps`, `effect_kind`, `side_effecting`, `requires_approval`, `exposed_over_mcp`:

| ToolDef | `required_caps` | `side_effecting` | `requires_approval` default |
|---|---|---|---|
| `issue.create` | `issue.create` on project | yes | no |
| `issue.update` (fields) | `issue.update` | yes | no (a *governed* field → yes) |
| `issue.transition` | `issue.transition` | yes | **yes if the workflow marks the transition governed** (flow B2) |
| `issue.comment` | `issue.comment` | yes | no |
| `issue.link` (relation) | `issue.update` | yes | no |
| `issue.estimate` | `issue.update` | yes | no |
| `issue.reorder` (rank) | `issue.update` | yes | no (same CAS path as a human — §5 of doc 02) |
| `issue.assign` | `issue.transition` | yes | no |
| `issue.close` | `issue.transition` | yes | **yes if confidential or governed** |

All side-effecting tools apply via `EffectApi::apply` (schema → capability → delegation → tenant → budget → HITL
gate → apply via the **public endpoint**, no carve-out → meter). A `Denied` is an ordinary tool error (no
privileged fallback — AG-5/8). The **forecast agent** registers an `issue.forecast` tool (compute-only, reads
OLAP; the at-risk threshold is config — resolves sketch-05 open Q11). `run --dry-run` returns the proposed
effects without applying (plan-then-apply testability — contract 8.7; the triage agent's S9 suggestions).

---

## 9. Reserve/settle — spend-bearing agent work (contract 11.7)

Where Issues runs spend-bearing work — the **triage agent**, the **forecast agent**, the **SLA-draft agent**,
any agent invoked via an automation/trigger — the run is a **durable workflow** with the reserve/settle gate as
its bookends: `reserve` at dispatch (no balance → no start — the run never begins), `settle` on completion
(never interrupt in-flight). Metering is integer minor-units; wholesale ≠ markup (contract 11.7; D8). The HITL
approval card surfaces a **live cost estimate** (EI-03 §5.1) before a human approves a gated effect (flow B2).
Issues does not own the wallet (Commercial, C-1) — it consumes the gate.

---

## 10. The stateful Trigger — the Issues-side ownership (ISS-1; contract 3.3; sketch 08B)

Issues **owns the Issues-side Trigger UX and semantics** (the armable conditions + the armed/resolved/stale
surface); it **consumes** the bus `arm_trigger`/`disarm_trigger` primitive, the `myelin-flow` `stale_after`
durable timer, and the one Notif inbox for `on_resolve`.

- **The armable-condition catalogue** (resolves sketch-08 open Q12) — each compiles to a safe-AST `EventMatcher`
  over `issue.*` events:
  - **"Remind me when unblocked"** (the flagship) — `condition: all blocked_by edges resolved` (reads
    `issue_relation`, §4 of doc 01).
  - **"Ping me when this leaves triage / state X"** — `condition: state_category transitions out of X`.
  - **"Notify me when assigned to me."**
  - **"Tell me when SLA at risk."**
  - **"Tell me when this initiative goes at-risk"** — `condition: initiative.health_changed crosses at-risk`.
- **Lifecycle:** `armed → {resolved | stale | disarmed}`, fires **once per arming** (contract 3.3). The last
  blocker closing → `issue.relation`/`issue.transitioned` → the bus resolves the trigger → **one** inbox item
  (humanised — NOTIF-1 — with the routable ArtifactRef). After `stale_after` (default 30d, a `myelin-flow`
  durable timer) with no resolution → a "still blocked after 30d — escalate?" nudge → the trigger goes stale.
  **No silent forever-armed promises.**
- **Why it is the make-or-break agent-adjacent UX:** instead of an agent *doing* something, the platform
  *watches on your behalf* and re-surfaces precisely when relevant — calm-by-default (P8), zero polling, durable
  across restarts/days. "My Work that comes to you." The **Trigger-fires-once-after-restart** drill proves the
  durability ([07](./07-drills-and-open-questions.md)).

Continue to [`04-views-cli-and-api.md`](./04-views-cli-and-api.md).
