# Identity & Agent Badge — one badge per `Principal` + the reserved agent treatment

> **Tier 2.** One avatar/identity badge for every `Principal` (human / agent / service), with the **agent
> treatment** (§3.2/§6) making agents unmistakable. **File date: 2026-06-20. Direction A "Instrument".**
>
> **Implements:** design-language §5.11 (identity, presence & attribution) + §8b.3 (agents-look-like-agents)
> + **R-14 §1** (the four-channel agent signature; the hard prohibitions) + R-17 §6.3 (agent-treatment a11y
> audit). Consumed *inside* the chip/unfurl, HITL card, comments, views cells, editor mention node, inbox
> rows, audit log ([R] consumers) — **specced once here, inherited everywhere.**
>
> **Tagging:** **PROVEN** = WCAG 1.4.1 / AI-Act transparency / architecture contract (`Principal`, `humanise`).
> **HOUSE STYLE** = the visual character (the no-sparkle rule is HOUSE STYLE; the legibility duty is PROVEN).

---

## 1. Purpose — the single "who/what" treatment

Across the whole platform, *who* (or *what*) did/owns/is-mentioned-in something is rendered by **one badge
component**, so a person, an agent, and a service read as one family and an **agent is never disguised as a
human** (AI-Act, ADR-08). The badge is the atom inside every place a `Principal` appears.

Three `Principal.kind`s, one component:

| kind | Treatment |
|---|---|
| **human** | avatar (initials or image) + name; presence dot optional |
| **agent** | the **same badge component** + the **always-present four-channel agent signature** (§3) |
| **service** | avatar (a plain service glyph) + name; labelled "Service" when attribution matters |

---

## 2. Anatomy

```
human:    (MØ) Mara Ø.                         ← avatar + name
agent:    [▣] FixAgent  [Agent]                ← plain geometric mark + name + "Agent" text badge
          on behalf of @mara · triggered by ci.failed     ← attribution string (when authoring)
service:  (▷) ci-runner  ·Service             ← service glyph + name + label
```

- **avatar:** `--surface-hover` circle, initials in `--text-muted` (or image), 22px default. Sizes
  `xs 16 · sm 20 · md 22 · lg 28` (token-driven; the dense surfaces use sm/md).
- **name:** `--text-primary`, weight-medium.
- **agent mark (HOUSE STYLE):** a **plain geometric square** — `1.5px solid var(--agent)` border, `--radius-1`,
  with a small inner dot in `--agent` (the `.amark`/`.mark` from the finalist screens). **NOT a circle avatar**
  — the shape itself disambiguates an agent from a human for colour-blind users.
- **"Agent" text badge:** uppercase caption, `--agent` text, hairline `--agent` border, `--radius-1` — the
  **primary** carrier (survives greyscale / SR / high-contrast).
- **presence dot:** small dot (online/away) — never the only carrier of status; tooltip names it.

---

## 3. The reserved agent treatment — the four-channel signature (R-14 §1.1) — PROVEN duty

**"An agent did/proposes this" is conveyed by FOUR redundant channels, never colour-alone:**

| Channel | Spec | Tag |
|---|---|---|
| **Label (text, always — PRIMARY)** | the literal word badge `Agent` adjacent to the name (`FixAgent · Agent`). Survives colour-blindness, greyscale, SR, high-contrast. | **PROVEN** (WCAG 1.4.1; AI-Act) |
| **Icon (shape)** | one stable agent glyph — **a plain geometric mark, NOT a sparkle / shimmer / magic-wand / star.** Disambiguates by shape. | **HOUSE STYLE** (no-sparkle rule); legibility duty PROVEN |
| **Colour (the `--agent` token)** | the reserved violet `--agent` family — distinct, non-alarming, **NOT a functional status colour** (never reads as success/warning/danger). The **redundant** channel, measured-contrast-validated like every pair (tokens §2). | **PROVEN** (colour is supplementary) |
| **Attribution string** | on agent-authored content: `on behalf of @<human> · triggered by <event>`, resolved via the ONE `humanise` surface — **never a raw id**, never an agent-authored raw string. | **PROVEN** (mechanism = agent-fabric C9 / §8b.5; AI-Act disclosure) |

**Hard prohibitions (verbatim from §8b.3 / R-14 §1.1) — PROVEN-as-rule:**
- **No sparkle / shimmer / magic-wand / star "AI" iconography.** Agents look like *labelled principals*, not magic.
- **No emoji as the agent marker** — an emoji can't inherit `currentColor` or re-theme for dark/high-contrast/RTL.
- **Agents are never disguised as humans** — same badge *component*, but the agent channels are always present.

