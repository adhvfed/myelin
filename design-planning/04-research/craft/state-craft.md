# R-21 — Empty / Loading / Error / Permission / Erased / Agent-Pending State Craft

> **Phase 4 research corpus** · WS-J (onboarding & craft) · Seq #19. Deliverable for prompt **R-21** in
> [`03-research-prompts.md`](../../03-research-prompts.md). **File date: 2026-06-20.**
> Methods: **#9 (job-flow per-screen state checklist)**, **#19 heuristics** (error-recovery; visibility of
> system status), **§8b.6 specifics** (loading-shows-structure / error-blames-system / fails-static).
>
> **This is the README §9 "unglamorous states" owner.** The completeness-critic explicitly routes the
> skipped states here ("route to R-21, enforce in every Phase-6 finalist"). This file turns the §9 list into
> **a concrete, designed pattern per state** and a **per-surface state matrix** that Phase-6 finalists must
> satisfy and Phase-7 scores against (rubric D8 + the switch test D10).
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = a cited external standard/source, OR an existing Myelin
> contract this file *surfaces* (the resolver no-leak ladder R-09 §1.1; the notifications storm shed-budget;
> the §8b.6 mandates; the EXT-1 prefetch). **HOUSE STYLE** = our state-design synthesis/taste. **Not
> user-validated;** the deferred comprehension/trust tests are in §6.
>
> **Builds ON prior `04-research` (does not duplicate — this file *composes* their states into one catalogue
> + matrix):**
> - [R-09 reference-unfurl](../interaction/reference-unfurl.md) §1.1 / §5 — the resolver→UI state ladder; the
>   chip/unfurl OWNS no-access, tombstone/erased, moved/outdated, cross-cell, rebase-orphan, degraded,
>   agent-pending *at the artifact level*. R-21 inherits these and multiplies them across surfaces; it does
>   not re-derive the resolver.
> - [R-10 shared-patterns](../interaction/shared-patterns.md) §2.2/§3.3/§4.3/§5.3 — the per-component state
>   sets for views/editor/inbox/overlays. R-10 gave R-21 "its per-component starting set, not the full
>   matrix" (R-10 §7). R-21 builds the full matrix.
> - [R-13 perceived-performance](../visual/perceived-performance.md) §A.2/§A.3/§3 — the skeleton *craft*
>   (per-surface shapes, no-spinner-token rule), the three-state optimistic + honest-rollback contract
>   (OPT-1..4), and the reconnecting/degraded/storm routing. R-13 explicitly hands R-21 "the perceived-perf
>   half of its per-surface state matrix" (R-13 §4). R-21 owns the *placement* per surface.
>
> Where this file says "the chip's no-access render" it means R-09 §5.4; "the skeleton craft" means R-13 §A.2;
> "the optimistic contract" means R-13 §A.3. Those are cited, not restated.

---

## 0. How to read this file

Three parts:

1. **§1 — The 14-state catalogue.** Each unglamorous state as a *designed pattern* (not an afterthought):
   the rule, the §8b.6 specific, the canonical render, the trap, PROVEN/HOUSE-STYLE tag. The six common
   states **plus** the eight the happy-path bias skips (the §9 list in full).
2. **§2 — The per-surface state matrix.** Every shared component (R-10) × every primary §7 surface ×
   every applicable state — the checklist Phase-6 finalists must satisfy and Phase-7 scores.
3. **§3** completeness-critic (README §9 — this file owns the list) · **§4** rubric/funnel actionability
   (D8 + the per-finalist unglamorous-states requirement; sketch-funnel 6c) · **§5** sources · **§6**
   `[DEFERRED-UNTIL-USERS]` · **§7** self-check.

**The one-sentence thesis (HOUSE STYLE):** *a state is unglamorous only when it is unowned; every state
below is a designed, named pattern with a single house rule — **never blank, never blame, never leak,
never lie** — so that the moments the pipeline reliably skips become the moments that prove the product is
finished (the switch test, §8b.7).* The four-word rule maps directly: **blank** → loading/empty (show
structure / show the next step); **blame** → error (blame the system); **leak** → permission/erased (the
resolver collapsed it before the wire); **lie** → optimistic-rollback/conflict/stale (honest revert,
honest staleness, honest conflict, never a silent overwrite).

---

## 1. The 14-state catalogue (each a designed pattern)

The §9 list = **six common states** (1–6) the prompt names as the floor + **eight skipped states** (7–14)
the happy-path bias drops. Each is specified once here as a reusable pattern; §2 places them per surface.

### The shared anatomy (HOUSE STYLE — one state-frame, never bespoke)

Every non-happy state renders in **one of three frames**, never an ad-hoc one (the coherence rule, D4):
- **In-place frame** — the state replaces *only its own region*; the shell and sibling surfaces stay live
  (the fails-static law, §8b.6 / B6). Used by loading, error, degraded, permission, erased, agent-pending.
- **Inline-affordance frame** — a quiet line/pill *attached to the element* that's in a non-happy state
  (optimistic-pending, rollback line, "moved"/"outdated" pill, stale dot). Never a modal, never a toast
  that steals focus.
- **Onboarding frame** — the empty state's CTA-forward layout (only for empty).

A state never escalates its frame uninvited: a single chip failing to refresh shows an inline dot (§1.13),
**not** an in-place error that blanks the paragraph; a whole view failing shows an in-place error,
**not** a shell-level dialog. *(HOUSE STYLE — the "fail at the smallest scope" rule, the inverse of the
§8b.6 fails-static law applied to UI scope.)*

---

### 1.1 EMPTY — onboarding-forward (PROVEN doctrine + cited best-practice)

**Rule.** An empty surface is **never a blank container**; it shows the *next action front and center*
(PROVEN — §5.10 onboarding-forward; the cited best-practice: "a new user shouldn't have to guess the first
step — they should see it, front and center", a ~2:1 instruction-to-delight ratio, [Pencil&Paper /
UserOnboard, 2025](https://www.pencilandpaper.io/articles/empty-states)). **The canonical layout:** concise
headline → one supporting line → the single primary CTA (keyboard hint included) → optional restrained
illustration/icon (no AI sparkle, §8b.3).

**Three empty *kinds* (HOUSE STYLE — they are not the same pattern):**
- **First-use (zero-data) empty** — the startup-persona-critical one (P1; R-20 owns the *guided-start
  sequencing*, R-21 owns the *per-surface render*): "No issues in this cycle yet — create one `C`"; "No
  repos yet — `git push` or import"; "Nothing in your inbox — you're all caught up" (the *rewarding* empty,
  not a nag; R-10 §4.3). **R-21 ↔ R-20 boundary:** R-20 designs the *cross-surface first-run journey*; R-21
  designs *each surface's empty render* so they compose into it.
- **Cleared empty (inbox-zero)** — a calm, *rewarding* payoff, not a celebration burst (no confetti — R-13
  B.5; R-12 anti-list). "You're all caught up."
