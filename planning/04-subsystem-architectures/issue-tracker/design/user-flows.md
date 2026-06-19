# Design — Key User Flows (Issue Tracker)

> Phase 4 design sketch (required before architecture). Covers the primary human flows, the **agent/HITL flows**
> (proposed effects, approval cards, attribution — design-language §6), and the **cross-subsystem flows** Issues
> participates in. Each flow names the shared contracts it rides so the architecture stage builds to them. Notation:
> `→` a step; **bold** = a shell/shared surface; `⟦…⟧` = an event on the bus.

---

## A. Core human flows

### A1 — Quick-create an issue (keyboard-first; design-language P3)
1. Anywhere: **⌘K** → "Create issue" (or `c`) → quick-create overlay (a **Dialog**, portalled to root — §8b.1).
2. Type title; inline `/` for type, `@` for assignee, `#` for project/cycle (query-AST-backed autocomplete §5.2).
   Optimistic: the issue appears in the current view *before* the server confirms (P2 / §8b.6).
3. Submit → `issue.create` via the public API → human key allocated (Hi/Lo, sketch 04) → ⟦`issue.issue.created`⟧
   via the outbox.
4. **Empty-title / validation** → inline error on the field, never a blocking modal. **Server reject** (e.g.
   required-field-on-create scheme) → optimistic row rolls back with one quiet line + the field to fix (§8b.6).

### A2 — Triage an incoming bug (S9; engineer + agent-assist)
1. Bug lands in **Triage** (created from chat / CI / a form). The triage list shows it unlabelled.
2. **Agent assist** (see B1): a triage agent has already *proposed* labels/severity/dup/owning-team — shown as a
   dismissible **agent suggestion** strip (the `agent` treatment, §6.1), not auto-applied.
3. Human accepts (one key) or edits → issue gets labels + moves out of triage. Bulk-triage: select many, apply.
4. **Duplicate-suspected cluster** state: suspected dups are grouped with a "merge / not a dup" action.

### A3 — Plan a cycle (S8; PM)
1. **Active cycle** ▸ Plan → drag backlog issues into the cycle (drag-rank, sketch 06; CAS-arbitrated).
2. Capacity bar updates live (committed estimate vs capacity); **over-capacity** state warns (glyph+label, not
   colour-only — §8b.3).
3. On cycle close: carry-over prompt (move unfinished to next cycle / backlog) — a designed state, not a silent
   drop. ⟦`issue.cycle.completed`⟧ feeds burndown/OLAP.

### A4 — Drive the roadmap (S5; PM/exec — the co-equal-view payoff)
1. **Roadmap** shows initiatives/epics (ranked `issue` types, sketch 01) on a time axis with **rollup progress**
   bars (sketch 05) and **dependency overlays** (`depends_on`/`blocked_by` edges, sketch 05).
2. The *same* issues are on the engineer board — editing an issue's dates/scope here patches the board live
   (bus-driven sync, sketch 08). **No parallel reality** — the roadmap and board read the same `issue` rows.
3. **Date-at-risk** state: a forecast (sketch 05, agent-powered) flags an initiative; clicking shows the
   contributing blocked issues (the system assembles context — §8b.6), pre-fetched.

### A5 — "Remind me when unblocked" (the stateful-Trigger flagship; ISS-1)
1. On a **blocked** issue (S1, transition-blocked state), the blocker list shows a one-click **"Remind me when
   unblocked."**
2. → `arm_trigger(Trigger{ owner: me, condition: all blocked_by resolved, arms_subject, on_resolve: inbox,
   stale_after: 30d })` (event-bus §3.6). The issue shows a subtle "you'll be notified when unblocked" pending
   cue.
3. Last blocker closes → ⟦`issue.relation`/`issue.transitioned`⟧ → bus resolves the Trigger → **one** inbox item
   (the Notif inbox, C-9), humanised at the backend (NOTIF-1) with the routable ArtifactRef. Fires once.
4. 30 days, still blocked → `stale_after` (`myelin-flow` durable timer) fires → "still blocked after 30d —
   escalate?" nudge → Trigger goes stale. No silent forever-armed promise.

---

## B. Agent / HITL flows (design-language §6; plan-then-apply, ADR-08)

> The same UI works for the **mock** runtime today and a real runtime later (the strategy-pattern payoff,
> ADR-08 / AG-4). Agents are **always labelled** (§6.1), **propose before they act** (§6.2), and consequential
> actions pass a **HITL approval card** (§6.3) backed by a durable `myelin-flow` gate.

### B1 — Agent triage proposal (mock now; deep-dive §6.2)
1. ⟦`issue.issue.created` (type:bug, state:triage)⟧ → the bus matches a registered **automation/trigger** → wakes
   the triage agent **on-behalf-of the reporter** under a `RunBudget` (reserve/settle, D8). The run is a durable
   workflow (durable-workflow §6.1).
2. `AgentRuntime::step` returns **proposed effects** (plan-then-apply, AG-1): `[set labels, set severity,
   ref.create(duplicate-of ENG-1390), comment("suspected dup of ENG-1390")]` — **no side effects yet**.
3. `EffectApi::apply` validates each effect: schema → capability → delegation → tenant → budget → HITL gate
   (Agent §5.2). **Non-sensitive** effects (label, comment) → applied via the **same permissioned tools a human
   uses** (no carve-out, AG-5). A **governed transition** → HITL-gated → B2.
4. The triage screen (S9) shows the agent's suggestions as the **agent strip** with **Accept / Edit / Dismiss**;
   every applied effect is **attributed to the agent** + audit-linked (§6.4), one `correlation_id` threading the
   flow, loop-depth capped (AG-6).

