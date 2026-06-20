# 03 — Design-System Strategy & Visual Direction

> Covers token strategy, component taxonomy, measured QA, and visual/aesthetic direction — the methods that
> turn design-language §3 (token direction) and §8 (stack direction) into buildable, coherent, *measured*
> artifacts. Tags: **PROVEN** / **HOUSE STYLE**.

---

## 10. Design tokens — W3C DTCG (2025.10 stable), three-tier — **ADOPT**

**What it is.** Design tokens are the named, indivisible design decisions (color, spacing, type, radius,
motion) consumed by every component. The **W3C Design Tokens Community Group Format Module reached its first
stable version, 2025.10, on 2025-10-28** — a standard JSON format for exchanging tokens across tools, covering
color spaces, composite types, and a resolver for sets/themes. (Sources: designtokens.org/tr/2025.10/format;
W3C DTCG announcement.) The DTCG spec deliberately standardises the *format*, leaving the *architecture*
(primitive → semantic → component tiers) to the design system.

**Why it fits Myelin (specifically).** design-language §3.1 *already* mandates exactly this three-tier
architecture (primitive → semantic → component) and says coherence (P1) must be **mechanical, not a matter of
discipline** — a token change is one PR that updates every subsystem (design-language §8.2). Adopting the
*DTCG standard format* (not just "tokens" generically) is the right house-style sharpening because: (a) it
makes tokens portable across the Phase-8 framework choice and any design tool, de-risking lock-in (which
mirrors the platform's own anti-lock-in/portability value, P14); (b) the resolver/themes feature directly
serves design-language §3.2's light/dark/high-contrast + bounded tenant-theming requirement; (c) the reserved
`agent` semantic token family (design-language §3.2, P7) and the functional-status palette become
standard-typed, exportable artifacts.

**How WE would use it.**
- *Phase 6:* sketches declare their token sets in DTCG-conformant structure (even as the sketches are HTML,
  the token values are authored as DTCG so they're portable into Phase 8). Concrete values (the open question
  design-language §9) get *proposed* here.
- *Phase 8:* the chosen framework must consume DTCG tokens (or transform from them) — a selection criterion.
- *Execution:* one DTCG token package, versioned with the contracts (ADR-01), is the single source of truth.

**Effort/cost.** Medium. **PROVEN (standard + established practice).**

**Uncertainties & risks.** 2025.10 is the *first* stable version; tooling support is maturing (though adopted
by Figma, Adobe, Salesforce, Tokens Studio, etc.). Risk of betting on a young-stable spec. Mitigation: DTCG
is additive/JSON — even partial tool support loses nothing; we author tokens as the source of truth and
transform as needed (Style-Dictionary-class pipeline).

**Verdict: ADOPT.** Confirms and sharpens design-language §3; the standard format is the portability win.

---

## 11. Atomic design (component taxonomy) — **ADAPT**

**What it is.** Brad Frost's atomic design: a component hierarchy of atoms → molecules → organisms →
templates → pages, giving a shared vocabulary for composing a component library. (Source: atomic-design
practice.)

**Why it fits Myelin (specifically).** design-language §5 already enumerates the shared components (nav shell,
command palette, reference chip/unfurl, HITL card, comments, views component, editor, notifications inbox,
identity badge) and §8.1 mandates *one shared component library*. Atomic design gives the **taxonomy** for
organising that library so every subsystem composes from the same atoms — the mechanical coherence (P1) goal.
It also clarifies the §8b.1 doctrine that "nine menus are three shapes": atomic thinking forces the question
"is this a new organism or a recomposition of existing molecules?"

**Why ADAPT not ADOPT.** Strict atomic taxonomy can over-formalise and bikeshed the atom/molecule boundary.
We adapt it as a *loose* organising vocabulary, anchored to the §5 component list (which is our real
inventory), not as a rigid five-layer doctrine. The doctrine's "single-purpose by shape" (§8b.1) overrides
atomic purity where they conflict.

**How WE would use it.**
- *Phase 6:* each sketch declares which atoms/molecules/organisms it uses, mapped to §5 — so reuse across
  the 15 sketches is visible and Phase 7 can score coherence.
- *Phase 8/execution:* the component library is structured by this taxonomy.

**Effort/cost.** Low (vocabulary). **PROVEN, adapted.**

**Uncertainties & risks.** Taxonomy debates waste time; some Myelin components (the editor, the views
component) are large "organisms" that resist neat decomposition. Mitigation: anchor to the §5 list; don't
relitigate atom/molecule lines.

**Verdict: ADAPT.** Useful vocabulary over the existing §5 inventory; not a rigid framework.

---

## 12. Measured-not-claimed token QA (contrast & spec gates) — **ADOPT**

**What it is.** Automated verification that token *values* meet their claimed properties — measured contrast
ratios, spacing-on-the-ramp, focus-token correctness — rather than trusting stated values. (Source: WCAG
contrast math; codified by external-insights §3 / design-language §8b.3.)

**Why it fits Myelin (specifically).** This is **binding doctrine**, not a choice: design-language §8b.3
mandates "measure contrast; never trust a stated ratio" (a brand accent at ~2.8:1 fails AA), "the focus token
is NOT the identity token," "status never by colour alone," "never set colour via inline style on an
interactive element," and "spacing on a fixed ramp." These are PROVEN (WCAG AA) gates that turn the §4
accessibility baseline and §3 token direction into things a CI check can fail on. It directly serves the
EU-sovereign procurement reality (EN 301 549 / EAA, §4) where accessibility is a legal bar.

**How WE would use it.**
- *Phase 6:* every sketch's token set is run through a contrast/spec check; a sketch that ships a failing
  accent or off-ramp spacing is non-conformant *before* aesthetics are even judged.
- *Phase 7:* measured-token pass/fail is a **gate in the judging rubric** — not a matter of taste.
- *Phase 5:* specified as a Phase-5 testing-strategy CI gate (per §8b.3 routing).
- *Phase 8:* the framework/token pipeline must support automated contrast checking.

**Effort/cost.** Low-medium (tooling: WCAG contrast lib + lint). **PROVEN (WCAG).**

**Uncertainties & risks.** WCAG 2.x contrast math has known limitations (APCA is the emerging successor); over-
indexing on the ratio can produce dull palettes. Mitigation: AA is the legal floor (use it as the gate);
allow APCA as advisory; let the focus-token-≠-identity-token rule (§8b.3) resolve the "accent fails AA"
tension rather than weakening the accent.

**Verdict: ADOPT.** Mandatory per doctrine; it's how "accessibility" stops being a claim.

---

## 13. Visual / aesthetic direction & mood-boarding — **ADOPT**

**What it is.** The deliberate establishment of a visual point of view *before* committing to a system:
mood-boards, reference collages, "tone words," and 1–3 distinct aesthetic directions explored side by side to
make the taste decision explicit and reviewable. (Source: visual-design / art-direction practice.) Inherently
HOUSE STYLE.

**Why it fits Myelin (specifically).** "Top-of-the-line design" (VISION §3) is not only usability — Myelin
must be *loved*, and that's partly aesthetic. design-language §3 sets *system* direction (neutral-led,
accent-restrained, borders-over-shadow, the `agent` treatment, no-sparkle AI iconography per §8b.3) but
explicitly defers the *values* and visual feel to later (§9). Mood-boarding is how we make the aesthetic
decision *deliberately* rather than by accident in sketch #1, and how we reconcile the dual-audience tension
visually (dense-but-calm for engineers, approachable-but-not-toylike for PM/corporate — the Sourcehut-purism
and the Notion-friendliness poles, competitive-landscape §1/§4). It also pins the *anti*-aesthetic: no
traffic-light status fills, no emoji-as-UI, no magic-wand AI sparkle (§8b.3).