- **Filtered-to-nothing empty** — distinct from zero-data: "No results for these filters" + a one-click
  "clear filters" path; must never read as "you have no data" when data exists behind the filter.

**Trap.** A generic illustration with no CTA (a dead-end blank with a mascot); or conflating
filtered-to-nothing with zero-data (the user thinks their data vanished). **Tag:** PROVEN doctrine; HOUSE
STYLE per-kind split.

### 1.2 LOADING — structure-skeleton, never a blank spinner (PROVEN — owned-craft in R-13)

**Rule.** Loading shows a **structure-matching skeleton** (geometry = the final layout), **never a spinner
on a blank page** (PROVEN — §8b.6 verbatim; skeletons perceived ~20–30% faster, R-13 §A.2). **There is no
spinner token in the system** (R-13 §A.2). **§8b.6 specifics applied:** suppress flash-of-spinner under
~1s (skeleton from frame 0, or nothing if it resolves instantly); over ~1s the skeleton *is* the wait UI.

**R-21's job over R-13:** R-13 specced the skeleton *shapes* per surface (table → ghost rows; board → ghost
columns+cards; editor → block-skeleton; PR pane → labelled section scaffold). R-21 **places them in the
matrix (§2)** and adds the *partial-load* render: a surface whose scaffold is known but whose sections fill
independently (the PR context pane — diff/issue/run/discussion each skeleton→fill as its EXT-1 bundle
resolves, R-13 CA-2) shows **per-section** skeletons, not one page-level spinner.

**Trap.** A generic shimmer rectangle (shows no structure → fails the doctrine's "matches final layout"
clause, R-13 §A.2); a spinner that flashes for 200ms then vanishes (the flash-of-spinner the <1s rule
bans). **Tag:** PROVEN.

### 1.3 ERROR — blame the system in one quiet line + a path (PROVEN — §8b.6 + NN/g)

