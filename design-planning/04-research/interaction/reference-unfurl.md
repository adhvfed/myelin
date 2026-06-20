# R-09 — Reference Chip + Artifact Unfurl Interaction Spec (the wedge component)

> **Phase 4 research corpus** · deliverable of prompt **R-09** (WS-D, Seq #9). **File date: 2026-06-20.**
> Methods: **#2 (Slack/GitHub unfurl teardown bar)**, **#9 (the full state set IS the point)**, **#8b
> (live-projection, humanised strings)**. This specs **the single most important shared component in the
> platform** (design-language §5.3 / P6) — the literal embodiment of the cross-artifact thesis: the
> reference chip + the rich unfurl card.
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = a cited external standard/source, OR an existing
> Myelin architecture contract this file *surfaces* (the `myelin-refs` resolver, ADR-13/03/12) — not
> invented. **HOUSE STYLE** = our interaction-design synthesis/taste. `[VERIFY]` = time-sensitive
> (vendor agent/AI behaviour). The component is **not user-validated**; the deferred validation is in §11.
>
> **Builds ON prior `04-research` (does not duplicate):**
> - [R-01 teardown-dossier](../north-star/teardown-dossier.md) §3.1 (Slack unfurl — live-vs-snapshot,
>   permission-leak, noise traps), §2.4 (Notion mention chip), §4.1/§4.4 (GitHub `#123` / Checks panel).
>   This file is where R-01's "Myelin beats Slack on three axes" claim is *cashed out*.
> - [R-06 platform-ia](../ia/platform-ia.md) §5 (the `ArtifactRef` / `myelin://` address spine + the four
>   resolution guarantees + §5.4 cross-cell rule) — the chip **is the address scheme made visible**.
> - [R-04 cross-surface-flows](../jtbd-flows/cross-surface-flows.md) — the flows the chip *threads*
>   (F-ENG-1 link-issue + diff-anchor; F-ENG-2 the 4-subsystem live-backlink chain; F-PM-1 runbook
>   unfurl-in-thread; F-GOV-1 erased/tombstone). Where this file says "see F-X", that is R-04's flow.
>
> **Surfaces, does NOT redesign, the backend:** the `myelin-refs` resolver
> ([reference-graph.md](../../../planning/05-refined-shared-systems-architecture/reference-graph.md)) — its
> **one `resolve(ref, viewer, mode)` ladder** (§4.6: `permission → root → sub{live/moved/outdated/gone} →
> erased`), the **content-anchored Git line-range** states (§3.5: exact/rebased/partial/content_gone), the
> **cell-local cross-cell** rule (C-5/§4.2), the **frozen `#sub` grammar** (§3.5), the **projection cache**
> (§3.6), and **humanised display-name resolution at source** (design-language §8b.5). Every chip/unfurl
> state below is a **rendering of a resolver state that already exists** — the UX job is to make each state
> legible, calm, and non-leaking, not to invent resolution.

---

## 0. How to read this file

Eight parts:

1. **§1** — the one-paragraph model (what a chip/unfurl *is*) + the resolver→UI state contract (the spine).
2. **§2** — the **compact chip** form: anatomy, the 3 interaction affordances (open/peek/act), per-type.
3. **§3** — the **rich unfurl card** form: anatomy + **a card spec per artifact type** (PR / issue / doc /
   run / thread / person-or-agent).
4. **§4** — the **inline-action surface** (re-run/transition/approve) + the permission behaviour.
5. **§5** — **every state** (the §9 unglamorous-states owner): live · peeking · loading · no-access ·
   moved · outdated · tombstoned/erased · cross-cell · rebase-orphaned · degraded · agent-pending.
6. **§6** — humanised strings (no raw ids) + the live-not-snapshot mechanism (surfaced).
7. **§7** — where it recurs (the recurrence matrix) + density/calm rules per surface.
8. **§8** a11y (G1) · **§9** rubric/funnel actionability (D4/D8/G1) · **§10** completeness-critic §9 ·
   **§11** `[DEFERRED-UNTIL-USERS]` · **§12** sources · **§13** self-check.

---

## 1. The model — what a chip/unfurl is, and the resolver→UI contract

> **A reference chip and an unfurl card are the *two render densities of one resolved `ArtifactRef`*.** You
> place a reference once (an `@mention`, a `#issue`, a pasted `myelin://` / web URL, a `/embed`); the
> platform resolves it **live, per viewer, every render**, and paints it **compact (chip)** inline or
> **rich (card)** when there's room or on demand. There is **one component**, parameterised by *density*
> (chip ↔ card), *artifact type* (PR/issue/doc/run/thread/principal), and *resolver state*. *(HOUSE STYLE
> framing over the PROVEN ADR-13 / `myelin-refs` resolver.)*

This is the embodiment of **P6 (reference everything)** and **P1 (one chip everywhere)**: the chip you see,
the URL in the address bar, and the handle pasted in the CLI are **the same identity** (R-06 §5; PROVEN
ADR-13).

### 1.1 The resolver → UI state contract (the spine — PROVEN, surfaced not invented)

Every chip/unfurl is the rendering of **exactly one** state returned by `resolve(ref, viewer, mode)`. The UI
**must not invent states the resolver doesn't return, and must render every state it does** — that
correspondence is the correctness guarantee (`myelin-refs` §4.6; D-9 drill). The frozen ladder:

| # | Resolver returns (`myelin-refs` §4.6) | Chip rendering (§2/§5) | Card rendering (§3/§5) | §9 unglamorous? |
|---|---|---|---|---|
| 1 | **`Tombstone{reason: denied}`** (permission fail, step 1) | "no-access" chip — type icon + *"Restricted"*, **no title** | "no-access" card — *"You don't have access to this {type}."* + request-access path | ✅ permission-denied |
| 2 | **Projection — LIVE** (step 3) | type icon + current title + status hint | full card (§3) | default |
| 3 | **Projection + `moved`** (step 3; Git rebased range / KN block moved) | title + a *"moved"* pill | card + a *"moved to current location"* banner | ✅ moved/outdated |
| 4 | **Projection(partial) + `outdated`** (step 3; Git partial range / KN edited block) | title + an *"outdated"* pill | card showing the surviving part + *"some content has changed"* | ✅ moved/outdated |
| 5 | **`Tombstone{reason: sub_gone, root}`** (step 3) | *"{root title} — that part is gone"* (root carried) | card of the **parent**, with *"the referenced section no longer exists"* | ✅ tombstoned |
| 6 | **`Tombstone{reason: root_gone}`** (step 2) | *"{type} no longer exists"* | *"This {type} has been deleted."* | ✅ tombstoned |
| 7 | **`Tombstone{reason: erased}`** (step 4; crypto/pseudonym shred) | *"Erased"* + type icon (GDPR-aware) | *"This {type} was erased under a data-rights request."* | ✅ erased |
| 8 | **Cross-cell projection** (C-5; resolved in home cell) | normal chip + a tiny **residency tag** (P9) | normal card + *"lives in {region}"* footnote | ✅ cross-cell |
| 9 | **Resolver unreachable / fail-static** (substrate degraded) | last-known chip + a *"can't refresh"* dot | card frozen at last projection + *"showing last known — couldn't refresh"* | ✅ degraded |

> **The load-bearing rule (HOUSE STYLE, from the resolver's correctness invariant):** the **denied** and
> **erased** states are *projections returned by the resolver after the permission/erasure check* — the UI
> **never receives a title it isn't allowed to show**. A no-access chip cannot leak because there is nothing
> to leak: `resolve` already collapsed it to a tombstone *before content crossed the wire* (`myelin-refs`
> §4.2 step 2). This is why the chip is non-leaking **by construction**, not by frontend discipline — and it
> is exactly where Slack/Notion are *structurally* weaker (R-01 §3.1/§2.4).

---

## 2. The compact reference chip (inline form)

### 2.1 Anatomy (HOUSE STYLE over §5.3)

`[ {type-icon} {status-glyph?} {humanised title / display key} {state-pill?} ]` — one inline run, baseline-
aligned with surrounding text, **never taller than a line of body text** (so a chip-dense paragraph stays
readable — the calm rule, P8). Components:

- **type-icon** — the one canonical icon per type (PR / issue / doc / run / commit / thread / person / agent),
  **shared with the views component, the palette, and the breadcrumb** (P1 coherence). The icon is the
  **persistent identity** that never varies with the persona-vocabulary lens (R-06 §6.3 bounding rule).
- **status-glyph** — for stateful types: PR open/merged/closed, issue state, run pass/fail, check status.
  **Glyph + label/position, never colour alone** (G1; §8b.3; R-01 §4.4 trap).
- **humanised title / display key** — the *current* title (PR/issue/doc) or the render-time display key
  (`#1421`, `@alice`, `~general`) projected per viewer per locale (PROVEN — `myelin-refs` §4.8 / §8b.5).
  **Never a raw URN, never a bare id.** Truncates with ellipsis + full text in the title attribute and the
  peek (§5.2); truncation must survive German +35% expansion and non-Latin scripts (G2; R-18).
- **state-pill** — present *only* in non-live states (moved / outdated / no-access / erased) — §5.

### 2.2 The three affordances (the chip's whole interaction surface)

1. **Open (primary)** — click / `Enter` when focused → navigate to the artifact (or open it in the context
   pane if the shell offers one, R-06 §3.4). The chip is a link with `role`/semantics of a link.
2. **Peek (hover *or* focus)** — reveals a **lightweight hovercard** (a small unfurl, §3.6) **after a short
   delay (~300ms HOUSE STYLE) on hover, immediately on keyboard focus**. The hovercard is **dismissable,
   hoverable, persistent** (PROVEN — WCAG 2.2 SC 1.4.13: dismiss via `Esc` without moving pointer/focus;
   pointer can move *onto* the card without it vanishing; stays until trigger removed/dismissed/invalid).
3. **Act (secondary, permission-gated)** — a small overflow/`···` or a long-press surfaces the **inline
   actions** (§4) *iff* the viewer is permitted; otherwise the affordance is **absent, not disabled-greyed**
   (don't advertise a power the viewer lacks; this also avoids leaking that an action exists). HOUSE STYLE.

### 2.3 Per-type chip (current-state hint table)

| Type | Status glyph + label | Display title | Notes |
|---|---|---|---|
| **PR** | open / draft / merged / closed (glyph + word) + checks roll-up dot (✓/✗/⏳ + count) | PR title | checks roll-up is the §4.4 GitHub-checks pattern in miniature |
| **Issue** | workflow state (e.g. "In progress") | issue title; key `#1421` projected | state colour-coded **and** labelled; vocabulary-lens label, canonical icon (R-06 §6.3) |
| **Doc / block** | — (docs have no run-state) | page / block heading | `b<id>` block anchor → "{page} › {heading}" |
| **CI run / step** | pass / fail / running (glyph) + duration | run name or `{pipeline} #{n}` | `step-<n>` / `check-<context>` jump-to-failure anchor (C-6) |
| **Commit** | — | short message + 7-char prefix (display projection) | `myelin-refs` §4.8 |
| **Thread / message** | — | "{channel}: first-line…" | `message-<id>` / `thread-<id>` anchor |
| **Person / Agent** | online/away (person); **agent treatment** (agent) | `@name` | agent chip carries the §3.2 agent badge — **never colour-alone, color-blind-safe** (P7; R-14) |

---

## 3. The rich unfurl card (per artifact type)

### 3.1 Card anatomy (shared shell, type-specialised body — HOUSE STYLE over §5.3)

```
┌───────────────────────────────────────────────────────────┐
│ {type-icon} {humanised title}            {status badge}    │  ← header (always)
│ {scope breadcrumb: org › repo/space/channel}  {residency?} │  ← context line
├───────────────────────────────────────────────────────────┤
│ {type-specific body — see §3.2}                            │  ← body
├───────────────────────────────────────────────────────────┤
│ {inline actions, permitted only — §4}    {open ↗}          │  ← action bar
└───────────────────────────────────────────────────────────┘
```

The **header + context line + action bar are identical across all types** (the coherence test, D4); only the
**body** varies. This is what makes "a PR unfurl in chat" and "a doc unfurl in an issue" feel like the same
component (P1). The card is **collapsible**; in dense streams it defaults **collapsed to the chip**, expanding
on demand or inline (§7 calm rule). *(HOUSE STYLE.)*

### 3.2 The card body per type (the projection fields each `project(ref, viewer)` returns)

These bodies render the `{title, state, icon, render_hint, sub_anchor?}` projection the owning subsystem
returns (PROVEN — `myelin-refs` contract 5.6) plus the lifecycle/reference edges the resolver exposes. We do
**not** define new backend fields.

| Type | Card body (current, live) | Grounded in |
|---|---|---|
| **PR** | branch → base; **checks roll-up** (required vs optional, pass/fail/pending, per-check rows on expand); reviewers + review state; linked issue/doc chips (the wedge); +N/−M lines; mergeability verdict | R-01 §4.1/§4.4 (PR overview + Checks API); F-ENG-1 |
| **Issue** | state · assignee · priority · cycle/sprint; sub-issue progress; linked PRs/docs chips; SLA timer if any; last activity | R-01 §1.3; design-language §7.3 |
| **Doc / block** | page title + breadcrumb; **the anchored block/section excerpt** (the `b<id>`/`h<id>` body); backlinks count; last-edited | R-01 §2.1/§2.2; F-PM-1 (runbook unfurl-in-thread) |
| **CI run / step** | run status + duration; the DAG/step roll-up; **the failing step + its log tail when `step-<n>`-anchored** (jump-to-failure, C-6); re-run affordance (§4) | R-01 §4.4; F-ENG-1 |
| **Thread / message** | channel + participants; the anchored message excerpt; reply count; unread state | R-01 §3.3; design-language §7.5 |
| **Person / Agent** | name + role/team (person); **for an agent**: the agent treatment, its scope/authority summary, "what it can do" — links to the governance console (R-14/R-15) | design-language §5.11/§6; P7 |

### 3.3 The hovercard (the peek) is a *bounded* unfurl
The hover/focus peek (§2.2) renders the **header + a 1–3 line body summary** — not the full action bar (an
action bar inside a transient hovercard is a 1.4.13 hazard and a mis-click hazard). To *act*, the user opens
the full card or the artifact. *(HOUSE STYLE; conforms to WCAG 1.4.13 hoverable/persistent.)*

---

## 4. The inline-action surface (act where permitted)

### 4.1 What it is (PROVEN-direction — R-01 §3.1: Myelin's beat over Slack is "inline actions, native")
The unfurl is **an action surface, not just a preview** (design-language §5.3; R-01 §3.1). A *subset* of each
artifact's actions appears in the card's action bar — the **high-value, low-risk, in-flow** ones:

| Type | Inline actions (the curated subset) | Risk class |
|---|---|---|
| **CI run / step** | **Re-run** (all / failed only) ; view full log | low — grounded in GitHub "Re-run failed checks" (PROVEN) |
| **Issue** | **Transition state** (status dropdown) ; assign to me ; add to my cycle | low/medium |
| **PR** | **Approve / Request changes / Comment** ; re-request review ; (merge only from the full PR, never the chip) | medium → **approve routes through the HITL/review shape (R-14)** |
| **Doc** | comment on the block ; copy ref | low |
| **Thread** | reply ; react ; mark read | low |
| **Agent proposal** | **Approve / Edit / Reject** (this is the §5.4 HITL card *as* an unfurl — R-14 owns) | high — plan-then-apply gate |

### 4.2 The four hard rules for inline actions (HOUSE STYLE; PROVEN where cited)

1. **Permission-gated by *presence*, not by greying.** An action the viewer can't perform is **absent**, not
   shown-disabled (don't advertise unavailable power; don't leak that the gate exists). The action set is
   itself permission-pre-filtered (PROVEN — ADR-03 `list_objects`; same guarantee as the palette, R-08).
2. **Optimistic + honest rollback.** The action applies optimistically in-card (state flips instantly,
   P2/§8b.6); on backend reject the card **reverts and shows one quiet line** ("Couldn't re-run — you have
   view-only access") — the optimism-for-latency / honesty-on-failure rule (R-01 §1.2; D8). Mirrors F-PM-1's
   optimistic-rollback branch.
3. **Consequential / irreversible / agent / GDPR actions confirm; the rest just undo.** Re-run, react,
   assign → reversible, no dialog (reversibility-over-confirmation, §8b.6). Approve-a-PR, transition-to-Done,
   approve-an-agent-effect → these route to the **§5.4 / R-14 HITL confirm shape** (plan-then-apply). A
   human's inline suggestion and an agent's proposal are **the same "approve a proposed effect" shape** (P1
   coherence; R-01 §4.3).
4. **The action carries the chip's provenance.** Acting from an unfurl is attributed and audit-linked
   identically to acting on the artifact's own surface (P7; the action is on the `ArtifactRef`, not a copy).

---

## 5. Every state (this component OWNS several §9 unglamorous states)

Each state below = a resolver state (§1.1) rendered at **both** densities. **Acceptance-critical: all present,
incl. no-access, tombstoned, moved/outdated, cross-cell, rebase-orphaned.**

### 5.1 Live (default)
The §1.1 row-2 projection, kept fresh by bus update events (§6.2). This is the **default and the differentiator**
(R-01 §3.1 — Slack's is a fetch-time snapshot; ours is current). Chip = §2; card = §3.

### 5.2 Peeking (hover/focus hovercard)
§3.3 bounded unfurl. **WCAG 2.2 SC 1.4.13** dismissable (`Esc`, no pointer move) / hoverable (move onto card)
/ persistent (no auto-timeout) (PROVEN). Keyboard focus shows it immediately; `Esc` dismisses; `Enter` opens.

### 5.3 Loading (resolving / projection-cache miss)
**Structure skeleton, never a blank spinner** (§8b.6; D8): chip → a pill with the type-icon + a shimmer where
the title will land (icon resolves first, it's known from the ref's `<type>`). Card → header skeleton + body
line skeletons matching the type's final layout. Suppress flash if it resolves < ~1s (§8b.6). The icon is
**known before resolution** (it's in the URN `<type>`), so even loading is type-true (HOUSE STYLE).

### 5.4 No-access (permission-denied) — `Tombstone{denied}` (OWNED §9 state)
**Graceful, never a leaked title** (PROVEN — `myelin-refs` §4.2 step 2; ADR-03; design-language §5.3 hard
rule). Chip: `{type-icon} Restricted`. Card: *"You don't have access to this {type}."* + a **request-access**
path where the policy allows one (HOUSE STYLE; F-ENG-2 "A linked decision you can't access"). **No title, no
snippet, no metadata** — the resolver returned a tombstone, so there is nothing to leak. The *type* is shown
(it's in the URN, not sensitive); if even the type is sensitive in a context, the card degrades to a neutral
"Restricted reference." This is the **beat over Slack/Notion** R-01 flagged.

### 5.5 Moved — `Projection + moved`
Live projection at its **current** location + a *"moved"* pill (chip) / banner (card): *"This was relocated;
showing the current version."* For Git: the content-anchored line-range was found at a shifted position via
3-way context match (PROVEN — `myelin-refs` §3.5 rebased). **The reference followed the content** — no dead
link. (F-ENG-2 "moved to ISSUE-413".)

### 5.6 Outdated — `Projection(partial) + outdated`
Some anchored content survives, some is gone. Chip: *"outdated"* pill. Card: shows the **surviving** part +
*"some referenced content has changed since this was linked."* For Git: the partial line-range (PROVEN —
§3.5 partial). Never silently re-anchor to wrong content (the GitHub failure mode F-ENG-1 calls out).

### 5.7 Tombstoned / erased (OWNED §9 state) — `Tombstone{sub_gone | root_gone | erased}`
The **GDPR-aware degraded state** (PROVEN — `myelin-refs` §4.6 steps 2/3/4; ADR-12; design-language §5.3 hard
rule "tombstones gracefully"). Three flavours, each **carrying the root** so the reference degrades to context,
never vanishes:
- **`sub_gone`** — *"{parent title} — the referenced section no longer exists"* (card shows the parent).
- **`root_gone`** (deleted) — *"This {type} was deleted."*
- **`erased`** (crypto/pseudonym-shred under a DSR) — *"This {type} was erased under a data-rights request."*
  **No content, no name, no recoverable PII** (PROVEN — drill D-5: 0 recoverable PII). This is the
  sovereignty-as-UX moment (P9; R-19): the tombstone is **honest and dignified**, not a broken-image icon.
The edge itself is preserved for graph integrity (F-ENG-2) — the tombstone is a *render*, not a deletion of
the link.

### 5.8 Cross-cell (OWNED §9 state) — resolved in the home cell (C-5)
A ref to an artifact homed in another residency cell/tenant resolves **cell-locally**: the home cell
permission-checks and renders; only the **already-filtered projection or a tombstone** crosses — never raw
rows, never PII (PROVEN — `myelin-refs` §4.2 / C-5; ADR-11 no-cross-region-PII). UI: a **normal chip/card**
*if visible*, plus a tiny **residency tag** ("lives in `eu-west`", P9 — the always-on cue, R-06 §5.4 / R-19);
**else the §5.4 no-access card** — *never a raw id, never the title* (R-06 §5.4; F-ENG-1 cross-cell branch).
A public-OSS cross-tenant ref resolves via the `public` userset only (PROVEN — `myelin-refs` §6.4); the
*inbound-visibility policy* is an open legal/product question (R-06 §6.3 / R-19), but the **structural
no-leak floor ships regardless**.

### 5.9 Rebase-orphaned diff-line chip (OWNED §9 state — the hardest case)
A chip/comment anchored to `#L42-L88` of a PR diff (PROVEN anchoring — `myelin-refs` §3.5 content-anchored
BLAKE3 fingerprint + 3-way context match). After a rebase/force-push the resolver returns one of four states;
the UI renders each (this is **the** state R-01 §4.2 and F-ENG-1 flagged GitHub gets wrong):
- **exact** → live chip at the line.
- **rebased** → §5.5 *moved* — the chip **relocates to the shifted line** (the content moved, the anchor
  followed). HOUSE STYLE UX: a subtle "moved" pill so the reader knows it relocated.
- **partial** → §5.6 *outdated* — anchored to the surviving sub-range.
- **content_gone** → the comment/chip **detaches to an "outdated — was on former line N" pill** and lifts
  to the file/conversation level; it **never silently jumps to a wrong line** (the explicit anti-pattern;
  F-ENG-1). The thread is preserved; the *anchor* is honestly marked stale.

### 5.10 Degraded (resolver unreachable / fail-static)
Substrate degraded: render the **last-known projection** from the projection cache (PROVEN — `myelin-refs`
§3.6 bounded cache) + a quiet *"showing last known — couldn't refresh"* dot; **fails static for this chip
only**, never a page error (§8b.6 "fails static"). Inline actions disable (with the honest line) until refresh.

### 5.11 Agent-pending
The unfurl *is* the agent surface when the referenced thing is an agent proposal: it carries the
**agent treatment** (color-blind-safe, never colour-alone — P7/R-14) and an *"awaiting your approval"* /
*"agent working"* state, with Approve/Edit/Reject (§4 / §5.4 / R-14 owns the HITL depth). Agent-authored
unfurls default **out of the main timeline** (threads/inbox) to keep volume calm (P8/§6.5; R-15).

---

## 6. Humanised strings + the live-not-snapshot mechanism (surfaced, not redesigned)

### 6.1 Humanised strings (no raw ids) — PROVEN, sourced at the backend
**The chip never shows a raw machine string.** Display-name resolution lives **at the source** — Reference-
Graph display-name resolution + Notifications templating (PROVEN — design-language §8b.5; `myelin-refs` §4.8
render-time display projection). So `#1421`, `@alice`, `~general`, "merged", "In progress" are all *projected*
per viewer per locale; the frontend owns **no** humanisation lookup and **no** id→name map. This kills the #1
"feels unfinished" tell (`merge_request merged`, raw ids) for **every** consumer for free (§8b.5). Locale-aware
(R-18/G2); humanised strings must survive expansion + non-Latin scripts.

### 6.2 Live, not snapshot — the mechanism (PROVEN, surfaced)
The unfurl is a **current projection**, kept fresh by **bus update events** (cache invalidation per
`ArtifactRef`) (PROVEN — design-language §5.3 hard rule; `myelin-refs` §4.2 step 4 / §3.6 / §4.3
`refs-projection-invalidator` busts on `*.updated`/`*.erased`). UX consequence: a PR chip that flips
red→green, an issue that transitions, a doc that's edited — **the chip updates in place** (a live-update
microinteraction, R-12). This is the **correctness-and-erasure-safety** reason live is the default (R-01 §3.1):
a snapshot can show content the target later restricted/erased — a GDPR leak; a live projection can't, because
the next resolve returns the tombstone. **Default live; snapshot is never an option for the chip.**

---

## 7. Where it recurs + the density/calm rules

### 7.1 Recurrence matrix (PROVEN — §5.3 "everywhere content lives" + R-04 flows)
The chip/unfurl is the same component in **every** surface, because `mention`/`artifact_ref`/`embed` are
structured nodes in the shared content model (PROVEN — ADR-05; `myelin-refs` §4.1):

| Surface | Densest form | Grounded in |
|---|---|---|
| **Chat messages** | inline chips; unfurl-in-thread (calm); the densest unfurl surface | R-01 §3.1; F-PM-1 (runbook unfurl-in-thread) |
| **Issue body / comments** | chips for linked PRs/docs/runs | F-ENG-1 (link-issue) |
| **Knowledge blocks** | chips + **live embeds** (a `/embed` of an issue board over an `ArtifactRef`) | R-01 §2.2/§2.3; design-language §5.9 |
| **PR description / review** | linked issue/doc/run chips = the **PR context pane** wedge | R-01 §4.1; system-overview §8.1; R-22 |
| **CI annotations** | step/check chips with jump-to-failure | C-6; F-ENG-1 |
| **Notifications inbox** | the chip + "why it fired" provenance | R-10 inbox; §8b.6 |
| **Context pane (shell)** | the backlinks/linked-artifacts list = chips | R-06 §3.4 |
| **CLI** | textual rendering of the same ref (`myelin://` handle) | R-06 §7.7 |

### 7.2 Density / calm rules (HOUSE STYLE — P8)
- **Compact-chip by default; expand on demand.** A message with 5 refs is 5 chips, not 5 fat cards (the R-01
  §3.1 "unfurl noise" trap — Slack's 5-fat-cards-per-message wall). Unfurl-to-card only on hover/focus/click
  or when the author explicitly embeds.
- **One card max auto-expanded per message** (HOUSE STYLE) — the first/primary ref; the rest stay chips.
- **Live embeds (knowledge) are framed and labelled** as live ("live · updated 2m ago") so the reader knows
  it's a projection, not a paste.
- **Agent-authored refs route out of the main timeline** (P8/§6.5; R-15).

---

## 8. Accessibility (G1) — the chip/unfurl is a named hard component

This component is on the rubric **G1** hard-component list implicitly (it's everywhere) and must demonstrate
(each PROVEN against its criterion):

- **Hovercard = WCAG 2.2 SC 1.4.13** dismissable / hoverable / persistent (§5.2; PROVEN — W3C Understanding
  1.4.13). Peek opens on **focus**, not hover-only (keyboard + touch parity).
- **Status never by colour alone** — every status-glyph carries glyph/label/position (§2.1; G1; §8b.3). The
  **agent treatment is color-blind-safe** (P7; R-14).
- **Visible focus** on the chip (it's a link) and on each inline action, in every theme (G1 focus-token).
- **Keyboard-operable, no trap** — `Tab` to chip, `Enter` opens, focus surfaces the peek, `Esc` dismisses the
  peek and returns focus to the chip; inline actions are reachable and a logical tab order (G1).
- **Semantics** — chip = `link` role with an accessible name = the humanised title + type ("Pull request:
  Fix login, merged"); the hovercard/card use the right overlay ARIA (R-10 overlay primitives; portal +
  focus management). **Live-region announcement** of an in-place live update **without spamming** (announce
  state *changes* a viewer is watching, not every background refresh — G1; §5.10 R-21).
- **Reflow/zoom + i18n** — chip truncation and card layout reflow at 200%/320px (G1); survive German +35%
  expansion, Greek/Cyrillic, and **RTL mirroring** (the whole card mirrors via logical properties — G2;
  R-18). The residency tag and status pills must not clip under expansion (§8b.4 fixed-width bug class).

---

## 9. Actionability toward the control artifacts

| Control artifact | What this component equips | Where |
|---|---|---|
| **rubric D4** (one-product coherence — the central problem, 14%) | The chip/unfurl is **the** coherence proof: the *same* component renders a PR in chat, a doc in an issue, a run in the PR pane. The D4 test "open the same chip in Code and Chat — identical?" (R-06 §4 invariant) is literally this spec. | §1, §3.1, §7 |
| **rubric D8** (perceived performance) | Loading = structure skeleton not spinner (§5.3); inline actions optimistic + honest rollback (§4.2); live-update-in-place (§6.2). | §5.3, §4.2, §6.2 |
| **rubric G1** (accessibility floor) | §8 — 1.4.13 hovercard, status-not-colour-alone, keyboard/focus/semantics, reflow/RTL. Checkable, not aspirational. | §8 |
| **sketch-funnel comparable screens** | This is the **wedge moment** every finalist must show (R-22): the live, permission-aware, in-place unfurl with an inline action. | §1, §5, §7 |
| **sketch-funnel Axis 6 (sovereignty visibility)** | The residency tag on cross-cell chips (§5.8) + the dignified erased tombstone (§5.7) are Axis-6 cues at the artifact level. | §5.7, §5.8 |
| **R-22 (wedge moments) + R-21 (state craft)** | R-22 reuses §5/§7 (live unfurl-in-thread, cross-subsystem backlink); R-21 inherits §5's state set per-surface. | §5, §7 |

---

## 10. Completeness-critic (README §9) — gloss-risks this item OWNS or routes

This item **owns** several §9 unglamorous states (acceptance criterion). Owned-and-specified here:
- **Permission-denied "no access" card (never a leaked title)** — §5.4 (OWNED; the beat over Slack/Notion).
- **Erased / tombstoned (GDPR-aware degraded)** — §5.7 (OWNED; three flavours, root carried).
- **Moved / outdated** — §5.5/§5.6 (OWNED).
- **Cross-cell-resolves-to-projection-or-tombstone** — §5.8 (OWNED; cell-local, residency-tagged).
- **Diff-line-anchored chip that relocates/orphans after rebase** — §5.9 (OWNED; the four content-anchor
  states; never silently mis-anchors).
- **Degraded / fails-static** — §5.10 (OWNED for this component).
- **Agent-pending** — §5.11 (named; depth → R-14/R-15).

**Routed (depth owned elsewhere):**
- **Storm / 30×-agent-surge** of unfurls → R-15 (calm volume) / R-21 (storm); §7.2 names the calm-by-default
  rule but the surge-shedding UX is R-21's (PROVEN backend: drill D-10 agent-lane sheds).
- **Optimistic-rollback** depth → R-13/R-21; §4.2 specifies it for inline actions, the per-surface catalogue
  is R-21.
- **The HITL Approve/Edit/Reject card depth** → R-14 (§5.11/§4.1 reference it as the same shape, don't
  re-spec it).
**Consciously deferred (with reason):** the *backend* resolver internals (cache TTLs, the Leopard reach
index, the cross-cell fan-out *build*) are `myelin-refs`' (PROVEN, surfaced not redesigned per the prompt);
the per-surface *full* six-state catalogue is R-21's (this file specs the component's states, R-21 multiplies
them across surfaces).

---

## 11. `[DEFERRED-UNTIL-USERS]` — validation plan (R-09 has `user-dep: none`, but the component carries testable bets)

R-09 is **not** a deferred-until-users item — the no-user substitute (this expert interaction spec, grounded
in the cited North-Star teardowns + the PROVEN resolver) **is** the deliverable. But three HOUSE-STYLE bets in
it are falsifiable and should be tested once users exist; recorded honestly per the standing rule:

- **`[DEFERRED-UNTIL-USERS]` — Does the no-access card read as "you lack access" (intended) and not "this is
  broken" (failure)?** *Test:* show engineers (P1–P5) + PMs (P6–P10) a stream with a restricted chip; ask
  "what does this mean / what would you do?" *Falsifies* §5.4 if users read it as a bug or try to "fix" it.
- **`[DEFERRED-UNTIL-USERS]` — Is the tombstone dignified-and-clear or alarming?** Especially the *erased*
  flavour with a DPO (P13) and a regular user. *Falsifies* §5.7 if the erased state reads as data-loss/error
  rather than a lawful, intended degradation.
- **`[DEFERRED-UNTIL-USERS]` — Is the rebase-relocated chip *trusted*?** *Test:* engineers review a PR whose
  comments relocated after a rebase; do they trust the "moved" pill or do they distrust the anchor? *Falsifies*
  §5.9's "moved" approach if reviewers prefer a hard-detach over a silent-follow even when context-matched.
- **Method:** per-segment RITE on the Phase-6 finalist that ships this component, on the F-ENG-1 + F-PM-1 +
  F-GOV-1 flows. **Caveat:** until then, treat the no-leak/erasure-safety as **PROVEN** (it's the resolver
  contract + drills D-1/D-5/D-9) but the **comprehension/trust** of each degraded state as **HYPOTHESIS**.

---

## 12. Sources (web-verified, 2024–2026; + surfaced architecture contracts)

**External (cited URLs):**
- WCAG 2.2 SC 1.4.13 Content on Hover or Focus (dismissable/hoverable/persistent; introduced 2.1, carried in
  2.2): https://www.w3.org/WAI/WCAG22/Understanding/content-on-hover-or-focus.html ·
  https://www.boia.org/blog/tips-for-meeting-wcag-1.4.13-content-on-hover-or-focus
- Slack interactive unfurls / Block Kit actions in unfurl cards / Work Objects (interactive previews; inline
  buttons in unfurls — the pattern Myelin makes native): https://docs.slack.dev/messaging/unfurling-links-in-messages/ ·
  https://docs.slack.dev/messaging/creating-interactive-messages/ · https://docs.slack.dev/block-kit/ ·
  https://docs.slack.dev/reference/block-kit/blocks/actions-block/ `[VERIFY]` Work Objects rollout/scope
- GitHub "Re-run failed checks" + required-status-checks in the merge box (the inline-action precedent for
  CI re-run): https://docs.github.com/en/desktop/working-with-your-remote-repository-on-github-or-github-enterprise/viewing-and-re-running-checks-in-github-desktop ·
  https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/collaborating-on-repositories-with-code-quality-features/troubleshooting-required-status-checks
- Prior R-01 dossier sources (Slack unfurl mechanics, Notion mentions, GitHub Files-changed) — see
  [R-01 §10](../north-star/teardown-dossier.md) (not re-listed; cumulative).

**Surfaced Myelin architecture contracts (PROVEN-as-existing, not invented):**
- `myelin-refs` resolver — the one ladder (§4.6), content-anchored line-ranges (§3.5), cell-local cross-cell
  (§4.2/C-5), frozen `#sub` grammar (§3.5), projection cache (§3.6), drills D-1/D-5/D-9/D-10:
  [reference-graph.md](../../../planning/05-refined-shared-systems-architecture/reference-graph.md).
- design-language §5.3 (the three hard rules), P6, §8b.5 (humanise at source), §8b.6, §5.10; ADR-13/03/12.

---

## 13. Self-check against R-09 acceptance criteria

| Criterion (prompt R-09) | Status | Evidence |
|---|---|---|
| **Both forms specced per artifact type** | ✅ Met | §2 (chip, per-type §2.3) + §3 (card, per-type body §3.2) for PR/issue/doc/run/thread/person-agent |
| **Inline actions specified with permission behaviour** | ✅ Met | §4 (the curated subset per type) + §4.2 (gated-by-presence, optimistic+rollback, confirm-class, attributed) |
| **ALL states present incl. no-access, tombstoned, moved/outdated, cross-cell, rebase-orphaned** | ✅ Met | §5.1–§5.11 (each a resolver state, §1.1 contract); the five named explicitly: §5.4, §5.7, §5.5/§5.6, §5.8, §5.9 |
| **Live-not-snapshot default shown** | ✅ Met | §1.1 (default = live projection), §6.2 (the bus-update mechanism + why-default); §5.1 |
| **Humanised strings (no raw ids)** | ✅ Met | §2.1, §6.1 — sourced at backend (§8b.5 / §4.8), frontend owns no map |
| **Maps onto the existing reference-graph resolver, not a new one** | ✅ Met | §1.1 resolver→UI contract; every state cites `myelin-refs` §4.6/§3.5/§4.2; §10 "surfaced not redesigned" |
| **Owns several §9 unglamorous states** | ✅ Met | §10 (owns no-access, tombstone, moved/outdated, cross-cell, rebase-orphan, degraded; routes storm/HITL) |
| **Builds ON R-01/R-06/R-04, doesn't duplicate** | ✅ Met | §0 + inline cites (R-01 §3.1 traps cashed out; R-06 §5 spine; R-04 flows threaded by ID) |
| **PROVEN/HOUSE-STYLE tags + date + cited URLs** | ✅ Met | tagged throughout; dated 2026-06-20; §12 URLs (WCAG/Slack/GitHub) + surfaced contracts |
| **Actionable toward rubric D4/D8/G1 + sketch-funnel** | ✅ Met | §9 mapping (D4 = the coherence test, D8 = §5.3/§4.2/§6.2, G1 = §8); the wedge-moment screen for R-22/Phase 6 |
| **Self-check restating acceptance criteria** | ✅ Met | this table |

**Top uncertainties (honest):**
1. **The *comprehension/trust* of degraded states (§5.4 no-access, §5.7 erased, §5.9 rebase-relocate) is
   HYPOTHESIS** — the no-leak/erasure-safety is PROVEN (resolver + drills), but whether users *read* each
   degraded render as intended-vs-broken is the §11 deferred test. The biggest is §5.9: silent-follow ("moved"
   pill) vs hard-detach is a genuine HOUSE-STYLE design bet.
2. **The inline-action *subset* per type (§4.1) is HOUSE-STYLE curation** — which actions are "in-flow enough"
   for a chip vs "open the artifact" is a taste call to validate (e.g. is inline PR-approve too consequential
   for a chip even via the HITL shape?).
3. **Cross-tenant *inbound* visibility (§5.8)** rests on an open legal/product policy (R-06 §6.3 / `myelin-refs`
   §6.4 / R-19); the structural no-leak floor ships regardless, but *whether* a public-OSS inbound ref is shown
   to the target tenant is undecided.

---

*End of R-09 deliverable. Date: 2026-06-20. Interaction design HOUSE STYLE over the PROVEN `myelin-refs`
resolver + design-language §5.3 hard rules + ADR-13/03/12; grounded in R-01/R-06/R-04 and cited
WCAG/Slack/GitHub sources; not user-validated — see §11. Feeds R-21, R-22, Phase 5, Phase 6.*
