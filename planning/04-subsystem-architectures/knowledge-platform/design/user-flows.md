# Knowledge Platform — Key User Flows

> Phase 4, Knowledge, **design sketch** (REQUIRED). Includes the agent/HITL flows (proposed effects,
> approval cards, attribution) and the cross-subsystem flows Knowledge participates in. Each flow
> notes the empty/loading/error touchpoints and the Phase-3 contracts it consumes. Canonical:
> design-language §6 (agent UX contract), §5.10 (state patterns), VISION §3 (agent-native, HITL).

---

## Flow A — Create & collaboratively edit a page (the core loop)

**Actors**: two human editors (+ later an agent). **Goal**: write a spec together, live.

1. **Create**: sidebar `+ New page` (or ⌘K → "Create page"). Empty state: a titled, empty page with a
   ghost "Type `/` for commands, or just start writing" prompt + the slash-menu hint (onboarding-forward,
   §5.10). Optimistic: the page appears in the tree immediately; the `knowledge.page.created` event is
   emitted via the outbox (BUS-2) in the same tx.
2. **Write**: type; `/` opens the slash-command menu (popover, flips above near viewport bottom, §8b.4)
   to insert blocks; markdown shortcuts (`#`, `-`, `>`, ` ``` `) transform inline. The **one editor
   render path** (KN-4) shows formatting as typed (controlled `contenteditable`, caret = md offset).
3. **Collaborate**: a second editor opens the page → live presence cursors/avatars (firehose presence,
   not durable bus). Each keystroke → op → **resume-cursor op-log** (KN-1, sketch 01) → firehose
   fan-out → the other client applies. **Conflict (CAS floor)**: if both edit the *same block*, the
   loser gets a soft-lock cue ("Alice is editing this block") — no silent overwrite, no merge (the
   named floor; CRDT promotion blends them later, sketch 01).
4. **Coalesce**: after a quiet period, a debounced `knowledge.doc.updated` semantic event hits the
   durable bus → Search reindexes the changed blocks, Refs updates edges, watchers notified. Agents
   react to *this*, never per-keystroke (bus §4.3).
5. **Reference**: type `@` → mention autocomplete (a person → `mention` node; `#` → an
   `artifact_ref`/`embed` of an issue/PR/doc). The structured node emits `refs.edge.created`; the
   referenced artifact's backlinks pane now shows this page (permission-filtered).

**States**: *loading* a huge doc → skeleton blocks matching final layout (partial/lazy load, never a
blank spinner, §8b.6); *error* on save → quiet one-line "Couldn't sync — retrying" + optimistic state
preserved + reconnect via the resume cursor (zero ops lost); *read-only* (no edit perm) → editor
disabled with a clear "view-only" affordance; *offline* → optimistic local edits queued behind CAS
(sketch 01), synced on reconnect.

**Reconnect drill touchpoint**: step 3's resume cursor is the KN-1 reconnect-loses-zero-ops property —
sever the connection mid-edit, reconnect, assert zero ops lost (T-5, the drill is mine).

---

## Flow B — Build a database & switch views (the structured half)

1. **Create DB**: `/database` inserts an inline database (or a full-page DB). Empty state: a one-row
   table with an "Add a property" + "Add a row" affordance.
2. **Schema**: add typed fields (text/number/select/date/person/relation/formula/rollup — the shared
   ADR-06 field-definition UI). A `person` field is personal-data-classified (drives erasure keying,
   sketch 06). A `relation` field links to another DB or any artifact (`db_relation`, sketch 07).
3. **Add rows**: inline-edit cells (property bag per row, sketch 03); a `formula`/`rollup` column shows
   a **read-time-computed** value (KN-3 — never stored, always correct).
