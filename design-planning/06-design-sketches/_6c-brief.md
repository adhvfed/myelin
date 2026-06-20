# Phase 6 — Stage 6c Shared Brief (DEEPEN FINALISTS)

> Shared brief for every 6c finalist agent. Read it fully, plus your finalist's section in
> `design-planning/06-design-sketches/6b-cull.md`, plus the carrier/graft concepts it names in
> `6a-concepts/`. Stage 6c = **realize each finalist enough that choosing it — or choosing AGAINST
> it — at the final human decision is NOT a restart.** This is the decision-ready end-state.

## What you build
A coherent **multi-screen mini-system** for ONE finalist: the same tokens, the same shell, the same
shared components across every screen (this internal coherence IS your D4 evidence). Realized,
inspectable HTML — not throwaway. 3–6 screens.

## The canonical scenario (ALL finalists render the SAME content — so the human compares DESIGN, not data)
Use this exact scenario and data across every screen so the four finalists are directly comparable:
- **Org/tenant:** `acme` (EU tenant, residency: EU-West / Frankfurt cell).
- **Repo:** `acme/payments-api`. **Main is RED** (CI failing).
- **PR #412** — "Fix cache eviction under load" (weighted LRU); author a human engineer; one **agent
  reviewer** has commented; links **issue ISS-377**, **CI run #1894** (failing on `payments-api`
  integration test), and a knowledge doc **"Cache Eviction Design"**.
- **Issue ISS-377** — "Payments cache thrashes under burst load" (priority High, SLA at risk),
  on the Q3 roadmap initiative **"Payments reliability"**.
- **Agent proposal (the HITL moment):** `FixAgent` proposes effects — open PR #412 follow-up,
  transition ISS-377 → In Review, post to `#payments-incidents` — awaiting **Approve / Edit / Reject**
  (opening a PR on the protected `main` is the consequential, gated effect).
- **Roadmap (PM/corporate):** Q3 initiatives incl. "Payments reliability" (now), "Ledger export"
  (next), "EU data-residency controls" (later), with delivery state pulled from the same issues.
- **Data-subject (sovereignty):** DSR for subject "Jürgen Vögel" (chosen for the umlaut/expansion
  test) spanning all five surfaces.
- **People:** include a human `Mara Ø.` and the agent `FixAgent` (agent identity, never disguised).

## The comparable screen set (EVERY finalist MUST include all of these — for side-by-side judging)
You may combine related ones into a single screen (e.g. shell framing the PR pane), but all must be present:
1. **The shell** — the one-product frame (primary nav + contextual sidebar + content + context pane),
   showing how ≥2 of the five surfaces compose into one skeleton at this finalist's axis position.
2. **A dense engineer surface** — the **PR #412 context pane + diff** (preferred, it's the flagship
   wedge W1) OR the issue board OR the CI run #1894 view. Earn the density; keep it calm.
3. **An approachable PM/corporate surface** — the **Q3 roadmap** (now/next/later over the same issue
   data) OR an exec dashboard. This + screen 2 must use the **same shared views/components** tuned by
   density/role — that is your dual-audience (D5) + one-product (D4) proof.
4. **An agent / HITL moment** — the `FixAgent` **plan-then-apply approval card** (Approve/**Edit**/Reject)
   in context, showing proposed effects, per-effect target chips, the gated effect, attribution, and
   the reserved agent treatment (label + plain geometric icon, NO sparkle/emoji).
5. **A wedge moment** — the **command palette** (⌘K over the universal graph) OR a **reference
   chip/unfurl** in action (paste-link-it-unfurls-live), making the cross-artifact thesis tangible.
- **Finalist D (Civic) additionally MUST include the DSR / sovereignty console** (the Jürgen Vögel
  request across five surfaces, with residency cues + a verifiable receipt). Other finalists should
  show their sovereignty *cue* treatment somewhere (residency/visibility), at their Axis-6 position.

## Tokens (DTCG-structured — required)
Ship a token file `tokens.json` in **W3C DTCG format** (`$type`/`$value`; groups for color, dimension,
duration, etc.) AND consume them as CSS custom properties in the screens (no hardcoded colors/space in
the HTML — vars only). Three tiers: **primitive → semantic → component**. Provide **light + dark** at
minimum. The **focus-ring / primary-action token is derived AA-safe and may differ from the brand
accent** (the focus token ≠ identity token rule). Keep it neutral-led + one restrained accent.

## Hard gates — DEMONSTRATE on the required screens (these are pass/fail at Phase 7)
- **G1 Accessibility (WCAG 2.1 AA / EN 301 549 floor):** state the **measured contrast ratios** for
  your text/UI token pairs (AA: 4.5:1 text, 3:1 large/UI) — measured, not claimed; **visible focus**
  on every interactive element (show a focused state); **full keyboard operability** (note the keyboard
  model for the hard components — diff, board, palette, HITL card); **status never by colour alone**
  (glyph+label+position everywhere); content **reflows** at narrow width.
- **G2 i18n / l10n / RTL:** show **German** (use "Jürgen Vögel" + a long compound label) AND a
  **non-Latin script (Greek or Cyrillic)** somewhere real; include **at least one mirrored RTL state**
  (the shell + one content surface in RTL with a real Arabic or Hebrew string, using logical start/end
  properties); no truncation/clipping under expansion; locale-aware date/number on the SLA/roadmap.

## Unglamorous states — design for ≥1 surface (don't skip them)
For at least one surface, show the designed states: **empty** (onboarding-forward) · **loading**
(structure skeleton, never a blank spinner) · **error** (quiet, blames the system, offers a path) ·
**permission-denied** (graceful no-access, never a leak) · **erased/tombstoned** (GDPR-aware degraded)
· **agent-pending** (working / awaiting approval). A small "states" panel/page is fine.

## Rails (non-negotiable — from design-language §1–§6, §8b)
Neutral-led + one accent · borders/surfaces over heavy shadow · status by glyph+label+position ·
agents look like agents (label + plain geometric icon, no sparkle/magic-wand/emoji) · one shell / one
chip / one identity / one palette / one editor / one views component · keyboard-first, mouse-complete ·
dense-but-calm · hierarchy from weight+colour before size · spacing on a 4/8 ramp · motion functional/
fast/interruptible with a reduced-motion path · "pages render, they don't animate in."

## Output structure
`design-planning/06-design-sketches/6c-finalists/finalist-<ID>/` where `<ID>` is one of
`A-instrument`, `B-workshop`, `C-wayfinding`, `D-civic`:
- `screens/*.html` — the comparable screen set (self-contained; consume `../tokens.css` or inline the
  vars; a single CDN font link is OK, note production self-hosts).
- `tokens.json` (DTCG) + optionally `tokens.css` (the CSS-var projection the screens consume).
- `README.md` — finalist identity + persona + the six axis positions; the screen list; the token
  approach; **how it meets G1 (with the measured contrast numbers) and G2 (which languages/RTL shown)**;
  an honest **self-assessment scoring vs rubric D1–D10** (0–4 each, with one-line rationale); known gaps.

## Rules
Honesty: tag PROVEN vs HOUSE STYLE; carry the `[DEFERRED-UNTIL-USERS]` caveat (these are expert
sketches, not user-validated). Realistic content (the canonical scenario), never lorem ipsum. Do NOT
commit (the orchestrator handles git).