**Why the agent token is its own axis, not a status colour (R-14 §1.2):** reusing success-green / warning-amber
for "agent" makes the screen a traffic light **and** conflates "an agent touched this" with "this is good/bad."
`--agent` is a **fourth neutral semantic axis** orthogonal to functional status — an agent comment on a
*failing* check reads red-for-CI **and** agent-for-author, two independent channels.

---

## 4. Variants + parameterization variant flags

**Variants.** kind (human/agent/service) · size (xs–lg) · `withName` (badge-only vs badge+name) ·
`withAttribution` (the on-behalf-of line, when authoring) · `withPresence` (dot).

**Parameterization variant flags.**

| Flag | Effect |
|---|---|
| **`density`** | default size (compact → sm; comfortable → md) and whether the attribution line wraps inline or stacks. |
| **`agentPresence`** | does **not** change the badge — the agent is **always** labelled/gated regardless (Axis 5 varies *presence*, never *legibility*, R-14 §4). |
| **`tone`** | no effect (identity is invariant chrome across all directions — decision-brief §6 "no fork in the set"). |

---

## 5. ALL states

| State | Behaviour |
|---|---|
| **default** | rendered badge per kind. |
| **hover** | if clickable (opens the person/agent unfurl — [R]) → cursor + subtle `--surface-hover`; a Tooltip names a presence dot. |
| **focus** | the one `--focus-ring` when the badge is an interactive trigger. |
| **active** | pressed (opening the unfurl). |
| **disabled** | non-interactive badge (pure attribution) has no focus/hover; it is text+mark only. |
| **loading** | resolving a name → skeleton name bar + `aria-busy`; the avatar/mark renders immediately (shape is known before the name resolves). |
| **empty** | n/a (a `Principal` always has a kind + a resolvable display). |
| **permission-denied** | a `Principal` the viewer may not see resolves via `humanise` to a graceful "Restricted" / "[hidden user]" — **never a leaked name** (ADR-03). |
| **erased / tombstoned** | an erased actor humanises to **"[erased user]"** (references-not-payloads makes this free; ADR-12) — never a dangling id. |
| **agent-pending** | when the agent is mid-run, the badge may carry a quiet status ("working / awaiting you") — the *card* state (R-14 §5) lives in the HITL card; the badge stays the stable identity. |

---

## 6. Keyboard + ARIA model (PROVEN — WCAG 1.4.1 + R-17 §6.3)

- **Non-interactive badge:** rendered as text + an `aria-hidden` decorative mark; the **accessible name
  includes the word "Agent"** and the attribution string (e.g. `aria-label`/visible text "FixAgent, Agent, on
  behalf of Mara") — so SR users get the agent-ness as **text**, never colour/icon-only.
- **Interactive badge (opens unfurl):** a `Button` with an accessible name; opens a [F] Popover hosting the
  [R] person/agent unfurl card; focus returns on close.
- **Agent-treatment audit (R-17 §6.3, M4):** in greyscale / `forced-colors` the agent-ness **must remain
  distinguishable by label + shape** — if "an agent did this" collapses to colour-only, or to an emoji that
  can't re-theme → **Blocker → G1 fail.**
- **React Aria mapping:** decorative badge = plain markup; clickable badge = `Button` (+ [F] `Popover`/
  `DialogTrigger` for the unfurl); presence dot named via `Tooltip`.

## 7. Semantic tokens consumed

`--agent` / `--on-agent` / `--agent-subtle` (the reserved fourth axis — `--c-agent-mark`), `--surface-hover`
(avatar bg), `--text-primary` (name), `--text-muted` (initials), `--text-subtle` (attribution line),
`--border` (hairlines), `--radius-1`, `--focus-ring` (interactive). **Status colours are never used for the
agent treatment** (the §3 rule).

## 8. Motion

The agent enter (a new agent appearing in a thread/pane) may use the reserved `--ease-emphasized` at
`--dur-deliberate` (240ms — the one >200ms token, "notice-without-interrupt"). Reduced-motion → instant; the
*information* (the label + mark) is present immediately regardless.

## 9. Usage do / don't

- **Do** use the one badge component for every `Principal`; **do** always render the "Agent" **text** label +
  the plain mark + attribution for agents; **do** resolve names through `humanise` (frontend never owns a lookup).
- **Don't** use sparkle/shimmer/magic-wand/star/emoji for agents; **don't** reuse a status colour for `--agent`;
  **don't** disguise an agent as a human; **don't** leak a name on permission-denied/erased — humanise to
  Restricted / [erased user].

## 10. Reuse invariant (the seam with the rich-components set)

This badge is the atom the [R] reference chip, HITL card, comments, views cells, editor mention node, inbox
rows, and audit log all embed. **The agent treatment is defined here once; the rich set inherits it and must
not redefine it.** Cross-component test (R-10 §1 / R-17): the *same* agent badge (label + mark + colour +
attribution) renders identically in a chat message, a PR reviewer row, an inbox item, and the audit log.