**How WE would use it.**
- *Phase 6 (before sketching):* establish 1–3 visual directions with mood-boards + tone-words, each tied to
  the §3 system constraints, so the 15 sketches explore *intentional* aesthetic variety rather than random
  variety. This makes Phase 7's aesthetic judgement comparable.
- *Phase 8:* the chosen direction informs which framework's default look is closest / most overridable.

**Effort/cost.** Low-medium. **HOUSE STYLE.**

**Uncertainties & risks.** Taste is subjective and unfalsifiable without users (README §5.6) — the biggest
risk is endless aesthetic debate. Mitigation: tie every direction to written §3 constraints + tone-words;
in Phase 7, let P1–P9 + measured gates decide and reserve pure aesthetics for tie-breaks (README §5.6).

**Verdict: ADOPT.** Necessary to hit "loved," and it makes the taste decision honest and reviewable rather
than implicit.

---

## 14. Live styleguide rendered from real tokens — **ADOPT**

**What it is.** A styleguide/component gallery that renders from the product's *actual* token + component
source (not a separate design file), runnable even with the backend down, so the reference can never drift
from the app. (Source: design-language §8b.6 / external-insights §3 — binding doctrine.)

**Why it fits Myelin (specifically).** Binding doctrine (§8b.6) and the mechanical-coherence guarantee (P1,
§8.2): if the reference is generated from real tokens/components, drift between "the design system" and "the
app" is structurally impossible. It's also the artifact that lets Phase-4 design agents and execution agents
*see* the shared components rather than re-implement them (the §8.3 coherence rules).

**How WE would use it.**
- *Phase 8 / execution:* a deliverable on the design-system package — every §5 component and §3 token rendered
  live, with all §5.10 states shown. This is where the "switch test" (#24) for *components* happens.

**Effort/cost.** Medium (build). **HOUSE STYLE (doctrine-mandated).**

**Uncertainties & risks.** Premature for Phase 6 sketches (no shared package yet). Mitigation: it's an
execution deliverable; in Phase 6 the 15 sketches *are* the temporary visual reference.

**Verdict: ADOPT.** Mandated by doctrine; the structural guarantee against design/code drift.

---

## SKIPs in this theme (do not relitigate)
- **Building a bespoke token format — SKIP.** DTCG 2025.10 is the standard; rolling our own loses portability
  and tooling for no gain.
- **Heavy brand-identity / logo / marketing-visual exercise — SKIP for now.** Out of scope for product-UX
  design phases; the `agent`/functional palette and product look are in scope, brand identity is a separate
  later effort.
