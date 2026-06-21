# R-04 — Named Cross-Surface Task Flows (Service Blueprints + Job Flows)

> **Phase 4 research corpus** · deliverable of prompt **R-04** (workstream
> [`ws-b-jtbd-and-flows.md`](../../02-research-roadmap/ws-b-jtbd-and-flows.md)).
> **File date: 2026-06-20.** No real users exist; personas P1–P15 are HYPOTHESES
> ([`personas.md`](../../../planning/01-research/personas.md) §0). This file **realises the
> jobs catalogued in [R-03](./jtbd-catalogue.md) as flows** — it does not re-derive them.
> Every flow below names the R-03 job(s) it proves. Depends on R-03; feeds Phase 5 (surface
> map), Phase 6 (sketch funnel — these flows ARE the comparable scenarios), R-14 (agent
> legibility), R-19 (sovereignty), R-22 (wedge moments).

## 0. How to read this file

Two notations per flow, per the prompt's two methods:

1. **Service blueprint** *(method #8)* — five lanes separated by the canonical lines.
   **PROVEN** *(service-blueprint structure: customer actions / frontstage / line of
   visibility / backstage / support processes / lines of interaction & internal interaction;
   NN/g, [Service Blueprints: Definition](https://www.nngroup.com/articles/service-blueprints-definition/)).*
   We adapt the lanes to Myelin: **Actor lane** (human or **agent** — agents are first-class
   actors, not backstage plumbing), **Frontstage §7 screen**, *line of visibility*, **Backstage
   events** (the canonical bus events / triggers, [`agent-native-design.md`](../../../planning/01-research/agent-native-design.md) §2.2),
   **Shared-system support** (Id/Refs/Bus/Audit/DSR/Workflow). **HOUSE STYLE:** putting agents
   *on the actor line* (above the line of visibility when they post to chat/PR, below it when
   they only emit effects) is our choice — standard blueprints have no agent lane.

2. **Job flow** *(method #9)* — entry points, the **keyboard + pointer paths** (P3), and the
   **full per-screen state set**. Per design-language §5.10 the baseline state set is
   **empty / loading / error / permission-denied / erased-tombstone / agent-pending**; we
   **add the states the happy-path bias skips** (the prompt's §9 mandate, grounded in the
   "design beyond the happy path" discipline — **PROVEN** method,
   [Pixelmatters, *Designing beyond the happy path*](https://www.pixelmatters.com/insights/designing-beyond-the-happy-path);
   [UX Knowledge Base, *Edge cases in UX*](https://uxknowledgebase.com/edge-cases-in-ux-design-ba3bc59228e6)):
   **gate-rejected, agent-error-mid-chain, budget-exceeded, loop-guard-tripped,
   cross-cell-ref→no-access/tombstone, diff-anchored-comment-relocates-after-rebase,
   notification-storm/30×-surge, optimistic-rollback, stale/reconnecting.**

**Seam marker 🔪** flags every moment where today's stitched stack forces a tab-switch / copy-
paste / context-loss — the moments Myelin dissolves. These are the wedge candidates R-22 deepens.

**Tags.** **PROVEN** = grounded in a cited standard/source or an existing architecture
mechanism we *surface* (not invent). **HOUSE STYLE** = our design synthesis. Every flow's
*specific screen choreography* is **HOUSE STYLE**; the *mechanisms* it rides (events, refs,
plan-then-apply, DSR fan-out) are **PROVEN** against the architecture docs. No flow is presented
as user-validated — see §7 `[DEFERRED-UNTIL-USERS]`.

**Agent-flow state legibility** is owned in depth by **R-14**; here we draw the branches so R-14
has concrete moments to spec. **Sovereignty consoles** are owned by **R-19**; here we draw the
DPO's cross-surface *flow* so R-19 has the blueprint.

---

## 1. Flow index (named flows, per audience)

| ID | Name | Audience | R-03 job(s) | §8.x basis |
|---|---|---|---|---|
| **F-ENG-1** | **Red-to-green** — failing CI → step → line → fix-PR → link-issue | Engineer (A) | E3, E2, E1 | (the wedge engineer flagship) |
| **F-ENG-2** | **Trace-to-change** — read unfamiliar line → blame → PR → issue → decision → safe edit | Engineer (A) | E5, E1 | (supporting; proves the live backlink chain) |
| **F-PM-1** | **Incident-to-runbook** — triage incident in chat → issue → knowledge runbook → back to chat | PM/delivery (B) | E9, M6, M2 | §8.1 (context pane) |
| **F-PM-2** | **Report-from-reality** — stakeholder asks "what's shipping" → roadmap reflecting real delivery | PM/delivery (B) | M1, M3 (D1 pair) | (dual-audience; proves no parallel reality) |
| **F-GOV-1** | **DSR fan-out** — DPO answers a data-subject access (+erasure) request across all five surfaces | Corp/gov (C) | G5, G6, G10 (D4 pair) | §8.3 (DSR/erasure) |
| **F-AGT-1** | **HITL flagship** — CI fail → triage agent → issue → chat → proposed fix PR → approval card → human approves → review agent | Agent HITL (cross) | E11/M9 plane, E3 | §8.2 (agent flagship) |

Coverage: **≥1 named flow per audience** (A: F-ENG-1/2; B: F-PM-1/2; C: F-GOV-1) **+ the agent
flagship** (F-AGT-1). F-AGT-1 carries the deepest partial-failure branch set; every other flow
carries the edge branches relevant to *its* surfaces.

---

## 2. F-ENG-1 — "Red-to-green" (the wedge engineer flagship)

**Realises R-03 E3** *(failing-CI → line, the wedge)* **+ E2/E1** *(PR already shows its linked
issue/run/doc; see the *why* without leaving flow)*. Entry: a red Checks badge on the engineer's
open PR, or a `myelin run watch` alert in the terminal (§7.7 — the job finishes in **either**
rendering of the one surface).

### 2.1 Service blueprint

| Lane | Step 1 | Step 2 | Step 3 | Step 4 | Step 5 |
|---|---|---|---|---|---|
| **Actor** (P1/P2 engineer; CLI-first allowed) | Sees red check on **own PR** | Opens failing **check** | Jumps to failing **step → line** | Opens **fix PR** | **Links issue**, requests review |
| **Frontstage §7** | PR overview · **Checks panel** (§7.1) | **Single-run view** (§7.2) | **Live log view** (§7.2) → **Diff/file view** at the line (§7.1) | PR overview (§7.1) prefilled w/ linked issue+run | **Ref chip** insert (§5.3/§5.5); review request (§7.1) |
| *— line of visibility —* | | | | | |
| **Backstage events** | `ci.pipeline.failed` → notif | `ci.step.failed` (granular) | log ref + `ci.read_logs`; **source-map step→commit→line** | `git.pr.opened` | `ref.created` issue↔PR↔run |
| **Shared-system support** | Bus + Notif (dedup, `reason`) | CI projection (perm-checked) | Refs: run→commit→diff-line anchor (content-anchored line-range) | Id: `check(author, write, repo)` | Refs graph edges; Bus fan-out |

🔪 **Seam dissolved (×3):** today this is *GitHub Checks tab → scroll opaque logs → guess the
file → switch to Files-changed tab → open a separate Jira tab to find the issue → paste a URL*.
Myelin makes step→line **one click** (the log line is itself a ref to the diff line, R-22), and
the new PR is **pre-populated** with the issue/run it descends from (no copy-paste of IDs).

### 2.2 Job flow + full state set

Keyboard path (P3, load-bearing per R-03 §2 closer): `g p` (go to PR) → `c` (focus checks) →
`Enter` on failing check → `f` (jump to failing step) → `Enter` on log line → lands on the
diff line → `x` (open fix PR) → palette `link issue` → type-ahead → `Enter`. Pointer path mirrors
each. **Latency budget:** keyboard nav <100ms; step→line resolve uses the **prefetch** of the
next hop (R-13) so the diff is warm before the click lands.

| Screen | empty | loading | error | permission | erased/tombstone | agent-pending | **edge branches (the skipped ones)** |
|---|---|---|---|---|---|---|---|
| Checks panel | "No checks yet — they appear when CI runs" | skeleton rows matching check shape (never blank spinner) | "We couldn't load checks — retry" (blames system, keeps PR usable) | check hidden if viewer can't read the run; **no leaked name** | run pipeline crypto-shredded → "Run no longer available" tombstone | "Triage agent is reviewing this failure" badge | **stale/reconnecting:** firehose drop → "Reconnecting… last updated 12s ago", auto-resume on `ci.*` replay |
| Single-run / log view | n/a | streamed log skeleton; tail follows | "Log stream interrupted — resume" | step output redacted if secret-scoped | log past TTL → "Logs expired (90-day retention)" | — | **log-line→diff-line orphaned:** if the diff was rebased between failure and click, the anchor relocates (see next row) |
| Diff/file view at the line | "File unchanged in this PR" | structure skeleton (gutters + line numbers first) | "Couldn't load diff" | file in a path viewer lacks access to → graceful "Part of this diff is restricted" | file deleted in HEAD → tombstoned hunk | inline agent-suggested fix marker | **🔪 diff-anchored comment relocates after rebase:** the content-anchored line-range re-resolves to the moved line; **if the anchored content is gone**, the comment **detaches to an "outdated, on former line N" pill** (never silently moves to a wrong line — the failure mode GitHub has). PROVEN mechanism (content-anchored ranges, reference-graph). |
| New fix PR | (template) "Describe the fix" | optimistic create; rolls back if `git.pr.opened` rejected | "Branch protected — you lack merge rights here" | open-PR effect denied → explains *which* grant is missing | base branch deleted → "Base no longer exists, pick another" | — | **optimistic-rollback:** PR card shows immediately; if backend rejects, card reverts with one quiet line + the typed body preserved |
| Link-issue (ref chip) | type-ahead "Search issues you can see" (**permission-pre-filtered**, ADR-03) | inline result skeleton | "Search unavailable — paste a link instead" | results exclude issues viewer can't read; never leaks a title | linked issue erased → chip renders **tombstone** "Issue erased" | — | **cross-cell ref:** issue lives in another cell/tenant → chip resolves to a **projection if visible, else a no-access card**; never a raw ID |

**Acceptance hooks:** this single flow exercises Checks→step→line (E3), the live perm-filtered
ref chip (E2/E1), the rebase-orphan branch, the cross-cell branch, and optimistic-rollback — the
exact §9 skipped states the prompt demands. Feeds R-22 (the step→line and pre-filled-PR wedges).

---

## 3. F-ENG-2 — "Trace-to-change" (the live-backlink chain)

**Realises R-03 E5** *(trace why a line exists, live, to change it safely)* **+ E1**.
Compact; included to prove the **live, not-snapshot** reference chain across four subsystems with
one click each — the second engineer surface Phase 6 should compare against F-ENG-1.

### 3.1 Blueprint (condensed)

Actor reads an unfamiliar line → `blame` gutter → **commit** unfurl → **PR** unfurl → **issue**
unfurl → **decision doc block** unfurl. Frontstage: File view + blame (§7.1) → ref unfurls
(§5.3) at each hop. Backstage: each hop is a `ref.created` edge already in the graph; Refs
resolves each `ArtifactRef` via its subsystem projection, perm-checked (§8.1 mechanism). Support:
Id `list-objects` pre-filter on every hop.

🔪 **Seam dissolved:** today = blame → copy SHA → GitHub commit → read PR → find "Closes JIRA-xx"
→ open Jira → find the linked Confluence decision (4 tabs, 3 logins). Myelin = 4 in-place unfurls,
each live.

### 3.2 State set (the chain-specific edges)

- **cross-cell-resolves-to-projection-or-tombstone** — the decision doc lives in a governance
  space the reader can't enter → unfurl is the **graceful no-access card** ("A linked decision
  exists but you don't have access — request access"), **never the title**.
- **erased/tombstone** — the original author exercised erasure → blame shows a **pseudonymised
  author** + the commit stands (PROVEN: pseudonym-mapping erasure, system-overview §8.3); the
  decision block, if erased, is a tombstone unfurl, the *edge* preserved for integrity.
- **moved/outdated** — the PR's linked issue was split → unfurl shows "moved to ISSUE-413" not a
  dead chip.

---

## 4. F-PM-1 — "Incident-to-runbook" (chat → issue → knowledge → chat)

**Realises R-03 E9** *(run an incident as one linked timeline)* **+ M6** *(promote a chat report
into tracked work and watch it flow)* **+ M2** *(intent linked to delivery)*. Entry: a `#prod-fire`
chat message "checkout 500s spiking". This is the PM/delivery flagship and the prompt's named PM
flow.

### 4.1 Service blueprint

| Lane | Step 1 | Step 2 | Step 3 | Step 4 | Step 5 |
|---|---|---|---|---|---|
| **Actor** (P6 PM / P7 EM; P3 SRE co-present) | Reads alert in **chat thread** | **Promotes thread → incident issue** | Pins the **runbook doc** into the thread | Works the runbook steps; status rolls up | **Resolves**; thread auto-summarised back |
| **Frontstage §7** | Incident channel/thread (§7.5) | Chat composer **convert-to-issue** (§7.5) → Incident issue (§7.3) | Runbook page unfurl (§7.4) **inline in thread** | Issue hierarchy/status (§7.3); deploy/run view (§7.2) refs | Thread; issue closed; release/postmortem note (§7.4) |
| *— line of visibility —* | | | | | |
| **Backstage events** | `chat.message.posted` | `issue.created` + `ref.created` thread↔issue | `ref.created` issue↔doc | `ci.deployment.*`, `issue.transitioned` | `issue.transitioned(resolved)`; agent-drafted summary |
| **Shared-system support** | Bus; Notif | Id (perm to create); Refs | Refs projection (doc §7.4) | Bus live updates → pane stays live | Refs; optional `DocBot` (agent-pending) |

🔪 **Seam dissolved:** today the PM copies a Slack message into Jira by hand, pastes a Jira link
back into Slack, then hunts Confluence for the runbook and pastes *that* link too — three tools,
three copy-pastes, and the thread and the issue immediately drift. Myelin: **convert-to-issue**
preserves the thread as a live backlink; the runbook unfurls **in** the thread; the issue and the
chat are **one timeline** (E9). Closing the issue posts the resolution back to the thread.

### 4.2 Job flow + full state set

Entry points: (a) hover-action on a chat message → "Convert to issue"; (b) palette `convert to
issue` while a message is focused; (c) `@myelin open an incident from this thread` (agent path,
overlaps F-AGT-1). Keyboard: focus message `j/k` → `.` (message actions) → `i` (convert) →
issue opens in a side pane (no navigation away from chat — the seam dissolves *by staying put*).

| Screen | empty | loading | error | permission | erased/tombstone | agent-pending | **edge branches** |
|---|---|---|---|---|---|---|---|
| Incident thread | "No messages yet" | message skeletons | "Couldn't send — retry, draft kept" | message hidden if channel-scoped | erased message → "Message deleted" tombstone, thread intact | "Summary agent drafting…" | **🔪 notification storm:** an incident floods the channel + fans out mentions → inbox **dedups by `origin_event`, collapses "23 updates on INCIDENT-9", agent volume routed out of the main timeline** (R-03 M5/D3; R-21 owns the storm state). Calm-by-default is the design requirement, not best-effort. |
| Convert-to-issue | prefilled title from message; "Pick a project" | optimistic issue stub | "Couldn't create — you can post but not file here" → offers *request access* | create-issue denied in this project → explains grant | — | agent can pre-fill labels (confirm-not-auto) | **optimistic-rollback:** issue chip appears in thread instantly; backend reject reverts it, message text untouched |
| Incident issue (side pane) | n/a | structure skeleton | "Couldn't load issue" | fields redacted per role | issue erased mid-incident → tombstone, thread link degrades gracefully | rollup recompute pending | **stale/reconnecting:** live status drops → "Reconnecting"; resumes on `issue.*` replay |
| Runbook unfurl in thread | "No runbook linked — search docs" | unfurl skeleton | "Couldn't load doc preview" | **cross-space no-access** → graceful card, never the doc title | runbook erased → tombstone "Document no longer available" | "DocBot proposes updating step 4 (stale)" agent-pending card (M9) | **cross-cell ref:** runbook in another residency cell → projection if visible else no-access card |

**Acceptance hooks:** covers chat↔issue↔knowledge↔chat round-trip (E9/M6), the notification-storm
branch, cross-space no-access, and the agent-pending doc-update card — feeds R-19 (incident-as-DSR-
adjacent), R-21 (storm), R-22 (live unfurl-in-thread wedge).

---

## 5. F-PM-2 — "Report-from-reality" (the D1 dual-audience proof, condensed)

**Realises R-03 M1/M3** over the **same issue model** engineers burn down on a board (the **D1**
same-data pair). Included because the prompt's thesis ("one product") is *only* proven if a PM
flow and an engineer flow demonstrably ride one schema. R-16 owns the per-lens critique; here we
draw the **flow that touches both lenses**.

Blueprint (condensed): stakeholder asks in chat/meeting → PM opens **Roadmap/now-next-later view**
(§7.3) → it is the **same records** the engineer sees as a **board** (§7.3), projected through one
query AST (ADR-06/07) → PM filters/groups (config, not a fork) → exports a rollup. Backstage: zero
new events — the roadmap is a *view*, so `issue.transitioned` by an engineer **moves the PM's
roadmap live**. 🔪 **Seam dissolved:** today PMs maintain a Productboard/slide reality parallel to
Jira and it is always stale; here the report **is** the delivery data.

State edges specific to D1: **stale** (a teammate just transitioned an issue → the roadmap shows
"updating…" then settles, optimistic); **permission** (a confidential epic the PM can roll up as a
count but not open → "1 restricted item in this rollup" without leaking it); **empty** ("No work
scheduled — drag from backlog or ask the planning agent").

---

## 6. F-GOV-1 — "DSR fan-out" (DPO across all five surfaces)

**Realises R-03 G5/G6/G10** *(locate/export/erase a person's data across all subsystems within
the GDPR clock, preserving integrity)* — the **D4** dual-audience pair (DPO view ↔ data-subject
view). Built directly on **system-overview §8.3** (DSR orchestrator + crypto-shred + tombstone
ladder) — we **surface** that mechanism as a flow, we do not redesign it. R-19 deepens the
console UX; this is its blueprint.

### 6.1 Service blueprint — the DPO view (and the data-subject view marked)

| Lane | Step 1 Locate | Step 2 Review | Step 3 Erase decision | Step 4 Fan-out erase | Step 5 Receipt |
|---|---|---|---|---|---|
| **Actor** | DPO (P13) opens DSR; **data-subject** sees the *same inventory* of their own data from the self-service side (D4) | DPO reviews the one inventory | DPO confirms erase; **system states the consequence first** | (automated) | DPO + subject get receipt |
| **Frontstage §7** | GDPR/data-rights console (§7.6); subject view = same inventory, own-scope | One inventory "everywhere this subject appears" + deadline clock | Consequence dialog ("git history keeps pseudonymised authorship; N artifacts tombstone") — HAX "convey consequences" | progress per holder | **Verifiable deletion receipt**; audit entry |
| *— line of visibility —* | | | | | |
| **Backstage events** | `gdpr.export.requested` → **fan-out to every PersonalDataHolder** (Git/Issues/KN/Chat/CI/Search/Refs/Bus-history/Agent-memory) | inventory assembled | `gdpr.erasure.requested` | crypto-shred keys; purge+re-index Search; **tombstone Refs nodes/edges**; delete pseudonym mapping; erase agent memory | audit (carved-out, retention-bounded) |
| **Shared-system support** | DSR orchestrator; Id (subject→PrincipalRef + pseudonym map) | all holders project | KMS / crypto-shred | every holder | Audit |

🔪 **Seam dissolved (the governance wedge):** in the stitched stack a DSR is **N manual searches
across N tools with N export formats and no proof of completeness** (UC-X-17); the GDPR clock runs
while the DPO scavenger-hunts. Myelin fans out from **one** subject identity to **one** inventory
with **one** verifiable receipt — because every subsystem is already a `PersonalDataHolder` on the
shared event/ref graph. This is the sovereignty payoff made operational.

### 6.2 Job flow + full state set (the GDPR-aware degraded states are the point here)

Entry: DPO console → "New data-subject request" → resolve subject (by email/handle, perm-gated to
DPO role). Keyboard fully operable (G1 — this is a regulated surface; a11y is a hard gate).

| Screen | empty | loading | error | permission | **erased/tombstone (first-class here)** | agent-pending | **edge branches** |
|---|---|---|---|---|---|---|---|
| Subject resolve | "Search by email/handle" | type-ahead skeleton | "Couldn't resolve — try identifier" | **only DPO/admin role** may open DSR; others get clean no-access | subject already fully erased → "No retained personal data for this subject" + prior-receipt link | — | **ambiguous subject:** two principals match → disambiguation step (never auto-pick) |
| Inventory (locate/export) | "No personal data found in scope" | **per-holder progress skeleton** (10 holders, each resolving) | **partial-failure:** "Chat holder timed out — 9/10 complete, retry holder" (never a silent partial — completeness is the legal point) | items the DPO themselves can't see are still **counted** for completeness but shown as "restricted item — present, withheld from this view" | already-tombstoned artifacts listed as "previously erased on <date>" | — | **cross-cell:** subject has data in another residency cell → inventory shows the cell + residency tag; export stays in-region (P9/ADR-11) |
| Consequence dialog | n/a | — | — | — | shows exactly what becomes a tombstone vs. what is crypto-shredded vs. what keeps pseudonymised authorship | — | **integrity guard:** if erasure would break audit carve-out, the dialog **blocks and explains** rather than proceeding |
| Erase fan-out progress | n/a | per-holder progress | **per-holder failure isolated:** "Search re-index failed — erasure of other holders stands; retry Search" (saga-style compensation, not all-or-nothing) | — | holders transition to tombstone live | — | **resume-after-crash:** the durable workflow resumes; idempotent erase means re-run is safe |
| Receipt | n/a | generating | "Receipt generation failed — data is erased; regenerate receipt" | DPO + subject only | the receipt itself records what was tombstoned vs shredded | — | **deadline-at-risk:** clock < 72h → surface escalates, not buried |

**Data-subject view (D4 lens 2):** the same inventory, **own-scope only**, read+export, no erase-
others power; "request correction/erasure" buttons that **open the DPO-side flow**. Proving
neither lens is a degraded compromise is **R-16's** job; here both lenses are drawn over one graph.

**Acceptance hooks:** covers all five surfaces as holders, the erased/tombstoned state as
first-class, partial-holder-failure (the branch a happy-path DSR demo skips and a regulator would
catch), cross-cell residency, and both D4 lenses. Feeds R-19 directly.

---

## 7. F-AGT-1 — Agent HITL flagship (with all partial-failure branches)

**Realises R-03 E11/M9 plane + E3.** Built on **system-overview §8.2** + **agent-native-design
§8.1** (plan-then-apply; effects validated by `EffectApi`; HITL gate as a durable wait; one
`correlation_id`; loop-depth cap). This is the flow with the **densest edge-branch set** — drawn
so R-14 can spec each agent state. **The agent is an actor on the blueprint, not backstage.**

### 7.1 Service blueprint (happy chain)

| Lane | 1 Detect | 2 Triage (agent) | 3 File+post | 4 Propose fix (agent) | 5 Gate | 6 Approve (human) | 7 Review (agent) |
|---|---|---|---|---|---|---|---|
| **Actor** | CI | **TriageAgent** (on-behalf-of pusher; budget) | TriageAgent | **FixAgent** | — | Human in chat | **ReviewerAgent** |
| **Frontstage §7** | — | agent-pending badge on issue/chat (§6) | Issue (§7.3) + chat post (§7.5) w/ live refs | proposed-PR card | **HITL approval card** (§5.4) in chat + inbox | Approve/**Edit**/Reject control | agent review comments on PR (§7.1) |
| *— line of visibility —* | | | | | | | |
| **Backstage events** | `ci.pipeline.failed` | trigger T1 → `agent.run.started`; `handle()`→`AgentDecision{effects}` | `issue.created`, `ref.created`×2, `chat.message.posted` | trigger T2 → plan `git.open_pr` (SENSITIVE) | `EffectApi`→`Gated`; durable wait | workflow signal (mins/days later) | `git.pr.opened`→ReviewerAgent |
| **Shared-system support** | Bus | Agent Fabric (plan-then-apply); Id (perms ∩ delegation ∩ tenant) | Refs; Audit (every action attributed, `correlation_id`) | Id validates sensitive effect | Durable-workflow | Notif/Chat | Audit; loop-depth accounting |

🔪 **Seams dissolved:** the agent works **across** CI→Issues→Chat→Git in one `correlation_id`
chain that a human can read end-to-end (R-22's "one correlation_id across surfaces" wedge); the
approval is **in chat** where the team already is, not a separate ops console.

### 7.2 Partial-failure branches (the prompt's required set — each is a designed state, not a 500)

Grounded in agent error-recovery doctrine: **preserve user context, acknowledge the limit, offer
the next step, degrade gracefully** *(PROVEN method,
[Clearly Design, *Designing for AI Failures*](https://clearly.design/articles/ai-design-4-designing-for-ai-failures);
[Agentic Design Patterns, *Error Recovery (ERP)*](https://agentic-design.ai/patterns/ui-ux-patterns/error-recovery-patterns)).*
**Doctrine note:** design-language §6 + the §6-contract are *stricter* than generic agent UX —
where they conflict, **doctrine wins** (carried to R-14); e.g. "never a silent agent edit,"
"never colour-alone for agent state."

| Branch | Trigger | Frontstage state (design) | Backstage | Recovery the user gets |
|---|---|---|---|---|
| **Gate rejected** | Human clicks **Reject** on the approval card | Card resolves to "Rejected by <human> · <reason>"; the proposed PR is **discarded, not opened**; issue stays open, **attributed** | workflow ends; `agent.run.finished{rejected}`; audit | Issue remains for a human to take; agent does not retry the same plan (dedup key) |
| **Gate edited** | Human clicks **Edit** → amends the proposed effect (e.g. changes the fix branch / scope) before approving | Card shows the **diff between proposed and human-amended** effect; applies the human's version, attributed to *human-edited-agent-proposal* | `EffectApi.apply(edited effect)` | The Edit path (R-14 owns its full spec) — human stays in control of the exact change |
| **Agent error mid-chain** | `FixAgent.handle()` returns `AgentError` (or a tool call fails) after the issue was already filed | Issue + chat post **stand** (saga: completed steps are not rolled back); a quiet card "FixAgent couldn't propose a fix — the issue is filed; take it from here" | `agent.run.failed`; `correlation_id` preserved; **no half-open PR** | Human inherits a *partial-but-coherent* state, never a corrupt one (compensation/saga pattern) |
| **Budget exceeded** | run hits max effects / wall-clock / cost cap | "Triage paused — budget reached. Resume / increase budget (admin) / take over" | `agent.run.failed{reason: budget}`; platform-enforced | Admin can raise budget (governance surface, R-15); work done so far stands |
| **Loop-guard tripped** | agent's effect would re-trigger itself past causation-depth cap | run stops; **operator alarm**, not a user-facing crash; "Automation paused to prevent a loop" | depth/cycle accounting; circuit breaker | Per-tenant kill-switch visible in governance console (G4) |
| **Cross-cell / no-access effect** | agent proposes an effect on an artifact in a cell/tenant it has no delegated grant for | effect returns **Denied** with the missing grant named; **never leaks** the target's content | `EffectApi`→`Denied`; audit | Card explains *which* grant is missing; nothing silently happens |
| **Approval card storm** | many gates pending at once (surge) | inbox **collapses gates into a grouped "7 approvals awaiting you" with per-item Approve/Reject**; agent chatter routed out of the main timeline | Notif dedup | Calm triage, not 7 separate pings (R-21 storm) |
| **Stale approval** | human approves days later; the base moved | card revalidates on resume; if the PR no longer applies, "The base changed — re-propose?" rather than opening a broken PR | durable-workflow re-check | Re-proposal, not a silent merge of stale work |

### 7.3 Job flow state set (the approval card itself)

`agent-pending` → `agent-working` → `gate-awaiting` → {`approved` | `gate-rejected` |
`gate-edited`} ; failure states `agent-error`, `budget-exceeded`, `loop-guard-tripped` cut across.
**Agent treatment** (badge + label + colour, **color-blind-safe, never colour-alone, never
sparkle**) is specced in **R-14**; here we only enumerate the states it must cover. Every card
shows **proposed effects per artifact + delegated authority** ("FixAgent, on behalf of @dev, may:
open PR #88") *before* anything happens — the plan-then-apply contract made visible (R-14/D6).

---

## 8. Cross-flow seam register (where Myelin dissolves the stitched stack)

| Seam (today's tab-switch / copy-paste) | Flow | What Myelin does instead | Mechanism (PROVEN) |
|---|---|---|---|
| Checks tab → logs → guess file → Files tab | F-ENG-1 | log line **is** a ref to the diff line; one click | content-anchored line-range; Refs |
| Copy issue ID into PR body by hand | F-ENG-1 | new PR pre-populated with linked issue/run | Refs graph + `ref.created` |
| Blame → SHA → commit → PR → Jira → Confluence (4 tabs) | F-ENG-2 | 4 in-place live unfurls | §8.1 projection-per-`ArtifactRef` |
| Paste Slack msg → Jira; paste Jira link → Slack; hunt Confluence | F-PM-1 | convert-to-issue keeps live backlink; runbook unfurls in thread | `ref.created` thread↔issue↔doc |
| Maintain a slide/Productboard reality parallel to Jira | F-PM-2 | the report **is** a view over the delivery data | one query AST (ADR-06/07) |
| N manual searches across N tools for a DSR, no proof | F-GOV-1 | one subject → one inventory → one receipt | DSR orchestrator fan-out (§8.3) |
| Agent ops in a separate console away from the team | F-AGT-1 | approval **in chat**, one `correlation_id` readable across surfaces | plan-then-apply + Bus + Audit |

These seven seams are the **wedge candidates** R-22 turns into named love-moments, and the
**comparable scenarios** Phase 6 finalists must demonstrate (sketch-funnel comparable-screen set).

---

## 9. Actionability toward the control artifacts

| Control artifact | What these flows equip | Where |
|---|---|---|
| **rubric.md D4** (one-product coherence) | Each flow spans ≥3 subsystems and must *feel* like one product; the seam register (§8) is the checkable list of where coherence is proven or fails. | §2–§8 |
| **rubric.md D6** (agent) | F-AGT-1 §7.2 enumerates the full partial-failure branch set + plan-then-apply visibility — the concrete moments D6 scores; feeds R-14. | §7 |
| **rubric.md D5** (dual-audience) | F-PM-2 (D1) and F-GOV-1 (D4) draw the *flow* touching both lenses of a same-data pair; R-16 critiques each lens. | §5, §6 |
| **rubric.md D8** (states/craft) | Every flow's job-flow table carries the full §5.10 set **plus** the skipped states (rollback, conflict/stale, rebase-orphan, storm, partial-holder-failure). | §2–§7 |
| **sketch-funnel comparable screen set** | The six named flows ARE the scenarios Phase 6 finalists sketch; the seam moments are the wedge screens. | §1, §8 |
| **sketch-funnel Axis 5** (agent presence) | F-AGT-1's branch set lets finalists occupy materially different positions on how present/legible the agent is. | §7 |
| **R-14 / R-19 / R-22** | F-AGT-1 → R-14; F-GOV-1 → R-19; the §8 seam register → R-22. | §6–§8 |

---

## 10. Completeness-critic (README §9) — which gloss-risks R-04 owns vs. defers

R-04 is the **cross-surface / edge-case flow** owner, so it **must not** gloss these:

- **Partial-failure agent branches (§9)** — **OWNED & covered**: F-AGT-1 §7.2 (gate-rejected,
  gate-edited, agent-error-mid-chain, budget-exceeded, loop-guard, cross-cell-denied, card-storm,
  stale-approval). The *visual* spec of each state is deferred to R-14 (named).
- **Cross-cell ref → no-access / tombstone (§9)** — **covered** in F-ENG-1, F-ENG-2, F-PM-1,
  F-GOV-1 state tables and §7.2.
- **Diff-anchored comment relocating after rebase (§9)** — **covered** in F-ENG-1 §2.2 (content-
  anchored re-resolve; detach-to-outdated-pill rather than silent wrong-line move).
- **Notification storm / 30×-agent-surge (§9)** — **covered as a flow branch** in F-PM-1 and
  F-AGT-1 (§7.2 card-storm); the *state-craft catalogue* is deferred to R-21 (named).
- **The DSR/erasure flow from data-subject AND DPO sides (§9)** — **OWNED & covered**: F-GOV-1
  draws both D4 lenses; console UX deferred to R-19.
- **CLI as a peer surface (§9, §7.7)** — **covered**: F-ENG-1 names the `myelin run`/CLI path so
  the job finishes in either rendering.
- **Optimistic-rollback, stale/reconnecting (§9)** — **covered** as edge branches; the per-
  component skeleton/rollback *pattern* is deferred to R-13/R-21 (named).
- **Touch/mobile, conflict CAS→CRDT (§9)** — **consciously deferred** to R-21 (state-craft) and
  the editor spec (R-10); these flows operate at the screen/action altitude, and no flow is
  *hidden* by omitting them. (Conflict is touched only where F-PM-2 shows live concurrent edits.)

---

## 11. `[DEFERRED-UNTIL-USERS]` — what these flows assume that only users can confirm

These are **expert-authored service blueprints + job flows (the no-user substitute), NOT
validated flows.** Per the standing no-user constraint, the validation is recorded as a plan:

- **What to test:** cognitive walkthrough + task-based usability on each named flow — can the
  recruited persona (a) find the entry point, (b) complete the cross-surface hop without a
  tab-switch they expected, (c) recover from each partial-failure branch (esp. F-AGT-1 gate-
  reject/edit and F-GOV-1 partial-holder-failure)?
- **With whom:** F-ENG-1/2 → P1/P2/P3 engineers; F-PM-1/2 → P6/P7 PMs/EMs; F-GOV-1 → **P13 DPO +
  P14 procurement** (the regulated-buyer review, run jointly with R-19); F-AGT-1 → mixed +
  **P12 security** (the agent-governance lens).
- **What would falsify the flow hypothesis:** (1) users still reach for a second tool at a marked
  seam (the seam is *not* actually dissolved); (2) a partial-failure branch leaves users unsure
  what state they're in (the saga/compensation design failed the legibility test); (3) the cross-
  surface hop creates *more* cognitive load than the tab-switch it replaced (coherence regressed);
  (4) for F-GOV-1, a DPO does **not** trust the receipt/completeness at a glance (the §9
  sovereignty-as-UX bet, owned by R-19's deferred review).
- **Caveat (carried to R-14/R-15):** F-AGT-1 is drawn against the **mock** agent runtime; mock-
  agent behaviour may not predict real-LLM behaviour. The **contract** (plan-then-apply, gates,
  budgets, one correlation_id) is designed to be trustworthy *regardless of runtime* — that is the
  thing to validate, not the mock's specific outputs.

---

## 12. Self-check against R-04 acceptance criteria

| Criterion (prompt R-04 / ws-b) | Status | Evidence |
|---|---|---|
| **≥1 named flow per audience + the agent flagship** | ✅ Met | A: F-ENG-1/2; B: F-PM-1/2; C: F-GOV-1; cross: F-AGT-1 (§1 index) |
| **Each shows frontstage §7 screens, backstage events, AND agent actors** | ✅ Met | Every blueprint has Actor (incl. agents on the actor line) / Frontstage §7 / line of visibility / Backstage events / Shared-system support (§2,4,6,7) |
| **Each job-flow enumerates the full state set incl. partial-failure agent branches** | ✅ Met | §5.10 set in every job-flow table; F-AGT-1 §7.2 is the full partial-failure branch set (gate-rejected, agent-error, budget, loop-guard) |
| **Partial-failure / skipped branches the happy-path bias misses (per §9)** | ✅ Met | gate-rejected/edited (§7.2); agent-error-mid-chain (§7.2); cross-cell→no-access/tombstone (§2.2/§3.2/§6.2/§7.2); rebase-orphan comment (§2.2); notification storm (§4.2/§7.2) |
| **Seams explicitly marked** | ✅ Met | 🔪 markers throughout + the §8 seam register (7 seams) |
| **Build ON R-03, don't duplicate** | ✅ Met | Each flow names the R-03 job(s) it realises; jobs not re-listed (§1, per-flow headers) |
| **Surface existing architecture mechanisms, don't redesign** | ✅ Met | Rides §8.1/§8.2/§8.3 + agent-native §8.1 + reference-graph anchors; tagged PROVEN where surfaced |
| **Date the file; PROVEN/HOUSE-STYLE tags; cited web sources** | ✅ Met | Dated 2026-06-20; tags in §0 and throughout; NN/g service-blueprint, happy-path/edge-case, agent-error sources cited |
| **Completeness-critic §9 gloss-risks addressed** | ✅ Met | §10 names which R-04 owns (partial-failure, cross-cell, rebase-orphan, storm, DSR both-sides, CLI) vs. defers (touch, conflict) |
| **Actionable toward rubric & sketch-funnel; feeds R-14/R-19/R-22, Phase 5/6** | ✅ Met | §9 mapping (D4/D5/D6/D8, comparable-screen set, Axis 5) + §8 wedge handoff |
| **Deferred validation recorded as a plan, not faked** | ✅ Met | §11 `[DEFERRED-UNTIL-USERS]`: what/with-whom/falsification + mock-vs-real caveat |

**Honest partials / top uncertainties.**
1. **All flows are expert blueprints, unvalidated** (§11) — the central risk is that a "dissolved"
   seam still triggers a tab-reach in practice; only the cognitive walkthrough + usability test
   resolves it.
2. **F-AGT-1 against the mock runtime** — real-LLM agent behaviour (error rate, plan quality) may
   reshape which partial-failure branches dominate; the contract is built to hold regardless, but
   the *frequency* of each branch is a HYPOTHESIS.
3. **F-GOV-1 partial-holder-failure UX is HOUSE STYLE / under-evidenced** — "a DPO trusts a
   partial-completion receipt" has no external playbook (inherited from R-03 §4 flag); R-19's
   regulated-buyer review is the real test.
4. **The rebase-orphan detach-to-outdated-pill** is our design choice (HOUSE STYLE) over silent
   relocation; it trades discoverability for safety and should be tested against engineer
   expectation in F-ENG-1's walkthrough.

---

*End of R-04 deliverable. Date: 2026-06-20. Service-blueprint + job-flow methods PROVEN
(NN/g; cited); all specific flow choreography HOUSE STYLE; no flow user-validated. Feeds R-14,
R-19, R-22, Phase 5, Phase 6.*
