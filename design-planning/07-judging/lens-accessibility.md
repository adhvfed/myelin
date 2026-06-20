# Phase 7 — Lens: Accessibility (owns hard gates G1, G2; scored dimension D9)

> **Judge lens:** Accessibility (a11y audit #21, measured tokens #12, sovereignty blueprint).
> **Method:** silent-first, written independently before panel discussion (rubric Part 3). This is the
> **expert audit floor** per R-17 §1/§2.2 — automated + manual expert pass only. **Assistive-technology
> USER testing is `[DEFERRED-UNTIL-USERS]`** (R-17 §8): every G1 PASS below means *"cleared the expert
> audit floor,"* **never** *"validated usable with AT."* G1-pass is **necessary, not sufficient.**
> **Contrast was recomputed, not trusted** — I ran the WCAG 2.1 relative-luminance formula over every
> finalist's actual DTCG tokens (script + outputs in my worktree), not the README's stated ratios.
> Status date: **2026-06-20.** Tags: **PROVEN** = inspectable in the artifact / a measured number /
> a cited SC. **JUDGEMENT** = my expert call.

---

## 0. How I judged (the instrument)

- **G1** is checked against the R-17 master checklist (M1–M10) + the seven hard-component keyboard/SR
  rows, **demonstrated-not-claimed** on the required screen set (rubric Part 1). A **Blocker** (sub-AA
  text pair, focus removed with no replacement, keyboard trap, status colour-only, leaked title) =
  **G1 FAIL**. Major/minor findings cost D3/are logged but do **not** fail the gate (R-17 §2.3).
- **G2** is checked against the R-18 demonstration set D-G2.1–D-G2.6 (German + non-Latin + a real-string
  mirrored RTL; no truncation; locale dates; no machine strings; logical CSS), shown and inspectable.
- **The Phase-6 medium caveat (JUDGEMENT, applied uniformly):** these are limited-HTML sketches. Where a
  keyboard model is *annotated but not wired in JS*, I treat it as **"claimed, not demonstrated"** — a
  **Major** finding (costs D3 / is a deferred risk), **not** a G1 Blocker, because the gate tests for
  *traps / removed focus / colour-only status*, none of which a missing handler creates. I note this so
  the panel does not read my G1 PASSes as "the keyboard model is proven." It is not; it is **not broken**.
- **Contrast recompute result (PROVEN):** all four finalists' README ratios reproduced within rounding.
  No finalist has a sub-AA text/UI pair on its shipped tokens; every finalist's `focus`/primary-action
  token is a **distinct, derived, AA-safe token ≠ the identity accent** (the §8b.3 / R-17 §3.2 rule).
  The one nuance: **D's status `green-600` measures 4.6–5.0:1 on white but 4.19:1 on the warm `band`
  (neutral-100)** — still ≥3:1 (UI floor) and always paired with glyph+label, so not a Blocker.
  **C's light tertiary ink (`paper.ink-2`/grey-7) measures ~3.98–4.98:1** — below 4.5:1 if used for
  *essential* body text, but it is bound to non-essential meta only; **JUDGEMENT: not a Blocker**, flagged.

---

## Finalist A — "Instrument"

**G1 — PASS** (PROVEN, expert-audit floor; AT-user testing deferred)
- **Contrast (M1):** recomputed — primary/secondary/tertiary text 16.0/9.25/5.81 (dark), 17.93/8.11/5.36
  (light); focus 8.13 (dark)/6.55 (light); all status, diff add/del text, and agent ≥ AA in both themes.
  `focus` (`--c-focus-ring` = derived blue) is a distinct token from `--accent`. **PASS.**
- **Focus (M2):** one `:focus-visible` rule on the derived token (`tokens.css:94`); **forced-colors
  fallback present** (`tokens.css:101`). The single `outline:none` (palette active row) **supplies a
  replacement** 2px inline-start indicator. PASS.
- **Status-not-colour (M4):** strong — diff add/remove carried by literal `+`/`−` text signs
  (`2-pr-diff.html`), CI/PR/board statuses by glyph (`○◐◑●`, `✓`, `⚠`) + word + `title`. PASS.
- **Agent (M4/§6.3):** "Agent" text badge + plain bordered-square mark, no sparkle/emoji; `--agent`
  documented as non-status. PASS.
- **Keyboard (M3):** **real JS** j/k handlers on diff + board, arrow-roving on palette. **Majors
  (JUDGEMENT, not Blockers):** advertised diff keys `] [ . v` and board drag keys are hint-only (not
  wired); palette is `role="dialog" aria-modal` but has **no focus trap and no working Esc** — an
  unfulfilled modal contract, not a trap. Logged; no trap exists.
- **Semantics/reflow/reduced-motion (M5/M7/M8):** landmarks, `aria-busy` skeletons, `lang`, reduced-motion
  global rule all present. Minors: skip link only on screen 1; palette is `listbox`+`textbox`, not the
  canonical `combobox` + `aria-activedescendant`.

**G2 — PASS** (PROVEN)
- German long compounds (`Konfigurierbarer Rate-Limiting-Schwellenwert`,
  `Vorhersagbares Backpressure-Schwellenwert-Verhalten`) with **no truncation** (no ellipsis/overflow/fixed
  width on any text container — grepped). Greek **with tonos** (`Αλέξανδρος Παπαδόπουλος`, `εκκαθάρισης`)
  + Cyrillic (`Очистка устаревших ключей…`). Real Arabic RTL (`7-states.html`) with `<bdi>`/`unicode-bidi:isolate`
  mixed runs (`PR #412`, SHA `a94bcc7`, `myelin://…`, `Mara Ø.`). Locale SLA date
  `Fällig 23.06.2026, 17:00 MESZ`. **Zero physical left/right** in the whole codebase — logical
  properties throughout; no machine strings leak. **All six D-G2 items demonstrated.**
- *Margin note (JUDGEMENT):* RTL is shown in **one mini-shell** (screen 7), not the flagship screens —
  conformant, but less RTL depth than D's full mirrored console.

**D9 — 2 / 4** (JUDGEMENT). Residency chip in the top bar, a DSR row in the sidebar, an erased-tombstone +
erasure-receipt cue. Legible and present, but **on-demand by design** — no always-on lawful-basis band, no
DSR console, no per-holder completeness. The P9 "where does this live / who processed / show me everything"
questions are *gestured at*, not *answerable in depth*. Competent, meets the incumbent bar.

---

## Finalist B — "Workshop"

**G1 — PASS** (PROVEN floor; AT-user testing deferred) — *the weakest-margin pass of the four.*
- **Contrast (M1):** recomputed — text 15.68/7.13/5.47 (light), 14.26/7.54/5.28 (dark); focus (derived
  blue) 6.23/6.49 (light), 7.09 (dark); terracotta primary-button white-on-fill 6.15; all status/agent ≥ AA
  both themes. `focus` ≠ `accent` (terracotta). **PASS.**
- **Focus (M2):** one `:focus-visible` rule, **zero `outline:none` anywhere** (cleanest of the set). PASS.
- **Status-not-colour (M4):** structurally enforced glyph + label; diff +/- via generated `::before`
  text content. PASS. Agent = "Agent" label + plain geometric SVG, no emoji. Serif (Fraunces) editor body
  causes no a11y harm.
- **Majors (JUDGEMENT, not Blockers):** **keyboard model is annotation-only** — no `<script>`, no
  `keydown`, no roving tabindex, no `aria-activedescendant` (j/k/n/p hints unwired); **no skip link;
  no `role=grid/listbox/combobox`** (diff is `role=region`, palette never a combobox); screen-6 state
  gallery has no landmarks.
- **Flagged gap (Major, not Blocker):** **no `forced-colors`/high-contrast media query anywhere** — the one
  G1 omission B has that A and C/D-context handle differently (A has it; C/D also lack it). Mitigated by
  glyph+label status; does not collapse a gate item, but it is the brief's explicit ask and is missing.
  Reflow, reduced-motion (4 blocks), `aria-busy`, `lang` (+ inline `lang="bg"`) all present.

**G2 — PASS** (PROVEN) — *exemplary i18n.* German compounds with **soft-hyphen** wrap hints and no
truncation; Greek-with-tonos + Cyrillic (`lang="bg"`); real-Arabic full-shell RTL (`5-g2-rtl-i18n.html`)
with `<bdi>` isolation of `PR #412`/`myelin://…`/`@Mara Ø.`; **Eastern-Arabic-digit** locale SLA
(`يُستحق ٧‏/٧‏/٢٠٢٦…`) + de-DE `due 07.07.2026, 17:00 CEST`; **zero physical left/right** (grepped);
no machine strings leak. All six D-G2 items demonstrated, with extra depth (soft hyphens, Eastern digits).

**D9 — 2 / 4** (JUDGEMENT). "Sovereign" + residency cue in the shell band, per-surface `Residency` lines on
diff/hitl/knowledge, and a genuine **GDPR DSR erased/tombstone state with a verifiable receipt**
(`subject=jv-2026-0118 · cells=eu-west · sha256:9f2a…c107`) + permission-denied "Request access" no-leak.
But there is **no DSR console, no lawful-basis band, no completeness/saga** — sovereignty is a chip + one
state, not a first-class surface. Competent; ties A.

---

## Finalist C — "Wayfinding"

**G1 — PASS** (PROVEN floor; AT-user testing deferred)
- **Contrast (M1):** recomputed — text strong in both themes; focus (derived blue) 8.19 (dark)/4.92
  (light, ≥3:1 UI floor); accent orange is **never a sole status carrier** (verified — the meaning channel
  is always glyph + word + ref). `focus` ≠ `accent`. **PASS.** *Flag (JUDGEMENT):* light tertiary ink
  measures ~3.98–4.98:1 — bound to non-essential meta only, so not a Blocker, but it is the thinnest
  contrast margin in the set; verify it never carries essential text.
- **Status-not-colour (M4):** strong — every status `glyph + word` (`✗ failed`, `✓ 11 passed`, `◴ running`,
  Greek `◐ Σε εξέλιξη`). Agent = label + plain square/pentagon, SVG commented "NOT a sparkle/wand." PASS.
- **Focus (Major, JUDGEMENT):** `outline:none` on the **command-palette input** with **no replacement**
  (`05-palette.html:23`) — the keyboard-first surface's primary, autofocused control has no visible ring.
  This is the closest thing to a Blocker in the set; I rule it **Major not Blocker** only because the
  global `:focus-visible` covers every *other* control and the palette is one screen — but it is a real
  visible-focus failure on the one control that matters most for this concept. **Panel should weigh this.**
- **Majors (JUDGEMENT, not Blockers):** **no JS keyboard model at all** (`<kbd>` hints decorative;
  `role="button"` chain has no Enter/Space handler); palette `role="dialog"` **missing `aria-modal`** +
  no listbox/combobox/`aria-activedescendant`/`aria-selected`; **no skip link; no `aria-busy`/live region**
  on the loading skeleton; **no forced-colors fallback.** Landmarks, `lang`, reduced-motion present.

**G2 — PASS** (PROVEN) — German compounds wrap freely (two ellipsis-clip sites hold **English only** —
latent risk, not a present defect); Greek-with-tonos present; **Cyrillic absent** (Greek alone satisfies
the AND/OR floor, so still PASS — but it is the only finalist with no Cyrillic proof). **Full mirrored
shell + content + overlay** real-Arabic RTL (`06-rtl-arabic.html`) with `<bdi>` isolation; de-DE SLA
`fällig 07.07.2026, 17:00 MESZ` + `0,04 €` comma decimals; **zero physical left/right** (grepped,
best-in-class); no machine strings leak. All required D-G2 items demonstrated.

**D9 — 2 / 4** (JUDGEMENT). Always-on-ish residency scope (`⌖ EU-West · Frankfurt cell`) per surface +
per-artifact residency unfurl in the palette + a GDPR erased/crypto-shred state with "audit hash never
rewritten" + a by-construction Restricted-reference no-leak. Stronger residency *presence* than A/B, but
**no DSR console, no lawful-basis (Art. 6) band, no completeness/receipt-proof surface** — so it cannot
answer "show me everything about this subject" as a surface. Competent.

---

## Finalist D — "Civic"

**G1 — PASS** (PROVEN floor; AT-user testing deferred)
- **Contrast (M1):** recomputed — text 17.41/7.68/4.80 (light), 14.28/7.28/4.33 (dark, meta only); accent
  8.24, accent-text/focus 10.26 (light), 8.00 (dark); status ok/warn/bad 5.04/5.69/6.84 on white; agent
  7.61; **white-on-accent-fill 8.24**, and the focus ring is **visible-by-offset** (`outline-offset:2px`
  real token, accent-fill buttons inherit the global ring → genuine surface gap). `focus` ≠ `accent`
  (blue-700 vs blue-600). **PASS.** *(green-600 = 4.19:1 on the warm band — ≥3:1 UI floor, glyph+label
  always — not a Blocker.)*
- **Status-not-colour (M4):** **exemplary** — distinct glyphs per state (`✓ Gelöscht`, `▣ Behalten,
  pseudonymisiert`, `✗ Re-Index fehlgeschl.`, `◐ Gefährdet`, `■ Nicht im Plan`, `▣ Restricted` vs
  `— No access`). Agent = "agent" label + plain bordered square. PASS.
- **Focus (M2):** one `:focus-visible` token rule; dense roadmap uses inset ring to avoid clip; the one
  `outline:none` (palette input) sits in an `aria-modal` autofocused dialog — acceptable pattern. PASS.
- **Majors (JUDGEMENT, not Blockers):** roadmap is **not a true roving tabindex** (all rows `tabindex=0`)
  and advertises `Enter`/`x` with no handler; palette's "focus-trap/Esc/listbox" is declared in ARIA/hints
  with **zero JS** + no `aria-activedescendant`. **One outright sub-item FAIL (Major):** the loading
  skeleton has **no `aria-busy`/live region** — SR users get no load signal. **No forced-colors fallback.**
  Skip link only on screen 1. Landmarks (+ inline `lang="el"`/`lang="bg"`), reduced-motion present.

**G2 — PASS** (PROVEN) — German compounds (`EU-Datenresidenz-Steuerung`, `Offline-Sync-Warteschlange`,
`Einschränkungs-Unterdrückung`) with no clip; Greek-with-tonos + Cyrillic (inline `lang`); **strongest RTL
of the set** — full mirrored DSR console (`08-rtl-mirror.html`), zero physical left/right in the RTL file,
`<bdi>` isolation of `myelin://…`/SHA/`@mara.o` and even the `[OPEN — LEGAL]` token; de-DE SLA deadline
clock `fällig 26.06.2026` + `96,4 %` (narrow-space). All six D-G2 items demonstrated.

**D9 — 4 / 4** (JUDGEMENT, **flagged `[UNDER-EVIDENCED]` per R-19**). The reason-to-exist: an **always-on
residency + lawful-basis band carrying Art. 6(1)(b) + BYOK + processor scope** (not just region); a full
**DSR console** — five-article holder ops, **per-holder erasure across all five surfaces + derived holders**,
**completeness as the unit ("9 von 10 Haltern")**, a **failure-isolated saga** (Chat holder retries
independently — "nie ein stiller Teilabschluss"), **per-holder residency** (incl. an honest cross-cell
EU-WEST anomaly), a **verifiable Merkle-receipt** ("Beweis, kein Versprechen" + inclusion-proof button +
independent-witness line), a **deadline clock** (Art. 12(3)), **consequence-first** erasure copy, and an
honest **`[OPEN — LEGAL]` residual** that refuses to claim completeness it can't deliver. **All three P9
questions are answerable in the UI** (residency band + per-holder column; one-`correlation_id` provenance
walk; the console *is* "everything about this subject"). The markup substantiates the prose.
**FLAG (R-19):** there is no external playbook for sovereignty-as-UX; "a DPO trusts it at a glance" is the
**unproven keystone**, falsifiable only by the deferred regulated-buyer (P13/P14) review. The 4 reflects
**coverage + craft against the blueprint, not validated trust.**

---

## D9 scores (summary)

| Finalist | D9 | One-line rationale |
|---|:--:|---|
| A Instrument | **2** | Residency chip + DSR row + erasure receipt; on-demand, no console. |
| B Workshop | **2** | Residency band + a real DSR erased-receipt state; no console/lawful-basis surface. |
| C Wayfinding | **2** | Per-surface residency + crypto-shred no-leak state; no console/Art.6 band. |
| D Civic | **4** | Always-on lawful-basis band + full DSR console (saga, completeness, Merkle receipt, [OPEN-LEGAL]); answers all three P9 questions. **[UNDER-EVIDENCED]** |

---

## Lens verdict (≤1 paragraph)

**All four finalists are CONFORMANT** — each clears **G1 and G2** at the **expert-audit floor**
(contrast recomputed and verified, focus≠identity, status-never-colour-alone, real German + non-Latin +
real-string mirrored RTL with `<bdi>` isolation, locale dates, logical-property CSS with zero physical
left/right, no leaked machine strings). **No hard Blocker** was found in any finalist; **none is
non-conformant.** The most serious accessibility risk in the set is **the universally aspirational
keyboard layer**: every finalist *annotates* a keyboard model but only A and D wire any real handlers,
none ships a true roving-tabindex + `aria-activedescendant` listbox/combobox or a working modal
focus-trap/Esc, and **C removes the visible focus ring on its keyboard-first command-palette input with
no replacement** (`05-palette.html:23`) — the single closest-to-Blocker finding, which the panel should
weigh against C's "wayfinding/keyboard" thesis. Secondary risks: **no `forced-colors` fallback** in B, C,
and D (A is the only one with it), and **no `aria-busy`/live region** on D's loading skeleton. These are
**Major** (cost D3 / are deferred-AT risks), **not gate failures** — but they mean every G1 PASS here is
*"cleared the floor,"* **not** *"usable with AT"* (AT-user testing `[DEFERRED-UNTIL-USERS]`, R-17 §8).
**D9 leader: Finalist D, score 4 — flagged `[UNDER-EVIDENCED]` per R-19** (no sovereignty-as-UX playbook
exists; DPO-trust is the unproven keystone); D's lawful-basis band + DSR console is the only entry that
makes the P9 questions answerable as first-class surfaces. A, B, and C tie at D9 = 2.
