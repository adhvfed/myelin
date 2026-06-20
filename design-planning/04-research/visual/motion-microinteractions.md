# R-12 — Motion, Microinteractions & Emotional-Tone Language

> **Phase 4 research corpus** · WS-E (visual & motion direction) · Seq #13. Deliverable for prompt
> **R-12** in [`03-research-prompts.md`](../../03-research-prompts.md). **File date: 2026-06-20.**
> Methods: **#13 (visual direction extended to motion — ADOPT)**, **#19 heuristics** (the single
> question motion must pass: *does it communicate a state change, or just decorate?*), **§8b motion
> budgets** (≈120–200ms, interruptible, reduced-motion first-class).
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = a cited perception/a11y/format standard, OR an
> existing Myelin contract this file *surfaces* (the §3.6 motion mandate; the resolver live-update of
> R-09; the optimistic+rollback of R-08/R-09/R-10). **HOUSE STYLE** = our motion-design synthesis /
> taste (the named microinteractions, the per-direction tone-mapping, the token *values*). The motion
> language is **not user-validated**; the deferred bets are in §10.
>
> **Builds ON prior `04-research` (does not duplicate — this file *animates* their already-specced
> moments):**
> - [R-11 visual-direction](visual-direction.md) — the three directions (**A Instrument** /
>   **B Civic** / **C Workshop**) whose *tone* motion must match (§6); R-11 §6 names R-12 as its
>   dependent and pre-states the mapping (Instrument = crisp/instant; Civic = composed/minimal;
>   Workshop = gentle). This file makes that mapping concrete and token-level.
> - [R-08 command-palette](../interaction/command-palette.md) §3.3 (palette "never animates in";
>   instant-show under reduced-motion), §3.1 (`→` peek), §8 (optimistic Act) — R-12 owns the
>   **palette-open** and **optimistic-settle** motions R-08 deferred to it by name (R-08 §13).
> - [R-09 reference-unfurl](../interaction/reference-unfurl.md) §2.2 (the ~300ms hover-peek delay +
>   WCAG 1.4.13 hovercard), §6.2 ("the chip updates in place — a live-update microinteraction, R-12"),
>   §5.5/§5.9 (the "moved" pill on a relocated chip) — R-12 owns **unfurl/hovercard** + the chip
>   **live-update-in-place** motion R-09 deferred to it by name.
> - [R-10 shared-patterns](../interaction/shared-patterns.md) §2.1 (drag = optimistic, "card moves
>   column", §3.6 cited), §2.2 (live-update transitions without scroll-jump), §4.2 (a new inbox item
>   "transitions in subtly"), §5.3 (overlay open/close "≈120–200ms … pages render, they don't animate
>   in"; reduced-motion = instant, first-class) — R-12 owns **card-moves-column**, **panel/overlay
>   open**, **live-row-update**, and the **agent-proposal appear/resolve** these specs reference.
>
> **The one-sentence thesis (HOUSE STYLE over the §3.6 PROVEN mandate):** *Motion in Myelin is a
> state-change notation, never decoration — a small, fast, interruptible alphabet of transitions whose
> only job is to make a change legible (where did the card go, did my edit stick, is this new, is an
> agent asking) — and every entry in that alphabet has a first-class reduced-motion spelling that
> conveys the same state change without movement.*

---

## 0. How to read this file

1. **§1 — The five motion laws** (the constitution every token + microinteraction obeys).
2. **§2 — The motion token set** (DTCG-structured: durations, easings, the composite transition
   tokens; the reduced-motion override table).
3. **§3 — The named-microinteraction catalogue** — the key moments (optimistic-settle,
   card-moves-column, unfurl/hovercard, palette-open, PR-going-green/live-update, agent-proposal
   appear/resolve, + the supporting set), each with: trigger → what moves → token → **what state it
   communicates** → **reduced-motion spelling**.
4. **§4 — Live event-driven updates** (the subtle-transition discipline for bus-pushed change).
5. **§5 — The agent motion signature** (the one recognisable agent motion, P7).
6. **§6 — Tone-mapping to R-11's three directions** (Instrument / Civic / Workshop motion registers).
7. **§7 — The delight set vs the explicit anti-list** (what earns delight; what is ruled out).
8. **§8 — a11y (G1) + §9 rubric/funnel actionability (D3/D8) + §11 completeness-critic.**
9. **§10 — `[DEFERRED-UNTIL-USERS]` + §12 sources + §13 self-check.**

---

## 1. The five motion laws (the constitution)

Each is the §3.6 mandate sharpened into a checkable rule. *(Laws 1–4 PROVEN against the cited
standard; the taste applications are HOUSE STYLE.)*

| # | Law | Basis | Tag |
|---|---|---|---|
| **L1** | **Motion communicates a state change or it does not ship.** Every animation answers exactly one of: *where did it go* (move), *did it work* (settle), *is this new* (enter), *is it gone* (exit), *who is asking* (agent). If a motion answers none, delete it (#19). | §3.6; #19 visibility-of-status | mandate **PROVEN**; per-motion call HOUSE STYLE |
| **L2** | **Fast + interruptible.** Functional UI transitions live in **120–200ms**; micro-feedback (a press, a toggle) ≤120ms; only a deliberately-noticed live/agent cue may reach ~240ms. **A new input interrupts any in-flight motion** (you never wait for an animation to finish to act). The ceiling exists because <100ms reads as instant and >300–400ms reads as sluggish — motion must stay inside the "responsive" band, never spend the user's latency budget. | [Nielsen response-time limits] (0.1s instant / 1s flow); [Doherty 400ms]; §3.6 (≈120–200ms) | **PROVEN** (perception thresholds) |
| **L3** | **Pages render, they don't animate in.** Layout, lists, cards, and rows **appear**; they do not stagger/slide/fade on first paint. Motion is reserved for **change *after* a stable state**, not for arrival. (Kills the "everything fades up on load" decoration class.) | §8b.6 verbatim | **PROVEN** (our binding rule) |
| **L4** | **Reduced-motion is a first-class spelling, not a removal.** Under `prefers-reduced-motion: reduce` every motion has a defined equivalent that conveys the **same state change** with no transl/scale movement — typically an **instant change + a brief (≤80ms) opacity/colour cross-fade** or a static marker. The user loses the *animation*, never the *information*. Triggered animation that can induce vestibular reactions must be suppressible — which honouring the OS/browser preference satisfies. | [W3C C39 prefers-reduced-motion]; [WCAG 2.3.3 Animation from Interactions]; §3.6/§4 | **PROVEN** (a11y) |
| **L5** | **One motion = one meaning, everywhere (the agent signature is reserved).** The same easing/duration pairing means the same thing across all five surfaces (a settle is a settle in Code, CI, Issues, Knowledge, Chat); the **agent enter/resolve motion is a distinct, reserved signature** (§5) so an agent change is recognisable at a glance — never colour-alone, motion *plus* the §3.2 agent treatment. | §3.6 (agent proposals get a consistent recognisable motion, P7); P1 | mandate **PROVEN**; the specific signature HOUSE STYLE |

> **Consequence (the cull-check for any finalist's motion, rubric D3):** point at any animation in a
> sketch and name the state change it communicates (L1) and its reduced-motion spelling (L4). If
> either is missing, it is decoration and fails the gate. This is the *checkable* form of "no
> decorative motion."

---

## 2. The motion token set (DTCG-structured)

Three-tier, mirroring the §3.1 token architecture: **primitive duration/easing → semantic
motion-role → composite transition** (consumed by components). DTCG `$type: duration` and
`$type: cubicBezier` are the stable W3C Design-Tokens types (PROVEN — [DTCG Format Module 2025.10];
`cubicBezier` = `[P1x,P1y,P2x,P2y]`, `duration` = `{value,unit}`). **Values are HOUSE STYLE** within
the L2 budget; the *structure* is PROVEN-portable so Phase-6 finalists author DTCG tokens directly.

### 2.1 Primitive duration tokens (`$type: duration`)

```jsonc
{
  "duration": {
    "instant":  { "$type": "duration", "$value": { "value": 0,   "unit": "ms" }, "$description": "reduced-motion / no-move state change" },
    "micro":    { "$type": "duration", "$value": { "value": 90,  "unit": "ms" }, "$description": "press, toggle, focus — sub-perceptual feedback (L2 ≤120)" },
    "fast":     { "$type": "duration", "$value": { "value": 140, "unit": "ms" }, "$description": "default functional transition (settle, chip update)" },
    "base":     { "$type": "duration", "$value": { "value": 180, "unit": "ms" }, "$description": "panel/overlay/card move (top of the 120–200 band)" },
    "deliberate":{ "$type": "duration","$value": { "value": 240, "unit": "ms" }, "$description": "live-update / agent cue meant to be NOTICED (capped; only roles in §2.3)" }
  }
}
```

> All five sit at/under the L2 ceiling; `deliberate` (240ms) is the **only** token above 200ms and is
> restricted to the "meant to be noticed without interrupting" roles (live-update, agent-enter) per
> §3.6's "subtle, non-jarring … notices without being interrupted."

### 2.2 Primitive easing tokens (`$type: cubicBezier`)

Grounded in the four canonical UI easing roles (PROVEN convention — [Material easing & duration]:
standard / decelerate-enter / accelerate-exit / emphasized). HOUSE STYLE curve values.

```jsonc
{
  "easing": {
    "standard":  { "$type": "cubicBezier", "$value": [0.2, 0.0, 0.0, 1.0], "$description": "move within view: quick out, soft in (the default)" },
    "enter":     { "$type": "cubicBezier", "$value": [0.0, 0.0, 0.2, 1.0], "$description": "decelerate — element arrives at peak velocity, rests (new content)" },
    "exit":      { "$type": "cubicBezier", "$value": [0.4, 0.0, 1.0, 1.0], "$description": "accelerate — element leaves, shorter, needs no focus" },
    "emphasized":{ "$type": "cubicBezier", "$value": [0.2, 0.0, 0.0, 1.0], "$description": "the reserved AGENT signature curve (paired w/ deliberate dur, §5)" },
    "linear":    { "$type": "cubicBezier", "$value": [0.0, 0.0, 1.0, 1.0], "$description": "progress/indeterminate only (a determinate bar moves linearly)" }
  }
}
```

> **No spring/bounce/overshoot primitive exists** — deliberately omitted (it reads as playful/
> decorative and conflicts with L1/L3 and the §8b.3 anti-aesthetic). A finalist on Direction C may
> *soften* `enter` slightly but may not introduce a bounce token (§6, §7 anti-list).

### 2.3 Semantic motion-role tokens (`$type: transition` — duration + easing + delay)

Components consume **only these roles**, never raw primitives (the §3.1 indirection that lets a
direction re-tune motion by table-swap — §6).

| Role token | duration | easing | Used by | State it notates (L1) |
|---|---|---|---|---|
| `motion.feedback` | `micro` (90) | `standard` | press, toggle, focus-ring, checkbox | "registered your input" |
| `motion.settle` | `fast` (140) | `standard` | optimistic-settle (§3.1), inline-edit commit, drag-drop confirm | "it worked / committed" |
| `motion.move` | `base` (180) | `standard` | card-moves-column (§3.2), row reorder, reflow after edit | "where it went" |
| `motion.enter` | `fast` (140) | `enter` | overlay/popover/hovercard/panel open (§3.3/§3.4), toast in | "this appeared" |
| `motion.exit` | `micro`–`fast` (90–140) | `exit` | overlay close, toast out, dismissed item | "this is gone" |
| `motion.liveUpdate` | `deliberate` (240) | `enter` | PR-going-green & bus-pushed row/chip change (§4) | "this changed in the background" |
| `motion.agentEnter` | `deliberate` (240) | `emphasized` | agent-proposal appear (§5) | "an agent is proposing — needs you" |
| `motion.agentResolve` | `base` (180) | `standard` | agent-proposal approve/reject settle (§5) | "the agent action resolved" |

### 2.4 The reduced-motion override table (L4 — first-class, not degraded)

A single semantic flag (`@media (prefers-reduced-motion: reduce)`, detected once at the substrate per
[C39]/[SCR40]) swaps **every** role to its no-movement spelling. The state change still lands.

| Role | Default spelling | **Reduced-motion spelling (conveys same state)** |
|---|---|---|
| `motion.feedback` | 90ms standard | `instant`; focus-ring/press shown statically (state still visible) |
| `motion.settle` | 140ms fade/scale | `instant` commit + ≤80ms opacity tick (or none); state flips immediately |
| `motion.move` | 180ms translate | **no translate** — the card/row **re-appears in its new location instantly**; a brief ≤80ms cross-fade marks the change; **live-region announces** "Moved to In Progress" (the canonical accessible substitute) |
| `motion.enter` | 140ms decelerate | `instant` show (the palette/overlay simply *is* there — already R-08 §3.3 / R-10 §5.3 behaviour) |
| `motion.exit` | 90–140ms accelerate | `instant` hide |
| `motion.liveUpdate` | 240ms enter | `instant` value swap + a **persistent static "updated" marker/colour-state** (e.g. the green check + "Passing" label simply appears) + polite live-region — the *information* (it's green now) is identical |
| `motion.agentEnter` | 240ms emphasized | `instant` appear, but the **§3.2 agent treatment + a static "needs your approval" marker** carry recognition (motion was never the only signal — L5/§5) |
| `motion.agentResolve` | 180ms standard | `instant` state change + result line |

> **The L4 guarantee, restated for the rubric:** removing motion removes *animation*, never
> *meaning*. Every reduced-motion cell above still answers its L1 question via a static marker, an
> instant position change, and (for background changes) a live-region announcement. This is why
> reduced-motion is **first-class**: a reduced-motion user and a default user learn the same state
> from the same surfaces.

---

## 3. The named-microinteraction catalogue (the key moments)

Each is a moment **already specced** by R-08/R-09/R-10; R-12 supplies its *motion*. Format:
trigger → what moves → role token → **state communicated (L1)** → **reduced-motion (L4)**. All
HOUSE STYLE motion over the PROVEN interaction spec it animates.

### 3.1 `optimistic-settle` — "did my action stick?"
*Owner moment:* R-08 §8 (palette Act), R-09 §4.2 (inline action), R-10 §2.1 (inline-edit commit).
- **Trigger:** a permitted action fires (transition issue, re-run, approve, commit a cell).
- **What moves:** the target's new state paints **immediately** (optimistic, P2); on server-ack a
  **`motion.settle`** (140ms) micro-confirm runs — a brief opacity/scale tick on the changed element,
  *not* a separate spinner. On honest-rollback (R-09 §4.2) the element **reverts with `motion.move`**
  back to its prior state + the one quiet line.
- **State communicated:** "applied" (settle) vs "couldn't apply, reverted" (reverse-move) — the two
  are *visually distinct* so optimism never hides a failure (L1; D8).
- **Reduced-motion:** instant commit; rollback = instant revert + the quiet line. No tick needed.

### 3.2 `card-moves-column` — "where did my card go?"
*Owner moment:* R-10 §2.1 (board/calendar/timeline drag, "optimistic, card moves immediately").
- **Trigger:** drag (pointer) or keyboard pick-up→move→drop (R-10 §2.4) across columns/positions.
- **What moves:** during pointer drag the card tracks the pointer (direct manipulation, no token —
  it's 1:1); on drop it **`motion.move`** (180ms `standard`) eases into its settled slot; neighbours
  reflow with the same role. Cross-column drop chains a `motion.settle` to confirm the transition.
- **State communicated:** the card's new column/position — the single most important "where did it
  go" motion (L1). Reflow shows the list re-ordered around it.
- **Reduced-motion:** card **re-renders in the destination instantly** (no fly); ≤80ms cross-fade +
  the keyboard path's live-region announcement ("Moved to In Progress, position 2", R-10 §2.4) is the
  accessible equivalent for *all* users in reduced-motion.

### 3.3 `unfurl-hovercard-peek` — "what is this reference?"
*Owner moment:* R-09 §2.2/§3.3 (the bounded hover/focus peek; WCAG 1.4.13).
- **Trigger:** hover (after the ~300ms intent delay, R-09 §2.2) **or** keyboard focus (immediate).
- **What moves:** the hovercard **`motion.enter`** (140ms `enter`/decelerate) — a small fade + ~2px
  rise into place, anchored, flipping per R-10 §5.3. Dismiss = **`motion.exit`** (90ms `exit`).
- **State communicated:** "a transient preview of *that* artifact appeared, anchored to *this* chip"
  (the spatial link between chip and card).
- **Reduced-motion:** instant show/hide (the hovercard simply appears) — note this *also* satisfies
  WCAG 1.4.13 dismissable/persistent, which is orthogonal to motion (R-09 §8). The ~300ms hover-intent
  delay is **not** an animation (it's a debounce) and is kept under reduced-motion.

### 3.4 `palette-open` — "the command surface is here"
*Owner moment:* R-08 §3.3 ("open/close … collapses to an instant show under reduced-motion; the
palette never *animates in*").
- **Trigger:** `⌘K`/`Ctrl-K` or the visible affordance.
- **What moves:** the palette overlay **`motion.enter`** (140ms `enter`) — backdrop fades, the panel
  fades + rises ~4px into position. It is a **floating layer appearing** (§3.5 shadow-reserved
  surface), explicitly **not** the page animating (L3). Close = `motion.exit` + return-focus (R-08
  §3.1).
- **State communicated:** "a modal command layer is now in front" — the appearance *is* the mode
  signal.
- **Reduced-motion:** instant show, instant hide — the L4 spelling R-08 §3.3 already mandates; this
  is the canonical case where reduced-motion = the existing behaviour, not a downgrade.

### 3.5 `overlay/panel-open` (Dialog/Popover/Dropdown/Toast) — generalised
*Owner moment:* R-10 §5.3 (overlay opening/closing "≈120–200ms … pages render, they don't animate
in"; reduced-motion = instant, first-class).
- **What moves:** `motion.enter` in / `motion.exit` out (durations as §2.3). **Toast** enters with
  `motion.enter` from the corner region and auto-dismisses with `motion.exit`; it **never steals
  focus** (R-10 §5.2) and its motion never blocks interaction (L2 interruptible).
- **State communicated:** appeared / dismissed; toast = "async result / undo available" (the
  reversibility-over-confirmation surface, R-10 §5.2).
- **Reduced-motion:** instant; nested overlays (R-10 §5.3) each resolve instantly, top-most first.

### 3.6 `live-update` incl. `PR-going-green` — "this changed while I watched"  →  full spec in §4.

### 3.7 `agent-proposal appear / resolve` — "an agent is asking / resolved"  →  full spec in §5.

### 3.8 Supporting set (smaller, same laws)
| Microinteraction | Token | State (L1) | Reduced-motion |
|---|---|---|---|
| **focus-move** (roving-tabindex cell→cell, R-10 §2.4) | `motion.feedback` | "focus is here now" | instant focus-ring (static) |
| **inbox-item-arrive** (R-10 §4.2 "transitions in subtly") | `motion.enter` (or `liveUpdate` if high-priority while watching) | "new thing needs you" | instant insert + polite live-region for critical/direct only (no spam, R-10 §4.5) |
| **skeleton→content swap** (R-08/09/10 loading) | `motion.settle` cross-fade | "the real content resolved" | instant swap (skeleton replaced) |
| **expand/collapse** (deduped inbox group "+N more", views group, card→chip) | `motion.move` (height) | "revealed / hid detail" | instant height change |
| **toast-undo** | `motion.enter`/`exit` | "you can still undo" | instant |
| **moved/outdated pill** on a relocated chip (R-09 §5.5/§5.9) | `motion.feedback` (the pill fades in) | "this reference relocated; anchor followed" | pill simply present (static) |

---

## 4. Live event-driven updates (the subtle-transition discipline)

§3.6 PROVEN mandate: *"Live event-driven updates (a PR going green, an issue moving) get a subtle,
non-jarring transition so the user notices without being interrupted."* This is the one place motion
duration is allowed to reach `deliberate` (240ms) — because the goal is **notice without
interrupt**, not speed.

**The canonical case — `PR-going-green` (HOUSE STYLE over R-09 §6.2's live-projection + R-10 §2.2's
"a PR going green … transitions in subtly without scroll-jump"):**
1. The bus pushes a check-status change to a PR chip/row/card the user is currently looking at
   (R-09 §6.2 `refs-projection-invalidator`).
2. The status hint cross-fades from `⏳ Pending` → `✓ Passing` with **`motion.liveUpdate`** (240ms
   `enter`) — a calm colour+glyph+label change in place. **Status is glyph+label+position, never
   colour-alone** (G1/§8b.3) — so "going green" is *also* "going to a check glyph + the word
   Passing," which is exactly what makes the reduced-motion and color-blind spellings free.
3. **No scroll-jump, no selection loss, no row re-sort** mid-glance (R-10 §2.2 binding rule) — the
   change happens *in place*; any re-sort waits for the next deliberate user action.

**The three discipline rules (HOUSE STYLE):**
- **R1 — In place, never relayout under the eye.** A background change updates the element where it
  sits; it does not reorder the list while the user reads it (the anti-pattern: a row jumping while
  you click it).
- **R2 — Coalesce, don't strobe.** During a storm (R-10 §4.3 / the 30×-agent-surge), live-update
  motion is **rate-limited / batched** — N rapid changes settle into one transition, never a
  flicker-wall. Calm-under-volume is a motion requirement, not just an inbox one.
- **R3 — Only animate what's on screen and watched.** Off-screen changes apply silently (state is
  correct on scroll-into-view, no entrance animation per L3); a watched change gets the subtle cue.

**Reduced-motion:** the status simply *is* green/Passing on next paint + a persistent "updated"
state; a **polite** live-region announces watched status changes (G1 4.1.3, no spam) — the
information is identical, the movement is gone.

---

## 5. The agent motion signature (P7 — one recognisable motion)

§3.6 PROVEN mandate: *"Agent proposals appearing/resolving get a consistent, recognisable motion
(P7)."* This is the **only reserved motion** in the system (L5) — a deliberate signature so an agent
action is identifiable at a glance, on **any** surface (chat HITL card, inbox row, inline unfurl —
R-09 §5.11, R-10 §4.2).

**The signature (HOUSE STYLE over the PROVEN mandate + §3.2 agent treatment):**
- **agent-proposal appear** = `motion.agentEnter` (`deliberate` 240ms + the reserved `emphasized`
  curve). The proposal card/row enters with a single, calm, slightly-more-present transition than an
  ordinary overlay — distinguishable from `motion.enter` by its *duration + curve pairing*, **paired
  always with the §3.2 agent treatment** (badge + label, color-blind-safe). **Motion is never the
  only agent signal** (L5; the §3.2 treatment carries it for reduced-motion and color-blind users) —
  motion is the *at-a-glance recognition accelerant* on top of the persistent legibility duty (AI-Act
  labelling, R-11 §1; R-14 owns the card itself).
- **agent-proposal resolve** = `motion.agentResolve` (180ms `standard`) on Approve / Edit / Reject —
  the card settles to its resolved state (approved → the effect's optimistic-settle §3.1; rejected →
  exit). The resolve motion is *ordinary* (it's a normal state change now that a human decided);
  only the **appear** is reserved-emphasized (it's the moment that needs recognition).
- **What it must NOT be (the §7 anti-list, restated for emphasis):** **no sparkle, shimmer,
  pulsing-glow, magic-wand, or "thinking" particle motion** — the §8b.3 / R-11 §1 anti-aesthetic is a
  *motion* rule too. An agent "working" state is a calm, **determinate-where-possible** progress
  indicator (or a quiet indeterminate `linear` bar), never an animated sparkle. The agent's fear-axis
  audience (P12/P13) must read agents as *scoped, labelled, auditable* — never "magic" (R-11 §3 Civic
  rationale).

**Reduced-motion:** the proposal **appears instantly** carrying the full §3.2 agent treatment + a
static "needs your approval" marker; resolve = instant. Recognition is preserved because it never
depended on motion alone (L5).

> **Why one reserved signature and not per-surface variety (D6/coherence):** an agent change must be
> recognisable in chat, inbox, and inline as the *same* event — so the appear motion is identical
> across all three (R-09 §5.11 / R-10 §4.2 dock to the *same* signature). A reviewer's coherence
> check: trigger an agent proposal in chat and in the inbox — the entrance motion + treatment are
> identical.

---

## 6. Tone-mapping to R-11's three visual directions

R-11 §6 states the dependency and pre-states the registers; this section makes them **token-level**.
The **token *structure* (§2) is shared by all three** (one product, R-11 §1/§5); a direction tunes
only the **semantic-role table values** — exactly the §3.1 indirection that lets tone change without
forking. *(All HOUSE STYLE; the budget L2 and the laws bind every direction.)*

| | **A — Instrument** (utilitarian-precise, dense) | **B — Civic** (institutional-calm, trust) | **C — Workshop** (warm-approachable, calm) |
|---|---|---|---|
| **Register (R-11 §6)** | crisp / instant | composed / minimal | gentle |
| **Default duration bias** | shorter end: `settle`=120, `move`=150, `enter`=120 | mid, steady: §2 defaults (140/180/140) | slightly longer-but-still-budget: `move`=190, `enter`=160 (never >200, L2) |
| **Easing bias** | tighter `standard` (snappier out) | the canonical curves unchanged (predictable, sober) | softer `enter` decelerate (gentler arrival; **no bounce**, §7) |
| **What motion it *omits*** | omits even the optional settle tick where instant reads cleaner (motion gets out of the way) | omits any flourish; motion is purely functional + the agent signature | keeps the full functional set but never adds decoration |
| **Live-update feel** | quick glyph swap (closer to `base` than `deliberate`) | calm, deliberate, sober (the 240ms is *for* legibility) | gentle cross-fade |
| **Agent signature** | present but ambient/quiet (R-11 §2: agents ambient) | present + **paired with attribution motion legibility** (R-11 §3: accountable-first) | present + gentle (R-11 §4: gentle-collaborator) — still no sparkle |
| **The risk it must dodge (motion form)** | feeling *cold/abrupt* — mitigated by keeping the settle confirm so actions feel acknowledged, not silent | feeling *lifeless* — mitigated by the one calm live-update cue so the surface feels alive, not frozen | feeling *toylike/slow* — mitigated by the L2 ceiling (never >200ms) + the no-bounce rule (R-11 §4 trap) |

> **The binding cross-direction invariant:** all three obey L1–L5 and the L2 budget; the **agent
> signature curve+duration pairing is the same** across directions (so an agent change is recognisable
> regardless of skin — L5/§5); only *magnitudes* (durations) and *softness* (the `enter`/`standard`
> curve tuning) differ. A finalist may mix (Instrument speed on a Workshop knowledge surface) — the
> directions are anchors, not locks (R-11 §5).

---

## 7. The delight set vs the explicit anti-list

R-12 must name **both** the microinteractions that *earn* delight and those *ruled out* (acceptance
criterion). Delight here is **earned by precision and honesty**, not by ornament — the §8b.3 / R-11
§1 anti-aesthetic applied to time.

### 7.1 The delight set (motion that earns love — HOUSE STYLE, all L1-passing)
- **The optimistic-settle that's *honest*** (§3.1): the action feels instant *and* a failure visibly
  reverts — the trust-through-honesty delight (the thing fragmented stacks fumble; D8).
- **The chip that goes green where it sits** (§4): you watch a PR pass without refreshing or losing
  your place — the live-projection wedge (R-09 §6.2) *felt*.
- **The card that lands in its column** (§3.2): drag/drop (and keyboard pick-up) that settles
  precisely, with the keyboard path announced — competent, not flashy.
- **The palette that's simply *there*** (§3.4): no animate-in latency tax (L3) — speed *is* the
  delight (the Linear-grade "instant" R-11 §2 Instrument prizes).
- **The recognisable agent arrival** (§5): you *know* an agent is asking before reading a word —
  legibility-as-delight (P7), the calm opposite of a sparkle.
- **Reduced-motion that loses nothing** (§2.4): the delight of an accessible path that's first-class,
  not a stripped fallback (a craft signal, D3).

### 7.2 The anti-list (explicitly ruled out — motion form of the §8b.3 anti-aesthetic)
| Ruled out | Why (which law/aesthetic it violates) |
|---|---|
| **Page/section animate-in** (staggered list reveals, fade-up-on-scroll, hero entrances) | L3 ("pages render, they don't animate in"); pure decoration (L1 fail) |
| **AI sparkle / shimmer / pulsing glow / magic-wand / "thinking" particles** | §8b.3 + R-11 §1 anti-aesthetic; §5 agent rule; misrepresents agents as "magic" (P7/P12-trust) |
| **Spring/bounce/overshoot/elastic** | no such primitive (§2.2); reads playful/decorative; conflicts with the calm + precise registers (R-11) |
| **Spinners on a blank surface** | §8b.6 (loading shows *structure*, skeletons not spinners); a spinner notates nothing (L1) |
| **Decorative hover-scale / parallax / tilt on cards** | L1 (notates no state change); decoration |
| **Confetti / celebration bursts** (PR merged, inbox-zero) | decoration (L1); the calm-by-default thesis (P8); inbox-zero is a *quiet* reward (R-10 §4.3), not a party |
| **Looping / ambient / attract-loop motion** | L1 (no state change); a perpetual animation is by definition not communicating a *change* |
| **Motion as the *only* signal of state** (e.g. a colour pulse with no glyph/label, motion-only "new") | L5 + G1 (status-not-by-colour/motion-alone); fails color-blind + reduced-motion users |
| **Long/blocking transitions (>~240ms; non-interruptible)** | L2 (budget + interruptibility); spends the user's latency budget |

---

## 8. Accessibility (G1) — motion is a named a11y surface

Each PROVEN against its criterion; motion's a11y contract feeds R-17's audit.
- **Reduced-motion honoured, first-class** — the §2.4 override table; detected once at the substrate
  via `@media (prefers-reduced-motion: reduce)` ([C39]/[SCR40]); every role has a no-move spelling
  conveying the same state (L4). PROVEN.
- **No motion-induced barrier (vestibular)** — no large-area parallax/zoom/auto-playing motion; all
  triggered animation is suppressible by the preference ([WCAG 2.3.3 Animation from Interactions];
  the §7 anti-list bans the vestibular-risky classes outright). PROVEN.
- **Motion never the sole status carrier** (L5) — every animated state change is *also* a static
  glyph+label+position (G1 1.4.1; §4 PR-going-green is the worked example). PROVEN.
- **Live-region announcement, not motion, carries background change to AT** — watched live-updates +
  card moves announce via a **polite** region, debounced, critical/direct only (G1 4.1.3; no spam —
  R-10 §4.5). PROVEN.
- **Motion is interruptible** (L2) — an AT/keyboard user's next action is never gated on an
  animation finishing (G1 operability). PROVEN.
- **Focus motion ≤ feedback budget** — focus-ring/roving-focus uses `motion.feedback` (90ms) and
  remains visible per the focus-token rule (G1 2.4.7/2.4.11; R-11 §1 focus≠identity). PROVEN.

---

## 9. Actionability toward the control artifacts

| Control artifact | What this file equips | Where |
|---|---|---|
| **rubric D3 (visual craft & emotional tone)** | Motion *is* a craft-and-tone signal: the token set (§2) makes motion discipline scoreable; the three per-direction registers (§6) make "tone" checkable; the anti-list (§7.2) is the "0 anchor = amateur tells present" detector (animate-in, sparkle, bounce, confetti). The L1/L4 cull-check (§1) is the per-motion gate. | §1, §2, §6, §7 |
| **rubric D8 (perceived performance)** | `optimistic-settle` + honest-rollback (§3.1), skeleton→content swap (§3.8), live-update-in-place (§4), interruptible budget (L2), "pages render don't animate in" (L3) are the literal D8 motion bars. Motion that *spends* latency is banned (L2). | §1, §3.1, §4 |
| **rubric G1 (gate)** | Reduced-motion first-class override table (§2.4); status-not-motion-alone (L5); live-region-not-motion for AT (§8); vestibular-safe anti-list (§7.2). Checkable, not aspirational. | §2.4, §8 |
| **sketch-funnel Axis 4 (emotional tone)** | The §6 tone-mapping gives each finalist its **motion register** per direction — the time-dimension of the tone axis R-11 seeded. Phase-6 finalists author the §2 DTCG role table at their direction's magnitudes. | §6 |
| **sketch-funnel Axis 5 (agent presence)** | The reserved agent signature (§5) is the *felt* dimension of agent presence — ambient (A) ↔ accountable-forward (B) ↔ gentle (C), same recognisable entrance. | §5, §6 |
| **R-13 (perceived-performance, next item)** | R-13 dresses the same components in skeletons/optimistic-rollback; it consumes §2's `motion.settle`/`motion.liveUpdate` + §3.1's optimistic-settle as the motion half of its patterns (R-13 lists R-12 as a Read). | §2, §3.1, §4 |
| **Phase 6 (tokens)** | The §2 DTCG token file is directly authorable per finalist (duration/cubicBezier/transition `$type`s are stable W3C). | §2 |

---

## 10. `[DEFERRED-UNTIL-USERS]` — what this motion language has NOT earned

R-12 is `user-dep: none` — the deliverable IS the no-user substitute (expert motion spec grounded in
perception/a11y standards + the prior interaction specs). But the following are **HOUSE-STYLE bets**
falsifiable once users exist; recorded as executable plans, not faked as validated:

- **`[DEFERRED-UNTIL-USERS]` — Are the duration *values* (§2) felt as "fast and confident" vs
  "abrupt" (A) or "slow" (C)?** *Test:* per-segment preference + the §8b.7 switch test on the
  Phase-6 finalist for each direction; instrument perceived-speed and "did it feel finished." *What
  would falsify:* engineers (P1–P5) report Instrument's 120–150ms settles as *missed/unacknowledged*
  (too fast to register), or PMs (P6–P10) report Workshop's 160–190ms as *sluggish*. The L2 band is a
  hypothesis-within-a-PROVEN-ceiling.
- **`[DEFERRED-UNTIL-USERS]` — Is the agent signature (§5) actually *recognisable as agent* without
  the label being read?** *Test:* show users an agent-proposal entrance vs an ordinary overlay
  entrance (motion only, label masked); can they tell which is the agent? *What would falsify:* users
  can't distinguish the signature → the `emphasized`+`deliberate` pairing isn't doing recognition
  work and the §3.2 treatment must carry it alone (which it must anyway for reduced-motion/color-blind
  — so this is a *delight* question, not a *safety* one).
- **`[DEFERRED-UNTIL-USERS]` — Does the live-update (§4) "notice-without-interrupt" balance hold?**
  *Test:* watch users during a PR-going-green / a busy board; do they *notice* the change without
  feeling *distracted* by it? *What would falsify:* users miss the change entirely (too subtle) or
  report the in-place updates as *jumpy/distracting* (the R2 coalescing or R1 no-relayout rule is
  insufficient).
- **Method:** per-segment RITE + the §8b.7 switch test on the Phase-6 finalists carrying these
  motions, on the F-ENG-1 (PR-going-green) and the agent-HITL flagship flows. **Caveat:** the
  *a11y/reduced-motion/vestibular* properties (§2.4/§8) are **PROVEN** (standards + the override
  table); only the **felt tone/recognition/balance** are HYPOTHESIS.

---

## 11. Completeness-critic (README §9) — gloss-risks this item touches

R-12 **owns** the motion layer of the §9 list and routes depth to the state owners:
- **Reduced-motion path** — **OWNED & covered** (§2.4 override table for *every* role; §8; L4). This
  is R-12's primary §9 obligation (the prompt names it) and it is first-class, not a degradation.
- **Optimistic-rollback (motion of)** — **OWNED for the motion** (§3.1); the per-surface rollback
  *craft* → R-13/R-21 (this file gives them the `motion.settle`/reverse-`motion.move` tokens).
- **Live-update / PR-going-green** — **OWNED** (§4); the perceived-performance dressing → R-13.
- **Storm / 30×-agent-surge (motion under)** — **covered as a motion rule** (§4 R2 coalesce-don't-
  strobe; §7.2 no-strobe); the inbox storm *experience* → R-21 (R-10 §4.3 owns the shed-budget).
- **Status-not-by-colour/motion-alone** — **covered** (L5, §4, §8); the static-glyph rule is R-09/
  R-11's, surfaced here as a motion constraint.
- **Consciously deferred (with reason):** the *state-craft catalogue* per surface (R-21), the
  *perceived-performance* skeleton/prefetch patterns (R-13), the *HITL card* itself (R-14), and the
  *concrete token values beyond direction/magnitude* ([OPEN → P4], like R-11's palette values) — this
  file commits the **motion grammar + budget + reduced-motion spelling + per-direction register**, not
  the final tuned milliseconds, which Phase-6 finalists set and the switch test validates.

---

## 12. Sources (web-verified, 2026-06; + surfaced contracts)

**Perception / response-time (PROVEN):**
- Jakob Nielsen / NN-g — Response Time Limits (0.1s instant / 1s flow / 10s attention):
  https://www.nngroup.com/articles/response-times-3-important-limits/
- Doherty Threshold (400ms productivity cliff, down to 50ms):
  https://uxuiprinciples.com/en/principles/doherty-threshold ·
  https://uxuiprinciples.com/en/principles/response-time-limits

**Accessibility / reduced-motion (PROVEN):**
- W3C WCAG — C39: Using the CSS `prefers-reduced-motion` query to prevent motion:
  https://www.w3.org/WAI/WCAG21/Techniques/css/C39
- W3C WCAG — SCR40: `prefers-reduced-motion` in JavaScript:
  https://www.w3.org/WAI/WCAG21/Techniques/client-side-script/SCR40
- W3C WCAG — Understanding 2.3.3 Animation from Interactions (vestibular; opt-out of triggered
  animation): https://www.w3.org/WAI/WCAG21/Understanding/animation-from-interactions.html
- Pope Tech — Design accessible animation and movement (2025):
  https://blog.pope.tech/2025/12/08/design-accessible-animation-and-movement/

**Token format / easing convention (PROVEN structure; HOUSE-STYLE values):**
- W3C Design Tokens Format Module 2025.10 (`$type: duration`, `$type: cubicBezier`, transition
  composite): https://www.designtokens.org/tr/drafts/format/ ·
  https://www.w3.org/community/design-tokens/2025/10/28/design-tokens-specification-reaches-first-stable-version/
- Material Design — Easing & duration (standard / decelerate-enter / accelerate-exit / emphasized
  roles): https://m3.material.io/styles/motion/easing-and-duration ·
  https://m2.material.io/design/motion/speed.html

**Surfaced Myelin contracts (PROVEN-as-existing, not invented):**
- design-language §3.6 (the motion mandate: functional/fast/interruptible, ≈120–200ms, reduced-motion
  first-class, subtle live-update, recognisable agent motion), §8b.6 ("pages render, they don't
  animate in"; latency budgets), §8b.3 (anti-aesthetic), §4 (a11y baseline).
- R-08 §3.3 (palette instant-show), R-09 §6.2 (live-projection invalidation), §2.2 (hover-peek delay),
  §5.5/§5.9 (moved pill), R-10 §2.1/§2.2 (drag optimistic / live-update no-jump), §4.2 (inbox arrive),
  §5.3 (overlay open budget + reduced-motion), R-11 §6 (the three directions' motion registers).

---

## 13. Self-check against R-12 acceptance criteria

| Criterion (prompt R-12) | Status | Evidence |
|---|---|---|
| **Motion tokens DTCG-structured and within the §3.6 budget** | ✅ Met | §2 (`$type: duration` / `cubicBezier` / `transition`, three-tier; all durations ≤240ms, functional band 120–200, L2) |
| **Every motion communicates a state change (no decoration)** | ✅ Met | L1 (the constitution) + the per-microinteraction "state communicated" column (§3) + the §7.2 anti-list (decoration ruled out) |
| **agent-proposal + live-update motions specced** | ✅ Met | §5 (reserved agent signature appear/resolve, P7) + §4 (PR-going-green / bus-pushed, the `deliberate`+`liveUpdate` role) |
| **Reduced-motion first-class for every motion** | ✅ Met | §2.4 override table (every role → a same-state no-move spelling) + L4 + §8; the prompt's named §9 gloss-risk |
| **Delight microinteractions named AND the anti-list explicit** | ✅ Met | §7.1 delight set (6) + §7.2 anti-list (9 ruled-out classes with the law each violates) |
| **"Pages render, they don't animate in" honoured** | ✅ Met | L3 (verbatim) + §7.2 (animate-in ruled out) + §3.4/§3.5 (overlays *appear*, the page does not) |
| **Each PROVEN (perception/a11y standard, cite) vs HOUSE STYLE** | ✅ Met | L1–L5 tagged; §2 structure PROVEN / values HOUSE STYLE; §12 cited URLs (Nielsen/Doherty/C39/SCR40/2.3.3/DTCG/Material) |
| **Matches R-11's three directions' tones** | ✅ Met | §6 token-level tone-mapping (Instrument crisp / Civic composed / Workshop gentle; shared structure, tuned magnitudes; reserved agent signature constant) |
| **Builds ON R-08/R-09/R-10/R-11, doesn't duplicate** | ✅ Met | §0 + inline: animates the moments those specs deferred to R-12 *by name* (R-08 §13, R-09 §6.2, R-10 §8) |
| **Date the file (2026-06-20); do NOT commit** | ✅ Met | header; no git actions taken |
| **Actionable toward rubric D3/D8 (+G1) and funnel Axis 4/5** | ✅ Met | §9 mapping |
| **§9 gloss-risks addressed (reduced-motion path)** | ✅ Met | §11 (reduced-motion OWNED; optimistic/live-update/storm-motion covered; rest routed) |
| **Deferred validation recorded as a plan, not faked** | ✅ Met | §10 (`[DEFERRED-UNTIL-USERS]`: duration-felt, agent-recognisability, live-update-balance — each with falsifier) |

**Top uncertainties (honest, per VISION §3):**
1. **The duration *values* (§2) and per-direction magnitudes (§6) are HOUSE STYLE within a PROVEN
   ceiling.** The 120–200ms band is perception-grounded (PROVEN); the exact 90/140/180/240 split and
   the A/B/C tuning are taste, falsifiable by the §10 switch test. *Largest uncertainty.*
2. **The agent signature's *recognisability from motion alone* (§5) is a HYPOTHESIS** — it is safe
   regardless (the §3.2 treatment + static marker carry it for reduced-motion/color-blind), so this is
   a delight question, not a correctness one; tested in §10.
3. **The live-update "notice-without-interrupt" balance (§4)** is the hardest taste call — too subtle
   (missed) vs too lively (distracting); the R1/R2/R3 discipline rules are the bet, validated by §10.
4. **`emphasized` ≡ `standard` curve in §2.2** — the agent signature currently leans on
   *duration*-difference (240 vs 180) more than curve-difference; whether a distinct emphasized curve
   is needed for recognition is a §10 question (kept conservative to avoid drama, per L1/§7).

---

*End of R-12 deliverable. Date: 2026-06-20. Motion language HOUSE STYLE over the PROVEN §3.6/§8b.6
mandates + cited perception (Nielsen/Doherty), a11y (WCAG C39/SCR40/2.3.3), and token-format (W3C
DTCG / Material easing) standards; not user-validated — see §10. Builds on R-08/R-09/R-10/R-11. Feeds
R-13, Phase 6 (DTCG tokens), rubric D3/D8/G1, sketch-funnel Axis 4/5.*
