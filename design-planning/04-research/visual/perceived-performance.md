# R-13 — Perceived-Performance & Density-Made-Calm Patterns

> **Phase 4 research corpus** · WS-E (visual & motion direction) · Seq #14. Deliverable for prompt
> **R-13** in [`03-research-prompts.md`](../../03-research-prompts.md). **File date: 2026-06-20.**
> Methods: **#19 (visibility of system status)**, **#24 (switch test — the "latency / feels-finished"
> bar)**, **§8b.6 hard latency budgets** (keyboard <~100ms; suppress flash-of-spinner <~1s; "pages
> render, they don't animate in").
>
> **Two halves, as the prompt mandates:** **§A — Perceived performance** (skeletons that show
> structure, optimistic-update + honest-rollback, the prefetch/context-assembly UX linked to its
> extension, latency budgets as checkable constraints) and **§B — Density-made-calm** (the concrete
> patterns that make dense surfaces calm: hierarchy from weight/colour before size, borders over
> shadow, agent-volume-out-of-the-timeline, one-prioritised-inbox, restraint-as-default).
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = a cited standard / measured study / an existing
> Myelin contract this file *surfaces* (the §8b.6 budgets, the R-10 component states, the R-12 motion
> tokens, the EXT-1 bundling extension, notifications.md). **HOUSE STYLE** = our synthesis/taste (the
> per-surface skeleton shapes, the calm-pattern rulings, the budget *table values* beyond the cited
> thresholds). **Not user-validated;** deferred bets in §6.
>
> **Builds ON prior `04-research` (does not duplicate — this file *dresses* their already-specced
> components and consumes R-12's motion):**
> - [R-10 shared-patterns](../interaction/shared-patterns.md) — the components these patterns dress:
>   the views state set (§2.2: loading-skeleton/optimistic-pending/live-update/conflict), the editor
>   states (§3.3), the inbox states + the storm/30×-surge (§4.3), the overlay async-loading state
>   (§5.3). R-13 supplies the *perceived-perf craft* R-10 routed here by name (R-10 §7).
> - [R-12 motion-microinteractions](motion-microinteractions.md) — the *motion half*: `optimistic-settle`
>   (R-12 §3.1), `motion.liveUpdate` / PR-going-green (§4), `skeleton→content swap` cross-fade (§3.8),
>   the L2 interruptible budget, L3 "pages render don't animate in". R-12 §9 lists R-13 as its
>   consumer; this file owns the *loading-state* dressing R-12 deferred here.
> - [`extension-planning/perceived-performance.md`](../../../extension-planning/perceived-performance.md)
>   (**EXT-1**) — the context-projection bundling/prefetch extension the §A.3 prefetch UX *requires*;
>   surfaced here as a UX dependency, not redesigned.
>
> **The one-sentence thesis (HOUSE STYLE over the §8b.6 / external-insights §4 PROVEN doctrine):**
> *Speed and calm are the same discipline seen from two sides — both protect the user's attention:
> perceived performance spends no wait the system can hide (skeleton-not-spinner, optimistic-not-
> blocking, prefetch-not-tab-switch), and density-made-calm spends no attention the content doesn't
> earn (weight-before-size, periphery-before-center, agent-volume-out-of-the-timeline) — and every
> claim is checkable against a hard budget or a falsifiable rule, never "be fast" or "be calm."*

---

## 0. How to read this file

- **§A — Perceived performance** (4 parts): A.1 the latency-budget table (the checkable constraints) ·
  A.2 per-surface skeletons (structure-matching) · A.3 optimistic-update + honest-rollback · A.4 the
  prefetch / context-assembly UX (linked to EXT-1) + the residency caveat.
- **§B — Density-made-calm** (the concrete pattern set, each PROVEN/HOUSE-STYLE-tagged + a checkable
  rule).
- **§3** completeness-critic (README §9 — the loading / optimistic-rollback / stale-reconnecting
  gloss-risks). **§4** rubric (D8/D7) + funnel actionability. **§5** sources. **§6**
  `[DEFERRED-UNTIL-USERS]`. **§7** self-check.

---

# §A — Perceived performance

> The doctrine (external-insights §4, PROVEN-as-our-contract): *"loading shows structure (skeletons
> that match the final layout), never a spinner on a blank page … optimistic updates, honest rollback …
> the system assembles context; the user never does … latency budgets are hard, not stylistic."*
> §A turns each clause into a **checkable** pattern.

## A.1 The latency-budget table — the checkable constraints (the heart of D8)

The budgets are **hard, not stylistic** (external-insights §4; §8b.6). The thresholds below are
**PROVEN** (cited perception research); the *per-interaction assignments* are HOUSE STYLE within them.
Every row is phrased so a Phase-7 reviewer (or a CI perf budget) can **check** it, not admire it.

| # | Interaction class | **Budget** | Basis (tag) | How it's checkable |
|---|---|---|---|---|
| **B1** | **Keyboard-driven action** (palette select, single-key triage, inline-edit commit, `j/k` move) | **<~100ms to visible response** | RAIL "Response" / Nielsen 0.1s "instant"; process input <50ms (PROVEN) | instrument input→next-paint per interaction; the bar is the RAIL/INP 100ms "feels instantaneous" line |
| **B2** | **Optimistic action with server round-trip** (transition issue, drag card, approve) | **response in <~100ms (optimistic paint); server-ack reconciled async** | optimistic-UI doctrine (external-insights §4); ~40% perceived-wait reduction (PROVEN, study) | the UI must paint the new state before the request resolves; ack runs `motion.settle` (R-12 §3.1) |
| **B3** | **Any wait the system can't optimistically hide** (view load, search, unfurl content) | **suppress flash-of-spinner <~1s; show a structure-skeleton, never a blank spinner** | §8b.6 verbatim; Nielsen 1s "flow"; skeletons ~20–30% faster perceived (PROVEN, studies) | <1s → no spinner at all (skeleton from frame 0); >1s → the skeleton *is* the wait UI |
| **B4** | **Page / view arrival** | **pages render, they don't animate in** (no stagger/slide/fade-in on first paint) | §8b.6 verbatim; R-12 L3 (PROVEN-as-our-rule) | first paint shows the laid-out page (or its skeleton); animation is reserved for change *after* a stable state |
| **B5** | **Background live update on a watched surface** (PR goes green, issue moves) | **noticed without interrupting; ≤~240ms `motion.liveUpdate`; in place, no scroll-jump / no re-sort under the eye** | §3.6 / R-12 §4 (PROVEN-as-our-rule) | the change paints where the element sits; selection/scroll preserved; reduced-motion = instant + static marker |
| **B6** | **Degraded surface** (a subsystem is down) | **fails *static*** — "temporarily unavailable" for *that* surface only; the shell + other surfaces stay live | §8b.6 verbatim; notifications.md §5.3 fails-static (PROVEN) | one surface erroring must not blank the shell; already-materialised content still renders |

> **The checkable form of "feels instant" (the switch test, #24/D10):** drive the real UI; every
> keyboard action must clear B1, every optimistic action B2, every unavoidable wait B3, and no surface
> may violate B4/B6. A surface that spinner-blanks under 1s, or animates its page in, or loses your
> place on a live update, **fails the bar regardless of actual server speed.** Perceived performance is
> scored on these, not on backend latency (D8).

## A.2 Per-surface skeleton patterns — structure that matches the final layout (PROVEN doctrine, HOUSE-STYLE shapes)

**The rule (PROVEN — §8b.6; external-insights §4; skeleton studies):** loading shows *structure* — a
skeleton whose geometry matches the final layout — **never a blank spinner.** Evidence: users perceive
skeleton-screen loads ~20–30% faster than identical spinner loads, and structure-matching skeletons
also prevent the content-arrival layout shift (PROVEN — Viget/skeleton studies, §5). **HOUSE STYLE: the
skeleton must be *honest about shape*** — ghost rows for a table, ghost cards for a board, a ghost
context-pane scaffold for a PR — so the wait *teaches the layout*; a generic shimmer block is only
marginally better than a spinner and is **ruled out** (it shows no structure → fails the doctrine's
"matches the final layout" clause).

Per-surface skeleton catalogue (each builds on the R-10 component's loading state, named there but not
shape-specced):

| Surface (R-10 owner) | Skeleton shape (HOUSE STYLE) | Honest-structure check |
|---|---|---|
| **Views — table/list** (R-10 §2.2) | ghost header + N ghost rows at the real row-height, real column count/widths; frozen first column present | row count ≈ viewport capacity; columns match the saved view |
| **Views — board** (R-10 §2.2) | ghost columns at real titles + ghost cards per column at card height | columns are the *real* status columns (titles known before rows) |
| **Views — timeline/calendar** (R-10 §2.2) | ghost lanes + ghost bars on the real time-axis | the axis (dates) is real, only the bars are ghosts |
| **Editor / knowledge page** (R-10 §3.3) | block-skeleton: ghost heading bar + ghost paragraph lines matching block structure | mirrors the doc's block outline, not a grey rectangle |
| **Notifications inbox** (R-10 §4.3) | ghost item-rows with provenance-line + triage-action placeholders | row shape = the real inbox-item-row molecule |
| **Reference unfurl / hovercard** (R-09; R-10 §5.3 async overlay) | the card chrome (type-icon + title slot + action row) renders instantly; the *body* skeletons | the chip→card spatial link is immediate; only remote body waits |
| **PR context-pane** (the wedge; §A.3) | the pane's *scaffold* (sections: diff / linked issue / CI run / discussion) renders as labelled skeleton slots; each fills as its bundle resolves | the structure of "what context exists" is shown before the content lands |

**The B3 + skeleton interaction (the flash-of-spinner suppression):** under ~1s, the skeleton may be
*all* the user sees before content (no spinner ever); over ~1s, the skeleton *is* the patient wait UI
(still no spinner). The `skeleton→content` swap is R-12 §3.8's `motion.settle` cross-fade (reduced-motion
= instant swap). **There is no spinner token in the system** — the only legitimate indeterminate
indicator is a quiet `linear` progress bar for a *determinate-where-possible* long operation (R-12 §2.2,
§7.2 anti-list: "spinners on a blank surface" ruled out).

## A.3 Optimistic updates + honest rollback (PROVEN doctrine, HOUSE-STYLE craft)

**The rule (PROVEN — external-insights §4: "optimism for latency, honesty on failure";
reversibility-over-confirmation):** a permitted action paints its result **immediately** (B2), the
server-ack reconciles asynchronously, and a **failure visibly and honestly reverts** — optimism never
hides a failure. Evidence: optimistic updates cut perceived wait by ~40% on a 200ms round-trip (PROVEN,
study, §5). This is the **same pattern R-10 placed** (views drag/inline-edit §2.2; editor save §3.3) and
**R-12 animates** (`optimistic-settle` §3.1); R-13 owns the *honesty contract*.

**The three-state optimistic contract (HOUSE STYLE — the checkable craft):**

| State | What the user sees | Motion (R-12) | The honesty rule |
|---|---|---|---|
| **Optimistic (pending)** | new state painted immediately + a **subtle** pending affordance (not a blocking spinner) | optimistic paint, no token (it's instant) | the action *feels* done; the affordance signals "not yet confirmed" without nagging |
| **Settled (ack)** | the pending affordance clears | `motion.settle` (140ms tick) | "it committed" — visually distinct from the rollback path (R-12 §3.1) |
| **Rolled back (reject/timeout)** | the element **reverts to its prior state** + **one quiet system-blaming line** + a path (retry / undo) | reverse `motion.move` (the element visibly un-does) | **the revert is visible and named** — never a silent swallow; the failure looks *different* from success (L1/R-12 §3.1) |

**Binding rules (HOUSE STYLE, falsifiable):**
- **OPT-1 — Optimism never hides failure.** The rollback is *more* visible than the settle, not less:
  the user must never believe an action succeeded that didn't. *Falsifier:* a failed action that leaves
  the optimistic state on screen with no revert.
- **OPT-2 — Reversibility over confirmation for the reversible set; Confirm for the consequential set.**
  Most actions → optimistic + **undo-toast** (R-10 §5.2). The carve-out is preserved verbatim:
  **irreversible / consequential + GDPR-erase + agent-HITL-approval actions still Confirm** (R-10 §5.2;
  §8b.6). *Falsifier:* a GDPR erase or agent-merge-approval fired optimistically with only an undo.
- **OPT-3 — Never clobber an in-flight edit.** A background live update (B5) must not overwrite an
  in-progress optimistic edit; the collision surfaces as the **conflict state** (CAS→CRDT, R-10
  §2.2/§3.3 → legible surfacing owned by R-21), never a silent loss. *Falsifier:* a live update dropping
  a cell the user was editing.
- **OPT-4 — Idempotent under retry.** The retry path (after rollback) must be safe to repeat — surfaced
  as a UX guarantee (the action can be re-fired without double-applying); this is the UX face of the
  idempotency best-practice (PROVEN, §5). *(Backend mechanic; surfaced, not designed here.)*

**Reduced-motion (R-12 §2.4):** optimistic paint is instant either way; settle = instant clear; rollback
= instant revert + the quiet line. The *information* (did it stick / did it fail) is identical without
animation.

## A.4 The prefetch / context-assembly UX — "the system assembles context; the user never does" (linked to EXT-1)

**The doctrine (PROVEN-as-our-contract — external-insights §4; §8b.6: "the system assembles context —
and pre-fetches it"):** wherever the reference graph links two things, the UI **shows the link and
pre-fetches the next hop**, so the user is never sent to another tab to assemble what the system already
knows is related. This is a core lovability + wedge promise (R-22 consumes it).

**This UX *requires* the EXT-1 extension** — surfaced, not redesigned:
[`extension-planning/perceived-performance.md`](../../../extension-planning/perceived-performance.md)
(**EXT-1 — client-facing context-projection prefetch / bundling**) provides exactly what these patterns
spend: a **permission-aware context-bundling projection mode** (an `ArtifactRef` + viewer → the artifact
**plus** its permission-filtered related projections in **one round-trip**) and a **client prefetch-hint
stream** to warm the next-hop projections ahead of navigation (EXT-1 §"What the extension is"). It is the
read-side complement to the reference graph: the graph *knows* the edges (R-09); EXT-1 *bundles* their
current per-viewer projections so the client makes **one** call, not N sequential ones. **The no-leak
invariant is load-bearing** (EXT-1 §risk): a pre-fetched projection the viewer can't see must **never**
be bundled — prefetch inherits the same permission-pre-filter / graceful-no-access behaviour as the chip
(R-09; ADR-03).

**The named context-assembly UX patterns (HOUSE STYLE over the PROVEN doctrine + EXT-1 mechanics):**

| Pattern | The moment | What prefetch buys (felt result) | Mechanic (EXT-1 / R-09) | Skeleton fallback (A.2) |
|---|---|---|---|---|
| **CA-1 — Failing-check → step → line** | a CI check fails; the engineer drills in | the failing **step** and the **line of code** are *already there* — no sequential drill | bundled projection of the run→job→step→diff-anchor chain (EXT-1) | each hop skeletons if a bundle is slow; never a blank drill |
| **CA-2 — PR context-pane assembly** | opening a PR | the linked issue + CI run + doc + discussion arrive **already projected** in the pane | one bundled context call for the PR's refs (EXT-1; the wedge flagship, system-overview §8.1) | the pane scaffold (A.2) renders instantly; slots fill as bundles resolve |
| **CA-3 — Notification "why + next hop"** | a notification arrives | it carries **"why it fired"** (`origin_event`+`reason`, R-10 §4.1) **and** the pre-fetched next hop, so acting is one step | inbox provenance (PROVEN, notifications.md) + EXT-1 prefetch-hint warms the target | the target opens from warm cache; cold = skeleton, never a tab-switch-to-assemble |
| **CA-4 — Hover-peek pre-warm** | hover over a chip (R-09 §2.2, ~300ms intent delay) | the unfurl card body is **already warm** when the card opens | the ~300ms intent delay *is* the prefetch window — warm the projection during it (EXT-1 hint) | if not warm in time, card chrome instant + body skeleton (A.2) |

**The residency / no-global-CDN caveat (PROVEN — P2 / ADR-11):** perceived speed is **not** bought with
global CDN replication of personal data — Myelin is EU-sovereign, so personal data does **not** fan out
to a global edge. Perceived speed is bought instead via **(a) optimistic UI** (A.3, hides round-trips
client-side), **(b) in-region edge / caching** (within the residency boundary), and **(c) prefetch /
context-bundling** (EXT-1, fewer round-trips, warmed locally), **never global replication.** This is a
*constraint that shapes the perceived-perf strategy*, not a limitation to apologise for: the patterns
above are precisely the residency-compatible way to feel instant. *(Honest flag: that these three buy
"enough" perceived speed within one EU region for the worst-case cross-region collaborator is a
HYPOTHESIS — §6.)*

---

# §B — Density-made-calm

> The philosophy (PROVEN-as-our-doctrine — external-insights §4; design-language P5 earned-density / P8
> calm; rubric D7): *"dense, calm, utilitarian — it shows a lot without shouting."* Grounded externally
> in **Calm Technology** (Case / Xerox PARC: *"technology should require the smallest possible amount of
> attention"*; *"move easily from the periphery to the center and back"* — PROVEN, §5). §B turns "be
> calm" into **concrete, checkable patterns** (the D7 anchor: *0 = noisy traffic-light screen; 4 = dense
> yet calm, the eye knows where to go, quiet is the default*).

**The thesis (HOUSE STYLE): attention is sacred; calm is the *default*, not a mode.** Density is *earned*
(P5) — a surface may be dense *because* every element earns its place, not because it crams. The patterns
below are the mechanisms; each carries a **falsifiable rule** so a reviewer can score it, not admire it.

## B.1 Hierarchy from weight + colour before size (PROVEN rule, HOUSE-STYLE application)

**Rule (PROVEN — external-insights §3: "hierarchy comes from weight and colour before size; very
large/heavy type is the amateur tell"):** establish the reading order with **font-weight and
text-colour/contrast tier first**, reach for **size only when weight+colour can't carry it.** Oversized
hero type is the D3/D7 "amateur tell" detector. **Checkable:** point at any dense surface; the primary/
secondary/tertiary tiers must be distinguishable with the type *sizes within ~one step* — if hierarchy
collapses without big size jumps, it was leaning on size. *(Feeds R-17's measured-contrast: each colour
tier must still pass its WCAG ratio — the focus token ≠ identity token rule, external-insights §3.)*

## B.2 Borders over shadow; space before hairline (PROVEN rule)

**Rule (PROVEN — external-insights §3: "borders carry separation; shadow is the rare exception … one
shadow token for genuinely floating surfaces"; "a boundary groups more strongly than whitespace — but
reach for space first"):** separate regions with **whitespace first**, a **hairline border** when space
can't carry it, and reserve the single **shadow token** for *genuinely floating* surfaces (overlays —
R-10 §5; R-12 §3.5 "shadow-reserved"). **Checkable:** count shadow values on a dense surface — more than
the one floating-surface token is a fork (external-insights §3 "dozens of ad-hoc shadow literals" is the
amateur tell). No decorative dividers; no saturated fills (B.3).

## B.3 Status by glyph + label + position, never colour-alone; no traffic-light fills (PROVEN — a11y + calm)

**Rule (PROVEN — external-insights §3 / §8b.3 / WCAG 1.4.1: "status is never conveyed by colour alone …
no saturated status fills — the screen is not a traffic light"):** every status is **glyph + label +
position**, with colour as *reinforcement only*; **no saturated/traffic-light fills.** This is
simultaneously a **calm** rule (saturated fills shout; a calm surface uses restrained colour as
*periphery* signal) and a **G1** rule (color-blind-safe). **Checkable:** desaturate the screen — all
status must remain legible (the color-blind / D7-calm dual test). The R-12 §4 PR-going-green example
already obeys this (green check *and* the word "Passing" *and* position), which is why its reduced-motion
+ color-blind spellings are free.

## B.4 Agent volume out of the main timeline (PROVEN contract — the calm-under-agents pattern)

**Rule (PROVEN — design-language §6.5; notifications.md §5.2; R-10 §4.2):** agent-generated volume is
**routed OUT of the main human stream** — collapsed into threads / collapsible summaries / a separately
shed-budgeted inbox lane — so **humans never queue behind agent runs.** This is the single most important
density-made-calm pattern for an *agent-native* product (the one incumbents have no answer for): an
agent-native platform that puts agent chatter in the main timeline re-creates notification-overload at
machine speed. **Checkable (the storm test, R-10 §4.3 / R-21):** under a **30×-agent-surge**, the agent
lane sheds first (`429 + Retry-After`, notifications.md §5.2/D-N5), the **human-direct inbox stays in
budget**, and agent volume shows **collapsed/threaded**, the human items **unburied**. *Falsifier:* a
surge that buries a human-direct item or blanks the human inbox-read. This is the calm-tech "periphery"
principle (PROVEN, §5) applied to agents: agent activity lives in the *periphery*, moving to the center
only when it needs a human (a HITL gate, R-14).

## B.5 One prioritised inbox; deterministic, explainable ranking; quiet by default (PROVEN contract)

**Rule (PROVEN — design-language §5.8; notifications.md §1.3/§3.1; R-10 §4):** there is **one** inbox
("everything else is a saved filter on it"), ranked by a **deterministic, explainable** priority
(`reason → base → class`, notifications.md §3.1 — *not* an ML black box that buries a critical item),
**deduped** (N events → one "+N more"), with **quiet as the default** (the user opts *into* more;
quiet-hours honoured; only critical pierces). **Checkable:** (a) reading an item in one view marks it
read everywhere (one read-state truth — R-10 §4.1); (b) every item answers "why am I getting this"
inline (provenance line); (c) "why ranked here?" is answerable (the explain-trace). *Falsifier:* a
second message store, a read-state drift, or a critical item ranked under an fyi (the D-N1 anti-pattern).
Inbox-zero is a **quiet** reward ("You're all caught up"), **not** a celebration burst (R-12 §7.2
anti-list: no confetti — calm is the default even at the payoff).

## B.6 Restraint as default; progressive disclosure done right (PROVEN doctrine, HOUSE-STYLE application)

**Rule (PROVEN — P4 progressive disclosure; P8 calm; external-insights §4 "one shell everywhere"; R-02
config-maze trap):** the **default surface is restrained** — a short frequency-ranked default set with
**depth behind search / one layer down**, not a wall of every option (the slash-menu "not a 60-item
wall" rule, R-10 §3.2; the palette's ranked-default rule, R-08). The enterprise-admin's
SSO/residency/agent-policy depth lives **one layer down, not in the startup's face** (R-20). **Checkable
(the anti-Jira-config-maze test, R-02):** count the controls visible by default on a primary surface —
if a new user faces the full option-space before they've earned it, it's the config-maze trap.
*Falsifier:* a default view that exposes power-user/admin depth to a first-run user (progressive
disclosure done *wrong* → Jira's maze, R-02). **The calm-tech check (PROVEN, §5):** a primary surface
should require *the smallest possible amount of attention* to do its primary job.

## B.7 The unifying density-made-calm invariant (HOUSE STYLE — the D7 reviewer test)

Density-made-calm is **not** low-density. The four finalists span Axis 1 (dense ↔ calm) deliberately
(funnel §A1) — but **every** position on that axis must obey B.1–B.6: *a dense engineer board and a
spacious PM roadmap are both calm if hierarchy is weight/colour-led, separation is space/border-led,
status is glyph+label, agent volume is peripheral, the inbox is one quiet prioritised store, and the
default is restrained.* **The single D7 reviewer test (HOUSE STYLE):** *show a reviewer the densest
surface in the sketch; can their eye find the one thing that matters in under a second, and is quiet the
default state?* If yes at high density, density was *earned* (P5) and made *calm* (P8). This is the
checkable form of D7's "dense yet calm; the eye knows where to go; quiet is the default."

---

## 3. Completeness-critic (README §9) — gloss-risks this item touches

R-13's prompt names three §9 states it must cover; it **owns the perceived-perf craft** of them and
routes the per-surface catalogue to R-21:

- **Loading state** — **OWNED & covered** (A.2 per-surface skeleton shapes; B3 flash-of-spinner
  suppression; the no-spinner-token rule). The full component×state *matrix* → R-21 (this file gives the
  skeleton *craft*; R-21 places it per surface).
- **Optimistic-rollback state** — **OWNED & covered** (A.3: the three-state contract + OPT-1..4; honest-
  revert visibly distinct from settle; the conflict-on-collision route). R-12 §3.1 owns the motion; R-21
  owns the per-surface placement.
- **Stale / offline / reconnecting state** — **covered as a perceived-perf concern** (B5/B6: fails-static
  degraded surface; the inbox firehose drop+resume → backfill-then-live, named-not-silent, notifications.md
  §7/D-N11, R-10 §4.3). The *reconnecting-state craft* (the "reconnecting…" affordance, offline buffer
  re-sync) → **R-21 owns it** (R-10 §3.3/§4.3 placed it; design-language §9 flags offline scope as
  `[OPEN → P4]`). Surfaced here, not duplicated.
- **Storm / 30×-agent-surge** — **covered as the calm-under-agents stress case** (B.4, surfaced from
  notifications.md §5.2/D-N5); the inbox *experience* of the storm → R-21.
- **Consciously deferred (with reason):** the full per-surface state matrix (R-21 owns it — duplicating
  breaks the cumulative-corpus rule); the HITL card itself (R-14); the motion tokens (R-12). Named-and-
  routed, not re-specced.

---

## 4. Actionability toward the control artifacts

| Control artifact | What this file equips | Where |
|---|---|---|
| **rubric D8 (perceived performance, 6%)** | The **checkable** D8 bar: the latency-budget table (A.1, B1–B6) makes "feels instant" scoreable against thresholds, not backend speed; per-surface skeletons (A.2) are the "loading shows structure" anchor; the optimistic three-state + honest-rollback contract (A.3) is the "optimistic with honest rollback designed" anchor; B4 is "pages render, they don't animate in." The §A switch test is the D8/D10 done-bar. | A.1–A.4 |
| **rubric D7 (density-made-calm, 8%)** | The concrete D7 pattern set (B.1–B.6) replaces "be calm" with falsifiable rules: weight-before-size, borders-over-shadow, glyph+label-not-colour, agent-volume-out-of-timeline, one-quiet-inbox, restraint-default; B.7 is the single D7 reviewer test (densest-surface, eye-finds-it-in-a-second, quiet-default). | §B |
| **sketch-funnel Axis 1 (density: dense ↔ calm)** | B.7: *every* position on Axis 1 must obey B.1–B.6 — density-made-calm is the rule that makes a dense finalist *and* a calm finalist both score on D7. Phase-6 finalists demonstrate skeleton + optimistic + a dense-but-calm surface. | §B, B.7 |
| **R-21 (state-craft, next-but-one)** | R-21 lists R-13 as a Read; it consumes A.2 (skeleton craft), A.3 (optimistic-rollback contract), and the reconnecting/degraded/storm routing (§3) as the perceived-perf half of its per-surface state matrix. | A.2, A.3, §3 |
| **R-22 (wedge moments)** | R-22 lists R-13 as a Read; it consumes A.4 (CA-1..4 prefetch/context-assembly + EXT-1) as the mechanic behind the PR-context-pane and "why+next-hop" wedge moments. | A.4 |
| **Phase 6** | Finalists author skeletons matching their layout, the optimistic contract, and a dense-but-calm surface; the budgets (A.1) are the perf bar; the prefetch UX (A.4) needs EXT-1 scheduled. | A.1–A.4, §B |

---

## 5. Sources (web-verified 2024–2026 + surfaced contracts)

**Latency / response-time budgets (PROVEN):**
- web.dev — Measure performance with the RAIL model (Response goal <100ms; process input <50ms):
  https://web.dev/articles/rail
- Nielsen / NN-g — Response Time Limits (0.1s instant / 1s flow):
  https://www.nngroup.com/articles/response-times-3-important-limits/
- INP replaced FID March 2024 as the Core Web Vitals responsiveness metric (Response/RAIL alignment):
  https://web.dev/articles/inp · https://madecurious.com/articles/inp-and-the-illusion-of-speed/

**Skeletons / perceived performance (PROVEN — measured):**
- Skeleton screens perceived ~20–30% faster than spinners; prevent content-arrival layout shift
  (Viget/skeleton studies, summarized): https://blog.logrocket.com/ux-design/skeleton-loading-screen-design/ ·
  https://www.onething.design/post/skeleton-screens-vs-loading-spinners ·
  https://ui-deploy.com/blog/skeleton-screens-vs-spinners-optimizing-perceived-performance

**Optimistic UI + honest rollback (PROVEN — pattern + ~40% perceived-wait reduction):**
- Optimistic updates make apps feel faster; rollback on failure; idempotency under retry:
  https://blog.openreplay.com/optimistic-updates-make-apps-faster/ ·
  https://javascript.plainenglish.io/optimistic-ui-in-frontend-architecture-do-it-right-avoid-pitfalls-7507d713c19c ·
  https://murtazaweb.com/blog/2026-03-22-optimistic-ui-updates-patterns/

**Density-made-calm / attention (PROVEN — doctrine + external grounding):**
- Calm Technology principles (smallest possible attention; periphery↔center; Case / Xerox PARC):
  https://www.calmtech.institute/calm-tech-principles · https://www.caseorganic.com/post/principles-of-calm-technology

**Surfaced Myelin contracts (PROVEN-as-existing, not invented):**
- external-insights §4 (loading-shows-structure / optimistic+honest-rollback / system-assembles-context /
  hard latency budgets / one shell), §3 (weight-before-size, borders-over-shadow, status-not-colour-alone,
  one shadow token); design-language §8b.6 (skeleton/error/budgets/fails-static), P2/P5/P8, §6.5 (agent
  volume), §5.8 (one inbox); P2/ADR-11 (residency — no global CDN for personal data).
- R-10 §2.2/§3.3/§4.3/§5.3 (component states this file dresses), §4.1/§4.2 (inbox provenance + agent lane).
- R-12 §3.1 (optimistic-settle), §4 (PR-going-green/liveUpdate), §3.8 (skeleton swap), L2/L3 budgets, §7.2
  (no-spinner / no-confetti anti-list), §2.4 (reduced-motion).
- **EXT-1** `extension-planning/perceived-performance.md` (permission-aware context-bundling projection +
  client prefetch-hint stream; the no-leak invariant; size M / risk M) — the §A.4 prefetch UX dependency.
- notifications.md (one inbox §1.3; deterministic ranking §3.1; dedup §3.2; storm shed-budget §5.2/D-N5;
  firehose resume §7/D-N11; fails-static §5.3); ADR-03 (permission-pre-filter); ADR-11 (residency).

**Honest limitation:** the skeleton/optimistic *percentage* figures are from practitioner/UX-blog
syntheses of studies (Viget, NN-g) rather than primary RCTs re-run here; cited as the *direction and rough
magnitude* (skeletons faster, optimism faster), tagged PROVEN-as-reported. The exact magnitude on Myelin's
surfaces is a §6 measurement.

---

## 6. `[DEFERRED-UNTIL-USERS]` — what these patterns have NOT earned

R-13 is `user-dep: none` — the deliverable IS the no-user substitute (expert spec grounded in
perception/perceived-perf studies + the prior component/motion specs + the EXT-1 mechanic). The following
are **HYPOTHESES** falsifiable once users + a real backend exist; recorded as executable plans, not faked:

- **`[DEFERRED-UNTIL-USERS]` — Do the budgets (A.1) actually read as "instant / feels finished"?**
  *Test:* the §8b.7 switch test on each Phase-6 finalist + instrument input→next-paint (B1), optimistic
  paint (B2), flash-of-spinner suppression (B3) on the real UI. *Falsifier:* keyboard actions measured
  >100ms felt as laggy; surfaces spinner-blanking under 1s; a finalist scoring D8≥3 in review that *feels*
  slow when driven (the switch test catches what the checklist misses, external-insights §7).
- **`[DEFERRED-UNTIL-USERS]` — Is the optimistic + honest-rollback contract (A.3) *trusted*?** *Test:*
  induce failures on a real backend; do users (a) believe failed actions failed (OPT-1) and (b) trust the
  surface enough to keep acting optimistically? *Falsifier:* users distrust the surface after a rollback,
  or miss a silent-looking revert. **Caveat:** the *contract* (visible-distinct revert) is designed to be
  trustworthy regardless of runtime; only the *felt trust* is the hypothesis.
- **`[DEFERRED-UNTIL-USERS]` — Does residency-bought perceived speed (A.4) feel "enough" for the
  worst-case cross-region collaborator?** *Test:* measure perceived speed for an in-region user vs a
  cross-region collaborator with EXT-1 bundling+prefetch + optimistic UI, no global replication.
  *Falsifier:* cross-region collaboration feels slow despite the three levers → the perceived-perf
  strategy needs re-thinking within the residency constraint (it must *stay* within it — P2/ADR-11). **This
  is the largest open question** (the residency↔perceived-speed tension is real, not rhetorical).
- **`[DEFERRED-UNTIL-USERS]` — Is the densest finalist surface *felt* as calm (B.7)?** *Test:* per-segment
  (engineer P1 vs PM P6) on the dense board *and* the calm roadmap — does each segment's eye find the
  primary thing in <1s, and is quiet the felt default? *Falsifier:* engineers find the dense surface noisy
  (density not earned) or PMs find the calm surface empty/slow (calm read as lacking).
- **Method:** per-segment RITE + the §8b.7 switch test on the Phase-6 finalists, on the F-ENG-1
  (failing-check→line, PR-context-pane) and the dense-board/calm-roadmap dual-audience surfaces.
  **EXT-1 must be scheduled** for the prefetch patterns (A.4) to be testable beyond mock.

---

## 7. Self-check against R-13 acceptance criteria

| Criterion (prompt R-13) | Status | Evidence |
|---|---|---|
| **Per-surface skeleton patterns specified (structure-matching, never blank spinner)** | ✅ Met | A.2 per-surface catalogue (table/board/timeline/editor/inbox/unfurl/PR-pane shapes); no-spinner-token rule; B3 suppression |
| **Optimistic-update + honest-rollback patterns specified** | ✅ Met | A.3 three-state contract (pending/settled/rolled-back) + OPT-1..4 (honesty, reversibility-vs-confirm carve-out, no-clobber, idempotent); reduced-motion |
| **Prefetch / context-assembly UX named AND linked to its extension** | ✅ Met | A.4 CA-1..4 patterns explicitly linked to **EXT-1** (`extension-planning/perceived-performance.md`); the no-leak invariant surfaced |
| **Latency budgets restated as design constraints (checkable)** | ✅ Met | A.1 table B1–B6 (keyboard <100ms; optimistic; flash-of-spinner <1s; pages-render-not-animate-in; live-update; fails-static) — each with a "how it's checkable" column |
| **Density-made-calm patterns concrete (not "be calm")** | ✅ Met | §B B.1–B.6 each a falsifiable rule + checkable test; B.7 the single D7 reviewer test; calm-by-default / attention-sacred / agent-volume-out (B.4) |
| **Each pattern PROVEN / HOUSE-STYLE-tagged** | ✅ Met | tags throughout; budgets/thresholds PROVEN (RAIL/Nielsen/skeleton/optimistic studies), values HOUSE STYLE; calm rules PROVEN (external-insights §3/§4 + Calm Tech) / applications HOUSE STYLE |
| **Builds ON R-10 + R-12, doesn't duplicate** | ✅ Met | §0 + inline: dresses R-10's component states, consumes R-12's motion tokens by name; routes per-surface matrix to R-21, motion to R-12 |
| **Date the file (2026-06-20); do NOT commit** | ✅ Met | header; no git actions taken |
| **Actionable toward rubric D7/D8 + funnel Axis 1; feeds R-21/R-22** | ✅ Met | §4 mapping |
| **§9 gloss-risks addressed (loading / optimistic-rollback / stale-reconnecting)** | ✅ Met | §3 (loading + optimistic-rollback OWNED; reconnecting/degraded/storm covered-and-routed to R-21) |
| **Deferred validation recorded as a plan, not faked** | ✅ Met | §6 (`[DEFERRED-UNTIL-USERS]`: budgets-felt, rollback-trust, residency-perceived-speed, dense-felt-calm — each with falsifier + method) |

**Top uncertainties (honest, per VISION §3):**
1. **The residency↔perceived-speed tension (A.4 caveat, §6).** Whether optimistic UI + in-region edge +
   EXT-1 prefetch buy "enough" felt speed for a worst-case cross-region collaborator — *without* global
   replication of personal data (P2/ADR-11) — is the **largest open question**, and it depends on EXT-1
   being built. The strategy is residency-correct by construction; the *sufficiency* is the hypothesis.
2. **The skeleton/optimistic percentage figures (§5)** are practitioner-synthesised study magnitudes, not
   RCTs re-run on Myelin; cited for *direction*, the Myelin-specific magnitude is a §6 measurement.
3. **The budget *assignments* (A.1) are HOUSE STYLE within PROVEN thresholds** — the 100ms/1s lines are
   perception-grounded; which interactions fall in B1 vs B2 vs B3, and whether all clear the bar on a real
   backend, is the §6 switch test.
4. **"Dense yet calm" is a taste call until per-segment-tested (B.7, §6)** — the B.1–B.6 rules are the bet
   that density can be *earned* without shouting; only engineers-on-the-dense-board + PMs-on-the-calm-
   roadmap settle whether each lens *feels* calm rather than noisy/empty.

---

*End of R-13 deliverable. Date: 2026-06-20. Perceived-perf + density-made-calm patterns HOUSE STYLE over
the PROVEN §8b.6 / external-insights §3–§4 doctrine + cited perception (RAIL/Nielsen), perceived-perf
(skeleton/optimistic studies), and calm-tech (Case/PARC) sources; the prefetch UX depends on EXT-1; not
user-validated — see §6. Builds on R-10, R-12. Feeds R-21, R-22, Phase 6, rubric D7/D8, funnel Axis 1.*
