# Phase 6 — Stage 6a Shared Brief (DIVERGE CHEAP)

> This is the shared brief for every 6a concept agent. Read it fully, plus the files it lists.
> Stage 6a = **divergence**: many quick single-screen concepts deliberately scattered across the
> axes so the design space is genuinely spanned before any deepening cost. Throwaway-grade but
> REAL. Convergence (cull → finalists) is 6b/6c, not now.

## What a 6a concept is
- **One screen, one self-contained HTML file** with inline `<style>` (minimal inline `<script>`
  only if it clarifies one interaction). No build step, no external JS deps. A single web-font via
  CDN link is acceptable for a throwaway concept (production self-hosts — note it, don't solve it).
- **Low fidelity is fine:** rough tokens OK; you do NOT need all UI states or full accessibility/
  i18n gate compliance at 6a (those are required only for 6c finalists). The point is to read the
  *idea* and its axis position at a glance.
- **Realistic Myelin content**, never lorem ipsum: real-ish repos, PRs, diffs, issues, roadmap
  items, docs, chat messages, CI runs, agent proposals. Drop at least one non-English EU string
  somewhere (German/French/Greek) to keep i18n in the back of the mind.

## The method — design as a SPECIFIC person (not the average)
Channel a **distinct named designer persona per concept** (vary them — no two of your concepts may
look like siblings; that is the whole point of divergence). For each, internalize their philosophy:
- **Commit:** their non-negotiables. **Refuse:** what they'd never do (this is what dodges the median).
- **Refuse BOTH attractors:** (1) the SaaS-median (Inter + purple gradient + centered hero + three
  equal cards + emoji icons + 0.1 shadows), AND (2) the editorial over-correction (giant serif
  masthead / magazine grid stapled onto a dev tool "to look distinctive"). Both are defaults.
- Roster to draw from (pick the one that serves the concept; span the roster across concepts):
  Systematic/Swiss (Müller-Brockmann, Rams, Vignelli) · Product Precision (Ive, Linear/Vercel/Rauno)
  · Data/Information (Tufte, Lupi) · Editorial/Luxury (Baron, Fili — only when content earns it)
  · Playful/Pop (Sagmeister, Draplin) · Raw/Brutalist/vernacular · Expressive type (Carson, Scher).
- Craft floor: type scale from a ratio (hierarchy via weight+colour before size); 4/8px spacing
  ramp (no 5/7/13); everything aligns; 60/30/10 colour with ONE saturated accent; two-layer shadows
  with one light source if any; one orchestrated motion moment, not scattered micro-interactions.

## Myelin's rails (the brief's NON-NEGOTIABLES — the persona works WITHIN these)
These are Myelin's product identity, not the median — honor them even while diverging. (Source:
`planning/02-holistic-architecture/design-language.md` §1–§6, §8b.)
- **Neutral-led, accent-restrained:** a long neutral ramp carries ~90% of the UI; ONE brand accent
  + functional success/warning/danger. Calm dense surfaces.
- **Borders & layered surfaces over heavy shadow.** Shadow only for genuinely floating layers.
- **Status NEVER by colour alone** — glyph + label + position (CI red/green, PR state, SLA, agent).
- **Agents look like agents, not magic:** a consistent label + plain geometric icon + reserved
  neutral `agent` treatment. NO sparkle/shimmer/magic-wand AI iconography. NO emoji as UI.
- **One product, five surfaces:** one shell, one identity badge, one reference chip, one command
  palette, one editor, one views component. Per-surface *density* is tuning, not a fork.
- **Keyboard-first, mouse-complete. Dense-but-calm. Hierarchy from weight/colour before size.**
- **Sovereignty/GDPR can be felt** (residency/visibility cues) where the concept calls for it.

## The axes of variation (tag every concept with its position)
(Source: `design-planning/02-research-roadmap/sketch-funnel.md`.)
1. **Density:** dense ↔ calm  · 2. **Navigation:** rail ↔ palette-led ↔ contextual ·
3. **Surface unification:** highly-unified-one-skin ↔ distinct-per-surface (THE central problem) ·
4. **Emotional tone:** utilitarian-precise ↔ warm-approachable ·
5. **Agent presence:** ambient ↔ foregrounded · 6. **Sovereignty visibility:** always-on ↔ on-demand.

## Read before building
- `planning/02-holistic-architecture/design-language.md` §1–§6, §8b (the rails above).
- `design-planning/04-research/visual/visual-direction.md` (R-11: the three directions
  **Instrument** / **Civic** / **Workshop** — anchor on these where assigned, or push elsewhere).
