# R-17 — Accessibility Audit Method & Per-Surface a11y Checklist (the G1 checkable instrument)

> **Phase 4 research corpus** · deliverable of prompt **R-17** (workstream ws-e, Seq #15).
> **File date: 2026-06-20.** Methods: **#21 (accessibility audit — WCAG 2.2 AA / EN 301 549 / EAA;
> automated + manual expert review)** + **#12 (measured-not-claimed token QA)**. This file produces
> the **audit method** and the **per-surface, per-criterion checklist** that the rubric's hard gate
> **G1** *references and is checked against* — so "accessible" stops being aspirational and becomes a
> binary, inspectable pass/fail on the Phase-6 sketch artifact.
>
> **Builds ON prior `04-research` (does not duplicate — this file is the AUDIT LENS over them):**
> - [R-10 shared-patterns](../interaction/shared-patterns.md) — the components to audit: views grid
>   (§2.4 roving-tabindex + keyboard-drag), block editor (§3.5 contenteditable-island caret), inbox
>   (§4.5), **overlay substrate (§5.5 — where most overlay G1 obligations discharge once)**. R-10 wrote
>   the keyboard *interaction specs*; **this file is the conformance test those specs are audited
>   against**, plus the components R-10 routed to R-17.
> - [R-08 command-palette](../interaction/command-palette.md) §11 — the palette's own a11y contract
>   (APG combobox + `aria-activedescendant`, no-trap, live-region-no-spam). Audited in §5.6.
> - [R-09 reference-unfurl](../interaction/reference-unfurl.md) §8 — the chip/unfurl (WCAG 2.2 1.4.13
>   hovercard, status-not-colour, live-update announcement). Audited in §5.7.
> - [R-14 legibility-and-hitl](../agent-ux/legibility-and-hitl.md) §1/§5 — the agent treatment
>   (four-channel, never colour-alone, WCAG 1.4.1) + the HITL card as a named hard component. Audited
>   in §5.5 (HITL card) and §6 (status-not-colour, agent-treatment, agent-proposal announcement).
> - [rubric.md](../../02-research-roadmap/rubric.md) Gate **G1** — the six G1 bullets are the spine of
>   §4; this file makes each one **checkable per surface**.
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = a cited normative standard (WCAG 2.2 SC, EN 301
> 549, EAA), a cited measured statistic, or a documented vendor/AT behaviour. **HOUSE STYLE** = our
> audit-process synthesis/taste (e.g. severity bands, the per-surface checklist *layout*). The
> **AT-user-testing** half is `[DEFERRED-UNTIL-USERS]` (§8) — recorded as an executable plan, never
> faked as done. Every checklist row carries its WCAG / EN 301 549 criterion (§5/§6).

---

## 0. How to read this file