**Rule.** An error **blames the system, never the user**, in **one quiet line**, and offers **a path** (retry
/ what-happened / contact), and is **scoped to its region** (fails static — the shell stays live) (PROVEN —
§8b.6 verbatim; NN/g error-message guidelines: phrasing must "unambiguously place accountability on the
system, not the user", be human-readable, polite, constructive, [NN/g Error-Message Guidelines](https://www.nngroup.com/articles/error-message-guidelines/),
[NN/g 10 Guidelines for Form Errors](https://www.nngroup.com/articles/errors-forms-design-guidelines/)).
**The user's input/work is never lost** (a save error keeps the typed buffer — R-10 §3.3).

**The quiet-line grammar (HOUSE STYLE):** `{what failed, system-voiced} — {the path}.` e.g. *"Couldn't load
this view — retry."* / *"Couldn't re-run — try again."* Never *"You don't have permission"* phrased as the
user's fault, never a stack trace, never an error code as the headline (the code goes in a "details"
disclosure for support, with the `correlation_id` for the audit trail — R-15). **Distinguish error from
permission:** a *system* failure (substrate down) is error (§1.3 + §1.7 degraded); a *permission* outcome is
never an error (§1.4) — calling a permission outcome "Error 403" is the trap.

**Trap.** Blaming the user ("invalid input" with no fix); a dead end (no retry/path); error-code-as-headline;
an error that blanks the whole shell instead of its region. **Tag:** PROVEN.

### 1.4 PERMISSION-DENIED — graceful no-access card, never a leak (PROVEN — resolver no-leak, R-09)

**Rule.** Permission-denied is **never an error and never a leak**: the resolver collapses the artifact to a
tombstone *before content crosses the wire*, so the UI **receives no title to leak** (PROVEN — R-09 §1.1
row-1 / §5.4; ADR-03 `list_objects` pre-filter; the chip is non-leaking *by construction*). Two distinct
renders, and the choice between them is **a policy input, not a frontend guess** — mirroring the PROVEN
403-vs-404 security distinction ([Authress 401/403/404 guidance](https://authress.io/knowledge-base/articles/choosing-the-right-http-error-code-401-403-404),
[CGI: when to return 404 instead of 403](https://www.insights.cgi.com/blog/when-should-you-return-404-instead-of-403-http-status-code)):

- **"Restricted" (you may know it exists)** — the **graceful no-access card**: `{type-icon} Restricted` /
  *"You don't have access to this {type}."* + a **request-access path** where policy allows one (R-09 §5.4).
  The *type* is shown (it's in the URN, non-sensitive). This is the 403-analogue: existence is acknowledged.
- **"Absent" (you may not know it exists)** — the artifact is **simply not present**: a forbidden *row in a
  view* is **not rendered at all** (you cannot infer it exists — R-10 §2.2 permission-pre-filter); a
  search/palette result is **absent**, not greyed (R-08). This is the 404-analogue: existence itself is
  withheld.

**The load-bearing rule (PROVEN):** the *Restricted-vs-Absent* choice is the resolver's/policy's, surfaced
faithfully — the UI never downgrades Absent to Restricted (which would leak existence) nor upgrades
Restricted to a full render (which would leak content). **Permission-denied is the GDPR/ADR-03 correctness
invariant surfaced as UX** (§9). **Trap.** Greying-out a forbidden action (advertises the power + leaks the
gate, R-09 §2.2 / §4.2); a generic 403 page that confirms a private resource exists when policy wanted it
hidden; phrasing it as the user's error. **Tag:** PROVEN.

### 1.5 ERASED / TOMBSTONED — GDPR-aware degraded (PROVEN — resolver tombstone ladder, R-09)

**Rule.** A deleted/erased artifact **degrades to a dignified tombstone, never a broken-image icon or a
dangling id** (PROVEN — R-09 §5.7; ADR-12; §5.10). Three flavours, each **carrying the root** so the
reference degrades to *context*, never vanishes:
- **`sub_gone`** — *"{parent title} — the referenced section no longer exists."*
- **`root_gone`** (deleted) — *"This {type} was deleted."*
- **`erased`** (crypto/pseudonym-shred under a DSR) — *"This {type} was erased under a data-rights request."*
  **No content, no name, no recoverable PII** (PROVEN — R-09 drill D-5: 0 recoverable PII). An erased *actor*
  humanises to *"[erased user]"* in attribution/inbox (PROVEN — R-10 §4.3 / notifications.md D-N6).

**The sovereignty-as-UX moment (P9; R-19 owns the consoles).** The erased tombstone is **honest and
dignified, not alarming** — it must read as a *lawful, intended* degradation, not data-loss/breakage (the
deferred comprehension test, §6 / R-09 §11). **R-21 ↔ R-19 boundary:** R-19 owns the DSR console + the
erasure *flow*; R-21 owns the *erased render* on every consuming surface (chip, unfurl, backlink, search
result, view cell, inbox subject, attribution line, page-history entry — §9 names all four+).

**Trap.** A broken-image/404 glyph for an erased artifact (reads as breakage); leaking the former
title/name "for context"; a different erased render per surface (incoherence). **Tag:** PROVEN.

### 1.6 AGENT-PENDING — working / awaiting-approval (PROVEN — agent contract; R-14 owns HITL depth)

**Rule.** When an agent is mid-action, the surface shows an explicit **agent-pending** state in two clearly
distinct sub-states (PROVEN — §5.10 / §6; R-14 owns the HITL card depth, R-15 the calm-volume routing):
- **Agent-working** — *"{agent} is working…"* with the **agent treatment** (badge + label, color-blind-safe,
  **never colour-alone**, never sparkle — P7/§3.2/R-14), an interruptible/cancellable affordance where the
  fabric allows, and a *budget/scope hint* where relevant (R-15). It is a **peripheral** state by default
  (out of the main timeline — threads/inbox, R-13 B.4), surfacing to center only when it needs a human.
- **Gate-awaiting (awaiting your approval)** — the durable-gate-waiting state: the HITL approval card
  (R-14) docked in chat (primary) + inbox (`reason=approval_requested`, critical priority, *never missed* —
  R-10 §4.3). Shows the **proposed effects per artifact + delegated authority** before they happen (R-14).

**The agent partial-failure states (PROVEN — R-14's agent state set; the §9 partial-failure branches).**
Agent-pending is one of a family R-14 owns; R-21 *places* them per surface and ensures none is skipped:
`gate-rejected` (the human rejected — shows the rejection + audit link, not a silent disappearance),
`agent-error` (the agent errored mid-chain — system-blamed line per §1.3 + the partial work preserved),
`budget-exceeded` / `loop-guard-tripped` (the run halted — *"stopped: budget/loop limit reached"* + the
governance/audit path, R-15). Each is honest, attributed, and audit-linked (P7).

**Trap.** An agent action that applies with no pending/approval state (the no-HITL leak — VISION
non-negotiable); agent volume in the main timeline (re-creates overload at machine speed, R-13 B.4); a
silent agent failure (the run vanishes with no trace). **Tag:** PROVEN (contract); depth → R-14/R-15.

---

> **States 7–14 — the eight the happy-path bias skips. These are the acceptance-critical ones (§9 names
> them; the prompt requires them "not just the six common ones").**

### 1.7 DEGRADED-SURFACE "temporarily unavailable" — fails static (PROVEN — §8b.6 / notifications.md)

**Rule.** When a subsystem/cell is down, **that surface fails *static*** — it shows *"temporarily
unavailable"* (or its last-known content + a "couldn't refresh" marker, §1.11) **for that surface only**;
the shell and every other surface **stay live** (PROVEN — §8b.6 verbatim / B6; notifications.md §5.3
fails-static — already-materialised items still render on an outage). **Already-loaded content is never
blanked by a refresh failure.**

**The render (HOUSE STYLE):** an in-place, calm panel — *"{Surface} is temporarily unavailable — retrying.
{other surfaces are unaffected}"* — with an auto-retry indicator (not a spinner-blank). A *cell/region*
degradation (residency cell down) degrades only artifacts homed there; cross-cell refs to a healthy cell
still resolve (R-09 §5.8/§5.10). **Trap.** One subsystem erroring blanks the whole shell (the cascading
blank-screen — the inverse of fails-static); a degraded surface that loses the user's already-loaded data.
**Tag:** PROVEN.

### 1.8 STALE / OFFLINE / RECONNECTING — firehose drop + resume (PROVEN — firehose resume; cited offline patterns)

**Rule.** A live surface (chat, presence, live CI log, collaborative editing, live chip) that loses its
realtime transport shows an **honest staleness cue + a reconnecting affordance**, keeps the **last-known
content visible** (never blanks), and **resumes losslessly** on reconnect (PROVEN — the firehose
resume-cursor protocol: resume from `last_seq`, backfill-then-live, zero items lost, `resync_required` →
full reload *named not silent* — notifications.md §7 / D-N11; R-10 §4.3). Grounded in cited offline-UX:
keep cached content visible + mark it ("offline / trying to reconnect"), warn on *very* stale data, *"couldn't
refresh — showing cached content" is honest without being dramatic* ([offline/reconnecting UI patterns, 2025](https://blog.logrocket.com/offline-first-frontend-apps-2025-indexeddb-sqlite/),
[Educative: stale-while-revalidate](https://www.educative.io/courses/learn-react/np/background-sync-and-the-stale-while-revalidate-pattern)).

**The three escalating cues (HOUSE STYLE — calm → honest, never dramatic):**
- **Reconnecting (transient)** — a quiet, non-blocking *"Reconnecting…"* affordance in the surface chrome
  (not a full-screen modal); content stays interactive where safe (optimistic writes buffer locally).
- **Stale (visible but old)** — last-known content + a quiet *"showing last known — couldn't refresh"* dot
  (the chip's degraded render, R-09 §5.10); for *very* stale, an explicit timestamp (*"updated 12m ago"*).
- **Offline (no transport)** — *"You're offline — showing cached content"* banner; writes buffer and
  re-sync on reconnect; **offline scope is `[OPEN → P4]`** (design-language §9; R-10 §3.3 — the *full*
  offline-editing model is unscoped; R-21 specifies the *render*, flags the scope).

**Trap.** Blanking the surface on disconnect (loses the user's context); a silent resync that loses items
(the firehose must be lossless); a dramatic full-screen "connection lost" that blocks reading cached
content. **Tag:** PROVEN (firehose contract + cited patterns); HOUSE STYLE per-cue split.

### 1.9 OPTIMISTIC-UPDATE ROLLBACK — honest failure (PROVEN — owned-contract in R-13)

**Rule.** An optimistic write that the server rejects **visibly and honestly reverts** — the rollback is
*more* visible than the settle, never a silent swallow (PROVEN — R-13 §A.3 OPT-1; "optimism for latency,
honesty on failure", §8b.6). R-13 owns the three-state contract (pending → settled → rolled-back) and the
four binding rules (OPT-1 honesty / OPT-2 reversibility-vs-confirm carve-out / OPT-3 never-clobber-in-flight
/ OPT-4 idempotent-retry). **R-21's job:** *place* the rollback render per surface (§2) and fix the **rollback
render**: the element **un-does** (reverse `motion.move`, R-12) back to its prior state + **one quiet
system-blaming line** (§1.3 grammar) + a path (retry / undo). The failure must look *different* from success.

**Trap.** A failed action that leaves the optimistic state on screen (the user believes it succeeded —
OPT-1 violation); a GDPR-erase or agent-merge fired optimistically with only an undo (OPT-2 carve-out
violation — these Confirm, never optimistic). **Tag:** PROVEN.

### 1.10 CONFLICT SURFACING — CAS→CRDT shown legibly (PROVEN-routed — never a silent overwrite)

**Rule.** Two near-simultaneous edits to the same field/block **surface the conflict legibly — never a
silent overwrite** (PROVEN-as-rule — §9: "shown legibly, not a silent overwrite"; R-10 §2.2/§3.3; the
collab-concurrency UX is `[OPEN → P4]`, design-language §9 / TE-15). The two paths, by data kind:
- **Structured fields (issue status/assignee/priority) — CAS (compare-and-set).** A losing write surfaces:
  *"This changed while you were editing — {their value} vs {yours}. Keep yours / take theirs."* The user's
  edit is **never dropped silently**; they choose. (HOUSE STYLE render over the CAS mechanic.)
- **Rich text / blocks (doc, long description) — CRDT/OT merge.** Concurrent edits merge automatically where
  the model can; where they genuinely collide, presence + the merge are shown (who is editing — R-10 §3.3),
  not a clobber. **Never lose an in-progress edit to a background live update** (OPT-3, §1.9).

**R-21 ↔ owners:** the *mechanism* is CAS/CRDT (architecture); R-21 owns the *legible surfacing render*; the
deep collab-concurrency model is `[OPEN → P4]`. **Trap.** Last-write-wins silent overwrite (the data-loss
trap §9 calls out); a merge that drops a cell the user was editing; a conflict modal so heavy users avoid
collaborating. **Tag:** PROVEN-as-rule; render HOUSE STYLE; deep model `[OPEN → P4]`.

### 1.11 MOVED / OUTDATED — the reference followed the content (PROVEN — resolver, R-09)

**Rule.** A reference whose target moved/changed **follows the content and marks it honestly** — never a
dead link, never a silent mis-anchor (PROVEN — R-09 §5.5/§5.6; resolver `moved`/`outdated` projections). A
chip/comment shows a *"moved"* pill (relocated, content found at shifted position via 3-way match) or an
*"outdated"* pill (surviving part shown + *"some content has changed"*). The **rebase-orphaned diff-line
chip** (the hardest case, R-09 §5.9) detaches to *"outdated — was on former line N"* and lifts to
file/conversation level — **never silently jumps to a wrong line**. **Trap.** A dead `#1421` link; a comment
that silently re-anchors to the wrong code after a rebase (the GitHub failure mode, R-09 §5.9). **Tag:**
PROVEN (resolver); R-09 owns the render, R-21 places it per surface.

### 1.12 CROSS-CELL — resolves to projection-or-tombstone (PROVEN — cell-local resolver, R-09)

**Rule.** A ref to an artifact homed in another residency cell resolves **cell-locally**: only the
already-permission-filtered projection **or a tombstone** crosses — never raw rows, never PII (PROVEN — R-09
§5.8; ADR-11 no-cross-region-PII). Render: a **normal chip/card + a residency tag** (*"lives in `eu-west`"*,
the always-on P9 cue) *if visible*, **else the §1.4 no-access render** — never a raw id, never the title.
**Trap.** Leaking a cross-cell title/id to a viewer who can't see it; fanning PII across regions for a
preview. **Tag:** PROVEN; R-09 owns the render.

### 1.13 STORM / 30×-AGENT-SURGE — the inbox under load (PROVEN — shed-budget; R-21 owns the *experience*)

**Rule.** Under a 30×-agent-surge the **agent lane sheds first** (`429 + Retry-After`), the **human-direct
inbox stays in budget**, and agent volume shows **collapsed/threaded** while human-direct items stay
**unburied** (PROVEN — notifications.md §5.2 / D-N5; R-10 §4.3 placed it as the inbox's defining stress
case; R-13 B.4 as the calm-under-agents test). **R-21 owns the *experience render*:** the inbox under storm
shows (a) human-direct items at top, untouched; (b) a **single coalesced agent-activity group** (*"42 agent
updates — expand"*) rather than 42 rows; (c) a calm, *non-alarming* surge indicator (no red firehose). The
calm-tech "periphery" principle applied to agents (R-13 B.4): the surge lives in the periphery, never buries
a human.

**The render is the proof of the agent-native calm claim** — an agent-native product that floods the human
inbox under load has *failed* the one thing incumbents can't do (R-13 B.4). **Trap.** Agent volume burying
a human-direct item; a blank/blocked human inbox-read because agents queued ahead (D-N5 violation); an
alarming red surge banner. **Tag:** PROVEN (shed-budget); experience render HOUSE STYLE.

### 1.14 NO-RESULTS / NO-MATCH — the search/palette/filter empty (HOUSE STYLE — distinct from §1.1)

**Rule.** A *query* that returns nothing is **not** a zero-data empty (§1.1): it is *"No results for
'{query}'"* + a path (broaden / clear filters / create-from-query where apt — R-08), and it is
**permission-honest** — *"no results"* may mean "none you can see" but **never reveals** that hidden matches
exist (the no-leak rule, §1.4; R-08). Applies to command palette, global search, in-view filters, code
search, audit-log explorer. **Trap.** "No results" that implies the dataset is empty when it's only the
query; a no-results that leaks count of hidden matches. **Tag:** HOUSE STYLE; permission-honesty PROVEN
(R-08/ADR-03).

---

## 2. The per-surface state matrix (the Phase-6 finalist checklist)

> **How to read.** Rows = the primary §7 surfaces grouped by subsystem + the shared components (R-10) +
> the CLI peer surface (§7.7). Columns = the 14 states (§1). Cell legend:
> **●** = applicable & **required** to demonstrate · **◐** = applicable, context-dependent · **○** = N/A ·
> **→Rxx** = the render is owned/specced by that prior item (this matrix *places* it).
> A Phase-6 finalist must depict the **●** states for **at least one surface** per the rubric (§4); the
> matrix tells finalist + judge *which* states each surface owes, so a finalist can't quietly skip the hard
> ones. **The eight skipped states (cols 7–14) are the ones the rubric specifically requires "not just the
> six common ones" (§9).**

**Column key:** 1 Empty · 2 Loading · 3 Error · 4 Perm-denied · 5 Erased · 6 Agent-pending · 7 Degraded ·
8 Stale/reconnect · 9 Optimistic-rollback · 10 Conflict · 11 Moved/outdated · 12 Cross-cell · 13 Storm ·
14 No-results.

### 2a. Shared components (R-10 / R-09 — the substrate; every surface inherits these)

| Component | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **Reference chip / unfurl** (R-09) | ○ | ● | ◐ | ●→R09 | ●→R09 | ●→R09 | ●→R09 | ●→R09 | ◐ | ○ | ●→R09 | ●→R09 | ○ | ○ |
| **Views component** (R-10 §2) | ● | ● | ● | ●(rows absent) | ●(cell tombstone) | ◐ | ● | ● | ●(drag/edit) | ●(CAS cell) | ◐ | ◐ | ○ | ●(filtered) |
| **Block editor** (R-10 §3) | ● | ● | ●(save, buffer kept) | ●(read-only/none) | ●(ref node) | ◐ | ● | ●(collab) | ●(save) | ●(CRDT block) | ●(ref node) | ◐ | ○ | ○ |
| **Notifications inbox** (R-10 §4) | ●(zero) | ● | ●(fails static) | ●(subject tombstone) | ●("[erased user]") | ● | ● | ●(firehose) | ◐ | ○ | ◐(subject) | ◐ | **●(owns)** | ◐ |
| **Overlays** (R-10 §5) | ○ | ●(async body) | ●(in-dialog, input kept) | ◐ | ◐ | ◐ | ◐ | ○ | ◐ | ○ | ○ | ○ | ○ | ○ |
| **Command palette / search** (R-08) | ◐ | ● | ● | ●(absent, no-leak) | ●(tombstone result) | ◐ | ● | ◐ | ○ | ○ | ◐ | ◐ | ○ | ●(no-match) |

### 2b. Git hosting & code review (§7.1)

| Surface | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **Repo home** | ●(no repos / empty repo) | ● | ● | ●(no-access card) | ○ | ◐ | ● | ◐ | ◐ | ○ | ○ | ◐ | ○ | ○ |
| **File tree / file view** | ●(empty dir) | ● | ● | ● | ●(erased file ref) | ○ | ● | ◐ | ○ | ○ | ◐(blame moved) | ◐ | ○ | ○ |
| **PR overview / context pane** (wedge, §8.1) | ◐ | ●(per-section skeleton, R-13 CA-2) | ● | ●(linked artifact no-access, R-09) | ●(linked erased) | ●(agent reviewer) | ● | ◐ | ●(approve/merge) | ◐ | ●(linked moved) | ●(cross-cell link) | ○ | ○ |
| **Diff / files-changed** | ◐ | ● | ● | ◐ | ◐ | ●(agent review) | ● | ◐ | ●(comment post) | ●(line comment relocate) | ●(rebase-orphan, R-09 §5.9) | ○ | ○ | ○ |
| **Review surface** | ◐ | ● | ● | ◐ | ◐ | ●(agent verdict) | ● | ◐ | ●(verdict optimistic) | ○ | ◐ | ○ | ○ | ○ |
| **Checks / CI panel** | ◐(no checks) | ● | ●(check infra) | ◐ | ○ | ◐ | ●(CI down → static) | ●(live status) | ●(re-run) | ○ | ○ | ○ | ○ | ○ |
| **Branch-protection / ruleset editor** | ●(no rules) | ● | ●(save) | ●(admin-only) | ○ | ○ | ● | ○ | ●(save) | ●(CAS rule) | ○ | ○ | ○ | ○ |

### 2c. CI/CD (§7.2)

| Surface | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **Run list / dashboard** | ●(no runs) | ● | ● | ●(scoped) | ○ | ◐ | ● | ●(live) | ◐ | ○ | ○ | ◐ | ◐ | ●(filtered) |
| **Single-run view (DAG)** | ◐ | ●(DAG skeleton) | ● | ◐ | ○ | ◐(agent triage) | ● | ◐ | ●(re-run) | ○ | ○ | ○ | ○ | ○ |
| **Live log view** | ◐(no output) | ●(tail) | ●(log infra) | ◐ | ●(secret-masked) | ○ | ●(static) | **●(stream drop/resume)** | ○ | ○ | ○ | ○ | ○ | ●(search-in-log) |
| **Environments / approvals queue** (HITL) | ●(none) | ● | ● | ●(approver-only) | ○ | **●(gate-awaiting)** | ● | ◐ | ●(approve) | ◐ | ○ | ○ | ○ | ○ |
| **Agent-surfaced triage view** | ◐ | ● | ●(+agent-error) | ◐ | ○ | **●(working/proposal)** | ● | ◐ | ◐ | ○ | ○ | ○ | ◐ | ○ |

### 2d. Issue tracker (§7.3)

| Surface | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **Issue detail** | ◐(new) | ● | ●(save, buffer kept) | ●(no-access card) | ●(linked erased) | ●(agent-suggested) | ● | ◐ | ●(field/transition) | ●(CAS field) | ●(linked moved) | ◐ | ○ | ○ |
| **List/board/table/timeline/calendar** (views, R-10 §2) | ●(empty cycle, CTA) | ●(per-projection skeleton) | ● | ●(rows absent) | ●(cell tombstone) | ◐ | ● | ●(live row update) | ●(drag/inline-edit) | ●(CAS cell) | ◐ | ◐ | ○ | ●(filtered) |
| **Triage inbox** | ●(zero) | ● | ● | ◐ | ◐ | ●(agent dedup) | ● | ◐ | ●(triage action) | ○ | ○ | ◐ | ◐ | ●(filtered) |
| **Roadmap / portfolio (exec lens)** | ●(no initiatives) | ●(timeline skeleton) | ● | ●(scoped rollup) | ◐ | ◐ | ● | ◐ | ●(drag bar) | ◐ | ○ | ◐ | ○ | ●(filtered) |
| **"My Work" hub** | ●(all caught up) | ● | ● | ◐ | ◐ | ◐ | ● | ◐ | ◐ | ○ | ○ | ◐ | ◐ | ●(filtered) |
| **Dashboards** | ●(no widgets) | ●(widget skeletons) | ●(per-widget static) | ◐ | ○ | ○ | ●(per-widget) | ◐ | ○ | ○ | ○ | ◐ | ○ | ○ |
| **Workflow / SLA / field admin** | ●(default scheme) | ● | ●(save) | ●(admin-only) | ○ | ○ | ● | ○ | ●(save) | ●(CAS) | ○ | ○ | ○ | ○ |

### 2e. Knowledge (§7.4)

| Surface | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **Block editor / page** (R-10 §3) | ●(slash-hint) | ●(block skeleton) | ●(save, buffer kept) | ●(view-only / no-access) | ●(ref node) | ◐(agent draft) | ● | **●(collab reconnect)** | ●(save) | **●(CRDT collab)** | ●(ref node) | ◐ | ○ | ○ |
| **Database views** (views, R-10 §2) | ●(empty db, CTA) | ● | ● | ●(rows absent) | ●(cell tombstone) | ◐ | ● | ●(live) | ●(drag/edit) | ●(CAS cell) | ◐ | ◐ | ○ | ●(filtered) |
| **Backlinks / references panel** | ●(no backlinks) | ● | ◐ | ●(no-access, R-09) | ●(tombstone chip) | ○ | ● | ◐ | ○ | ○ | ●(moved ref) | ●(cross-cell) | ○ | ○ |
| **Page history / restore** | ◐(one version) | ● | ●(restore) | ◐ | ●(erased version) | ○ | ● | ○ | ●(restore) | ○ | ○ | ○ | ○ | ○ |
| **Sharing & permissions UI** | ●(default ACL) | ● | ●(save) | ●(admin-only) | ◐(erased grantee) | ○ | ● | ○ | ●(save) | ●(CAS) | ○ | ○ | ○ | ○ |

### 2f. Chat (§7.5)

| Surface | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **Channel list (sidebar)** | ●(no channels) | ● | ● | ●(absent, no-leak) | ◐ | ◐ | ● | ●(presence) | ◐ | ○ | ○ | ◐ | ◐ | ●(filtered) |
| **Message timeline** | ●(empty channel) | ●(message skeleton) | ●(fails static) | ◐ | ●("[erased user]"/msg) | ◐(agent msg) | ● | **●(reconnect, lossless)** | ●(send optimistic) | ○ | ◐ | ◐ | ◐ | ○ |
| **Composer** | ●(slash/@ hint) | ◐ | ●(send fail, draft kept) | ◐ | ◐ | ◐ | ● | ●(offline buffer) | ●(send) | ○ | ○ | ○ | ○ | ○ |
| **Thread pane** (agent/incident home) | ◐ | ● | ● | ◐ | ◐ | **●(agent detail/HITL)** | ● | ●(reconnect) | ◐ | ○ | ◐ | ◐ | **●(agent surge)** | ○ |
| **Unfurl cards** (R-09, densest) | ○ | ●(card chrome+body skel) | ◐ | ●→R09 | ●→R09 | ●→R09 | ●→R09 | ●→R09(stale dot) | ◐ | ○ | ●→R09 | ●→R09 | ○ | ○ |
| **HITL approval-card surface** (primary home) | ○ | ● | ●(+agent-error/budget) | ◐ | ◐ | **●(gate-awaiting, R-14)** | ● | ◐ | ◐(approve→Confirm) | ○ | ○ | ○ | ◐ | ○ |

### 2g. Shared / identity / admin / GDPR (§7.6) + CLI (§7.7)

| Surface | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **Unified inbox** (R-10 §4) | ●(all caught up) | ● | ●(fails static) | ●(subject tombstone) | ●("[erased user]") | ● | ● | **●(firehose resume)** | ◐ | ○ | ◐ | ◐ | **●(owns storm)** | ◐ |
| **Global search** (R-08) | ◐ | ● | ● | ●(absent, no-leak) | ●(tombstone result) | ○ | ● | ◐ | ○ | ○ | ◐ | ◐ | ○ | ●(no-match) |
| **Permission / role mgmt (RBAC)** | ●(no roles) | ● | ●(save) | ●(admin-only) | ◐(erased principal) | ○ | ● | ○ | ●(save) | ●(CAS) | ○ | ○ | ○ | ●(filtered) |
| **Agent governance console + kill-switch** (R-15) | ●(no agents) | ● | ●(+kill-switch effect) | ●(admin-only) | ◐ | **●(working/halted/budget)** | ● | ◐ | ●(policy save) | ●(CAS policy) | ○ | ○ | **●(surge view)** | ●(filtered) |
| **Audit-log explorer** (R-15/R-19) | ●(no events) | ● | ● | ●(scoped) | ◐(erased subject ref) | ◐ | ● | ◐ | ○ | ○ | ◐ | ◐ | ◐ | ●(no-match) |
| **GDPR / DSR console** (R-19) | ●(no requests) | ● | ●(+per-holder fail) | ●(DPO-only) | **●(erasure outcome)** | ◐(DSR agent) | ●(per-holder static) | ◐ | ●(action) | ◐ | ○ | ●(cross-holder cell) | ○ | ●(filtered) |
| **Data-map / RoPA & residency console** (R-19) | ●(no data yet) | ● | ● | ●(scoped) | ○ | ○ | ● | ◐ | ◐ | ○ | ○ | ●(residency view) | ○ | ●(filtered) |
| **Onboarding / empty-platform** (R-20) | **●(zero-data shell)** | ● | ● | ◐ | ○ | ◐(first agent run) | ● | ◐ | ◐ | ○ | ○ | ○ | ○ | ○ |
| **CLI (peer surface, §7.7)** | ●(no-data msg) | ●(progress, not spinner-spam) | ●(system-blamed, exit code + path) | ●(no-access, no-leak) | ●(tombstone text) | ●(agent-pending text) | ●(degraded text) | ◐ | ◐ | ◐(conflict text) | ●(moved text) | ●(residency note) | ◐ | ●(no-match) |

> **CLI note (§9 device-gloss):** the CLI is "in scope for consistency" — its error states and reference
> rendering must follow the **same vocabulary** (system-blamed errors with a path + `correlation_id`;
> humanised refs not raw ids; no-leak on permission; tombstone text for erased; *progress* not
> spinner-spam). Easy to forget; named here so a finalist that ships a CLI screen owes these.

### 2h. The matrix's load-bearing reads (for a Phase-6 finalist + a Phase-7 judge)

- **No surface escapes loading + error + permission-denied + degraded** (cols 2/3/4/7 are ● or ◐ almost
  everywhere) — these are the *floor*; a surface missing any is unfinished (the switch test, §8b.7).
- **The "owns" cells** mark where a state has a *defining* home a finalist should demonstrate there: the
  **inbox owns storm** (col 13); the **diff owns rebase-orphan** (col 11); the **collab editor owns conflict
  + reconnect** (cols 10/8); the **live log + chat timeline own stream-drop/resume** (col 8); the
  **approvals queue / HITL card / triage view own agent-pending** (col 6); the **DSR console owns the erasure
  outcome** (col 5); the **chip/unfurl owns no-access/tombstone/moved/cross-cell** (cols 4/5/11/12).
- **Cols 7–14 are the §9 "skipped eight"** — the matrix forces them into view per surface so the
  happy-path bias can't drop them (the prompt's acceptance criterion: "not just the six common ones").

---

## 3. Completeness-critic (README §9) — this item OWNS the list

R-21 is the named owner of the §9 "Unglamorous UI states (route to R-21, enforce in every Phase-6
finalist)" block. **Owned-and-covered here:**

| §9 named state | Covered | Where |
|---|---|---|
| Loading shows structure (skeleton, never blank spinner) | ✅ owned | §1.2 + matrix col 2 (per-surface) |
| Empty states onboarding-forward | ✅ owned | §1.1 (three kinds) + col 1 |
| Error blames system in one quiet line + path | ✅ owned | §1.3 (grammar) + col 3 |
| Permission-denied graceful no-access, never a leak | ✅ owned | §1.4 (Restricted vs Absent) + col 4 |
| Erased / tombstoned (GDPR-aware degraded) | ✅ owned | §1.5 (three flavours, every consuming surface) + col 5 |
| Agent-pending (working / awaiting-approval) | ✅ owned | §1.6 (+partial-failure family) + col 6 |
| Degraded-surface "temporarily unavailable" (fails static) | ✅ owned | §1.7 + col 7 |
| Stale / offline / reconnecting (firehose drop+resume) | ✅ owned | §1.8 (three cues) + col 8 |
| Optimistic-update rollback (honest failure) | ✅ owned | §1.9 (render; R-13 owns contract) + col 9 |
| Conflict surfacing (CAS→CRDT, legible not silent) | ✅ owned | §1.10 (two paths) + col 10 |

**Also covered (§9 edge-case/cross-surface + device blocks routed partly here):**
- **Partial-failure agent branches** (gate-rejected / agent-error / budget / loop-guard) — §1.6 (placed;
  R-14 owns depth) + matrix cols 3/6.
- **Cross-cell ref → no-access/tombstone** — §1.12 + col 12 (R-09 owns render).
- **Diff-anchored comment relocate/orphan after rebase** — §1.11 + col 11 (R-09 §5.9 owns render).
- **Storm / 30×-agent-surge inbox experience** — §1.13 (R-21 owns the *experience render*) + col 13.
- **Touch / mobile glosses** (hover-only actions invisible; clipped panels; off-screen popovers) — the
  overlay/views states inherit R-10 §5.3 (flip + real-anchor-test) + §8b.4; **not re-specced** (R-10 owns).
- **CLI as a peer surface** — §2g row + the CLI note (its error/reference states follow the same vocabulary).

**Consciously deferred (with reason):**
- **The full collab-concurrency UX** (beyond legible conflict surfacing) is `[OPEN → P4]` (design-language
  §9 / TE-15); R-21 specs the *render*, flags the scope (§1.10).
- **The full offline-editing model** is `[OPEN → P4]` (§1.8); R-21 specs the *offline render*, flags scope.
- **Accessibility of every state** (focus on the error/empty/skeleton; live-region announcement of state
  *changes* without spamming) — routed to **R-17** (the a11y audit owns the per-state a11y checklist); R-21
  notes the obligation (§4) but does not duplicate R-17's measured checklist.
- **The HITL card depth + agent-volume routing** — R-14/R-15 own; R-21 places agent-pending per surface.

---

## 4. Actionability toward the control artifacts

| Control artifact | What this file equips | Where |
|---|---|---|
| **rubric D8** (perceived performance: skeletons / optimistic designed) | The loading-skeleton placement (§1.2 + matrix col 2) and the optimistic-rollback render (§1.9 + col 9) make D8's "loading shows structure / optimistic with honest rollback / pages render not animate in" *checkable per surface*, not aspirational. | §1.2, §1.9, §2 |
| **The per-finalist "unglamorous-states requirement"** (rubric §"every finalist must depict the unglamorous states named in README §9 for ≥1 surface") | **§2 is that checklist made concrete.** A finalist picks a surface and the matrix tells it *exactly which states it owes* (the ● cells), so it can't depict only empty+loading and skip permission/erased/degraded/conflict. The "owns" cells (§2h) tell it which surface best demonstrates each hard state. | §2, §2h |
| **The switch test (D10)** | A surface is done only when it survives being *driven* — and driving hits the unglamorous states (a failed save, a denied ref, a dropped connection). §1's "never blank/blame/leak/lie" rule is the switch-test pass condition for states. | §0, §1 |
| **sketch-funnel 6c states** | 6c requires each finalist deepen into a mini-system *with* "empty / loading-skeleton / error / permission / erased / agent-pending … for ≥1 surface". §2 is the exact 6c state checklist, extended with the eight skipped states the rubric also requires. | §2 |
| **sketch-funnel Axis 5 (agent presence) / Axis 6 (sovereignty)** | Agent-pending + storm (§1.6/§1.13) are Axis-5 state evidence; erased-tombstone + cross-cell residency tag (§1.5/§1.12) are Axis-6 cues at the state level. | §1.5/§1.6/§1.12/§1.13 |
| **R-17 (a11y) / R-14·R-15 (agent) / R-19 (sovereignty)** | R-17 audits each state's a11y (focus/live-region); R-14/R-15 own agent-pending depth; R-19 owns the erasure flow — R-21 hands them the per-surface placement. | §3 |

---

## 5. Sources (web-verified 2024–2026 + surfaced contracts)

**Empty states (PROVEN doctrine + cited best-practice):**
- Empty-state best practices — next-step front-and-center, ~2:1 instruction-to-delight, first-use vs cleared
  vs filtered: https://www.pencilandpaper.io/articles/empty-states ·
  https://www.useronboard.com/onboarding-ux-patterns/empty-states/ ·
  https://blog.logrocket.com/ux-design/empty-states-ux-examples/

**Error messages (PROVEN — NN/g):**
- NN/g Error-Message Guidelines (blame the system not the user; human-readable, polite, constructive):
  https://www.nngroup.com/articles/error-message-guidelines/
- NN/g 10 Guidelines for Reporting Errors in Forms (actionable, next to the problem, edit-don't-restart):
  https://www.nngroup.com/articles/errors-forms-design-guidelines/

**Permission-denied no-leak (PROVEN — 403-vs-404 security distinction):**
- Choosing 401/403/404 — return 404 (Absent) when existence itself is sensitive; 403 (Restricted) when the
  user may know it exists: https://authress.io/knowledge-base/articles/choosing-the-right-http-error-code-401-403-404 ·
  https://www.insights.cgi.com/blog/when-should-you-return-404-instead-of-403-http-status-code

**Stale / offline / reconnecting (PROVEN — cited offline-UX patterns):**
- Offline-first UI 2025 (keep cached content visible, mark "offline/reconnecting", warn on very-stale,
  "couldn't refresh — showing cached" honest-not-dramatic): https://blog.logrocket.com/offline-first-frontend-apps-2025-indexeddb-sqlite/ ·
  stale-while-revalidate (responsive-with-slightly-stale > slow-but-consistent): https://www.educative.io/courses/learn-react/np/background-sync-and-the-stale-while-revalidate-pattern

**Skeletons + optimistic (PROVEN — inherited from R-13 §5, not re-listed):** RAIL/Nielsen budgets, skeleton
~20–30% faster, optimistic ~40% perceived-wait reduction — see [R-13 §5](../visual/perceived-performance.md).

**Surfaced Myelin contracts (PROVEN-as-existing, not invented):**
- design-language §5.10 (cross-cutting state patterns), §8b.6 (loading-structure / error-blames-system /
  fails-static / latency budgets), §8b.7 (switch test), §8b.3 (status-not-colour / no-sparkle), §6 (agent
  contract), §9 (offline/collab `[OPEN → P4]`); ADR-03 (permission-pre-filter), ADR-11 (residency),
  ADR-12 (GDPR/erasure).
- R-09 §1.1/§5 (resolver no-leak ladder; chip/unfurl state renders); R-10 §2.2/§3.3/§4.3/§5.3 (per-component
  state sets) + §4.1/§4.3 (one inbox, storm shed-budget); R-13 §A.2/§A.3 (skeleton craft, optimistic
  contract) + §3 (reconnecting/degraded/storm routing); notifications.md §5.2/D-N5 (storm), §7/D-N11
  (firehose resume), §5.3 (fails-static); EXT-1 (prefetch, R-13 §A.4).

**Honest limitation:** the empty/error/offline best-practice sources are practitioner/NN-g syntheses
(PROVEN-as-reported for *direction*); the *Myelin-specific* renders are HOUSE STYLE over the PROVEN
resolver/notifications/§8b.6 contracts; *comprehension/trust* of each degraded state is the §6 hypothesis.

---

## 6. `[DEFERRED-UNTIL-USERS]` — what this catalogue has NOT earned

R-21 is `user-dep: none` — the deliverable IS the no-user substitute (the expert state-craft catalogue +
per-surface matrix, grounded in the cited best-practice + the PROVEN resolver/notifications/§8b.6
contracts + the prior R-09/R-10/R-13 specs). The following are **HYPOTHESES** falsifiable once users exist;
recorded as executable plans, **not faked as validated**:

- **`[DEFERRED-UNTIL-USERS]` — Does each degraded state read as *intended* vs *broken*?** The biggest:
  **erased-tombstone** (§1.5 — does *"erased under a data-rights request"* read as lawful/dignified or as
  data-loss/error?, with a DPO P13 + a regular user) and **permission-denied** (§1.4 — does *"Restricted"*
  read as "you lack access" or "this is broken"?, R-09 §11 inherits this). *Falsifier:* users read either as
  a bug and try to "fix"/report it.
- **`[DEFERRED-UNTIL-USERS]` — Is the storm render (§1.13) felt as *calm* under real surge?** *Test:* drive
  a 30×-agent-surge inbox in front of engineers + PMs; can they still find/act on a human-direct item?
  *Falsifier:* the coalesced agent group is read as alarming, or a human item *feels* buried even though it's
  technically at top. (The shed-budget is PROVEN; the *felt-calm* is the hypothesis — R-13 B.4 / §6.)
- **`[DEFERRED-UNTIL-USERS]` — Is the conflict surfacing (§1.10) *trusted* over silent merge?** *Test:* two
  users edit the same issue field / doc block; do they trust the "keep yours / take theirs" choice, or find
  it disruptive? *Falsifier:* users prefer last-write-wins convenience to the honest-conflict prompt (would
  challenge the "never silent overwrite" render, though not the no-data-loss rule). Deep model is `[OPEN → P4]`.
- **`[DEFERRED-UNTIL-USERS]` — Do the empty states *teach the next step*?** *Test:* a first-use cognitive
  walkthrough (no users → R-20's substitute now; users later) — does a new P1/P6 know what to do from the
  empty render alone? *Falsifier:* the CTA is missed or misread (R-20 owns the cross-surface journey test).
- **Method:** per-segment RITE on the Phase-6 finalist(s) that ship these states, on the F-ENG-1 /
  F-PM-1 / F-GOV-1 flows (R-04), driving each unglamorous state deliberately (the switch test as the
  no-user substitute now; user-RITE later). **Caveat (VISION §3):** the *correctness* invariants
  (no-leak, lossless-resume, fails-static, never-silent-overwrite, honest-revert) are **PROVEN** (resolver +
  notifications + §8b.6 contracts + drills); the *comprehension/trust* of each render is **HYPOTHESIS**.

---

## 7. Self-check against R-21 acceptance criteria

| Criterion (prompt R-21 / §9) | Status | Evidence |
|---|---|---|
| **Every shared component + primary surface has its full state set** | ✅ Met | §2 matrix: shared components (2a) + all §7 surfaces (2b–2g) + CLI (2g) × 14 states |
| **The six common states made concrete designed patterns (not afterthoughts)** | ✅ Met | §1.1–§1.6 — each a rule + §8b.6 specific + render + trap + tag (empty onboarding-forward; loading structure-skeleton; error blames-system-one-line+path; perm graceful no-leak; erased GDPR-degraded; agent working/awaiting-approval) |
| **The skipped states present (optimistic-rollback, conflict, reconnecting, degraded-static, storm) — not just six** | ✅ Met | §1.7 degraded-static, §1.8 stale/offline/reconnect, §1.9 optimistic-rollback, §1.10 conflict, §1.11 moved/outdated, §1.12 cross-cell, §1.13 storm, §1.14 no-results — matrix cols 7–14 |
| **§8b.6 specifics applied to each** | ✅ Met | loading-shows-structure (§1.2), error-blames-system-one-line (§1.3), fails-static (§1.7), pages-render-not-animate (via R-13/B4), suppress-flash-of-spinner <1s (§1.2) |
| **Per-surface state matrix Phase-6 finalists must satisfy** | ✅ Met | §2 (●/◐/○/→Rxx legend; the per-finalist checklist; §2h the load-bearing reads + "owns" cells) |
| **Usable as the Phase-6 state checklist; covers README §9 list explicitly** | ✅ Met | §3 (the §9 table — all 10 owned; partial-failure/cross-cell/rebase/storm/touch/CLI routed); §4 (the per-finalist requirement = §2) |
| **Builds ON R-09/R-10/R-13, doesn't duplicate** | ✅ Met | §0 + inline: inherits R-09 resolver renders, R-10 component state sets, R-13 skeleton+optimistic contract; places not re-derives (→Rxx in matrix) |
| **PROVEN/HOUSE-STYLE tags + date + cited URLs** | ✅ Met | tags per pattern; dated 2026-06-20; §5 URLs (empty/NN-g error/403-404/offline) + surfaced contracts |
| **Actionable toward rubric D8 + the per-finalist unglamorous-states requirement + sketch-funnel 6c** | ✅ Met | §4 mapping (D8 checkable per surface; §2 = the per-finalist checklist; 6c state set + the skipped eight) |
| **Deferred validation recorded as a plan, not faked** | ✅ Met | §6 (`[DEFERRED-UNTIL-USERS]`: erased/perm comprehension, storm felt-calm, conflict trust, empty teaches — each with falsifier + method; correctness PROVEN vs comprehension HYPOTHESIS) |
| **Self-check restating acceptance criteria** | ✅ Met | this table |

**Top uncertainties (honest, per VISION §3):**
1. **Comprehension/trust of degraded states is HYPOTHESIS** (§6) — the no-leak/lossless/honest-revert
   *correctness* is PROVEN (contracts + drills), but whether users *read* erased-tombstone / permission-
   denied / conflict as intended-vs-broken is untested. The **erased state** is the highest-stakes (a P9
   sovereignty moment that must read as lawful, not as data-loss).
2. **The ●/◐ assignments in §2 are HOUSE-STYLE curation** — which states a given surface *must* vs *may*
   demonstrate is a considered call, not a measured one; a Phase-6 finalist or human reviewer may
   reasonably re-weight a ◐→● for their chosen surface.
3. **The conflict (§1.10) and offline (§1.8) renders sit over `[OPEN → P4]` models** — the *render* is
   specified, but the underlying collab-concurrency + offline-editing scope is unsettled (design-language
   §9 / TE-15); the render may need revision once that model lands.
4. **Storm felt-calm (§1.13) depends on the shed-budget holding under real agent load** — PROVEN by
   drill D-N5 at the architecture layer; the *UI experience* of it is the §6 hypothesis.

---

*End of R-21 deliverable. Date: 2026-06-20. State-craft catalogue + per-surface matrix HOUSE STYLE over the
PROVEN §8b.6 / §5.10 mandates, the R-09 resolver no-leak ladder, the R-10 component state sets, the R-13
skeleton+optimistic contract, the notifications storm/firehose contracts, and cited empty/error/403-404/
offline best-practice; not user-validated — see §6. Owns the README §9 unglamorous-states list. Feeds rubric
D8 + the switch test (D10), the per-finalist unglamorous-states requirement, and sketch-funnel 6c + Phase 6.*
