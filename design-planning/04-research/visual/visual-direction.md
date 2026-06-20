# R-11 — Visual Direction & Mood-Boards (3 directions, tone-words)

> Phase 4 research corpus · WS-E (visual & motion direction) · Seq #3-parallel (foundational band).
> Deliverable for prompt **R-11** in [`03-research-prompts.md`](../../03-research-prompts.md).
> **Status date: 2026-06-20.** Method #13 (visual/aesthetic direction & mood-boarding — ADOPT).
> **The entire substance of this file is `HOUSE STYLE`** (taste / synthesis); the few embedded
> *constraints* that are PROVEN (a11y, AI-Act labelling) are tagged inline and are the floor every
> direction must clear, not the thing this file decides.

## 0. What this file is (and is not)

This file proposes **three genuinely distinct visual directions** for Myelin so the Phase-6 funnel
spans aesthetic variety *on purpose* (sketch-funnel §"the failure mode this plan exists to prevent":
an autonomous pipeline converging on one instinct and producing four variations of the same idea). It
is **not** a token dump (concrete palette values are an [OPEN → P4] item, design-language §9) and it
does **not** re-derive P1–P9 or the §3 token architecture — it *applies* them.

**The tie-break rule it is subordinate to (HOUSE STYLE, README §5.6 / rubric Part 4):** P1–P9 + the
measured hard gates (G1 accessibility, G2 i18n) decide; **pure aesthetics break ties only at the very
end.** A beautiful direction that cannot clear G1 contrast or carry the `agent` treatment
color-blind-safe is *disqualified before aesthetics are considered* (rubric Part 1). So each direction
below is specified to be **gate-passable**, and the three are deliberately scattered so the final human
sees real edges of the space — not so one "wins" on looks.

**How to read each direction:** tone-words → mood-board (cited real sources) → the §3 constraints it
honours → its position on **sketch-funnel Axis 4 (emotional tone)** and **Axis 1 (density)** (plus a
note on Axis 3, surface-unification, where the direction implies a stance) → the **anti-aesthetic it
explicitly avoids** → the one-line risk.

---

## 1. The shared floor (true of ALL three directions — non-negotiable)

These are the §3 / §8b.3 / §4 invariants. No direction may trade them for looks; they are what keep the
three as *three skins of one product* rather than three products (P1, rubric D4).