| § | Contents |
|---|---|
| **§1** | The standards floor stated correctly: WCAG 2.1 AA / EN 301 549 / EAA gate vs WCAG 2.2 AA house target; the **2.2 ⊇ 2.1** relationship; the 4.1.1-Parsing carve-out. |
| **§2** | The audit method — **two passes** (automated CI pass + manual expert pass) — what each catches, the ~30–40% coverage cap, the severity bands, the per-finalist evidence record. |
| **§3** | The two measured-token rules (the #12 core): **contrast-measured-not-claimed** + **focus-token ≠ identity-token** derivation rule. |
| **§4** | The **G1 master checklist** — the rubric's six G1 bullets turned into row-level checkable items, each cited. (Applies to every surface.) |
| **§5** | The **per-hard-component checklist** — diff · board-drag · views-inline-edit · block-editor · HITL-card · palette · nested-overlays — each with a keyboard entry + a screen-reader entry. |
| **§6** | **Live-region announcement of event-driven updates without spamming** (the §9 gloss-risk this item owns) + status-not-by-colour + agent-treatment audit. |
| **§7** | Actionability toward rubric G1 + funnel · **§8** `[DEFERRED-UNTIL-USERS]` AT-user-testing plan · **§9** completeness-critic · **§10** sources · **§11** self-check. |

**The one-line thesis (HOUSE STYLE):** *G1 is not a vibe; it is a checklist of cited success criteria,
measured (not claimed) on the sketch's actual tokens and markup, with a keyboard path and a
screen-reader announcement specified for each of the seven hard components — and the ~60% of real
accessibility that an audit cannot catch is recorded as a deferred AT-user-testing plan, not pretended
to be covered.*

---

## 1. The standards floor — stated correctly (the prompt's exactness requirement)

*(All PROVEN; cited §10.)*

- **The hard floor (the G1 gate) is WCAG 2.1 Level AA via EN 301 549.** The **European Accessibility
  Act (EAA)** became **enforceable 2025-06-28**; **EN 301 549** is the EU harmonised standard that
  gives a **presumption of conformity** with the EAA, and its current published version (**V3.2.1**)
  **incorporates WCAG 2.1 AA in its entirety** (PROVEN — EAA enforcement date and EN 301 549 V3.2.1 ⊃
  WCAG 2.1 AA, §10). For Myelin's core market (EU public-sector procurement), this is a **legal
  eligibility requirement, not polish** (rubric G1 / design-language §4) — failing it makes the product
  **ineligible**, which is why G1 is a hard gate.
- **The house target is WCAG 2.2 AA** (design-language §4). **WCAG 2.2 ⊇ WCAG 2.1**: WCAG 2.2 **adds
  nine success criteria and removes none** — *except* **4.1.1 Parsing**, which 2.2 **obsoletes**
  (browsers now handle the malformed-markup cases it covered; minor validation errors no longer
  constitute a WCAG failure — PROVEN, §10). Therefore **meeting 2.2 AA automatically satisfies the 2.1
  AA floor.** EN 301 549 **V4.1.1 (expected 2026) will fold in WCAG 2.2** (PROVEN — §10), so the house
  target front-runs the regulatory floor.
- **Judging rule (carried from rubric G1, restated as audit law):** audit against **2.1 AA as the
  binary gate**; **reward the nine 2.2-specific SC in the scored dimension D3**, not the gate. The
  2.2-specific SC Phase 6 should be rewarded for demonstrating: **2.4.11 Focus Not Obscured (Minimum)**,
  **2.5.8 Target Size (Minimum, 24×24 CSS px with the spacing exception)**, **3.3.8 Accessible
  Authentication (Minimum)**, plus 2.4.12 (Focus Not Obscured Enhanced, AAA — bonus), 2.5.7 Dragging
  Movements, 3.2.6 Consistent Help, 3.3.7 Redundant Entry (PROVEN — WCAG 2.2 new-SC list, §10).
- **The single most important honesty caveat (PROVEN — §2.3):** passing G1 is **necessary, not
  sufficient**, for "usable with assistive technology." Automated + expert audit catches ~30–40% of
  real issues; **AA ≠ usable-with-AT**. The remaining ~60% is the `[DEFERRED-UNTIL-USERS]` AT-user-test
  (§8). G1 must never be reported as "accessible-with-AT confirmed."

> **Plain-language gate summary for Phase 7:** *a sketch passes G1 iff every §4 master-checklist row is
> demonstrably met on the required screen set, AND every §5 hard component shown has its keyboard +
> screen-reader rows met, AND the §3 token rules hold on the shipped token set — measured, not claimed.*

---

## 2. The audit method (#21) — two passes, severity-banded, evidenced

Accessibility conformance is established by **a two-pass audit** (PROVEN-standard hybrid method — the
WebAIM/Deque/GDS consensus that automated tools alone are insufficient, §10): an **automated CI pass**
(cheap, every build, regression net) and a **manual expert pass** (the real gate). Neither is optional;
neither alone is enough.

### 2.1 Pass A — automated (CI, every build) — the regression net

| Property | Spec | Tag |
|---|---|---|
| **Tooling** | An axe-core-class engine (axe-core / Pa11y / Lighthouse a11y) wired into CI over the rendered sketch DOM + a measured-contrast pass over the DTCG token table (§3). | PROVEN (tool class) |
| **What it reliably catches** | Programmatic/objective failures: **missing/zero-contrast text-bg pairs (1.4.3), missing form labels (1.3.1/4.1.2), missing alt (1.1.1), bad heading order (1.3.1), missing lang (3.1.1), invalid ARIA roles/states (4.1.2), missing names on controls (4.1.2), duplicate ids/landmarks.** | PROVEN — §10 |
| **What it canNOT catch (the cap)** | **~60–70% of WCAG issues** are out of reach of automation: is the alt text *meaningful*; is the focus order *logical*; is the keyboard path *complete and trap-free*; does the live region *actually announce sensibly*; is status conveyed *non-visually*; does the diff make sense to a screen reader. **Best-tool studies (incl. the GDS audit: ~30–40% of 142 known issues) put automated coverage at ~30–40%** (PROVEN — §10). | PROVEN — §10 |
| **CI gate behaviour** | A **new** automated violation **fails the build** (regression-blocking); the contrast pass over the token table is the §3 measured-not-claimed gate. Automated *green* is a **prerequisite, never a pass-certificate**. | HOUSE STYLE over PROVEN |

### 2.2 Pass B — manual expert review (the real G1 gate) — per surface

Performed **per surface** by an a11y-literate reviewer. The expert pass is the **§4 master checklist +
the §5 per-hard-component checklist + the §6 dynamic-update checklist**, executed with:

1. **Keyboard-only sweep** — unplug the mouse. Tab/Shift-Tab/arrows/Home/End/Esc/Enter/Space through
   every interactive element on the surface. Verify: reachable, operable, **visible focus at all times**,
   **no trap** (you can always Tab *out*), **logical order**, focus **returned** after overlays close.
   *(Covers 2.1.1, 2.1.2, 2.4.3, 2.4.7, 2.4.11 — PROVEN.)*
2. **Screen-reader sweep** — drive the surface with **≥2 of {NVDA+Firefox, JAWS+Chrome, VoiceOver+Safari,
   Orca+Firefox}** (the cross-AT matrix; combinations behave differently — PROVEN expert practice). Verify:
   roles/names/states announced; landmarks/headings navigable; **dynamic updates announced sensibly and
   without spam** (§6); **no leaked title** on no-access states (cross-checks the R-08/R-09 anti-oracle).
3. **Zoom/reflow sweep** — 200% zoom and 320 CSS px reflow on the **dense** surfaces; no loss of content
   or function, no 2-D scroll for 1-D content. *(Covers 1.4.4, 1.4.10 — PROVEN.)*
4. **Theme sweep** — repeat the keyboard sweep in **light / dark / high-contrast (`forced-colors`)** to
   confirm the `focus-ring` token survives every theme (§3.2). *(Covers 1.4.11, 1.4.3 — PROVEN.)*
5. **Reduced-motion sweep** — with `prefers-reduced-motion` set, confirm every motion has a first-class
   instant path (no essential info conveyed only by motion). *(Covers 2.3.3 (AAA, bonus) + the
   design-language §4 first-class-reduced-motion rule.)*
6. **Token QA (#12)** — the §3 measured-contrast + focus-token-derivation check over the shipped DTCG
   token table.

### 2.3 Severity bands & the per-finalist evidence record (HOUSE STYLE)

Each manual finding is banded so G1's binary verdict is defensible:

| Band | Meaning | Effect on G1 |
|---|---|---|
| **Blocker** | A G1-gate criterion fails on a required screen (e.g. a keyboard trap, a sub-AA text pair, focus removed, status colour-only, a leaked title). | **Fails G1.** |
| **Major** | A non-gate but serious barrier (e.g. a 2.2-only SC missing, an awkward-but-operable SR path). | Does not fail G1; **costs D3**; logged. |
| **Minor** | Polish (verbose announcement, sub-optimal but valid order). | Logged; advisory. |

**Evidence record (per finalist, Phase 7 — pre-registration discipline, rubric Part 4):** for **each
surface in the required screen set**, record (a) automated pass result, (b) the §4 master checklist
row-by-row pass/fail with the **inspected markup/token as evidence**, (c) the §5 hard-component
keyboard+SR rows for each hard component shown, (d) any Blocker → G1 fail. **"Demonstrated, not claimed"**
means the role/state/token/announcement is *actually present and inspectable* in the limited-HTML sketch
(rubric Part 1) — prose assertions do not count.

---

## 3. The two measured-token rules (#12 — measured-not-claimed token QA)

These are the **#12 core** the prompt foregrounds; both are **PROVEN** (design-language §8b.3; WCAG AA).

### 3.1 Contrast measured, not claimed (PROVEN — WCAG 1.4.3 / 1.4.11)

- **Every** text/background pair and **every** essential-UI/graphics pair in the sketch's DTCG token set
  is run through a contrast checker and must meet **AA: 4.5:1 normal text, 3:1 large text (≥24px, or
  ≥18.7px bold) and UI components/graphical objects** (PROVEN — WCAG 1.4.3 text, 1.4.11 non-text).
- **A stated ratio is never trusted** — "passes AA" in prose is not evidence; the measured value on the
  *actual* token is (design-language §8b.3: "a brand accent at ~2.8:1 fails AA"). The CI contrast pass
  (§2.1) automates this; the expert confirms essential non-text pairs the tool may miss (focus ring vs
  adjacent colours, glyph-on-fill).
- **The brand-accent carve-out (PROVEN — rubric G1 / §8b.3):** the identity/brand accent **may** fail AA
  **only if** the **focus token and primary-action token are derived, AA-passing tokens distinct from
  the accent** (the §3.2 rule below). A sketch whose focus/action affordance rides a sub-AA accent
  **fails G1**.

### 3.2 The focus token is NOT the identity token (PROVEN — design-language §8b.3)

- **Derivation rule (PROVEN, carried verbatim as audit law):** the `focus-ring` token (and the
  primary-button/action token) **may need to differ from the brand accent**, because the accent can fail
  AA. The token system must define focus/action as a **derived token** with an explicit
  contrast-satisfying relationship to its backgrounds — **not an alias of the identity token**.
- **Audit check (checkable):** in the DTCG token table, confirm `focus-ring` (and primary-action) is a
  **distinct token** from `accent`/`brand`, and that it meets **≥3:1 against every adjacent surface**
  (1.4.11) in **light / dark / high-contrast** (1.4.11 + forced-colors). If `focus-ring === accent` and
  `accent` < 3:1 anywhere → **Blocker → G1 fail**.
- **One focus-ring token, everywhere** (design-language §4): the same `focus-ring` renders on the
  palette active row, diff line, board card, editor block, HITL button, overlay control — so focus
  visibility is a single audited token, not N per-component reinventions.

---

## 4. The G1 master checklist (the rubric's six bullets → checkable rows, each cited)

Applies to **every surface**. Each row is **PROVEN** (its cited SC). G1 passes only if **all** are met
on the required screen set (rubric G1; demonstrated-not-claimed per §2.3).

| # | Checkable item (the auditor verifies this is *present & inspectable*) | WCAG 2.2 SC / EN 301 549 | Pass evidence |
|---|---|---|---|
| **M1** | **Contrast measured.** Every text-bg & essential-UI/graphics token pair meets AA (4.5:1 / 3:1); measured on the actual DTCG token, not asserted. Accent-fails-AA allowed *only* with a derived focus/action token (§3). | 1.4.3, 1.4.11 (EN 9.1.4.3 / 9.1.4.11) | the token table + measured values |
| **M2** | **Visible focus, every interactive element, every theme.** One `focus-ring` token (≠ identity token, §3.2), never removed, **not obscured** when focused, ≥3:1, in light/dark/high-contrast. | 2.4.7, 2.4.11, 1.4.11 | tab through in 3 themes |
| **M3** | **Full keyboard operability, no traps.** Every interactive element reachable & operable by keyboard; logical tab order; you can always Tab/Esc *out*; focus returns to trigger after overlays. (The seven hard components → §5.) | 2.1.1, 2.1.2, 2.4.3 | keyboard-only sweep |
| **M4** | **Status never by colour alone.** Every status (CI pass/fail, PR/issue state, SLA breach, success/warning/danger, the **`agent` treatment**) carries **glyph and/or text label and/or position** in addition to colour. | 1.4.1 | inspect each status token / greyscale render (§6.2) |
| **M5** | **Semantic structure & ARIA.** Correct landmarks/roles/headings; dialogs/menus/comboboxes/grids use the right APG pattern; controls have accessible names; one `lang`. | 1.3.1, 4.1.2, 2.4.6, 3.1.1 | SR landmark/heading nav + role inspection |
| **M6** | **Live regions announce event-driven updates — without spam.** Dynamic updates (a check goes green, a card moves, an agent proposal arrives, a new inbox item) announced via correct-politeness live regions; **not per-keystroke, not per-background-refresh** (§6.1). | 4.1.3 | SR sweep while a live update fires |
| **M7** | **Reflow & zoom.** Content reflows at **200% zoom** and **320 CSS px** on the dense surfaces shown without loss of content/function; no 2-D scroll for 1-D content. | 1.4.4, 1.4.10 | zoom/reflow sweep |
| **M8** | **Reduced motion first-class.** Every motion has an equivalent instant path under `prefers-reduced-motion`; no essential info motion-only. | 2.3.3 (AAA bonus) + design-language §4 | reduced-motion sweep |
| **M9 (2.2 bonus → D3, not gate)** | **Target size ≥24×24 CSS px** (or spacing exception); **focus not obscured (enhanced)**; **accessible authentication** (no cognitive-test-only login). Reward in D3. | 2.5.8, 2.4.11/2.4.12, 3.3.8 | measure target boxes; inspect login |
| **M10** | **No-access never leaks** (cross-cuts a11y + GDPR). A permission-denied target announces "no access," **never a title/metadata** to AT — the same anti-oracle the visual UI honours (R-08 §6, R-09 §5.4). | 1.3.1 + ADR-03 (no-leak) | SR on a restricted chip/row |

---

## 5. Per-hard-component checklist (each has a keyboard entry + a screen-reader entry)

The rubric's **G1 hard-component list** plus the prompt's explicit set: **diff · board-drag ·
views-inline-edit · block-editor · HITL-card · command-palette · nested-overlays**. Each component's
**keyboard** + **screen-reader** rows below are the **acceptance criterion** ("every hard component has a
keyboard + screen-reader entry"). Where a prior file already specced the interaction, this file **audits
it** and cites it; the **diff** has no prior owner, so its full spec is here.

### 5.1 Diff / files-changed viewer (the hardest; no prior R-owner — specced + audited here)

The code diff is the **single hardest a11y component** and the one most commonly broken in the wild
(PROVEN — Monaco issue #411 "diff +/- not keyboard/SR accessible"; GitLab's `display:block`-on-`table`
strips semantics; GitHub's code-view a11y rework — §10). The two killers: (a) **+/- conveyed by colour
and a leading glyph only** → a screen reader reading line text **cannot tell added from removed**
(1.4.1 + 1.3.1 fail); (b) **code-as-`<table>`** forces line-by-line nav and breaks char/word reading
(PROVEN — §10).

| Aspect | Checkable requirement | SC / source |
|---|---|---|
| **Keyboard** | Every line/hunk reachable; **jump-to-next/prev-change** key (the VS Code F7/Shift-F7 *Diff Review* model — PROVEN, §10); expand/collapse hunks by keyboard; add a comment on a focused line by keyboard; switch unified↔split by keyboard; no trap. | 2.1.1, 2.1.2 |
| **Screen reader** | Each changed line announces its **change status as TEXT** — "added line", "removed line", "unchanged" — **not by colour/+/- glyph alone** (a visually-hidden text prefix or `aria-label`); the diff exposes a **linear, SR-friendly review mode** (unified patch reading order, à la VS Code Diff Review Pane) so the user is not trapped in 2-D table nav; line numbers announced; the rebase-relocated/orphaned **comment-anchor** (R-09 §5.9) announces its "moved/outdated" state as text. | 1.3.1, 1.4.1, §10 |
| **Other** | Syntax-highlight colour is **never the only** semantic carrier (1.4.1); diff add/remove fills meet 1.4.11 against text; **does not rely on a `<table>` with `display:block`** (the GitLab anti-pattern — PROVEN §10). | 1.4.1, 1.4.11 |

> **Diff verdict rule (HOUSE STYLE over PROVEN):** a diff that conveys add/remove **only** by green/red
> (or only by a `+`/`-` the SR doesn't announce) is an automatic **Blocker → G1 fail**, regardless of
> beauty. This is *the* most likely silent G1 failure in a code-first product, so it is called out first.

### 5.2 Board drag (kanban) — audits R-10 §2.4

| Aspect | Checkable requirement | SC / source |
|---|---|---|
| **Keyboard** | A **keyboard drag equivalent** exists (R-10 §2.4 pick-up grammar: focus card → Space/Enter pick-up → arrows move between columns/positions → Space/Enter drop → Esc cancel). **A drag-only/pointer-only board is a Blocker → G1 fail** (2.1.1). Roving-tabindex or `aria-activedescendant`; board is one Tab-stop; Tab exits cleanly. | 2.1.1, APG grid (§10) |
| **Screen reader** | The card move **announces each step via a live region** — "Picked up <card>", "Column In Progress, position 2", "Dropped" (R-10 §2.4) — using **polite** announcements (§6). Columns are labelled regions; card focus announces its column. | 4.1.3, 1.3.1 |
| **2.2 bonus** | Dragging has a non-dragging single-pointer alternative (a move-via-menu) → satisfies **2.5.7 Dragging Movements**. | 2.5.7 (D3) |

### 5.3 Views inline-edit (table/list cell editing) — audits R-10 §2.4

| Aspect | Checkable requirement | SC / source |
|---|---|---|
| **Keyboard** | Grid is **one Tab-stop**; arrows/Home/End/Ctrl+Home/End move within; Enter/F2/typing enters cell-edit; Enter/Tab commits; **Esc reverts and returns to cell-nav** (no trap — the AG-Grid trap class R-10 §2.4 warns of); Tab while editing moves to next cell (spreadsheet contract). | 2.1.1, 2.1.2, APG grid (§10) |
| **Screen reader** | `role=grid`/`row`/`gridcell` (or `treegrid` when rows expand); on cell-edit entry, **the field name + type is announced** (R-10 §2.4); the active cell announced on arrow move (`aria-activedescendant`); a **live update to a visible row does not clobber an in-progress edit** and is announced politely. | 4.1.2, 4.1.3, 1.3.1 |

### 5.4 Block editor (contenteditable) — audits R-10 §3.5

| Aspect | Checkable requirement | SC / source |
|---|---|---|
| **Keyboard** | All block ops by keyboard (slash-insert, Enter-split, Backspace-merge, mark toggles, block move); **no trap around embedded non-editable nodes (chips)** — caret passes through and Tab exits (the ProseMirror contenteditable-island pitfall, R-10 §3.5 / §10); slash-menu is an APG-correct listbox/menu, Esc-dismissable. | 2.1.1, 2.1.2 |
| **Screen reader** | Editor exposes correct semantics once (component contract, design-language §4); **block type announced on entry**; the round-trip markdown is the accessible text fallback (R-10 §3.1); a `mention`/`artifact_ref` chip announces its humanised name + type; **IME/composition** events handled for CJK + accented EU input (also G2/R-18). | 4.1.2, 1.3.1, §10 |

### 5.5 HITL approval card — audits R-14 §2/§5

| Aspect | Checkable requirement | SC / source |
|---|---|---|
| **Keyboard** | Approve / **Edit** / Reject reachable and operable; per-effect controls on a multi-effect card individually focusable (R-14 §3.4); Edit-mode form fields keyboard-operable; if the card is in a dialog, the overlay focus contract applies (§5.7). | 2.1.1, 2.4.3 |
| **Screen reader** | The card's **proposed effects, per-effect target (live chip), authority + GATE marker, scope, budget** are all in the accessible name/structure — **not conveyed by layout/colour alone** (R-14 §2); the **agent treatment is announced as text** ("Agent", the attribution string), never colour/icon-only (R-14 §1.1, WCAG 1.4.1); **arrival of a gate-awaiting card is announced politely** (assertive only if it is a critical/blocking gate — §6.1); state transitions (approved/edited/rejected/error/budget-exceeded) announce as text (R-14 §5). | 1.4.1, 4.1.2, 4.1.3 |

### 5.6 Command palette — audits R-08 §11

| Aspect | Checkable requirement | SC / source |
|---|---|---|
| **Keyboard** | APG **editable combobox + listbox**; DOM focus stays on input, `aria-activedescendant` tracks the active row; ↑/↓/Home/End/Enter/Esc per R-08 §3; **focus trapped in the modal and returned on close**; active option scrolled into view at 200% zoom; **no trap** (R-08 §3.3, §11). | 2.1.1, 2.1.2, 2.4.3, APG combobox (§10) |
| **Screen reader** | `role=combobox`/`listbox`/`option`; group headers are non-option separators arrow-nav skips; **result count / loading / error announced via ONE polite live region, debounced — not per keystroke** (R-08 §11 / §6.1); the mode pill is a **word** not colour (M4); a no-access target announces "no access," never a leaked title (M10). | 4.1.2, 4.1.3, 1.4.1 |

### 5.7 Nested overlays — audits R-10 §5.5

| Aspect | Checkable requirement | SC / source |
|---|---|---|
| **Keyboard** | Focus moves in on open; **trapped** in modals (Tab/Shift-Tab cycle, no background escape); **returned to trigger** on close; **Esc closes the top-most only** in a nested stack (Confirm-over-Dialog-over-Popover, R-10 §5.3); a deliberate modal trap you *can* Esc is correct, a trap you *can't* is a Blocker (2.1.2). Tooltip/Toast **never steal focus**; tooltip shows on **focus as well as hover**. | 2.1.2, 2.4.3, APG dialog (§10) |
| **Screen reader** | `role=dialog`+`aria-modal=true`+labelledby/describedby (Confirm = `alertdialog`); **background inert** so AT can't wander; the focus-trap **stack** keeps order correct when nested; Toast via a `status`/`alert` live region (§6.1); hovercard (popover) obeys **WCAG 2.2 1.4.13 dismissable/hoverable/persistent** (R-09 §5.2). | 4.1.2, 1.4.13, APG dialog (§10) |

> **Why these seven (HOUSE STYLE):** they are the components where the "build-once accessible substrate"
> either pays off (overlays §5.7, palette §5.6 — discharged once and inherited) or where bespoke
> rendering most easily breaks AT (diff §5.1, board-drag §5.2, inline-edit §5.3, editor §5.4) — and the
> HITL card §5.5 because agent legibility *is* an a11y duty (colour-alone agent marking fails 1.4.1).

---

## 6. Live-region announcement of event-driven updates without spam (+ status-not-colour + agent treatment)

This is the §9 a11y gloss-risk **R-17 owns**: a real-time, agent-native, multi-surface product fires
*many* background updates; naïvely wiring them to a live region makes the screen reader **unusable from
spam**, while wiring none makes blind users **miss state changes**. The discipline (PROVEN — §10):

### 6.1 The announcement-politeness rules (PROVEN, WCAG 4.1.3 + AT best practice — §10)

| Rule | Spec | Tag |
|---|---|---|
| **Politeness by consequence** | **`aria-live="polite"`** (or `role="status"`) for the **vast majority** — non-critical updates (search results, an inbox item, a card settling, a check turning green): waits for a natural pause, never interrupts. **`aria-live="assertive"`** (or `role="alert"`) **only** for genuinely time-critical/blocking events (a connection-lost error, a *critical* on-call gate). | PROVEN — §10 |
| **Announce changes the user cares about, not every background refresh** | A live PR chip going red→green announces **only if the user is watching/subscribed**; a background firehose refresh that doesn't change *this user's* state is silent (R-09 §8 "announce state *changes* a viewer is watching, not every background refresh"; R-10 §4.5 inbox "announce critical/direct, not every fyi"). | PROVEN-intent + HOUSE STYLE |
| **One region, debounced — not per keystroke** | The palette/search announces a **debounced** result-count via **one** polite region, **not on every keystroke** (R-08 §11). Coalesce rapid-fire updates into a single sensible announcement. | PROVEN — §10 |
| **Don't over-populate** | Avoid many simultaneous live regions; a few well-placed ones (a global polite status region for the shell; an assertive region reserved for critical alerts) (PROVEN — §10 "avoid overloading pages with too many live regions"). | PROVEN — §10 |
| **Region exists before it updates** | The live region is in the DOM **on load** (or, if injected, allow the AT API ~2s before injecting text — PROVEN, §10), so the first announcement isn't dropped. | PROVEN — §10 |
| **Concise, humanised messages** | Announcements are short, humanised strings (no raw ids — §8b.5; R-09 §6.1) — "Check passing", "Moved to In Progress", "Agent proposes 2 changes — awaiting approval". | PROVEN + HOUSE STYLE |

**The checkable test (M6):** drive a surface with a screen reader while a live update fires (a check
flips, a card moves, an agent gate arrives, an inbox item lands) and confirm: it **is** announced, it is
**polite unless critical**, it is **not repeated per background tick**, and rapid updates are
**coalesced** — not a stream of interruptions.

### 6.2 Status-not-by-colour-alone audit (M4 — PROVEN WCAG 1.4.1)

- **Checkable:** render the surface in **greyscale** (or `forced-colors`/high-contrast) — **every**
  status must remain distinguishable by **glyph + label + position** (CI pass/fail, PR/issue state, SLA
  breach, success/warning/danger). If any status collapses to indistinguishable in greyscale → **Blocker
  → G1 fail** (1.4.1). No saturated traffic-light fills (design-language §8b.3 — "the screen is not a
  traffic light").

### 6.3 Agent-treatment audit (M4 + AI-Act legibility — audits R-14 §1)

- **Checkable:** the `agent` treatment must carry **text label ("Agent") + shape (a plain geometric
  glyph, NOT sparkle/shimmer/magic-wand/star) + attribution string** in addition to the reserved
  (measured-contrast) `agent` colour token — **never colour-alone, never emoji** (R-14 §1.1; WCAG 1.4.1;
  design-language §8b.3). The agent colour is a **fourth neutral semantic axis**, never a functional
  status colour (R-14 §1.2). If "an agent did/proposes this" is conveyed by colour or icon alone, or by
  an emoji that can't re-theme for dark/high-contrast/RTL → **Blocker → G1 fail**.

---

## 7. Actionability toward the control artifacts

| Control artifact | What this file equips | Where |
|---|---|---|
| **rubric G1 (the gate)** | The **checkable instrument** G1 references: §4 turns the six G1 bullets into row-level cited items; §5 gives each hard component a keyboard + SR row; §3 the two token rules; §2.3 the demonstrated-not-claimed evidence record + Blocker→fail bands. **G1 is now a binary checklist, not "be accessible."** | §2–§6 |
| **rubric D3 (visual craft, 12%)** | The 2.2-specific SC (M9: 2.4.11, 2.5.8, 3.3.8 + dragging/consistent-help/redundant-entry) are routed to D3 reward, not the gate — the prompt's "reward 2.2 in D3" rule made checkable. | §1, §4 M9 |
| **sketch-funnel ("comparable screens" + token set)** | Every finalist ships a DTCG token set (→ §3 measured-contrast gate) and depicts the hard-gate states on the required screens; the §5 hard-component checklist tells Phase 6 *which* components must be a11y-demonstrated (diff, board, inline-edit, editor, HITL, palette, overlays). Designed-in from sketch #1, not retrofitted. | §3, §5 |
| **R-18 (i18n/RTL, gate G2)** | Hands off the shared concerns: logical-property focus rings, RTL mirroring of the hard components, IME in the editor, non-Latin contrast — flagged here, demonstrated by R-18. | §3.2, §5.4 |
| **Phase 7 panel (Accessibility lens owns G1+G2)** | The per-finalist evidence record (§2.3) is the lens's worksheet; the severity bands give a defensible binary verdict under the pre-registration discipline. | §2.3 |

---

## 8. `[DEFERRED-UNTIL-USERS]` — the assistive-technology USER-testing plan (the ~60% the audit can't catch)

**The no-user substitute is fully executed above** (the automated + manual expert audit method §2, the
measured-token QA §3, the §4/§5/§6 checklists). But **AA ≠ usable-with-AT**: the manual expert audit
catches a *complementary* slice to automation, yet **neither catches whether real AT users can actually
do the job** — comprehension, efficiency, frustration, real AT/browser/OS combinations, real
configurations. This is recorded as an **executable plan, NOT presented as done** (standing rule §A;
rubric G1 "AT user testing deferred to Phase 4"). *(This is the R-17 deferred core per §C of
03-research-prompts.)*

**What to test (the decisive ~60%):**
1. **Task success with real AT** on the seven hard components in real flows: a blind engineer reviews
   the **diff** and adds a line comment (§5.1); a screen-reader user **moves a board card** (§5.2) and
   **edits a views cell** (§5.3); authors a **block-editor** page with a mention (§5.4); **approves/edits
   a HITL card** (§5.5); drives the **palette** to find-and-act (§5.6); operates a **nested overlay**
   (§5.7).
2. **Dynamic-update comprehension** (§6): do live announcements *inform* without overwhelming on a
   busy/agent-active surface? Is the storm/30×-agent-surge inbox (R-10 §4.3) *survivable* with a screen
   reader, or a spam wall?
3. **Magnification & low-vision** (200%–400%, Windows Magnifier / ZoomText): does focus stay on-screen
   (2.4.11), does reflow hold on the dense diff/board?
4. **Switch-access & voice control** (Dragon / Voice Control): are targets nameable and ≥24px (2.5.8)?
5. **Cognitive load** of the agent legibility surfaces (does the HITL card *read* as legible, R-14 §10
   joins here).

**With whom (which persona/segment):**
- **AT users across the disability spectrum**: screen-reader users (blind/low-vision), keyboard-only
  (motor), low-vision/magnification, switch/voice, and cognitive — recruited as **real users of
  Myelin's segments**: engineers (P1–P5) on the diff/board/editor/palette; PM/delivery (P6–P10) on
  views/inbox/HITL; corporate/governance incl. **P13 DPO / P15 admin** on governance/audit surfaces.
- **Method:** moderated **task-based usability testing with AT**, cross-AT matrix (NVDA+Firefox,
  JAWS+Chrome, VoiceOver+Safari, Orca+Firefox; iOS VoiceOver + Android TalkBack for any mobile surface),
  plus an **expert AT-user audit** as the bridge before recruited users exist.

**What would falsify "this is operable with AT":**
- An AT user **cannot complete** a core task on any hard component (e.g. cannot tell added from removed
  in the diff; cannot move a board card; cannot understand what a HITL card will do) → §5 spec failed
  in practice despite passing the audit.
- Live-update announcements make a busy surface **unusable** (spam) or cause users to **miss** critical
  state (silence) → §6 politeness model mis-tuned.
- Task **completion time / error rate** for AT users is disproportionate vs sighted-keyboard users on
  the same task (a "technically operable but practically hostile" failure).
- The agent treatment is **not understood** as "an agent" by an AT user → R-14 §1 / §6.3 failed.

**The honesty caveat (PROVEN — §1, §2.2):** until this runs, **G1-pass means "cleared the expert audit
floor," never "validated usable with AT."** Phase 7 must report G1 as *necessary, not sufficient*.

---

## 9. Completeness-critic (README §9) — gloss-risks R-17 owns vs routes

R-17 **owns** the a11y gloss-risks and is the *audit lens* the others' keyboard specs are checked by:
- **Keyboard operability + no-trap of every hard component** — **OWNED**: §5 (each of the seven with a
  keyboard row); audits R-10/R-08/R-09/R-14's interaction specs.
- **Screen-reader announcement of event-driven updates without spamming** — **OWNED & covered**: §6.1
  (the politeness/debounce/coalesce/one-region rules); M6.
- **Status-not-by-colour-alone** — **OWNED**: §6.2 + M4 (greyscale test, Blocker→fail).
- **Focus-token ≠ identity-token + measured-contrast** — **OWNED**: §3 (the #12 core, both rules
  checkable).
- **Visible focus in light/dark/high-contrast** — **OWNED**: M2 + §2.2 theme sweep.
- **200%-zoom/reflow on dense surfaces** — **OWNED**: M7 + §2.2 zoom sweep (the diff/board are the hard
  cases).
- **Reduced-motion first-class** — **OWNED**: M8 (cross-checks R-12 motion; demonstration there).
- **No-access never leaks to AT** — **OWNED as an a11y row**: M10 (the SR-side of the R-08/R-09
  anti-oracle).
- **Routed (depth owned elsewhere, named not duplicated):** i18n/RTL/non-Latin/IME demonstration →
  **R-18 (G2)** (§3.2/§5.4 flag the shared concerns); the *storm* state-craft → **R-21** (§8 names it as
  an AT-test target); motion-token values → **R-12** (M8 cross-checks); the *interaction* specs
  themselves → R-08/R-09/R-10/R-14 (this file audits, does not re-spec — except the diff §5.1, which had
  no owner).

---

## 10. Sources (web-verified, 2024–2026 + cited internal contracts)

**Standards & legal (PROVEN):**
- WCAG 2.2 (W3C Recommendation; the nine new SC incl. 2.4.11, 2.5.8, 3.3.8; 4.1.1 Parsing obsoleted):
  https://www.w3.org/TR/WCAG22/ · https://tetralogical.com/blog/2023/10/05/whats-new-wcag-2.2/ ·
  https://vispero.com/resources/new-success-criteria-in-wcag22/
- WCAG 2.2 2.4.11 Focus Not Obscured / 2.5.8 Target Size / 3.3.8 Accessible Authentication (AA-new):
  https://www.levelaccess.com/blog/wcag-2-2-aa-summary-and-checklist-for-website-owners/ ·
  https://testparty.ai/blog/wcag-22-new-success-criteria
- EN 301 549 (V3.2.1 incorporates WCAG 2.1 AA; V4.1.1 expected 2026 folds in 2.2) + EAA enforceable
  2025-06-28 + presumption-of-conformity: https://www.deque.com/en-301-549-compliance/ ·
  https://en.wikipedia.org/wiki/EN_301_549 · https://askem.com/compliance/eaa/ ·
  https://www.levelaccess.com/blog/eu-accessibility-requirements-and-eaa-compliance/
- Automated catches only ~30–40% of WCAG issues; hybrid (automated CI + manual expert) required (Deque/
  WebAIM/GDS consensus): https://www.levelaccess.com/blog/automated-accessibility-testing-a-practical-guide-to-wcag-coverage/ ·
  https://testparty.ai/blog/automated-accessibility-testing-guide ·
  https://exceedability.com/manual-vs-automated-testing.html
- ARIA live regions — polite vs assertive, avoid spamming, one region, region-exists-before-update,
  ~2s injection delay (WCAG 4.1.3): https://developer.mozilla.org/en-US/docs/Web/Accessibility/ARIA/Guides/Live_regions ·
  https://www.a11y-collective.com/blog/aria-live/ · https://www.uxpin.com/studio/blog/aria-live-regions-for-dynamic-content/
- Accessible code/diff viewing — +/- not conveyed to SR by colour/glyph alone; code-as-table is hostile;
  VS Code Diff Review Pane (F7/Shift+F7) linear-patch pattern; GitHub code-view a11y; GitLab
  display:block-on-table strips semantics: https://github.com/Microsoft/monaco-editor/issues/411 ·
  https://vscode-docs1.readthedocs.io/en/latest/editor/accessibility/ ·
  https://github.blog/engineering/user-experience/accessibility-considerations-behind-code-search-and-code-view/ ·
  https://gitlab.com/gitlab-org/gitlab-foss/-/issues/59411
- (carried, R-10 §9) W3C ARIA-APG Modal Dialog & Data Grid patterns; (R-08 §15) APG Combobox; (R-09 §12)
  WCAG 2.2 SC 1.4.13 Content on Hover or Focus.

**Internal contracts surfaced (PROVEN-as-existing, not invented):** design-language §4 (a11y baseline),
§8b.3 (measured tokens / focus≠identity / status-not-colour / agents-look-like-agents); rubric.md G1;
R-10 §2.4/§3.5/§4.5/§5.5; R-08 §11; R-09 §8/§5.2/§5.9; R-14 §1/§5; ADR-03 (no-leak pre-filter).

---

## 11. Self-check against R-17 acceptance criteria

| Criterion (prompt R-17) | Status | Evidence |
|---|---|---|
| **Checklist specific enough that G1 is *checkable* (not "be accessible")** | ✅ Met | §4 (10 row-level cited master items + Blocker→fail bands §2.3) + §5 (per-component keyboard+SR rows) + §3 (token rules); "demonstrated-not-claimed" evidence record §2.3 |
| **Every hard component has a keyboard + screen-reader entry** | ✅ Met | §5.1 diff · §5.2 board-drag · §5.3 inline-edit · §5.4 editor · §5.5 HITL card · §5.6 palette · §5.7 nested overlays — each a keyboard row + an SR row |
| **Focus-token rule + measured-contrast rule present** | ✅ Met | §3.1 (contrast measured-not-claimed + accent carve-out) + §3.2 (focus-token ≠ identity-token derivation, checkable: `focus-ring` distinct token ≥3:1 in 3 themes) |
| **Each item cites its WCAG / EN 301 549 criterion; tagged PROVEN** | ✅ Met | §4/§5/§6 every row carries its SC (1.4.3/1.4.11/2.4.7/2.4.11/2.1.1/2.1.2/2.4.3/1.4.1/1.3.1/4.1.2/4.1.3/1.4.4/1.4.10/2.5.8/1.4.13); standards PROVEN, cited §10 |
| **Live-region announcement of event-driven updates without spamming** | ✅ Met | §6.1 (politeness-by-consequence, announce-changes-not-refreshes, one-debounced-region, region-before-update, concise) + M6 checkable test |
| **2.1-floor vs 2.2-target relationship stated correctly** | ✅ Met | §1 (2.1 AA via EN 301 549 = the gate; 2.2 AA = house target; 2.2 ⊇ 2.1 except 4.1.1 obsoleted; reward 2.2 SC in D3) |
| **Audit method = automated pass + manual expert pass per surface** | ✅ Met | §2 (Pass A automated CI net + the ~30–40% cap; Pass B manual expert per-surface sweeps; severity bands; evidence record) + §3 (#12 token QA) |
| **`[DEFERRED-UNTIL-USERS]` AT user-testing recorded as a concrete plan, not faked** | ✅ Met | §8 (what to test — the seven hard components + dynamic updates + magnification/switch/voice/cognitive; with-whom — AT users across the spectrum × persona segments + cross-AT matrix; falsifiers; the AA≠usable-with-AT caveat) |
| **Builds ON R-10 (+R-08/R-09/R-14), doesn't duplicate; audits them** | ✅ Met | §0 + §5/§6 audit-and-cite R-10/R-08/R-09/R-14 by section; only the diff (§5.1, no prior owner) is fully specced here |
| **§9 a11y gloss-risks covered** | ✅ Met | §9 (owns keyboard-ops, SR-announcement-no-spam, status-not-colour, focus≠identity+measured-contrast, focus-in-3-themes, 200%-reflow, reduced-motion, no-leak-to-AT; routes i18n/RTL→R-18, storm→R-21, motion-values→R-12) |
| **PROVEN/HOUSE-STYLE tags + date + cited URLs** | ✅ Met | tagged throughout; dated 2026-06-20; §10 cited (WCAG 2.2, EN 301 549/EAA, 30–40% stat, ARIA live, diff a11y) |
| **Directly supports rubric G1 + self-check** | ✅ Met | §7 mapping (G1 instrument, D3 reward routing, funnel token/screen set, R-18 handoff, Phase-7 lens worksheet); this table |

**Top uncertainties (honest):**
1. **The diff a11y spec (§5.1) is the least-precedented and highest-risk.** The "announce add/remove as
   text + offer a linear SR review mode" requirement is grounded (VS Code Diff Review Pane; Monaco/GitLab
   anti-patterns), but **no widely-loved fully-accessible code-diff** exists to copy — it is the most
   likely silent G1 failure and the most important §8 AT-test target. *Largest uncertainty.*
2. **The audit is a no-user substitute (§8).** The expert audit + measured tokens establish the *floor*;
   **whether real AT users can do the job is unproven** until §8 runs — G1-pass is necessary, not
   sufficient. The live-region politeness tuning (§6.1) especially needs real-AT validation on a busy/
   agent-active surface (spam-vs-silence is hard to get right without users).
3. **The 2.2 vs 2.1 boundary will move:** EN 301 549 V4.1.1 (2026) folds in WCAG 2.2, so today's
   "house-target bonus" 2.2 SC become floor on the regulatory side mid-flight — `[VERIFY]` the EN
   version before any external conformance claim.
4. **Cross-AT variance** (NVDA vs JAWS vs VoiceOver vs Orca) means an audit that passes on one
   AT/browser pair can fail another; the §2.2 ≥2-pair rule mitigates but does not eliminate this — only
   the §8 cross-AT matrix settles it.

---

*End of R-17 deliverable. Date: 2026-06-20. Audit method PROVEN (#21 hybrid audit, #12 measured tokens;
WCAG 2.2 / EN 301 549 / EAA cited); per-component checklist HOUSE-STYLE in layout over PROVEN cited SC;
the diff component §5.1 specced here (no prior owner), the other six audited against R-08/R-09/R-10/R-14;
AT-user testing `[DEFERRED-UNTIL-USERS]` (§8), recorded as a plan not faked. Builds on R-10 (+R-08, R-09,
R-14). Feeds rubric G1 (the checkable instrument), D3, R-18, Phase 6, Phase 7 Accessibility lens.*
