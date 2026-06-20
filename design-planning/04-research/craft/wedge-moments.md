# R-22 — The Cross-Artifact "Wedge" Moments (delight at the seams)

> **Phase 4 research corpus** · WS-J (onboarding & craft) · Seq #20. Deliverable for prompt **R-22** in
> [`03-research-prompts.md`](../../03-research-prompts.md). **File date: 2026-06-20.**
> Methods: **#8 (service blueprinting — the wedge flows)** + **#9 (job-flow, moment-by-moment experience)**.
>
> **What this file is.** P6 ("reference everything, everywhere — the wedge made visible") and
> `competitive-landscape.md` §6 ("the integration *is* the differentiator") are the literal thesis of the
> product. This file names the **highest-leverage seam moments** — the specific instants in a cross-surface
> flow where one-product integration produces delight the fragmented stack *structurally cannot* — and
> specs each as a **deliberate love-moment**: the moment, the cross-surface mechanics that make it possible,
> the design that makes it *felt* (not buried), and the "the old stack can't do this" contrast. It is the
> **wedge screen every Phase-6 finalist must include** ([`sketch-funnel.md`](../../02-research-roadmap/sketch-funnel.md)
> §"comparable screen set" #5), and it feeds rubric **D4** (one-product coherence) and **D10** (switch test).
>
> **It SURFACES, does not redesign.** Every mechanic below already exists in the corpus — the reference
> graph resolver and its backlink projection (R-09 / [`reference-graph.md`](../../../planning/05-refined-shared-systems-architecture/reference-graph.md)),
> the prefetch/context-bundling extension (R-13 §A.4 / EXT-1), the cross-surface flows and their seam
> register (R-04 §8), the PR context pane (`system-overview.md` §8.1), the agent `correlation_id` chain
> (R-04 §7 / agent-fabric). R-22's job is to make each seam a *named, designed, lovable peak* — not to
> invent a backend.
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = a cited external standard/study, OR an existing
> Myelin contract this file *surfaces* (the resolver, the bundling extension, the flows, §8.1). **HOUSE
> STYLE** = our love-moment design synthesis/taste (which moments are wedges, how they're staged to be
> *felt*). **Not user-validated;** the falsifiable bets are in §6.
>
> **Builds ON prior `04-research` (does not duplicate):**
> - [R-04 cross-surface-flows](../jtbd-flows/cross-surface-flows.md) — **the flows each wedge lives in**,
>   and the **§8 seam register** (the seven seams) this file turns into love-moments. Every wedge below
>   names its R-04 flow + seam-register row. *This file does not re-draw the blueprints; it deepens the
>   marked 🔪 moments into staged peaks.*
> - [R-09 reference-unfurl](../interaction/reference-unfurl.md) — **the component** every wedge is built
>   from (chip ↔ unfurl; the resolver→UI state contract §1.1; live-not-snapshot §6.2; no-access/erased
>   §5.4/§5.7; cross-cell §5.8; rebase-orphan §5.9). Every wedge below is *this component in a flow*.
> - [R-13 perceived-performance](../visual/perceived-performance.md) — **the felt-instant mechanics**: the
>   prefetch/context-assembly patterns CA-1..4 (§A.4) + EXT-1, the skeleton-not-spinner scaffold (§A.2 PR
>   context-pane row), the latency budgets (§A.1). A wedge that *assembles* context only delights if it
>   arrives fast; R-13 is how. The residency caveat (§A.4) is inherited verbatim.

---

## 0. How to read this file

- **§1** — what makes a *wedge moment* (vs. a feature); the four-part anatomy; the design doctrine (peak-end
  + the seam-register tie).
- **§2** — **the catalogue: W1…W7 named wedge moments**, each with {the moment · cross-surface mechanics ·
  the design that makes it felt · the "old stack can't" contrast · the unglamorous seam (§9 gloss-risk) ·
  R-04 flow + R-09 component}. (W1–W5 are the prompt's required ≥5; W6–W7 extend.)
- **§3** — the **anti-wedges**: integration moments that are traps, not delight (so finalists don't gild).
- **§4** — completeness-critic (README §9 gloss-risks: cross-cell no-access/tombstone, diff-anchor relocation).
- **§5** — actionability (rubric D4/D10; the funnel wedge screen). **§6** `[DEFERRED-UNTIL-USERS]`.
  **§7** sources. **§8** self-check.

---

## 1. What a wedge moment is

### 1.1 Definition (HOUSE STYLE)

> **A wedge moment is a single instant in a cross-surface flow where the integration produces a result the
> user would otherwise have assembled by hand across N tabs/tools — and where that result is staged to be
> *noticed and loved*, not silently delivered.** It has three necessary properties:
> 1. **Cross-surface** — it spans ≥2 of the five subsystems (git · CI · issues · knowledge · chat), so the
>    fragmented stack would force a tab-switch / copy-paste / context-loss to reproduce it (R-04 §8).
> 2. **Structurally exclusive** — it is possible *because* of the shared reference graph + one identity +
>    one bus, i.e. it can't be bolted onto a stitched suite (`competitive-landscape.md` §6.1 — the
>    "stitched-together, not unified" failure). This is the "old stack can't do this" test.
> 3. **Felt, not buried** — it is *designed as a peak*: the moment is legible at the instant it happens,
>    fast (R-13 budgets), and calm enough that the delight lands instead of adding noise (P8).

A wedge moment is **not** a feature list ("we have unfurls, we have backlinks"). The same mechanism delivered
without staging is just plumbing. The wedge is the *experience of the seam dissolving in front of you*.

### 1.2 The design doctrine — why staging matters (PROVEN psychology, HOUSE-STYLE application)

People remember an experience by its **most intense moment and its end**, not its average — the **peak-end
rule** (Kahneman; well-established in UX practice — PROVEN, [Laws of UX](https://lawsofux.com/peak-end-rule/);
[NN/g-aligned summaries](https://www.ux-bulletin.com/peak-end-rule-ux-designing-memorable-experiences/)). The
*differentiator* a person carries away from a trial is therefore disproportionately set by a few designed
peaks. **HOUSE-STYLE consequence:** the wedge moments are where Myelin spends its peak budget — the instants
that make "one product, integration-is-the-feature" *felt and loved*, not merely true. Crucially, the
peak-end rule cuts both ways: a single broken seam (a leaked title, a chip silently pointing at the wrong
line) is a *negative* peak that poisons the memory — so each wedge's unglamorous failure state (§4) is part
of the love-moment, not an afterthought.

The pain these peaks relieve is **measured**: developers lose ~1–2 hours/day to context-switching and switch
tools ~tens of times per hour across ~9 apps (PROVEN, practitioner-synthesised industry data —
[Hivel](https://www.hivel.ai/blog/context-switching-crisis-quantifying-the-cost-and-finding-solutions);
[Asana Anatomy of Work](https://asana.com/resources/context-switching); ~23-min refocus cost, UC-Irvine
lineage). Each wedge below is the *positive inverse* of a specific switch.

### 1.3 The four-part anatomy (every W-entry uses it)

| Part | Question it answers | Grounds in |
|---|---|---|
| **The moment** | What does the user *see/feel* at the instant the seam dissolves? | R-04 job-flow (#9) |
| **Cross-surface mechanics** | Which events/refs/projections make it possible (surfaced, not invented)? | R-09 resolver; R-13 §A.4; §8.1; bus |
| **The design that makes it felt** | How is it staged as a *peak* — legible, fast, calm, not buried? | peak-end (§1.2); R-13 budgets; P8 |
| **The "old stack can't" contrast** | What N-tab dance does the fragmented suite force instead? | R-04 §8 seam register; competitive §6 |

### 1.4 The wedge index (each maps to an R-04 flow + an R-04 §8 seam-register row + an R-09 component state)

| # | Wedge moment | R-04 flow | R-04 §8 seam row | R-09 component / state | Required? |
|---|---|---|---|---|---|
| **W1** | **The PR context pane assembles itself** | F-ENG-1 / F-ENG-2 | "Blame→SHA→…(4 tabs)"; §8.1 | unfurl card §3 · live §5.1 · no-access §5.4 | ✅ (the flagship) |
| **W2** | **Paste-a-link, it unfurls live with an inline action** | F-PM-1 | "Paste Slack msg→Jira…" | chip→card §2/§3 · live §6.2 · inline action §4 | ✅ |
| **W3** | **The notification that carries *why* + the pre-fetched next hop** | F-PM-1 / F-AGT-1 | (inbox; CA-3) | chip + provenance §7.1 · prefetch CA-3 | ✅ |
| **W4** | **Failing check → step → line → fix, one warm chain** | F-ENG-1 | "Checks tab→logs→guess file→Files tab" | run/step chip §2.3 · diff-anchor §5.9 · CA-1 | ✅ |
| **W5** | **Backlinks appear automatically — the reverse trail no one curated** | F-ENG-2 / F-PM-1 | "Blame→…Confluence" | backlinks panel = chips §7.1 · live §6.2 | ✅ |
| **W6** | **One `correlation_id` you can read across all five surfaces** | F-AGT-1 | "Agent ops in a separate console" | agent chip §5.11 · attribution §4.2 | ➕ extends |
| **W7** | **Convert-this-to-an-issue without leaving the conversation** | F-PM-1 | "Paste Slack msg→Jira…" | chip created in-place §2 · optimistic §4.2 | ➕ extends |

Coverage: **≥5 named moments** (W1–W5) as the prompt mandates; W6–W7 extend to the agent and chat→issue
seams. Every moment maps to a **real R-04 flow** and a **real R-09 component state**.

---

## 2. The catalogue — W1…W7

### W1 — The PR context pane assembles itself *(the flagship wedge)*

> **R-04:** F-ENG-1 (red-to-green) + F-ENG-2 (trace-to-change). **Seam:** §8.1 / R-04 §8 "Blame→SHA→commit
> →PR→Jira→Confluence (4 tabs)". **Component:** the unfurl card (R-09 §3) × N, live (§5.1), permission-
> filtered (§5.4). **Prefetch:** R-13 CA-2 + the §A.2 PR-context-pane skeleton scaffold.

**The moment.** You open PR #88. Without a click, the pane beside the diff already holds: the **linked issue**
(its current state, not last-week's), the **CI run** (green or the exact failing step), the **doc section**
the change implements, and the **discussion thread** — each a live card, each showing *only what you're
allowed to see*. You read *why this change exists and whether it's safe to merge* without opening a single
other tab. *(This is the single clearest demonstration that the glue works — `system-overview.md` §8.1.)*

**Cross-surface mechanics (surfaced, PROVEN).** On PR open, Refs returns the PR's edges; Id `list-objects`
pre-filters them to the viewer's visible set; each subsystem **projects** its own artifact for this viewer in
parallel; the bus keeps the pane live via per-`ArtifactRef` cache invalidation (`system-overview.md` §8.1
sequence — surfaced verbatim). The whole set arrives in **one bundled context call** (R-13 CA-2 / EXT-1),
not N sequential fetches — and **a projection the viewer can't see is never bundled** (EXT-1 no-leak
invariant; R-09 §5.4).

**The design that makes it felt (HOUSE STYLE).** (a) The pane's **scaffold renders instantly** as labelled
skeleton slots — "Issue · CI · Doc · Discussion" — so the *shape of the available context is visible before
the content lands* (R-13 §A.2 PR-context-pane row); each slot fills as its bundle resolves, never a blank
spinner. (b) The cards are **the same chip/unfurl component** the user sees in chat and issues (R-09 §3.1
shared shell) — so the pane reads as *one product*, not an embedded mini-GitHub (D4). (c) The **live**
quality is the peak: while you read, a teammate re-runs CI and the run card **flips red→green in place**
(R-09 §6.2; R-13 B5 `motion.liveUpdate`, no scroll-jump) — the pane is *alive*, not a snapshot.

**The "old stack can't do this" contrast (PROVEN).** GitHub shows you a PR; the issue is in Jira (another
tab, another login), the decision is in Confluence (a third), and the only "link" is a copy-pasted URL that
goes stale. Reproducing W1 in the stitched stack is the 4-tab blame→SHA→commit→PR→Jira→Confluence dance
R-04 §8 names — *and even then nothing is permission-filtered per viewer or kept live*. The wedge isn't
"we have a side panel"; it's that the side panel is **the reference graph made visible, live, and leak-free**
(P6) — structurally impossible without one identity + one ref graph (`competitive-landscape.md` §6.1).

**Unglamorous seam (the negative-peak guard, §4).** A linked artifact you *can't* see renders as the R-09
§5.4 **no-access card** ("You don't have access to this issue" — never the title); a **cross-cell** ref shows
the residency tag or the no-access card (§5.8); an **erased** one is the dignified tombstone (§5.7). The pane
**fails static per-slot** (R-13 B6) — one subsystem down greys one slot, never the pane.

---

### W2 — Paste-a-link, it unfurls live with an inline action

> **R-04:** F-PM-1 (incident-to-runbook). **Seam:** R-04 §8 "Paste Slack msg→Jira; paste Jira link→Slack;
> hunt Confluence". **Component:** chip→card (R-09 §2/§3), live (§6.2), inline action (§4), unfurl-in-thread
> calm rule (§7.2).

**The moment.** Mid-incident you paste a `myelin://` link (or `@`-mention an issue) into the `#prod-fire`
thread. It **unfurls inline into a live card** — the issue's *current* state, the runbook's anchored step,
the CI run's status — and the card has a **button you can press right there**: *Re-run failed checks*,
*Transition to Mitigating*, *Approve*. You act on the artifact **without leaving the conversation**.

**Cross-surface mechanics (surfaced, PROVEN).** The pasted ref resolves through the one resolver ladder per
viewer (R-09 §1.1); the card body is the owning subsystem's projection (§3.2); inline actions are the
permission-pre-filtered action subset (R-09 §4.1, ADR-03) applied **on the `ArtifactRef`** with full
attribution (§4.2 rule 4) — pressing *Re-run* in chat is identical to pressing it on the CI surface.

**The design that makes it felt (HOUSE STYLE).** (a) **Live, not snapshot** is the peak: the card you pasted
at 14:02 shows the issue's 14:09 state because it re-resolves (R-09 §6.2) — the incident channel becomes a
*live dashboard*, not a wall of dead links. (b) **Calm by default:** one card auto-expands; the rest stay
chips (R-09 §7.2) — five refs is five one-line chips, not five fat cards (the explicit anti-Slack-noise
rule). (c) The inline action is **optimistic + honestly rolled back** (R-09 §4.2; R-13 A.3) — it flips
instantly; if you lacked the grant it reverts with one quiet line, never a dead-end.

**The "old stack can't do this" contrast (PROVEN).** Slack unfurls a **cached snapshot** (cleared ~every 30
min; a re-posted URL won't even re-unfurl), it **404s on anything private**, and the card is a *preview, not
an action surface* — to actually re-run the job you leave Slack for the CI tool (PROVEN —
[Slack link-unfurling docs](https://api.slack.com/reference/messaging/link-unfurling);
[snapshot/cache + private-link 404 behaviour](https://www.fuzzyraygun.com/blog/fixing-preview-or-unfurl-in-slack)).
Myelin's unfurl is **live, permission-aware per viewer, and an action surface** — the three axes R-01 §3.1
flagged Slack as structurally weaker on. *Acting from the place you already are* is the seam dissolving.

**Unglamorous seam (§4).** A pasted link to something you can't see is the **no-access card, never the
title** (R-09 §5.4 — non-leaking *by construction*: the resolver collapsed it to a tombstone before content
crossed the wire). Cross-cell → residency-tagged or no-access (§5.8). The action you lack → the button is
**absent, not greyed** (R-09 §4.2 rule 1 — don't advertise power you can't use).

---

### W3 — The notification that carries *why* + the pre-fetched next hop

> **R-04:** F-PM-1 / F-AGT-1 (inbox branches). **Seam:** the inbox; R-13 CA-3. **Component:** chip +
> provenance line (R-09 §7.1; R-10 inbox §4.1); prefetch (R-13 CA-3 / EXT-1).

**The moment.** A notification arrives: *"INCIDENT-9 transitioned to Mitigating — because you're assigned."*
It tells you **why you got it** in the same line, and when you click, the target is **already warm** — it
opens instantly, the next hop pre-fetched. One read, one click, done — no "wait, why am I getting this?" and
no spinner on arrival.

**Cross-surface mechanics (surfaced, PROVEN).** The "why" is the inbox's `origin_event` + `reason`
provenance (PROVEN — `notifications.md`; R-10 §4.1) — *not* a new mechanism. The warm target is R-13 CA-3:
the inbox provenance + EXT-1 prefetch-hint warms the next-hop projection ahead of the click, leak-free
(EXT-1 inherits the chip's permission-pre-filter, R-09 §5.4).

**The design that makes it felt (HOUSE STYLE).** (a) The **"why it fired" line is inline and always present**
— it converts the universal incumbent dread ("another ping, do I care?") into a one-glance triage. (b) The
**target opens warm** — the notification is not a *pointer to a tab-switch-to-assemble*; it's the next action
already loaded (R-13 CA-3). (c) **Calm:** one prioritised inbox, deduped ("+23 updates on INCIDENT-9" not 23
pings), agent volume routed out of the main lane (R-13 B.4/B.5; P8). The delight is *the absence of
firehose*, staged as "you're caught up" quiet (no confetti — R-13 B.5).

**The "old stack can't do this" contrast (PROVEN).** Stitched-stack notifications are N separate firehoses
(Jira email + Slack badge + GitHub bell), none of which say *why* you got this one or what to do next, and
every one of which drops you into a cold tab to reconstruct context — the measured ~23-min-refocus,
tools-35×/hour tax (PROVEN, §1.2 sources). Myelin's single inbox answers "why" and pre-loads "next" — the
two questions the fragmented stack makes you answer yourself.

**Unglamorous seam (§4).** Under a **30×-agent-surge** the agent lane sheds first and the human-direct inbox
stays in budget (R-13 B.4 storm test; R-21 owns the storm-state craft); a prefetched hop you've since lost
access to **re-resolves to no-access on open**, never a leaked warm card (R-09 §5.4 — live re-resolution is
the erasure-safety guarantee, §6.2).

---

### W4 — Failing check → step → line → fix, one warm chain

> **R-04:** F-ENG-1 (red-to-green, the wedge engineer flagship). **Seam:** R-04 §8 "Checks tab→opaque
> logs→guess the file→Files-changed tab". **Component:** run/step chip (R-09 §2.3) → diff-line-anchored chip
> (§5.9); prefetch R-13 CA-1.

**The moment.** A check is red. You click it and you are **on the failing line of code** — not on a logs
tab, not scrolling opaque output guessing which file. The failing **step**, its **log tail**, and the exact
**diff line** are *already there* (pre-fetched), and the inline agent-suggested fix (if any) sits at the
line. You open a fix PR and it arrives **pre-populated with the issue and run it descends from** — no
copy-pasting IDs.

**Cross-surface mechanics (surfaced, PROVEN).** The log line **is a ref** to the diff line — a
content-anchored line-range (R-09 §5.9; `reference-graph.md` §3.5 BLAKE3 fingerprint + 3-way context match).
The run→job→step→diff-anchor chain arrives as **one bundled projection** (R-13 CA-1 / EXT-1), warm before
the click lands (the keyboard path resolves <100ms, R-13 B1; the diff is pre-fetched, R-04 §2.2). The new
PR's pre-population is the Refs graph + `ref.created` edges (R-04 §8), not a paste.

**The design that makes it felt (HOUSE STYLE).** (a) **One click from red to the line** is the peak — the
step→line resolve is staged so the diff is *warm*, arriving as structure-skeleton-then-content, never a blank
drill (R-13 CA-1). (b) The **fix PR is pre-linked** — the issue/run chips are *already in the new PR body*
(F-ENG-1 §2.2), so the engineer's last manual chore (copy the issue ID) is gone. (c) The chain reads as **one
surface in two renderings** — it finishes identically in the web UI or via `myelin run watch` in the terminal
(R-04 §2; CLI is a peer surface).

**The "old stack can't do this" contrast (PROVEN).** GitHub Checks → scroll opaque logs → *guess* the file →
switch to Files-changed → open Jira to find the issue → paste a URL (R-04 §8). Each hop is a manual
reconstruction of a link the system never stored. Myelin stored the link (log-line→diff-line is a ref) and
warmed it — the seam is *the guess and the paste both disappearing*.

**Unglamorous seam (§4 — the load-bearing one).** If the diff was **rebased between failure and click**, the
content-anchored chip re-resolves: **exact** → live at the line; **rebased** → relocates with a "moved" pill;
**partial** → "outdated"; **content_gone** → it **detaches to an "outdated — was on former line N" pill and
lifts to file level — it never silently jumps to a wrong line** (R-09 §5.9; the explicit anti-pattern GitHub
gets wrong). This honest-relocation *is part of the wedge* — a chain you can trust beats a chain that's fast
but lies.

---

### W5 — Backlinks appear automatically — the reverse trail no one curated

> **R-04:** F-ENG-2 (trace-to-change) / F-PM-1. **Seam:** R-04 §8 "blame→…hunt Confluence". **Component:**
> the backlinks/"linked references" panel = chips (R-09 §7.1; design-language §5.11 backlinks panel), live
> (§6.2).

**The moment.** You open a doc, an issue, or a line of code, and a **"Linked references / Mentioned in"**
panel is *already populated* — every PR, issue, chat thread, and doc that references *this* thing, that
**nobody curated**. You discover the decision that drove a line, the incident that spawned a runbook, the
discussion behind a merge — by *following a trail that maintained itself*.

**Cross-surface mechanics (surfaced, PROVEN).** Backlinks are **event-sourced projections** — an edge exists
because a `refs.edge.created` event exists; the backlink inverse index is rebuilt from those events; and
**every backlink read is gated by Id's `list_objects` pre-filter, so the answer is leak-free by
construction** (PROVEN — `reference-graph.md` §4, C-4; the backlink index as the associative trail, Bush
1945 lineage). The forward mention you (or an agent) wrote *is* the backlink the other end sees — one fact,
both directions.

**The design that makes it felt (HOUSE STYLE).** (a) The trail is **automatic and reverse** — the delight is
*finding the context you didn't know to look for* (the opposite of a dead "see also" you have to maintain).
(b) Each backlink is the **same live chip** (R-09 §6.2) — clicking through, every hop is a live, permission-
filtered unfurl (the F-ENG-2 4-subsystem chain), never a stale link. (c) **Peek before you leap** — hover a
backlink for the bounded hovercard (R-09 §3.3 / §5.2) so you triage the trail without navigating away.

**The "old stack can't do this" contrast (PROVEN).** In the stitched stack, "what references this?" is
*unanswerable across tools* — Confluence backlinks don't know about Jira, Jira doesn't know about the PR, and
none know about the Slack thread. You hunt, manually, across N tools, and the trail is only as good as
whoever remembered to paste a link (`competitive-landscape.md` §6.1; R-04 §8). Myelin's trail is the
**reference graph's reverse index** — complete, live, and leak-free, *for free, because every mention is an
edge* (P6 — "cross-references rot" is the #1 pain killed).

**Unglamorous seam (§4).** A backlink to something you can't see is **counted-but-withheld or absent per the
leak-free read** (never the title — `reference-graph.md` §4 `list_objects` gate); an **erased** source shows
the dignified tombstone (R-09 §5.7); the **edge survives even when the target tombstones** (R-09 §5.7 — the
tombstone is a *render*, not a deletion of the link), so the trail's integrity outlives the artifact.

---

### W6 — One `correlation_id` you can read across all five surfaces *(the agent wedge)*

> **R-04:** F-AGT-1 (HITL flagship). **Seam:** R-04 §8 "Agent ops in a separate console away from the team".
> **Component:** agent chip/unfurl (R-09 §5.11); attribution-on-the-ref (§4.2 rule 4). **Depth:** R-14/R-15.

**The moment.** A CI failure is triaged by an agent that files an issue, posts to chat, and proposes a fix
PR — and you can **read the whole chain end-to-end as one story**: *this run → this triage → this issue →
this chat post → this proposed PR*, all stamped with **one `correlation_id`**, every step attributed
("FixAgent, on behalf of @dev"). The approval is **a card in the chat where the team already is**, not a
separate ops console.

**Cross-surface mechanics (surfaced, PROVEN).** The agent is a first-class actor on the bus; plan-then-apply
emits attributed effects under one `correlation_id`; each effect creates `ref.created` edges so the chain is
*navigable* (R-04 §7.1 blueprint; agent-fabric — surfaced). The approval card is the §5.4 HITL shape
rendered as an unfurl in chat (R-09 §5.11 / §4.1).

**The design that makes it felt (HOUSE STYLE).** (a) The `correlation_id` is **a readable thread, not a log
field** — following it is the same live-chip navigation as any other backlink (W5), so "what did the agent
do, and why?" is answered *by clicking the trail*, not by grep. (b) The **plan is visible before the apply**
— the card shows *proposed effects per artifact + delegated authority* ("may: open PR #88") before anything
happens (R-04 §7.3; P7). (c) **Approval in chat** keeps the human in the team's flow; agent volume otherwise
stays out of the main timeline (R-13 B.4).

**The "old stack can't do this" contrast (HOUSE STYLE over PROVEN doctrine).** Bolt-on bots in the stitched
stack act in *one* tool, leave no cross-surface trail, and bury their reasoning — an agent that opened a Jira
issue can't show you it also posted to Slack and pushed a branch as *one attributed chain*. Myelin's agent
rides the same ref graph + one identity as humans, so its work is **as legible and navigable as a person's**
(P7) — the integration that makes humans coherent makes agents *governable*.

**Unglamorous seam (§4).** Every partial-failure branch is a *designed* state, not a 500: gate-rejected →
PR discarded, issue stands, attributed; agent-error-mid-chain → completed steps stand (saga), "take it from
here"; budget-exceeded / loop-guard → paused with a recovery path; cross-cell effect → **Denied with the
missing grant named, never leaking the target** (R-04 §7.2; R-09 §5.8). The chain stays *coherent* even when
it breaks — the agent-wedge's negative-peak guard.

---

### W7 — Convert-this-to-an-issue without leaving the conversation

> **R-04:** F-PM-1 (incident-to-runbook). **Seam:** R-04 §8 "Paste Slack msg→Jira by hand". **Component:**
> a chip *created in place* (R-09 §2), optimistic (§4.2 rule 2).

**The moment.** A chat message says "checkout 500s spiking." You hover it, hit **Convert to issue** (or
`@myelin open an incident from this thread`), and an **issue is filed in a side pane without navigating
away** — the thread keeps a **live backlink** to the new issue, and closing the issue later **posts the
resolution back to the thread**. The conversation and the tracked work are *one timeline*, not two drifting
copies.

**Cross-surface mechanics (surfaced, PROVEN).** `issue.created` + `ref.created` (thread↔issue) on the bus;
the back-post on `issue.transitioned(resolved)` (R-04 §4.1 blueprint). The new chip is the standard
component created in-place (R-09 §2), optimistically (§4.2 rule 2).

**The design that makes it felt (HOUSE STYLE).** (a) **The seam dissolves *by staying put*** — the issue
opens in a side pane, the thread never scrolls away (R-04 §4.2: "the seam dissolves by staying put"). (b)
**The backlink is live and bidirectional** — the thread shows the issue's current state; the issue shows its
origin thread (W5). (c) **Optimistic** — the issue chip appears in the thread instantly; a backend reject
reverts it with the message text untouched (R-09 §4.2; R-13 A.3).

**The "old stack can't do this" contrast (PROVEN).** The stitched dance: copy the Slack message → open Jira →
file an issue → paste the Jira link back into Slack → and the thread and the issue immediately drift, because
nothing keeps them in sync (R-04 §4.1 / §8). Myelin: one action, a live two-way backlink, no copy-paste, no
drift. The wedge is *the second tool never opening*.

**Unglamorous seam (§4).** If you can post but not file in this project, the optimistic stub **reverts and
offers request-access** (R-04 §4.2; R-09 §4.2) — never a silent failure; the message you typed is preserved.

---

## 3. Anti-wedges — integration moments that are traps, not delight (HOUSE STYLE)

Phase-6 finalists must **not** gild these; they are where "integration" turns into noise (the negative-peak
risk, §1.2). Each is a falsifiable "don't":

| Anti-wedge | Why it's a trap | The rule |
|---|---|---|
| **Auto-expanding every ref to a fat card** | 5 refs → 5 fat cards = the Slack-unfurl-noise wall (R-01 §3.1) | compact chip by default; one card auto-expands (R-09 §7.2) |
| **Agent chatter in the main timeline** | integration at machine speed re-creates notification overload (R-13 B.4) | agent volume routed out of the main lane (P8; W3/W6) |
| **"Smart" surprise navigation** | a wedge that *moves the user* unasked breaks control/visibility (P3) | prefetch warms; it never auto-navigates (R-13 CA-* warms, doesn't jump) |
| **Celebratory confetti on completion** | calm is the default even at the payoff; confetti is the anti-aesthetic (R-12 §7.2) | inbox-zero is *quiet* ("caught up"), not a burst (R-13 B.5) |
| **A leaked title in any degraded state** | a single leak is a negative peak that poisons trust + a GDPR breach | no-access is non-leaking *by construction* (R-09 §5.4); §4 |
| **A chip that silently points at the wrong line after rebase** | fast-but-lying destroys the trust the wedge is built on | honest detach-to-pill, never silent mis-anchor (R-09 §5.9; W4) |

---

## 4. Completeness-critic (README §9) — gloss-risks this item must cover

R-22 is a *cross-artifact* item, so the §9 cross-surface gloss-risks are **load-bearing inside every wedge**
(a wedge that delights on the happy path but leaks/lies on the seam is a *negative* peak, §1.2). Coverage:

- **Cross-cell ref → no-access / tombstone (§9)** — **covered in every wedge's "unglamorous seam".** W1/W2
  render the **no-access card, never a leaked title** (R-09 §5.4 — non-leaking *by construction*: the
  resolver returns a tombstone before content crosses the wire); cross-cell shows the **residency tag** or
  the no-access card (R-09 §5.8). W5's backlink read is **leak-free by construction** (`reference-graph.md`
  §4 `list_objects` gate). W6's cross-cell *effect* is **Denied with the missing grant named** (R-04 §7.2).
  This is the wedge's load-bearing trust property, not a footnote: the integration that surfaces everything
  must *never* surface what you may not see.
- **Diff-anchor relocation after rebase (§9)** — **OWNED inside W4.** The content-anchored chip re-resolves
  to exact/rebased(moved)/partial(outdated)/content_gone(detach-to-pill); it **never silently jumps to a
  wrong line** (R-09 §5.9). Surfaced as *part of the love-moment* — a trustworthy chain beats a fast liar.
- **Erased / tombstoned (§9)** — covered: erased refs render the dignified tombstone (R-09 §5.7) inside
  W1/W2/W5; the **edge survives the target's tombstone** so the trail's integrity outlives the artifact
  (W5).
- **Notification storm / 30×-agent-surge (§9)** — covered in W3/W6: the agent lane sheds first, the
  human-direct inbox stays in budget (R-13 B.4); the *storm-state craft* depth → **R-21** (named-and-routed).
- **Degraded / fails-static (§9)** — covered: W1 fails static **per-slot** (R-13 B6); a degraded resolver
  renders last-known + "couldn't refresh" (R-09 §5.10).
- **Consciously routed (not re-specced here):** the HITL card depth → **R-14**; agent attribution/calm depth
  → **R-15**; the per-surface full state catalogue → **R-21**; the EXT-1 prefetch *build* → EXT-1. R-22
  *surfaces* these inside the wedges; it does not own their depth (the cumulative-corpus rule).

---

## 5. Actionability toward the control artifacts

| Control artifact | What this file equips | Where |
|---|---|---|
| **rubric D4** (one-product coherence, 14% — the central problem) | The wedges **are** the D4 proof in motion: each spans ≥3 subsystems rendered by the *same* shared component (R-09 §3.1), so "the five surfaces feel like one product" is demonstrated as a *flow*, not asserted. W1 (the context pane) is the canonical D4 set-piece. The §3 anti-wedges keep coherence from becoming clutter. | §2 (all), §3 |
| **rubric D10** (switch test, 8%) | Each wedge's **"old stack can't" contrast** is a switch-test argument: a team moving off Jira/Slack/Notion *gains* W1–W7 and *loses nothing* — because the wedges are the things the fragmented stack literally cannot do (R-04 §8; competitive §6.1). The unglamorous-seam guards (§4) ensure no *regression* hides inside a wedge (the D10 "no dead ends" bar). | §2 contrasts, §4 |
| **sketch-funnel — the wedge screen (comparable-screen #5)** | **Every finalist must include ≥1 of W1–W7 as its wedge screen** (sketch-funnel §"comparable screen set" #5). W1 (PR context pane) or W2 (live unfurl-in-thread w/ inline action) are the strongest single-screen demonstrations; W4 is the engineer-flagship alternative. The four-part anatomy (§1.3) is the *spec* for that screen. | §1.3, §2, §1.4 |
| **sketch-funnel Axis 5** (agent presence) | W6 lets finalists occupy different positions on how *present/legible* the agent's cross-surface chain is. | W6 |
| **Phase 6 / Phase 7** | The §1.4 index is the menu finalists pick from; the §3 anti-wedges are the gilding-guard Phase 7 critiques against; the §4 seam-guards are the unglamorous-state demonstrations a wedge screen must also show. | §1.4, §3, §4 |

---

## 6. `[DEFERRED-UNTIL-USERS]` — what these wedges have NOT earned

R-22 is `user-dep: none` — this expert love-moment catalogue (grounded in the cited flows, the PROVEN
component/prefetch contracts, peak-end + context-switch evidence) **is** the no-user substitute. But the
core bet — *that these specific moments are felt as delight, not merely as features* — is a HYPOTHESIS only
users settle. Recorded as executable plans, not faked:

- **`[DEFERRED-UNTIL-USERS]` — Does each wedge land as a *peak* (delight), or just register as "a panel"?**
  *Test:* per-segment first-encounter sessions on the Phase-6 finalist's wedge screen — engineers (P1–P5) on
  W1/W4, PMs (P6–P10) on W2/W3/W7, mixed on W6; capture spontaneous reaction + a delight/peak rating; run a
  **switch-test interview** (#24): "what would you miss moving back to your current tools?" *Falsifier:* the
  wedge elicits no peak reaction and isn't named when users recall what made the product feel different
  (peak-end fails) → it's a feature, not a wedge, and the peak budget is mis-spent.
- **`[DEFERRED-UNTIL-USERS]` — Is the live-not-snapshot quality (W1/W2/W5) *noticed*, and is it trusted?**
  *Test:* let a card flip state under the user's eye (W1 PR red→green); ask if they noticed and whether they
  trust the card as current. *Falsifier:* users treat the card as a stale snapshot anyway (the live peak is
  invisible) or distrust the in-place update (B5 motion reads as a glitch).
- **`[DEFERRED-UNTIL-USERS]` — Do the unglamorous seams (§4) read as intended, not as breakage?** Inherits
  R-09 §11 verbatim: the **no-access card** ("you lack access" vs "this is broken"), the **erased
  tombstone** (lawful degradation vs data-loss), the **rebase-relocated chip** (trusted "moved" pill vs
  distrusted anchor). *Falsifier:* a degraded seam reads as a bug → the negative-peak poisons the wedge.
- **`[DEFERRED-UNTIL-USERS]` — Does the agent chain (W6) read as *one legible story*?** *Test:* show the
  CI→issue→chat→PR chain; ask the user to narrate what the agent did and why. *Falsifier:* users can't
  reconstruct the chain or don't trust the attribution → legibility (P7) failed. **Caveat (from R-04 §11):**
  W6 is drawn against the *mock* agent runtime; the *contract* (one correlation_id, plan-then-apply,
  attribution) is designed to be legible regardless of runtime — that is what to validate, not the mock's
  outputs.
- **`[DEFERRED-UNTIL-USERS]` — Does residency-bought perceived speed make the wedges feel instant for a
  cross-region collaborator?** Inherits R-13 §6 (the largest open question): W1/W3/W4 depend on EXT-1
  prefetch within the residency boundary, *no* global replication of personal data (P2/ADR-11). *Falsifier:*
  the assembled-context wedges feel slow cross-region → the perceived-perf strategy needs rethinking *within*
  the constraint. **EXT-1 must be scheduled** for W1/W3/W4 to be testable beyond mock.
- **Method:** per-segment RITE + switch-test interview on the Phase-6 finalist wedge screen, run over the
  F-ENG-1 (W1/W4), F-PM-1 (W2/W3/W7), and F-AGT-1 (W6) flows. Until then, treat the *mechanics* as PROVEN
  (surfaced contracts) and the *felt delight* as HYPOTHESIS.

---

## 7. Sources (web-verified 2024–2026 + surfaced contracts)

**External (cited URLs):**
- **Peak-end rule** (delight peaks + endings dominate memory of an experience; Kahneman lineage) —
  https://lawsofux.com/peak-end-rule/ · https://www.ux-bulletin.com/peak-end-rule-ux-designing-memorable-experiences/
  · https://medium.com/nudge-notes/the-peak-end-rule-crafting-memorable-user-experiences-bf9a93de0056
- **Context-switching / tool-toggling cost** (the measured pain each wedge inverts: ~1–2 hrs/day lost,
  ~tens of tool-switches/hour, ~9 apps/day, ~23-min refocus) —
  https://www.hivel.ai/blog/context-switching-crisis-quantifying-the-cost-and-finding-solutions ·
  https://asana.com/resources/context-switching ·
  https://super-productivity.com/blog/context-switching-costs-for-developers/ `[VERIFY]` exact magnitudes
  (practitioner-synthesised industry data, not a single RCT).
- **Slack unfurl = cached snapshot + private-link 404 + preview-not-action** (the W1/W2 "old stack can't"
  contrast) — https://api.slack.com/reference/messaging/link-unfurling ·
  https://www.fuzzyraygun.com/blog/fixing-preview-or-unfurl-in-slack (cache cleared ~30 min; re-post won't
  re-unfurl; 403/404 on private links). Prior R-01 §3.1 cashed this out; not re-listed.

**Surfaced Myelin contracts (PROVEN-as-existing, not invented):**
- **R-04** cross-surface-flows (the flows + §8 seam register); **R-09** reference-unfurl (the component +
  resolver→UI state contract, live-not-snapshot, no-access/erased/cross-cell/rebase-orphan states); **R-13**
  perceived-performance (CA-1..4 prefetch + EXT-1, the PR-context-pane skeleton, latency budgets, residency
  caveat, B.4 storm/B.5 inbox).
- `system-overview.md` §8.1 (the PR context pane sequence — W1); `reference-graph.md` §3.5 (content-anchored
  line-ranges — W4), §4 / C-4 (event-sourced, `list_objects`-gated leak-free backlinks — W5); design-language
  P6 (reference everything), P7 (agents legible), P8 (calm), §5.11 (backlinks panel), §8b.6 (system assembles
  + pre-fetches context); `competitive-landscape.md` §6.1 (integration *is* the differentiator; the
  "stitched, not unified" failure); EXT-1 `extension-planning/perceived-performance.md` (context bundling /
  prefetch).

---

## 8. Self-check against R-22 acceptance criteria

| Criterion (prompt R-22) | Status | Evidence |
|---|---|---|
| **≥5 named wedge moments** | ✅ Met | W1–W7 (W1–W5 are the required five; W6–W7 extend) — §2, index §1.4 |
| **Each with cross-surface mechanics (the events/refs)** | ✅ Met | every W-entry "Cross-surface mechanics" part (surfaced from §8.1, R-09 resolver, R-13 CA-*/EXT-1, bus, agent-fabric) |
| **Each with the design that makes it felt (not buried)** | ✅ Met | every W-entry "The design that makes it felt" part; staged as peaks per §1.2 (peak-end) |
| **Each with the "old stack can't do this" contrast** | ✅ Met | every W-entry "old stack can't" part; grounded in R-04 §8 + competitive §6.1 + cited Slack/context-switch sources |
| **Each maps to a real R-04 flow AND a real R-09 component** | ✅ Met | §1.4 index columns + per-entry header line (flow · seam row · component/state) |
| **Designed as deliberate love-moments, not a feature list** | ✅ Met | §1.1 definition (3 necessary properties) + §1.2 peak-end doctrine + §3 anti-wedges (the gilding-guard) |
| **Usable as the Phase-6 wedge-moment screen** | ✅ Met | §5 (funnel comparable-screen #5); §1.3 four-part anatomy = the screen spec; §1.4 the finalist menu |
| **Feeds rubric D4 / D10** | ✅ Met | §5 (D4 = wedges-as-coherence-in-motion, W1 canonical; D10 = the "old stack can't" contrasts + §4 no-regression guards) |
| **Covers §9 gloss-risks (cross-cell no-access/tombstone, diff-anchor relocation)** | ✅ Met | §4 (cross-cell no-access non-leaking-by-construction in every wedge; W4 OWNS diff-anchor relocation; erased/storm/degraded covered-and-routed) |
| **PROVEN/HOUSE-STYLE tags + date + cited URLs** | ✅ Met | tagged throughout; dated 2026-06-20; §7 URLs (peak-end, context-switch, Slack) + surfaced contracts |
| **Builds ON R-04/R-09/R-13, doesn't duplicate** | ✅ Met | §0 + per-entry cross-refs (flows by ID, component states by §, prefetch by CA-#); no blueprint/component re-spec |
| **Deferred validation recorded as a plan, not faked** | ✅ Met | §6 (`[DEFERRED-UNTIL-USERS]`: peak-landing, live-noticed, seams-read-right, agent-chain-legible, residency-speed — each with falsifier + method) |
| **Self-check restating acceptance criteria** | ✅ Met | this table |

**Top uncertainties (honest, per VISION §3):**
1. **The central bet is unproven by definition:** *whether these specific moments are felt as delight (peaks)
   rather than registered as features* is a HYPOTHESIS (§6) — the peak-end evidence is PROVEN, but "W1 is a
   peak *for these users*" is taste until per-segment first-encounter testing. This is the load-bearing risk
   of a "delight" deliverable.
2. **Several wedges depend on EXT-1 being built** (W1/W3/W4 prefetch/bundling). The mechanics are
   residency-correct by construction, but the *felt-instant* quality — especially cross-region without global
   replication (P2/ADR-11) — is inherited from R-13 §6's largest open question, and is mock until EXT-1 ships.
3. **W6 is drawn against the mock agent runtime** (inherited R-04 §11): the legibility *contract* (one
   correlation_id, plan-then-apply, attribution) is designed to hold regardless of runtime, but whether a
   real-LLM chain reads as "one legible story" is a HYPOTHESIS.
4. **Which wedge is the *strongest* single-screen funnel demonstration is a HOUSE-STYLE call** (W1 vs W2 vs
   W4) — finalists may differ, and Phase 7 should not penalise a defensible alternate pick.

---

*End of R-22 deliverable. Date: 2026-06-20. Wedge-moment design HOUSE STYLE over the PROVEN reference-graph /
prefetch-bundling / cross-surface-flow contracts (R-04 / R-09 / R-13) + cited peak-end, context-switch, and
Slack-unfurl sources; surfaces existing mechanics, does not redesign them; not user-validated — see §6.
Builds on R-04, R-09, R-13. Feeds rubric D4/D10, the sketch-funnel wedge screen, Phase 6, Phase 7.*