### B2 — HITL approval card (the withhold→approve→resume bridge; AG-8 / durable-workflow §6.3)
1. The agent proposes a **gated** action (e.g. transition a governed issue to Done, or close a confidential
   issue). The gated tool is **withheld** — it returns an error, does **not** mutate (AG-8).
2. The workflow emits ⟦`agent.approval.requested`⟧ (tool name, arg ArtifactRefs, **risk**, a **live cost
   estimate** — EI-03 §5.1) → Notif/Chat renders the **approval card** (§5.4), humanised at the backend
   (NOTIF-1). The card surfaces **in chat AND in the Issues inbox/My Work** (so a gate is never missed) and can
   appear **inline on the issue** (S1, "agent proposal pending" state).
3. The workflow is `state=waiting`, holding **no runtime**, for up to the gate window (may be **days**) — the
   `myelin-flow` durable signal (durable-workflow §4.3).
4. Human clicks **Approve / Edit / Reject** (Edit lets them amend the effect before applying — §6.3). →
   `Id.check(human, approve, run)` → `DurableExecutor::signal(run, "approval:<call>", {approved, by}, idem=card)`.
5. **Approved** → the step re-runs **with the tool now allowed** → the transition applies, attributed to the
   agent under the approver's delegation. **Rejected** → withheld, agent continues. **Timeout** (window elapsed)
   → auto-deny path + notify. Durable across restarts/deploys.

### B3 — SLA-draft & escalation agent (deep-dive §7.3; sketch 07)
1. SLA at 80% → the `myelin-flow` timer fires `sla.at_risk` → a Signal → wakes a drafting agent (proposes a
   holding response, HITL-gated) **and/or** notifies on-call (Notif `oncall_now`/`page` → a durable escalation
   workflow). On breach → ⟦`sla.breached`⟧ → escalation chain + OLAP compliance feed.

### B4 — Forecast-drift agent (deep-dive §6.4; sketch 05)
1. Incremental rollup updates an initiative's progress/date projection → a forecast agent (swappable strategy)
   crosses an at-risk threshold → ⟦`initiative.health_changed`⟧ → a trigger flags the PM **in chat** with the
   contributing blocked issues (context pre-assembled). The roadmap (S5) shows **date-at-risk**.

---

## C. Cross-subsystem flows (the wedge; Issues consumes/emits the bus)

### C1 — Engineer git loop: branch → PR → merge auto-transitions (deep-dive §4.1 / Phase-2 §6.1)
1. Engineer creates branch `feature/ENG-1421-fix-sso` in **Git**. ⟦`git.branch.created`⟧ references ENG-1421.
2. Issues **consumes** it (idempotent on `event_id`), creates the **ref edge** issue↔branch (a producer emits
   `refs.edge.created`, reference-graph §4.1), and — workflow-permitting — auto-transitions ENG-1421 to *In
   Progress*.
3. Engineer opens PR "Closes ENG-1421" → ⟦`git.pr.opened`⟧ → Issues links PR↔issue (`closes` typed edge,
   issue_relation). **CI** runs; the workflow guard "can't mark Done while CI red on the linked PR" reads CI
   status (a safe-AST guard, sketch 02) — opt-in per workflow.
4. PR merges → ⟦`git.pr.merged`⟧ → Issues transitions ENG-1421 → *Done* (guard satisfied). ⟦`issue.transitioned`
   (from,to,category)⟧ → OLAP (cycle-time) + Notif (assignee/watchers).
5. **Conversely:** the **PR context pane** (Git, system-overview §8.1) shows ENG-1421 inline via the Issues
   **`project(ref, viewer)` projection** — Git never reads the Issues DB (no cross-DB; ADR-13.1).

### C2 — Create-issue-from-chat (deep-dive §4 / §7.2)
A chat message → "create issue" → an `issue.create` with the chat message as a `relates` ref edge; the chat
unfurl now shows the live issue. Permission-aware unfurl both directions.

### C3 — User deactivated / erased (GDPR; deep-dive §8 / ADR-12)
⟦`identity.*` deactivated/erased⟧ → Issues reassigns/anonymises: the actor becomes a **pseudonym** ("Former user
8a2f") across history/comments/mentions; the erasable pseudonym map lives **outside** immutable structures
(ADR-12 §4) so destroying it + crypto-shred satisfies erasure without rewriting issues others own. Free-text PII
is the hard residual ([OPEN — LEGAL] GD-6): agent-assisted scan + redaction-tombstone + crypto-shred of
attachments, residual documented honestly.

### C4 — Reindex-from-source (resilience; REF-4 / SEARCH-1)
On Search/Refs/OLAP rebuild, Issues' `replay(scope, since)` re-emits `*.snapshot` events through the **live
consumer path** — never read from another system; the derived stores rebuild drift-free. Imported data (flow
A-import) rebuilds the same way (one indexing path).

---

## D. Empty / loading / error as designed states (every flow; §5.10 / §8b.6)

Each flow above has its non-happy states designed in `wireframes.md`. The platform rules they obey: **empty**
explains + offers the create action; **loading** shows skeletons matching the final layout (never a spinner on
blank); **error** blames the *system* in one quiet line + a path; **permission-denied** is a graceful "no access"
(never a leak); **erased** tombstones gracefully; **agent-pending** shows the "working / awaiting your approval"
state. **Reversibility over confirmation** (§8b.6) — destructive issue actions get an undo window + restorable
soft-delete, except irreversible/consequential + GDPR/agent-HITL actions, which still confirm (§6.3).