4. **Switch views**: table → board (drag cards between status columns) → calendar (drag to reschedule)
   → timeline. Each view = a query-AST projection (ADR-07), **permission-filtered by construction**
   (rows the viewer can't see are *absent*, never post-filtered, §5.6).
5. **Filter/sort/group**: built as the same query AST the palette + agent triggers use; shared-vs-
   personal view split (a shared def + personal overrides).

**States**: *empty database* → guided "add property / add row"; *filtered-empty* → "No rows match this
filter" + clear-filter action; *loading large result set* → skeleton rows + the structured query routed
to the Search structured index (ACL-pre-filtered, paginated, sketch 03), never a full OLTP scan;
*formula-recomputing* → a subtle inline spinner on the cell, suppressed under ~1s (§8b.6);
*permission-filtered* → simply fewer rows, no leak.

---

## Flow C — The wedge: an incident runbook embeds a live failing CI run

(`knowledge-platform.md` §6.1 — the cross-subsystem flagship.)

1. On-call opens *Incident: API 5xx spike*. Types `/embed`, pastes the CI run URL → an
   `embed(ArtifactRef → ci/run/991)` node.
2. Knowledge renders it via **Refs `resolve(ref, viewer)` → CI's `project(ref, viewer)`** (no cross-DB)
   → the run's current status, inline. **Permission-aware per viewer**: a viewer without `ci.run.view`
   gets a graceful "no access" card, never a leaked title (§5.3).
3. **Live**: the client subscribes to `ci.run.*` on that subject → when the run finishes, the embed
   refreshes (red→green) without a reload (embed liveness, sketch 05).
4. `@`-mention the responsible issue → `refs.edge.created` → the issue's backlinks show "mentioned in
   Incident runbook" (permission-filtered).

**States**: *loading* embed → skeleton card matching the unfurl layout; *error* (CI unreachable) →
"Couldn't load this run" + retry, the run's `ArtifactRef` still shown (degraded, not dead, §8b.6);
*erased/deleted* target → graceful tombstone ("this run was removed"), never a dangling crash.

---

## Flow D — Agent turns meeting notes into issues (the agent/HITL flagship)

(`knowledge-platform.md` §6.2 — agent-native, mock-now, plan-then-apply.)

1. **Invoke (explicit-first, CHAT-1)**: in a meeting-notes page, the author clicks **"Ask agent → turn
   action items into issues"** (explicit "run an agent here" — implicit auto-dispatch on casual mention
   is a separately-decided product feature, not v1). A `kind=agent` Principal is dispatched (per-run
   token minted, reserve opened — `agent-fabric.md` §5).
2. **Agent reads** the page *projection* (not the DB) and **proposes effects** (plan-then-apply, never
   acts): `issue.create ×3`, `refs.create ×3`, `knowledge.page.update` (link the created issues back).
3. **The plan is shown** (design-language §6.2): the **approval card** (the `agent` treatment, §5.4)
   lists the concrete proposed effects — *what* will change, *on which artifacts*, *under whose
   delegated authority*, with a **live cost estimate** (AG-8). The card surfaces primarily **in Chat**
   (the HITL surface, system-overview §8.2) **and in the Inbox** (§5.8) so a gate is never missed; it
   can also appear inline on the page (the "agent-pending" state, §5.10).
4. **EffectApi validates** each effect against `agent.policy ∩ delegation ∩ tenant.policy` + budget +
   HITL gate (`agent-fabric.md` §5.2). Non-consequential effects (create issue on a public project)
   may auto-apply; the page edit is **gated** (consequential) → withheld until approval (returns a
   `Gated` error, does NOT mutate — AG-8).