- The `design-planning/05-user-facing-surfaces/<group>.md` entry for each screen you're assigned
  (so the content + composition are right): `git.md`, `ci.md`, `issues.md`, `knowledge.md`,
  `chat.md`, `shared-admin-sovereignty.md`.
- For the wedge/agent/state specifics, skim the relevant `design-planning/04-research/` file
  (`interaction/reference-unfurl.md`, `agent-ux/legibility-and-hitl.md`, `craft/wedge-moments.md`).
- `design-planning/02-research-roadmap/rubric.md` — design *directionally* toward D1–D10; the hard
  gates G1/G2 are only enforced at 6c, but don't design something that can NEVER pass them.

## Output
- One file per concept: `design-planning/06-design-sketches/6a-concepts/concept-NN-<slug>.html`.
- **At the very top of each file, an HTML comment block:**
  ```
  <!-- CONCEPT C-NN | screen: <name> | persona: <designer> (commit: …; refuse: …)
       axes: density=<…> nav=<…> unification=<…> tone=<…> agent=<…> sovereignty=<…>
       rationale: this should feel like ___ because ___ -->
  ```
- Do NOT commit (the orchestrator handles git).

## The 20-concept assignment (deliberate spread — ≥2 per pole on Axes 1–4, all screen types, honest extremes)

| C | Screen | density | nav | unification | tone | agent | sov | suggested persona lineage |
|---|---|---|---|---|---|---|---|---|
| C01 | Shell + PR context pane | dense | rail | highly-unified | utilitarian | ambient | on-demand | Product Precision (Linear/Rauno) |
| C02 | Shell + roadmap | calm | contextual | distinct-per-surface | warm | ambient | on-demand | Editorial-warm (restrained) |
| C03 | Shell + exec dashboard | medium | rail | highly-unified | sober | ambient | always-on | Systematic/Swiss (Civic) |
| C04 | Shell (max-distinct EXTREME) | medium | contextual | distinct-EXTREME | utilitarian | foregrounded | on-demand | Raw/vernacular (honest extreme) |
| C05 | PR pane / diff | dense | palette-led | highly-unified | utilitarian | ambient | on-demand | Product Precision (Vercel) |
| C06 | Diff (code-feels-like-code) | dense | rail | distinct-per-surface | utilitarian | ambient | on-demand | Raw/Brutalist (mono honesty) |
| C07 | CI run + live log | dense | contextual | highly-unified | utilitarian | foregrounded (triage) | on-demand | Data/Information (Tufte) |
| C08 | Issue board (engineer) | dense | palette-led | highly-unified | utilitarian | ambient | on-demand | Product Precision |
| C09 | Roadmap now-next-later | calm | contextual | distinct-per-surface | warm | ambient | on-demand | Editorial/Workshop |
| C10 | Exec dashboard | calm | rail | highly-unified | sober | ambient | always-on | Data/Information |
| C11 | DSR / sovereignty console (always-on EXTREME) | calm | rail | distinct-per-surface | sober | ambient | always-on-EXTREME | Systematic/Swiss (institutional) |
| C12 | Roadmap pushed DENSE (calm-pole stress test) | dense | rail | highly-unified | sober | ambient | on-demand | Data/Information |
| C13 | HITL approval card in chat (foregrounded) | calm | contextual | highly-unified | warm | FOREGROUNDED | on-demand | Product Precision (trust) |
| C14 | HITL approval card (ambient/quiet) | dense | rail | highly-unified | utilitarian | AMBIENT | on-demand | Systematic (quiet) |
| C15 | Command palette overlay (palette-led EXTREME) | dense | palette-led-EXTREME | highly-unified | utilitarian | ambient | on-demand | Product Precision (Linear) |
| C16 | Live unfurl in chat (the wedge) | calm | contextual | distinct-per-surface | warm | ambient | on-demand | Editorial-warm (seam delight) |
| C17 | Engineer surface at CALM EXTREME (diff or board) | calm-EXTREME | contextual | highly-unified | warm | ambient | on-demand | Editorial/Workshop (honest test) |
| C18 | Notifications inbox | calm | rail | highly-unified | sober | ambient | on-demand | Systematic/Swiss |
| C19 | Knowledge page / block editor | calm | contextual | distinct-per-surface | warm | ambient | on-demand | Editorial-warm (writing) |
| C20 | Chat w/ Zulip-topics + agents | medium | rail | distinct-per-surface | utilitarian | foregrounded | on-demand | Product Precision / vernacular |

**Agent assignments:** Agent A → C01–C04 · Agent B → C05–C08 · Agent C → C09–C12 ·
Agent D → C13–C16 · Agent E → C17–C20. Each agent channels a DIFFERENT named designer for each of
its four concepts (no siblings), within the rails above.