| Invariant | Source | Tag |
|---|---|---|
| **Neutral-led, accent-restrained** — a long neutral ramp carries ~90% of the UI; one brand accent + a small functional set (success/warning/danger/info) does the rest. | design-language §3.2 | HOUSE STYLE |
| **Borders-and-surfaces first; shadow reserved** for genuinely floating layers (palette, menus, popover, unfurl hovercard, toast, overlaid HITL card). | §3.5 / §8b.3 | HOUSE STYLE |
| **Three-tier tokens** (primitive → semantic → component); components consume *only* semantics → dark/high-contrast/tenant theming is a table swap. | §3.1 | HOUSE STYLE |
| **Dark mode is co-designed, not derived;** a high-contrast variant is part of the a11y baseline. | §3.2 / §4 | HOUSE STYLE |
| **Reserved `agent` semantic treatment** — consistent, distinct, *non-alarming*, recognisable at a glance everywhere; **never color-alone** (always icon + label). | §3.2 / §6.1 / §8b.3 | legibility duty **PROVEN** (AI-Act, ADR-08); the *specific look* HOUSE STYLE |
| **Contrast measured, not claimed** — every text/UI pair meets AA (4.5:1 text, 3:1 large/UI); **the focus token ≠ the identity token** (a ~2.8:1 brand accent fails AA, so the focus-ring/primary-action is a *derived* AA-safe token). | §8b.3; [WebAIM contrast]; rubric G1 | **PROVEN** (WCAG 2.1 AA / EN 301 549) |
| **Status never by color alone** — glyph + label + position carry it too; **no saturated status fills** ("the screen is not a traffic light"). | §8b.3; [WCAG 1.4.1] | rule **PROVEN** (WCAG 1.4.1 / colour-blind ~8% of men); "no saturated fills" HOUSE STYLE |
| **Hierarchy from weight & colour before size; spacing on a fixed ramp** (4px grid; off-ramp 5/7/13px is the amateur tell). | §3.3 / §3.4 / §8b.3 | HOUSE STYLE |
| **The global anti-aesthetic (§8b.3): no sparkle/shimmer/magic-wand AI iconography; no emoji-as-UI** (an emoji can't inherit `currentColor` or be re-themed); no gradient-text-on-numbers; no "purple-gradient AI-slop" default. | §8b.3; [Google Design — sparkle]; [prg.sh AI-slop] | legibility-of-agents **PROVEN**; the taste calls HOUSE STYLE |

> Everything in §1 is the *shared substrate*. The three directions diverge in **emotional tone,
> default density, type personality, accent character, surface texture, and how loud the agent/
> sovereignty cues are** — i.e. exactly the variables sketch-funnel Axes 1, 3, 4, 5, 6 contest.

---

## 2. Direction A — **"Instrument"** (Linear-grade, utilitarian-precise, dense-by-default)

> *The midnight command deck for the engineer: razor-thin chrome, one rationed accent, every pixel
> earns its place.*

### Tone-words
`Precise · Instant · Quiet · Engineered · Surgical · Confident · Restrained`

### Mood-board (cited real references)
- **Linear (2024–25 redesign).** Cut color back to near-monochrome black/white with very few bold
  colors; accent rationed to a *single primary action per screen*; cards gain presence through 1px
  inset borders + soft shadow rather than fills; Inter Variable at instrument-panel density. This is
  the spine of Direction A. — [Linear: how we redesigned the UI II]; [LogRocket — "Linear design"].
- **Vercel Geist.** "Pure black, pure white, Geist, and almost nothing else"; the shadow-as-border
  technique (`box-shadow 0 0 0 1px`, rgba ~0.08); near-zero radius; *"no second accent color … restraint
  is the product."* — [DesignSystems.one — Geist]; [SeedFlip — Vercel].
- **Sourcehut** as the *value, not the template* (competitive-landscape §1): radical performance,
  no-JS-required pages, uncluttered. Direction A borrows its **speed-signalling minimalism** while
  rejecting its developer-purist alienation of PMs (which is half Myelin's mandate).

### §3 constraints it honours (and how it *interprets* them)
- **Neutral-led / accent-restrained → taken to the extreme:** a long cool-grey ramp; *one* acid/electric
  accent reserved for the single primary action + focus-derived token; functional status colors muted,
  never saturated (§8b.3).
- **Borders-over-shadow → maximal:** hairline 1px separators do nearly all grouping; shadow only on
  true overlays. Near-zero/small radius for a sharp, fast read.
- **Density modes (§3.4):** `compact` is the *default* here; `comfortable` is the opt-out — the inverse
  of Direction C.
- **Type (§3.3):** the UI sans tuned tight (Linear-like stylistic sets), monospace load-bearing and
  visible (SHAs, diffs, run logs feel native, not quarantined).

### Axis placement
- **Axis 4 (emotional tone): hard `utilitarian-precise` pole.** The tool gets out of the way; copy is
  terse and exact; no encouragement, no warmth-for-warmth's-sake.
- **Axis 1 (density): `dense` pole.** Maximal information per screen; breathing room is *earned* (P5),
  not given.
- **Axis 3 (surface unification) implication:** leans **highly-unified-one-skin** — the same tight grid
  and chrome everywhere; distinctness is *density tuning*, not personality. (A finalist drawing on A can
  still be placed at a less-unified Axis-3 position; this is the *natural* pairing, not a lock.)
- **Axes 5/6 implication:** agents **ambient** (output in threads/inbox/collapsible, P8/§6.5);
  sovereignty cues lean **on-demand consoles** to keep the daily surface clean.

### The anti-aesthetic it explicitly avoids
No traffic-light status fills; no emoji-as-UI; no AI sparkle/glow on the agent treatment (the agent
badge is a *flat, labelled* monochrome-plus-`agent`-token mark, never a magic-wand); no decorative
gradients; no oversized display type (hierarchy from weight/colour, §8b.3). **The dense-pole-specific
trap it must dodge:** density-without-calm (the enterprise trap — Jira/GitLab dense-and-noisy). A wins
its dense pole *only if* it stays calm (rubric D7) — hairline hierarchy and muted status, not a wall of
saturated chips.

### Risk (one line)
**Reads as engineer-only / cold to P6 PM & P11–P15 corporate-governance** — the Sourcehut failure mode
(alienates non-engineers, who are half the mandate, §2). Mitigation lives in the *comfortable* density
default for PM surfaces and in Direction B/C existing as the human's alternative.

---

## 3. Direction B — **"Civic"** (institutional-calm, trust-forward, medium density, sovereignty-legible)

> *The surface a DPO trusts at a glance: plain, robust, legible, nothing decorative between the user
> and the facts — calm authority rather than speed-flex or friendliness.*

### Tone-words
`Trustworthy · Legible · Plain · Robust · Composed · Accountable · Sober`

### Mood-board (cited real references)
- **GOV.UK Design System / Government Design Principles.** "Accessible design is good design …
  everything should be as inclusive, legible and readable as possible"; built on
  perceivable/operable/understandable/**robust** + progressive enhancement; plainness *as* trust. The
  spine of Direction B's "no decoration between the user and the facts." — [GOV.UK Design System];
  [Government Design Principles].
- **USWDS / institutional-finance design systems.** Reference-grade systems (Bloomberg, Stripe, Gov.uk,
  USWDS) studied for regulated/compliance contexts; one such system "absorbed 8 regulatory rewrites
  without a rebuild cycle" — the *durability + legibility* posture Myelin's governance surfaces (DSR
  console, RoPA/residency, audit-log explorer, agent governance) need. — [Ed Chen — institutional
  finance design system].
- **Stripe (public-sector framing).** Calm, high-trust, "full visibility into financial data" framing —
  the *legible-by-default, decoration-free* tone Myelin's sovereignty surfaces emulate (P9). —
  [Stripe — public sector].

### §3 constraints it honours (and how it *interprets* them)
- **Neutral-led → warm-neutral, not cool:** a slightly warmer (less blue) neutral ramp reads composed
  and institutional rather than clinical; accent is a *steady, low-chroma* brand colour (a deep
  blue/teal), never an energetic acid — authority, not speed-flex.
- **Borders-over-shadow → strong:** clear hairline regions and tables; B is the most *structured-grid*
  of the three (governance data wants legible columns, RoPA tables, audit rows). Restrained radius.
- **Status & a11y → foregrounded as a feature, not a constraint:** B treats §4 / §8b.3 as a *visible
  value* — high-contrast feel by default, generous focus, status always glyph+label+position. This is
  where the **sovereignty/visibility cues** (residency badge on the scope indicator, per-artifact
  visibility chip — Axis 6 always-on) feel native rather than bolted on (P9, design-language §5.1).
- **Density (§3.4):** `comfortable`/medium default — denser than C (governance users read tables),
  calmer than A.

### Axis placement
- **Axis 4 (emotional tone): center-to-utilitarian, but *via gravity not coldness*.** Not warm/friendly
  (B distrusts whimsy, like the corporate/governance audience does, sketch-funnel Axis 4 rationale) and
  not as terse as A — its register is **sober trust**. Closest pole: `utilitarian-precise`, tinted with
  institutional warmth.
- **Axis 1 (density): center / lean-calm.** Dense enough for tables; calm enough that a non-engineer
  DPO/procurement reader (P13/P14) is never overwhelmed.
- **Axis 6 (sovereignty visibility) implication:** the natural home of the **always-on cues** pole —
  residency/lawful-basis/visibility legible *near the data*, which is exactly the P9 "felt, not fine
  print" mandate and the rubric **D9** anchor.
- **Axis 5 implication:** agents present but **accountable-first** — the agent treatment is paired with
  visible attribution/audit affordances (who/on-behalf-of/trigger), because B's whole thesis is
  legibility-of-authority (§6.4).

### The anti-aesthetic it explicitly avoids
No saturated traffic-light status (a governance surface that looks like a traffic light reads as
*alarming*, undermining the calm-trust thesis); no emoji-as-UI (corporate-governance reads it as
unserious); **no sparkle/magic on the agent treatment** (the audience whose deepest fear is ungoverned
automation, P12/P13, must see agents as *labelled, scoped, audited* — never "magic," §6.1, AI-Act
PROVEN duty); no decorative illustration that competes with data. **B's specific trap:** *dull-grey
government-form blandness* — legible must not become lifeless. B earns its keep with crisp typography,
confident spacing, and one steady accent, not with decoration.

### Risk (one line)
**Reads as bureaucratic / un-loved by the engineer (P1–P5)** who wants speed and edge, and as too
sober for the P6 PM who rewards approachability — i.e. B optimises the *adoption gatekeepers* (who
decide purchase) potentially at the cost of the *daily engineer love* that drives bottom-up adoption.
That very tension is *why it must be one of the three the human sees.*

---

## 4. Direction C — **"Workshop"** (Notion-grade, warm-approachable, calm-by-default, density-on-demand)

> *The friendly knowledge surface: warm minimalism, a touch of serif character, soft surfaces that
> invite a PM or a newcomer to start writing — without becoming toylike.*

### Tone-words
`Approachable · Warm · Inviting · Humane · Clear · Generous · Crafted`

### Mood-board (cited real references)
- **Notion (2025).** "Warm minimalism, serif headings, soft surfaces"; consistent 8/12px radius
  softening a blocky structure; a high-contrast black/white foundation with a functional blue and a
  *warm, muted neutral scale*; restrained hand-drawn-style illustration for personality. The spine of
  Direction C. — [getdesign.md — Notion]; [DesignMD — Notion tokens]; [super.so — Notion examples].
- **Typography-2025 "warm/editorial" trend.** Serif/serif-display headings paired with a clean UI sans
  for body — the controlled way to add warmth without whimsy. — [Designity — typography trends 2025].
- **Outline** (competitive-landscape §4) as the *sovereignty-safe* proof a clean, warm, approachable
  knowledge tool can be self-hostable — the EU-runnable cousin of the Notion tone.

### §3 constraints it honours (and how it *interprets* them)
- **Neutral-led → warm + soft:** a warm-grey ramp + cream/off-white surfaces (never pure `#fff`/`#000`
  — and dark mode uses an off-white text on a near-black-but-not-pure surface, which also *helps*
  AA-comfortable contrast and reduces eye strain, [boia/greeden dark-mode guidance]); a single warm
  brand accent + a functional blue.
- **Borders-over-shadow → softened:** still borders-first, but with a *slightly larger radius* (8/12px)
  and the *single* reserved soft-shadow token for floating layers — warmth through radius + surface
  tone, **not** through extra shadows (§8b.3 keeps one shadow token).
- **Type personality (§3.3):** the one place a **serif/serif-display heading** is permitted (knowledge
  reading surfaces, empty-state guidance) atop the shared UI sans — adds humanity while the EU-
  multilingual coverage requirement (Latin-extended/Greek/Cyrillic, §3.3) still governs selection.
- **Density (§3.4):** `comfortable` is the *generous* default; `compact` is one toggle away — the
  inverse of A, matching the PM/corporate "calm default, density behind a toggle" instinct (Axis 1
  calm-pole rationale).

### Axis placement
- **Axis 4 (emotional tone): `warm-approachable` pole** — friendlier copy, softer shapes, more guidance
  and encouragement in empty/first-run states (rubric **D2**) — **held off the toylike line by the §1
  floor** (no emoji-as-UI, no sparkle, restrained illustration, §2/§8b.3).
- **Axis 1 (density): `calm` pole** — generous whitespace, fewer things per screen, density earned/opt-in.
- **Axis 3 (surface unification) implication:** leans **distinct-per-surface** — knowledge & roadmap get
  more pacing/personality, while still sharing the chip/identity/palette/editor (so it tests the
  "how-much-distinctness-before-fracture" edge of the central problem, rubric D4 / sketch-funnel Axis 3).
- **Axes 5/6 implication:** agents framed as **helpful collaborators surfaced gently** (foregrounded-ish
  but calm); sovereignty more **on-demand** to keep the daily writing surface uncluttered.

### The anti-aesthetic it explicitly avoids
This is the direction *most at risk* of the §2/§8b.3 anti-aesthetic, so the guardrails are explicit:
**no emoji-as-UI** (the most tempting "friendly" shortcut — banned: can't inherit `currentColor`/
re-theme); **no AI sparkle/shimmer/magic-wand** on the agent treatment (warmth must not become
"magic" — agents stay flat-labelled, §6.1 PROVEN duty); **no purple-gradient AI-slop / gradient-text-
on-numbers** ([prg.sh]/[Google sparkle research]); no saturated traffic-light status; no oversized
display type as the only hierarchy. **C's specific trap:** *toylike / unserious* — which would fail the
engineer (P1) and the governance gatekeeper (P11–P15) simultaneously. C stays *crafted, not cute*:
warmth comes from neutral tone, radius, serif headings, and humane copy — never from decoration the
floor forbids.

### Risk (one line)
**Reads as too soft / slow / "not a real tool" to the keyboard-first engineer (P1–P5)** and risks the
"calm engineer surface is cluttered/starved" failure of over-distinctness (central problem) — a dense
diff or CI log must not be forced into generous pacing. Mitigation: the `compact` toggle and shared
dense components must stay first-class even inside C's warm shell.

---

## 5. Why these three are genuinely distinct (not three shades of one)

The funnel's binding spread rule (sketch-funnel §6b) requires finalists to occupy **materially
different positions on Axis 3** and differ on at least one of Axes 1/2/4. These three directions are
constructed to *guarantee* that spread survives into 6a/6b:

| | **A — Instrument** | **B — Civic** | **C — Workshop** |
|---|---|---|---|
| **Axis 4 (tone)** | hard utilitarian-precise | sober/institutional (utilitarian, warmed by gravity) | warm-approachable |
| **Axis 1 (density)** | dense (compact default) | medium (comfortable, table-dense) | calm (generous default) |
| **Axis 3 (unification) leaning** | highly-unified one-skin | unified, structured-grid | distinct-per-surface |
| **Accent character** | rationed acid/electric, single | steady low-chroma authority | single warm accent + functional blue |
| **Surface texture** | hairline + sharp radius, near-mono | strong hairline regions, structured tables | soft surfaces, larger radius, cream |
| **Type personality** | tight sans + visible mono | crisp sans, legible columns | sans + permitted serif headings |
| **Axis 5 (agent) leaning** | ambient | accountable-first (attribution-forward) | gentle-collaborator |
| **Axis 6 (sovereignty) leaning** | on-demand consoles | always-on cues | on-demand |
| **Primary audience it flatters** | engineers (P1–P5) | corporate/governance (P11–P15) | PM/delivery + newcomers (P6–P10) |
| **Its signature risk** | cold/engineer-only | bureaucratic/un-loved | toylike/not-a-real-tool |

They differ on **all of Axes 1, 3, 4** (the three required-spread axes) and lean different ways on 5/6.
Critically, each flatters a *different one of the three audiences* (README §1 three-audience frame) and
carries a *different signature risk* — so the human review sees three honest, non-overlapping bets, and
choosing any one (even a non-recommended one) is a real decision, not a near-duplicate (sketch-funnel
decision-ready-spread requirement).

**They remain ONE product** because all three obey §1 in full: same three-tier tokens, same chip/
identity/palette/editor/views components, same agent treatment grammar, same a11y floor. The variance
is *skin + tone + default density + cue-loudness*, exactly the layer the design language says is
tuneable without forking (§2; rubric D4 — "per-surface density is tuning of shared components, not a
fork"). A finalist can also *mix* the natural Axis pairings (e.g. Instrument density with Workshop
warmth on a knowledge surface) — the directions are **anchors for divergence**, not exclusive presets.

---

## 6. How this feeds downstream (what each consumer takes)

- **Sketch-funnel Axes 1 & 4 (and 3/5/6 leanings)** are now *seeded with concrete anchors* — 6a can
  scatter the 16–20 concepts across real tone/density poles (≥2 per pole) instead of inventing them
  cold. (Unblocks the funnel's tone/density axes early — the foundational-band purpose of R-11.)
- **R-12 (motion & emotional-tone language)** inherits the three tones: motion must *match* the chosen
  direction's register (Instrument = crisp/instant; Civic = composed/minimal; Workshop = gentle), within
  the §3.6 budget (≈120–200ms, reduced-motion first-class). R-11 → R-12 is a stated dependency.
- **Phase-6 tokens (6c):** each finalist authors DTCG tokens; this file tells the token author *which
  neutral temperature, accent character, radius scale, and type personality* the chosen direction
  implies — and reminds that the **focus token must be derived AA-safe** (§8b.3, rubric G1).
- **Rubric D3 (visual craft & emotional tone):** each direction is specified to *be scoreable* — token
  discipline (weight/colour-before-size, ramp spacing, borders-over-shadow), a coherent intentional
  aesthetic with a *clear tone*, and the *absence of the amateur tells* (§8b.3). The "0 anchor =
  amateur tells present / 4 = distinctive, intentional, loved" axis is exactly what §1's anti-aesthetic
  list and each direction's tone-words make checkable.
- **Rubric G1 (contrast):** the §1 floor (measured contrast, focus≠identity, status-not-by-colour,
  high-contrast variant, off-pure-black dark mode) is restated as a per-direction requirement, so no
  direction can reach Phase 7 with an un-passable accent.
- **Phase 8 look-fit:** the three anchors + their §3 interpretations give the framework pick a concrete
  target (e.g. Geist/shadow-as-border feasibility for A; structured-table/a11y maturity for B; soft-
  surface + serif coverage for C).

---

## 7. Completeness-critic §9 gloss-risks addressed

The README §9 pass names states/cases autonomous pipelines skip. Those *owned* by R-11 (the visual
layer) and how this file covers or consciously defers them:

- **"Status-not-by-colour-alone across CI green/red, PR states, SLA breach, agent treatment"** (§9
  a11y) — **covered** as a §1 floor invariant binding on all three directions and re-stated in each
  direction's anti-aesthetic (no traffic-light fills). PROVEN (WCAG 1.4.1).
- **"Visible focus on every surface in every theme … the focus-ring token, not the identity token"**
  (§9 a11y / §8b.3) — **covered**: §1 focus≠identity invariant; each direction's accent is specified as
  *not* the focus source. PROVEN.
- **"Agents look like agents — no sparkle/emoji-as-UI"** (§8b.3, the visual half of agent legibility) —
  **covered**: the reserved `agent` treatment is in the §1 floor *and* called out in each direction's
  anti-aesthetic, including for the warm Direction C where the temptation is highest. AI-Act labelling
  duty PROVEN; the specific flat-mark look HOUSE STYLE.
- **"RTL mirroring / non-Latin / text-expansion"** (§9 a11y, G2) — **consciously deferred** to R-18
  (i18n/RTL patterns) and the rubric G2 gate; *but* this file imposes the EU-multilingual type-coverage
  selection criterion (§3.3 Latin-extended/Greek/Cyrillic) on every direction's type personality —
  notably Direction C's serif heading must clear coverage, not just look warm.
- **Dark-mode contrast comfort** (not in §9 verbatim but a visual-craft gloss-risk) — **covered**: §1
  + Direction C note that dark mode uses off-white-on-near-black, not pure `#fff`/`#000`, to stay
  AA-comfortable and reduce eye strain. PROVEN guidance ([boia], [greeden]).

Gloss-risks *not* owned here (loading/empty/error/permission/erased/agent-pending *state craft*;
optimistic-rollback; storm-surge; conflict; mobile/touch; CLI) are correctly routed to R-21/R-13/R-04
and are out of scope for a visual-direction file — noted so they are deferred *consciously*, not glossed.

---

## 8. Self-check against the R-11 acceptance criteria

| Acceptance criterion (prompt R-11) | Status |
|---|---|
| **Three genuinely distinct directions** (not three shades of one). | **Met** — §2 Instrument / §3 Civic / §4 Workshop, proven distinct in §5 (differ on all required Axes 1/3/4, each flatters a different audience, each carries a different signature risk). |
| **Each tied to §3 constraints.** | **Met** — every direction has a "§3 constraints it honours (and interprets)" block; the shared §3/§4/§8b.3 floor is §1. |
| **Each has tone-words.** | **Met** — 7 tone-words per direction. |
| **Each placed on sketch-funnel Axis 4 (emotional tone) AND Axis 1 (density)** (plus Axis 3/5/6 leanings). | **Met** — explicit "Axis placement" block per direction; cross-table in §5. |
| **The anti-aesthetic explicitly avoided** (no traffic-light fills, no emoji-as-UI, no AI sparkle, §8b.3). | **Met** — global list in §1; per-direction "anti-aesthetic it avoids" with the direction's *specific* trap (incl. C's heightened risk). |
| **Every direction HOUSE-STYLE-tagged.** | **Met** — file-level HOUSE STYLE tag (§0); PROVEN floors (a11y, AI-Act) tagged inline so taste isn't masqueraded as settled. |
| **Tie-break rule referenced** (P1–P9 + measured gates decide; pure aesthetics break ties only, README §5.6). | **Met** — §0; reinforced by the §1 gate-passable floor and §6 rubric-feed. |
| **Web research grounded with cited URLs (2024–2026).** | **Met** — §9 source list; each mood-board cites real current sources. |
| **Completeness-critic §9 gloss-risks covered.** | **Met** — §7. |
| **Actionable toward rubric (D3 visual craft, G1 contrast) and sketch-funnel (the axes).** | **Met** — §6. |
| **Date the file (2026-06-20).** | **Met** — header. |
| **Do NOT commit.** | **Honoured** — no git actions taken. |

**Honest uncertainties (named, per VISION §3):**
1. **All three are HOUSE STYLE taste** — they are *hypotheses about what will be loved*, untested
   against real users; only the embedded a11y/AI-Act floors are PROVEN. `[DEFERRED-UNTIL-USERS]` —
   which tone the three audiences actually prefer is a Phase-6 sketch-evaluation + (later) preference
   test question; the funnel exists precisely to defer this aesthetic call to a spread the human picks
   from, not to settle it here.
2. **The Axis-3 leanings are *natural pairings*, not locks** — a finalist may decouple tone from
   density/unification (e.g. warm-but-dense). Phase 6 should treat the three as divergence anchors.
3. **Concrete token values (exact ramps, accents, radius/type families) are [OPEN → P4]** (design-
   language §9); this file commits *direction and character*, not values — the measured-contrast and
   EU-multilingual-coverage checks bind whatever values 6c chooses.

---

## 9. Sources (web-verified, 2024–2026)

- Linear — How we redesigned the Linear UI (part II): https://linear.app/now/how-we-redesigned-the-linear-ui
- LogRocket — "Linear design": https://blog.logrocket.com/ux-design/linear-design/
- DesignSystems.one — Geist (Vercel): https://www.designsystems.one/design-systems/vercel-geist
- SeedFlip — Vercel Design System Breakdown: https://seedflip.co/blog/vercel-design-system
- getdesign.md — Notion design analysis: https://getdesign.md/notion/design-md
- DesignMD — Notion tokens/typography: https://designmd.cc/benchmarks/notion
- super.so — Notion website examples: https://super.so/blog/notion-website-examples
- Designity — Typography Trends 2025: https://www.designity.com/blog/typography-trends
- GOV.UK Design System: https://design-system.service.gov.uk/
- GOV.UK — Government Design Principles: https://www.gov.uk/guidance/government-design-principles
- Ed Chen — Institutional finance design system showcase: https://edwson.com/design-system-showcase.html
- Stripe — Public Sector: https://stripe.com/industries/public-sector
- Google Design — Rise of the AI Sparkle Icon (research): https://design.google/library/ai-sparkle-icon-research-pozos-schmidt
- Jurgen Gravestein — How the Sparkle Icon Became the Universal Symbol for AI: https://jurgengravestein.substack.com/p/how-the-sparkle-icon-became-the-universal
- prg.sh — Why Your AI Keeps Building the Same Purple Gradient Website (AI-slop): https://prg.sh/ramblings/Why-Your-AI-Keeps-Building-the-Same-Purple-Gradient-Website
- WCAG 1.4.1 Use of Color (Web Standards Commission): https://wsc.us.org/wcag-141-use-of-color
- WebAIM — Contrast and Color Accessibility: https://webaim.org/articles/contrast/
- BOIA — Offering a Dark Mode Doesn't Satisfy WCAG Contrast: https://www.boia.org/blog/offering-a-dark-mode-doesnt-satisfy-wcag-color-contrast-requirements
- greeden.me — Accessibility guide for dark mode & high contrast (2026): https://blog.greeden.me/en/2026/02/23/complete-accessibility-guide-for-dark-mode-and-high-contrast-color-design-contrast-validation-respecting-os-settings-icons-images-and-focus-visibility-wcag-2-1-aa/