5. **Human decides**: **Approve / Edit / Reject** on the card. *Edit* lets the human amend the proposed
   effect before applying (control of content, not just yes/no, §6.3). Approve → the durable-workflow
   signal resumes the run → the effect applies. The page edit is applied **through the collab protocol**
   with **"suggested by agent" attribution** (Flow A's op-log), which the author can still accept/reject
   inline.
6. **Attribution + audit**: every agent action is attributed (who/what, on-behalf-of-whom, under which
   trigger, `correlation_id` threading) and audit-linked (§6.4). The agent is **always labeled as an
   agent** (the agent badge, never disguised, §6.1; no sparkle/magic-wand iconography, §8b.3).
7. **The trace**: the run's execution trace is written as a **content-addressed Knowledge document**
   (AG-7, sketch 06) — `run.trace_ref` — an erasable holder, distinct from the audit log.

**Same UX for mock and real** (the strategy-pattern payoff, §6): `MockAgentRuntime` today,
`LlmAgentRuntime` later — the frontend renders proposed-effects + gates + attribution identically. Must
work with mock agents in dev (VISION §3).

**States**: *agent working* → "Agent is reviewing this page…" (the agent-pending state, §5.10); *gate
pending* → the approval card with Approve/Edit/Reject, persists across days (durable, never silently
lost, §6.3); *denied effect* → surfaced as an ordinary outcome ("the agent couldn't create issue X — no
permission"), no privileged fallback (AG-5).

---

## Flow E — DSAR export & erasure for a departed contributor (compliance on the same structure)

(`knowledge-platform.md` §6.3; sketch 06.)

1. A DPO opens the **GDPR / data-rights console** (shell §7.6, the DSR orchestrator UI) → "export
   subject: Alice" → Knowledge's `locate(subject)` + `export(subject)` runs (lossless JSON spine,
   scoped to Alice). Receipt issued.
2. "Erase subject: Alice" (with `--dry-run` preview) → Knowledge's `erase(subject)`: anonymise
   authorship (→ pseudonymous "Deleted user", preserving others' work), **crypto-shred Alice's
   per-subject DEK** (reaches live blocks + the op-log + history snapshots + backups, sketch 06),
   tombstone mentions (Refs degrades backlinks), purge + reindex Search (incl. embeddings), CDN-purge
   any published page.
3. The DSR orchestrator collects the Knowledge receipt into the Merkle-proven bundle
   (`gdpr-and-audit.md` §4). Deadline tracked; the operation is part of the platform DSR fan-out.

**States**: *history of an erased segment* → redacted placeholder ("content erased per DSR"), never the
data; *restore a version* → post-restore re-erasure runs (can't un-erase Alice, sketch 06 / GD-14);
*free-text-about-Alice* → flagged for the tooling+process path (the honest limitation, surfaced as a
"review flagged content" task, not a silent gap).

---

## Flow F — Living document maintained by a scheduled agent (durable-workflow)

1. A scheduled automation ("daily-notes from template" / "keep this status doc in sync with reality")
   is a **durable workflow** (ADR-09) bound to a schedule/event matcher.
2. On fire, a `kind=agent` run reads current artifact projections (issue states, CI health) and
   **proposes** page/row updates (plan-then-apply). Non-consequential updates apply; consequential ones
   gate (Flow D). Edits flow through the collab protocol (attribution).
3. The page shows "last updated by [agent] · [time]" with the agent badge; the human can pin/unpin the
   automation, inspect the agent's scope/budget (agent governance console, §6.4).

---

## Cross-subsystem flows Knowledge participates in (summary)

| Flow | Knowledge's role | Contract used |
|---|---|---|
| **Doc embeds issue board / CI run / PR / chat thread** (Flow C) | consumer of others' `project(ref, viewer)` via Refs `resolve`; subscribes to update events | Refs resolve; ADR-13 projection API; bus update events |
| **Issue/PR/chat references a doc/block** | producer of `project(ref, viewer)` for *its* pages/blocks; emits `refs.edge.*` | the projection API I implement; outbox |
| **Agent run trace stored** | accepts a content-addressed agent-trace write → `ArtifactRef`; erasable holder | AG-7; my trace write path |
| **DSAR/erasure across all holders** | a `PersonalDataHolder` in the DSR fan-out | `gdpr-and-audit.md` §4 |
| **Search over docs/rows (incl. agent RAG)** | declares `IndexSpec` + `project`; emits semantic events | Search; my IndexSpec + projection |
| **HITL approval card** | originates proposed effects; the card surfaces in Chat + Inbox | Agent Fabric `EffectApi`; Chat HITL surface; Notif inbox |

## Cross-references

- design-language §6 (agent UX contract: labeled, plan-then-apply, HITL, attribution), §5.4 (approval
  card), §5.10 (state patterns), §8b (day-one primitives).
- sketches 01 (collab), 05 (embed liveness), 06 (GDPR/agent trace).
- `agent-fabric.md` §5 (loop, EffectApi, HITL); `knowledge-platform.md` §6 (usage examples).
