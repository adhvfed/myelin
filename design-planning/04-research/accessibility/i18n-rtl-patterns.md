# R-18 — i18n / l10n / RTL Interaction-Pattern Research (the G2 basis)

> **Phase 4 research corpus** · deliverable of prompt **R-18** (workstream ws-c, Seq #16).
> **File date: 2026-06-20.** Methods: **#21 (a11y audit — the i18n/RTL portion)** + **#6 (IA
> labelling as an i18n surface)**. This file is the **checkable basis for rubric hard-gate G2**
> (i18n/l10n/RTL): it turns "be international" into a per-pattern, per-component set of rules and a
> **named, demonstrable G2 set** that Phase-6 finalists must show *from sketch #1* (rubric Part 5).
> It is the i18n/RTL twin of R-17 (which owns G1).
>
> **Builds ON prior `04-research` (does not duplicate — extends):**
> - [R-06 platform-ia](../ia/platform-ia.md) §6 (labels held in **tokens/config**, externalised,
>   i18n-ready; the persona-adaptive vocabulary; the depth-≤4 tree) and §3.5 (the §8b.4 shell
>   containment mandates) — labels are an i18n surface *because* R-06 already externalised them.
> - [R-08 command-palette](../interaction/command-palette.md) §2.2 (mode pill at `logical-start`),
>   §9.1 (the **Language facet**, non-Latin query input), §11 (the palette's i18n/RTL a11y hooks) —
>   this file owns the *demonstration* R-08 deferred to it.
> - [R-09 reference-unfurl](../interaction/reference-unfurl.md) §2.1 / §6.1 / §8 (humanised strings
>   sourced at the backend; chip truncation must survive +35% expansion + non-Latin + RTL mirroring).
> - [R-10 shared-patterns](../interaction/shared-patterns.md) §2.4 (views grid + RTL), §3.5 (editor
>   IME/composition + bidi), §4 (inbox humanised strings via the render path), §5 (overlay
>   substrate — overlays must mirror) — **these are the components that must survive expansion + RTL.**
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = a cited external standard/source (Unicode/CLDR,
> WHATWG/MDN, WCAG, ECMA-402 `Intl`, Material bidi, Google Fonts metrics) OR an existing Myelin
> contract this file *surfaces* (R-06 externalised labels, §8b.5 backend humanisation, ADR-13).
> **HOUSE STYLE** = our design synthesis/taste. **`[VERIFY]`** = time-sensitive. The patterns are an
> **expert audit** (the no-user substitute); the i18n/RTL *correctness* is PROVEN-by-standard, but
> **comprehension/quality of localized copy** is a translator/locale-user question — see §10.

---

## 0. How to read this file

1. **§1** — the thesis + the five i18n/RTL problem classes G2 binds.
2. **§2** — **text expansion** (German +35%; the no-truncation rules; the §8b.4 fixed-width bug classes).
3. **§3** — **non-Latin scripts** (Greek/Cyrillic font coverage, line-height/clipping, the self-hosted
   type constraint).
4. **§4** — **full RTL** via logical properties: the whole shell + editor + views + overlays mirrored,
   what mirrors and what must NOT, tested with a **real RTL string**.
5. **§5** — **locale-aware dates/numbers/calendars** (the SLA/business-calendar load-bearing surface).
6. **§6** — **humanised machine strings** (no raw ids; sourced at the backend — surfaced from §8b.5).
7. **§7** — the **G2 demonstration set** (the exact thing every Phase-6 finalist must show) + the
   per-component i18n/RTL obligation matrix.
8. **§8** a11y/G1 intersection · **§9** rubric/funnel actionability · **§10** `[DEFERRED-UNTIL-USERS]`
   · **§11** completeness-critic · **§12** sources · **§13** self-check.

---

## 1. The thesis + the five problem classes

> **Myelin is *operated and read* in the user's own EU language — and, for Arabic/Hebrew-speaking
> EU residents and tenants, in their own writing direction — with no truncation, no clipping, no
> mojibake, and no raw machine strings, because i18n is designed into the shared substrate (one
> shell, one chip, one editor, one views component, one overlay layer) rather than bolted onto each
> surface.** *(HOUSE STYLE thesis; mechanically enabled by R-06 externalised labels + §8b.5 backend
> humanisation + the design-language §4 i18n/RTL baseline, which are PROVEN-required.)*

For an EU-sovereign product i18n is a **requirement, not an enhancement** (design-language §4; rubric
G2): an English-only or LTR-only build is **ineligible for its core market** (EU public-sector
procurement, multilingual operation as part of the sovereignty value proposition), the same way a
G1 failure is — disqualifying *before* aesthetics (rubric Part 1). The five problem classes G2 binds:

| # | Problem class | The failure it prevents | Owner section |
|---|---|---|---|
| **C1** | **Text expansion** | German/Finnish strings clipped, truncated, overflowing fixed widths | §2 |
| **C2** | **Non-Latin scripts** | Greek/Cyrillic glyphs missing (tofu) or clipped by tight line-height | §3 |
| **C3** | **RTL / bidi** | Arabic/Hebrew shell not mirrored; physical left/right leaks; bidi text corrupted | §4 |
| **C4** | **Locale formatting** | `06/07` ambiguous dates, wrong decimal mark, SLA breach computed on wrong calendar | §5 |
| **C5** | **Machine strings** | `merge_request merged`, raw ids, untranslated enum keys leaking to the UI | §6 |

These map 1:1 to the **five G2 pass conditions** (rubric Part 1, Gate G2) — §7 makes each *demonstrable*.

---

## 2. Text expansion (C1) — German +35%, no truncation, no fixed widths

### 2.1 The expansion budget (PROVEN data → design constraint)
German labels run **~30–40% longer than English** (compound nouns: *Einstellungen* = "Settings";
*Bitte geben Sie Ihre Daten unten ein* = +30% over "Please enter your details below"). **Short
strings expand worst — design for up to ~2× on a single short label/button** (PROVEN — industry
localization data: [SimpleLocalize, text expansion](https://simplelocalize.io/blog/posts/text-expansion-ui-localization/);
[jem-products, managing text expansion 2025](https://jem-products.com/how-to-manage-text-expansion-in-translation-localization-2025/);
[Wordpar, German grammar breaks UI](https://wordpar.com/german-grammar-ui-localization-fix/)). The
budget below is the design constraint (HOUSE STYLE thresholds over the PROVEN data):

| String length | Plan for expansion | Example |
|---|---|---|
| Very short (≤10 chars: buttons, nav labels, chips) | **up to +100% (2×)** | "Run" → "Ausführen"; "Save" → "Speichern" |
| Short (10–20: field labels, menu items) | **+50%** | "Pull request" → "Pull-Request" / "Änderung" |
| Medium (20–50: provenance lines, descriptions) | **+35%** | inbox "why it fired" lines |
| Long (>50: body prose) | **+20–30%** | doc paragraphs, empty-state copy |

### 2.2 The no-truncation rules (HOUSE STYLE over §8b.4 PROVEN bug classes)
**The verdict: layouts must *grow*, not *clip*.** "Fixed width + `overflow:hidden` + truncate" is a
G2 failure (it is also an a11y failure — content loss; [SimpleLocalize, localization as
accessibility](https://simplelocalize.io/blog/posts/localization-and-accessibility/)). Binding rules:

1. **No fixed-width text containers.** Containers size to content (`min-content`/`max-content`/`fit-content`
   or `ch`-based maxima), never a `px` width chosen for the English string. Use the spacing scale for
   padding, never to reserve "exactly enough" for the English word (R-06 §6 labels are config; their
   *width* is never assumed).
2. **No CSS truncation of essential labels.** `text-overflow:ellipsis` is allowed **only** where the
   full text is recoverable losslessly elsewhere — the §5.2/R-09 rule: ellipsis + full text in the
   `title`/peek/tooltip (PROVEN — R-09 §2.1). A nav label, a button label, a status word: **never
   truncated** — they wrap or the container grows.
3. **Two-line tolerance on dense labels.** Buttons/chips/nav items must remain legible at **2 lines**
   (vertical growth, not horizontal clipping); the shell regions (R-06 §3) must not assume single-line
   labels. Icons + label use `align-items:start` so a wrapped label doesn't desync from its glyph.
4. **Numbers and the label are separate slots.** A count badge ("12") never shares a fixed box with a
   word that expands (German "12 Probleme") — the §8b.4 "fixed-width assumption" bite.
5. **Pseudo-localization is the cheap pre-flight test** (PROVEN — the standard technique): render the
   UI with each string **+40% padded and accented** (`[!!! Ŝéttîñgŝ !!!]`); *every element that breaks
   under pseudo-loc will break under real German* — so it is a Phase-6 self-check, not a post-hoc QA
   ([SimpleLocalize](https://simplelocalize.io/blog/posts/text-expansion-ui-localization/)). **HOUSE
   STYLE mandate:** a finalist's German screen IS the real-string version of this test.

### 2.3 The §8b.4 fixed-width-assumption bug classes to design around (named, PROVEN)
Per the standing instruction to *name* the §8b.4 classes; these bite first under expansion + RTL:

- **Pinned-shell single-line labels** — rail/secondary-nav labels assumed one line; German wraps →
  rail overflows or clips. *Fix:* the §3 shell regions size to content + 2-line tolerance.
- **`width:100%` panel beside a present column** — under RTL/expansion a drawer that should take over
  is clipped off the `inline-start` edge. *Fix:* collapse the other column at the breakpoint (R-06 §3.5).
- **Hover-only row actions** — not touch-reachable AND their labels expand off the row. *Fix:* explicit
  affordance + content-sized action slot.
- **Popover under a bottom-pinned composer renders off-screen** — worse in RTL (flips to the wrong
  edge). *Fix:* flip + max-height + **test against the REAL anchor in RTL** (R-10 §5.3; §4.4 here).
- **Fixed-width buttons / badges / SLA timers** — the count/word/timer clips under expansion. *Fix:*
  content-sized, two-slot.

---

## 3. Non-Latin scripts (C2) — Greek/Cyrillic coverage, line-height, no clipping

### 3.1 Font-coverage requirement (PROVEN — surfaces design-language §3.3)
The UI sans + monospace **must cover broad Latin-extended + Greek + Cyrillic at minimum** as a
*selection criterion* (PROVEN — design-language §3.3). Binding rules:

1. **Glyph coverage is a font-selection gate, not a runtime fallback.** A missing glyph renders as
   **tofu (□)** — a visible "feels-broken" tell and a comprehension failure. The chosen variable
   sans/mono must ship Greek + Cyrillic in the *same* family so weights/metrics match (no Frankenstein
   fallback that shifts baseline/weight mid-string). *(HOUSE STYLE rule over the §3.3 criterion.)*
2. **Self-hosted, no font CDN** (PROVEN — design-language §3.3 / ADR-11/12: no request metadata
   leaving the cell to a font host). This is a **sovereignty constraint that intersects i18n**: the
   self-hosted family must therefore *itself* carry full EU-script coverage — we cannot lean on a
   third-party multilingual CDN font. Subset per script, lazy-load non-Latin subsets, but **never**
   route to an external host.
3. **A documented per-script fallback stack** (Greek → Cyrillic → Latin-ext within the family;
   last-resort system font) so a rare glyph degrades to a *legible* fallback, never tofu. `[VERIFY]`
   the shipped family's coverage against the EU-24 language set before Phase 8.

### 3.2 Line-height & clipping (PROVEN — Google Fonts / SIL vertical-metrics guidance)
Greek and Cyrillic are within the "standard" vertical-metric band *with Latin* — but **diacritics are
the clipping risk**: stacked/combining marks (Greek tonos+dialytika, Cyrillic breve, Latin-ext
caron/ring) overflow a tight line-box (PROVEN — [Google Fonts, vertical metrics
guide](https://googlefonts.github.io/gf-guide/metrics.html); [SIL, line
metrics](https://silnrsi.github.io/FDBP/en-US/Line_Metrics.html)). Rules:

1. **Line-height is a token with diacritic headroom, not 1.0.** A 120%-line-height assumption "can be
   too tight" once you cover more than basic Latin (PROVEN — Google Fonts). The design-language type
   scale's line-heights (§3.3) **must be validated against accented Greek/Cyrillic + Latin-extended**
   (Czech *ř*, Polish *ł/ż*, Hungarian *ő/ű*, Greek *ΐ*, Cyrillic *й*), not English. *(HOUSE STYLE:
   set body line-height with ≥ the family's recommended metric, never `line-height:1`.)*
2. **No `overflow:hidden` on text rows that can carry diacritics.** A single-line chip/cell/badge with
   `overflow:hidden` + a too-tight height **clips the tonos/caron** — the non-Latin twin of the
   truncation bug (§2.2). Row heights derive from line-height + padding tokens, never a `px` height
   tuned for lowercase Latin.
3. **Monospace coverage matters too** — code/log/diff/SHA surfaces are monospace (load-bearing,
   §3.3); a Greek/Cyrillic *comment* in a diff or a log line must render in a mono that covers it
   without falling back to a proportional face (which would break column alignment).
4. **`lang` attribute set correctly** so the browser applies correct script shaping, hyphenation, and
   font features (PROVEN — HTML spec; also a G1 SC 3.1.1/3.1.2 obligation, R-17).

---

## 4. Full RTL (C3) — logical properties, whole-shell mirroring, a REAL RTL string

This is the §9 gloss-risk this item **owns**: *RTL mirroring of the **whole shell**, not just text
direction* (completeness-critic). The acceptance criterion: mirroring covers **shell + editor + views
+ overlays**, tested with a **real RTL string** (Arabic/Hebrew), not a flipped mockup.

### 4.1 The mechanism: logical properties everywhere (PROVEN — CSS Logical Properties)
**One source of truth, no `[dir=rtl]` override sheet.** Replace every physical
`left/right/margin-left/padding-right/text-align:left/float:left` with **logical**
`inline-start/inline-end/margin-inline-start/text-align:start/float:inline-start`; the inline axis
flips automatically with `dir`/`direction` (PROVEN — [MDN, CSS logical
properties](https://developer.mozilla.org/en-US/docs/Web/CSS/Guides/Logical_properties_and_values/Floating_and_positioning);
[rtlstyling.com](https://rtlstyling.com/posts/rtl-styling/); [Mozilla Firefox RTL
guidelines](https://firefox-source-docs.mozilla.org/code-quality/coding-style/rtl_guidelines.html)).
Binding rules (HOUSE STYLE over the PROVEN mechanism):

1. **Physical `left/right` in component CSS is banned** (a lint rule, the i18n twin of the §8b.1
   "magic z-index" ban). `start/end` only. This makes RTL **free and correct by construction**, the
   same way R-09's chip is non-leaking by construction.
2. **Set `dir` at the document root from the locale**; bidi-isolate embedded opposite-direction runs
   with `dir="auto"` / `<bdi>` / `unicode-bidi:isolate` so an Arabic title inside an LTR chrome (or an
   LTR `myelin://`/SHA/`@handle` inside Arabic prose) doesn't scramble (PROVEN — bidi isolation;
   [rtlstyling.com](https://rtlstyling.com/posts/rtl-styling/)). **Code, SHAs, URLs, and
   `ArtifactRef` handles stay LTR inside RTL prose** via `<bdi>`/isolation — a load-bearing rule for a
   dev tool (a diff or a `myelin://` ref must never visually reverse).
3. **Logical properties also cover scroll, position, and border-radius** (`inset-inline-start`,
   `border-start-start-radius`) — the corners of a card/popover mirror too (PROVEN — MDN logical
   floating/positioning).

### 4.2 What mirrors vs. what must NOT (PROVEN — Material bidirectionality)
Mirroring the *layout* is necessary but not sufficient; **icons and some content must be handled
selectively** (PROVEN — [Material Design 3, Bidirectionality &
RTL](https://m3.material.io/foundations/layout/bidirectionality-rtl);
[Material, bidirectionality](https://m2.material.io/design/usability/bidirectionality.html)):

| Mirror in RTL | Do NOT mirror in RTL |
|---|---|
| **Whole layout** — rail on `inline-start` (visually right), context pane on `inline-end`, sidebars, alignment, indentation | **Logos, brand marks, photos** |
| **Directional/navigational icons** — back/forward, breadcrumb chevrons, "next step", tree-disclosure carets, expand-into-context-pane arrows | **Media transport controls** (play/pause/seek are always LTR) |
| **Progress / linear time** — burndown, CI pipeline DAG left→right becomes right→left (a delivery timeline reads in reading order) | **Clocks & circular progress** (a refresh spinner stays clockwise) |
| **Sliders whose fill implies direction** | **Checkmarks, symmetric glyphs, the `agent` glyph, status glyphs** (non-directional) |
| **The text-direction-bearing icons** (e.g. an alignment/list icon, a `?` for Arabic/Farsi) | **Numbers themselves** (digits render in the locale's numbering system but are not "mirrored") |

**The Hebrew exception (PROVEN — Material):** linear timelines and media controls **stay LTR in
Hebrew**; the `?` mark mirrors in Arabic/Farsi but **not** Hebrew. So "RTL" is not one switch — the
demonstration string's *language* matters (§7). *(Surfaced as a rule; we ship Arabic + Hebrew as the
two RTL test locales precisely because they differ.)*

### 4.3 The whole shell + the four shared components mirrored (the acceptance core)
Each must mirror correctly — naming exactly what flips, building ON R-06/R-08/R-09/R-10:

- **The shell (R-06 §3):** primary rail → `inline-start` (renders on the right in RTL); contextual
  sidebar → `inline-start` of content; context pane (the wedge surface) → `inline-end` (renders on the
  left); top-bar scope selector + `⌘K` + Inbox + identity badge reflow to logical order; **breadcrumb
  chevrons reverse** (R-06 §5.2 web URL is LTR/`<bdi>`-isolated but the *rendered* breadcrumb mirrors).
- **The command palette (R-08):** mode pill, scope chip, query chips, and result rows already use
  `logical-start` (R-08 §2.2/§11) — they mirror for free; the kbd-hint moves to `inline-end`; the
  "drill" `Tab` direction and `→` peek follow reading order (a `→` in RTL peeks toward `inline-end`).
- **The views component (R-10 §2):** table columns lay out `inline-start`→`inline-end` (first column
  on the right in RTL); board columns reverse order; the timeline/Gantt mirrors (bars grow toward
  `inline-end`); the frozen-first-column freezes on `inline-start`; **keyboard arrows are
  reading-relative** (`→` moves toward `inline-end`) so roving-tabindex stays intuitive in RTL
  (R-10 §2.4). Drag targets mirror with the columns.
- **The editor (R-10 §3):** paragraph direction per-block (`dir="auto"`) so a mixed Arabic+code doc is
  legible; block handles move to `inline-start`; the slash-menu and mention/ref chips mirror; **IME /
  composition events** (already in R-10 §3.5) cover CJK + accented EU input — RTL adds the bidi caret
  movement (logical caret motion, not physical). The markdown-subset string is direction-neutral; the
  *render* is bidi-correct.
- **The overlays (R-10 §5):** the substrate positions popovers/dropdowns by `inline-start/end` anchor
  (flip logic is reading-relative); dialogs center (no change); the toast region moves to the logical
  corner; **focus order follows reading order**. The "flip when off-screen, test the REAL anchor" rule
  (R-10 §5.3) must be **re-tested in RTL** — the off-screen edge is the opposite one (§2.3 bug class).

### 4.4 Tested with a REAL RTL string, not a flipped mockup (the binding method)
A mockup flipped in a design tool hides the hard bugs: **bidi runs, digit shaping, isolation of
LTR tokens, and the asymmetric mirroring (§4.2)**. The mandate (HOUSE STYLE): the RTL demonstration
**renders actual Arabic or Hebrew content** — a real issue title, a real chat message, a real chip —
with **at least one mixed-direction run** (Arabic prose containing an LTR `myelin://` ref / SHA /
`@handle`) so the `<bdi>` isolation is *proven*, not assumed. A flipped Lorem-ipsum screen does **not**
satisfy G2.

---

## 5. Locale-aware dates / numbers / calendars (C4) — the SLA load-bearing surface

### 5.1 Never hand-format; use the platform locale APIs (PROVEN — ECMA-402 `Intl` / CLDR)
Date/number formatting is "one of the areas with the most variation across locales" — month/day
order, separators, decimal mark, grouping, calendar, 12/24h all differ; **`Intl.DateTimeFormat` /
`Intl.NumberFormat` handle this from CLDR data** and are the standard, not a hand-built formatter
(PROVEN — [MDN Intl](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Intl);
[Smashing, the Intl API guide 2025](https://www.smashingmagazine.com/2025/08/power-intl-api-guide-browser-native-internationalization/)).
Binding rules:

1. **No string-concatenated dates/numbers in the UI, ever.** `06/07/2026` is ambiguous (US vs EU);
   render via `Intl.DateTimeFormat(locale, {dateStyle})` so a German user sees `07.06.2026`, a French
   user `07/06/2026`, an ISO context `2026-06-07`. Decimal mark / thousands grouping via
   `Intl.NumberFormat` (German `1.234,56`, English `1,234.56`).
2. **Relative time is localized too** (`Intl.RelativeTimeFormat`): "2m ago" / "vor 2 Min." — the
   inbox/chip "updated 2m ago" (R-09 §7.2) and notification provenance lines localize.
3. **Cache one formatter per (locale, options) pair** — constructing per-row is the slow path (PROVEN —
   [Smashing 2025](https://www.smashingmagazine.com/2025/08/power-intl-api-guide-browser-native-internationalization/));
   relevant for the views component rendering thousands of date cells (R-10 §2).
4. **Plurals via ICU/`Intl.PluralRules`, not `count + "s"`** — "1 issue / 2 issues" vs the 4–6 plural
   categories of Polish/Russian/Arabic; an English-shaped `n + 's'` is a C5-class machine-string tell.
5. **This is a backend-sourced humanisation too** (§6 / §8b.5): the *value* is locale-formatted at the
   render surface, but the *template* ("{n} {issues}", "{actor} updated {ref}") is one templating
   surface (notifications + Refs), so every consumer inherits correct plural/format for free.

### 5.2 SLA / business-calendar awareness — load-bearing, locale-AND-tenant-scoped (PROVEN-direction)
SLAs and business calendars are where locale formatting becomes **load-bearing, not cosmetic**
(design-language §4: "business-calendar awareness matters for SLAs"). A breach time computed on the
wrong working week/holiday set is a *correctness* bug, not a display bug (PROVEN-direction —
[Deviniti, SLA business hours](https://deviniti.com/support/addon/cloud/sla-time-management/latest/use-cases-set-up-business-hours/);
[Atlassian, SLA calendars done right](https://community.atlassian.com/forums/App-Central-articles/SLA-calendars-in-JSM-done-right-tips-tricks-and-time-savers/ba-p/3143788)).
Rules (HOUSE STYLE over the PROVEN SLA-calendar pattern):

1. **SLA timers display in the *viewer's* timezone + locale format** ("Due 07.06., 17:00 CEST" /
   countdown "in 3h 12m" localized), while the **breach is computed on the *policy's* business
   calendar** (working days/hours, breaks, holidays). Two distinct things — never conflate display
   locale with calculation calendar.
2. **Business calendars are configurable per region/team** (working days Mon–Fri *or* Sun–Thu for
   some EU/Gulf-facing tenants; working window; holidays imported per **ISO 3166** country) — surfaced
   in the §7.6/R-06 admin tree, not hardcoded. A Friday/Saturday vs Saturday/Sunday weekend is a real
   EU-tenant variation, not an edge case.
3. **Weekend / non-working / holiday differs by locale** — the SLA gauge and the "due" chip must
   reflect the *applicable* calendar, and the **deadline label localizes** (date format) while the
   **remaining-time honors working hours**.
4. **Calendar *system* support is scoped, flagged `[VERIFY]`:** Gregorian is the floor; **Hijri /
   other calendar systems** are expressible via `Intl.DateTimeFormat`'s `calendar` option but are a
   `[DEFERRED-UNTIL-USERS]`/Phase-8 *scope decision* (do EU-sovereign tenants need Hijri *display*?).
   The **structural** rule — never hand-format, always go through `Intl` with an explicit calendar —
   ships regardless, so adding a calendar is config, not a rewrite. *(HOUSE STYLE scoping; PROVEN
   mechanism.)*

> **Why SLA is the G2-load-bearing surface (HOUSE STYLE):** it is the one place an i18n bug is a
> *broken promise*, not an ugly label — a DPO/PM (P6/P13) trusting a wrong breach time. So the G2
> demonstration set (§7) **requires a locale-formatted date on an SLA/due surface**, not just a
> "last edited" timestamp.

---

## 6. Humanised machine strings (C5) — no raw ids, sourced at the backend

**The #1 "feels unfinished" tell** is a raw machine string (`merge_request merged`, `status_in_progress`,
a bare id, an untranslated enum key) (PROVEN — design-language §8b.5). This file **surfaces** the
existing fix, it does not invent one:

1. **Humanisation lives at the source, not in a frontend string map** (PROVEN — §8b.5): Notifications
   copy/templating + Reference-Graph display-name resolution (ADR-13), paired with a routable
   `ArtifactRef`, so **every consumer and every agent-authored message** inherits localized, humanised
   strings free. R-09 §6.1 and R-10 §4.1/§3.2 already rely on this; this file states it as a **G2 pass
   condition**: the frontend owns **no** id→name map and **no** enum→label map that could drift or stay
   English.
2. **Enum/state values are localized labels, never the wire key** — "In progress" / "In Bearbeitung",
   "Merged" / "Zusammengeführt"; the chip/cell shows the projected label, the AST/wire stores the
   canonical key (the same projection rule as R-06 §6.3 persona-vocabulary + R-08 §4.1 chip grammar).
3. **The label is a *template*, not a concatenation** — "Review requested by {actor} on {ref}" is one
   localizable, plural-aware, RTL-safe template (§5.1.4), so word order localizes (verb-final German,
   RTL Arabic) instead of an English-ordered string-join. This is the C5↔C4 join: humanisation and
   locale-formatting are **one backend templating surface** (§8b.5 + notifications.md §3.3, PROVEN).
4. **Erased/restricted humanise safely** — "[erased user]", "a restricted issue" (PROVEN — R-09 §5.7,
   R-10 §4.3): the localized humanisation is *also* permission/erasure-safe, so i18n never re-opens a
   leak.

---

## 7. The G2 demonstration set + the per-component obligation matrix

### 7.1 The exact G2 demonstration set (what EVERY Phase-6 finalist must show)
Per the prompt + rubric Part 1 (G2) + Part 5 (designed in from sketch #1). A finalist **passes G2 only
if its sketch artifact demonstrates ALL** of the following on the required screen set — *shown and
inspectable, not claimed in prose* (rubric Part 1). This is the checkable list:

| G2 demo item | Concrete requirement | Maps to |
|---|---|---|
| **D-G2.1 — Long-word language** | **≥1 screen rendered in German** with real strings, **no truncation/clipping/overflow** on a dense surface (a board or table or the shell rail) — the real-string pseudo-loc test (§2.2.5) | C1 / §2 |
| **D-G2.2 — Non-Latin script** | **≥1 screen in Greek OR Cyrillic** with real strings incl. **diacritics** (Greek tonos, Cyrillic accents), no tofu, no diacritic clipping, in the **self-hosted** family | C2 / §3 |
| **D-G2.3 — Mirrored RTL state** | **≥1 fully-mirrored state** — shell + a content surface (views/editor) + ≥1 overlay — in **real Arabic or Hebrew**, with **≥1 mixed-direction run** (LTR `myelin://`/SHA/`@handle` inside RTL prose, `<bdi>`-isolated); directional icons mirrored, clocks/media not (§4.2) | C3 / §4 |
| **D-G2.4 — Locale-formatted dates/numbers** | dates/numbers via `Intl` on the shown screens, including **one on an SLA/due surface** (localized deadline + working-calendar-aware remaining time) | C4 / §5 |
| **D-G2.5 — No machine strings** | no raw ids / wire enum keys / `merge_request merged` anywhere in the shown UI; states and provenance are humanised, localized templates | C5 / §6 |
| **D-G2.6 — Logical properties (inspectable)** | the sketch's CSS uses **logical (start/end) properties**, not physical left/right, on the shared components — so RTL is by-construction, inspectable in the markup (the rubric "demonstrate, not claim" bar) | C3 / §4.1 |

**Minimum spread (HOUSE STYLE, satisfying rubric "at minimum one long-word + one non-Latin + one
RTL"):** the cheapest conforming demonstration is **3 localized screens** — one German, one
Greek-or-Cyrillic, one Arabic-or-Hebrew-RTL — plus the locale-date + no-machine-string + logical-CSS
properties *carried across all three*. A finalist may localize more for tie-break margin (rubric
Part 4 tie-break 1 rewards "more languages/RTL depth").

### 7.2 The per-component i18n/RTL obligation matrix (build ON R-08/R-09/R-10)
Every shared component must satisfy every problem class; this matrix is the checklist (✓ = obligation,
→ = owned by the cited section). It is the i18n/RTL parallel to R-17's per-surface a11y checklist.

| Component | C1 expand | C2 non-Latin | C3 RTL mirror | C4 locale fmt | C5 humanised |
|---|---|---|---|---|---|
| **Shell** (R-06 §3) | rail/labels grow, 2-line (§2.2) | label glyphs (§3) | whole-shell mirror (§4.3) | scope/clock in top bar | scope/region labels |
| **Command palette** (R-08) | verb/label rows grow (§2.2) | non-Latin query input (R-08 §9.1) | pill/chip/row mirror (R-08 §11→§4.3) | result date hints | humanised actions/results |
| **Reference chip/unfurl** (R-09) | chip truncates losslessly only (§2.2.2) | title glyphs + mono (§3) | card mirrors, refs `<bdi>` (§4.1) | "updated 2m ago" (§5.1.2) | humanised at source (R-09 §6.1) |
| **Views (table/board/timeline)** (R-10 §2) | cells/headers grow; no clip | Greek/Cyrillic cells + mono | columns/timeline mirror (§4.3) | **date/number/SLA cells (§5)** | enum labels localized (§6.2) |
| **Editor** (R-10 §3) | block content reflows | full-script + IME (R-10 §3.5) | per-block `dir`, bidi caret (§4.3) | — | mention/ref humanised |
| **Inbox** (R-10 §4) | provenance lines grow | subject glyphs | rows mirror | relative-time + due dates | "why fired" templates (R-10 §4.1) |
| **Overlays** (R-10 §5) | dialog content grows | — | anchor/flip reading-relative (§4.3) | — | — |
| **SLA / status surfaces** | timer+word two-slot (§2.2.4) | — | gauge mirrors (linear time §4.2) | **business-calendar (§5.2)** | breach state humanised |

---

## 8. Accessibility (G1) intersection — i18n is partly an a11y obligation

i18n/RTL is not separate from G1; several items are *both* (R-17 owns the G1 audit, this file the G2
demonstration — they cross-reference):

- **Text-expansion clipping = content loss** → WCAG 1.4.4 (resize text) / 1.4.10 (reflow) — a C1
  failure is *also* a G1 failure ([SimpleLocalize, localization as
  accessibility](https://simplelocalize.io/blog/posts/localization-and-accessibility/)). The §2.2
  no-truncation rules satisfy both.
- **`lang`/`dir` set correctly** → WCAG 3.1.1/3.1.2 (language of page/parts) — required for screen
  readers to switch voice/pronunciation per script (R-17/G1).
- **Reflow at 200% / 320px** (G1) must hold **under German expansion AND in RTL** — the hardest case
  is a dense German RTL... (no RTL German exists, but a dense Arabic table at 200% is the real stress;
  §4.3 views + R-10 §2.4). The two gates are tested *together* on the same dense surface.
- **Logical properties** make the RTL mirror correct for the keyboard/focus order too (focus follows
  reading order, §4.3 overlays/palette) — a G1 focus-order obligation discharged by the C3 mechanism.
- **Status-not-by-colour** (G1) survives localization because status is glyph+**localized label**
  (§6.2), never colour or an English word alone.

---

## 9. Actionability toward the control artifacts

| Control artifact | What this file equips | Where |
|---|---|---|
| **rubric.md G2 (the gate this item exists for)** | The five pass conditions made checkable (§1 C1–C5 → §2–§6), and **the exact demonstration set D-G2.1–D-G2.6** a finalist's artifact must *show* (§7.1) — turns "passes G2" from prose into an inspectable checklist; tie-break "more languages/RTL depth" supported (§7.1). | §1, §7.1 |
| **rubric.md G1 (cross-check)** | The i18n↔a11y intersection (§8): expansion-clip = 1.4.4/1.4.10; `lang`/`dir` = 3.1.x; reflow-under-expansion-and-RTL; logical-properties = focus order. R-17 audits; this file supplies the i18n half. | §8 |
| **sketch-funnel (designed in from sketch #1)** | G2 is a *required state* on the comparable screen set (rubric Part 5): the German + non-Latin + RTL screens are not retrofits. The per-component matrix (§7.2) tells each finalist exactly which shared component carries which obligation. | §7 |
| **R-17 (a11y, the twin gate)** | §8 hands R-17 the i18n items that are also WCAG criteria; R-17's per-surface checklist + this matrix together cover G1∩G2. | §8 |
| **R-06 / R-08 / R-09 / R-10 (the components localized)** | This file is where their "labels externalised", "logical-start chips", "humanised at source", "mirror in RTL" hooks are *cashed out* as a demonstrable gate. | §4.3, §7.2 |

---

## 10. `[DEFERRED-UNTIL-USERS]` — what the expert audit has NOT earned

R-18 is `user-dep: none`: the expert audit (this file, grounded in cited standards + the PROVEN Myelin
contracts) **is** the deliverable, and the i18n/RTL *correctness* is PROVEN-by-standard (logical
properties flip; `Intl` formats per CLDR; self-hosted family covers the scripts). But three things are
**HYPOTHESES** only translators/locale-users settle — recorded as executable plans, not faked:

1. **`[DEFERRED-UNTIL-USERS]` — Translation quality & fit.** *What:* native-speaker review of real
   German/Greek/Cyrillic/Arabic/Hebrew copy in-context (not just "does it fit" but "does it read
   naturally / is the term right" — esp. the persona-adaptive vocabulary, R-06 §6.3, which now also
   varies by *language*). *With whom:* native-speaker translators + locale users per cluster. *Falsifies:*
   a term that is technically-fitting but reads wrong/awkward, or a vocabulary lens that doesn't
   survive translation (the §6.3 fracturing risk in another language).
2. **`[DEFERRED-UNTIL-USERS]` — RTL real-use comprehension.** *What:* Arabic/Hebrew users operate the
   mirrored shell + a diff/views surface; do mixed LTR-code-in-RTL-prose runs read correctly; is the
   mirrored DAG/timeline intuitive; is the Hebrew-vs-Arabic asymmetry (§4.2) handled. *Falsifies:* the
   whole-shell mirror if a real RTL user finds the code/ref isolation or the mirrored timeline
   confusing.
3. **`[DEFERRED-UNTIL-USERS]` — SLA business-calendar correctness in the field + Hijri scope.** *What:*
   confirm the working-week/holiday/weekend variants match real EU + Gulf-facing tenant expectations,
   and decide whether non-Gregorian calendar *display* is in scope (§5.2.5). *Falsifies:* a breach time
   wrong against a tenant's real calendar; or a market that needs Hijri display we scoped out.
- **Method:** native-speaker translation review + per-locale RITE on the Phase-6 finalist that ships
  the localized screens. **Caveat:** until then, treat **layout-survives-expansion / glyph-coverage /
  mirror-correctness / locale-format-correctness as PROVEN** (standards + by-construction), but
  **copy quality and real-RTL comprehension as HYPOTHESIS**.

---

## 11. Completeness-critic (README §9) — gloss-risks this item OWNS or routes

R-18 **owns** the i18n/RTL gloss-risk; covered here:

- **RTL mirroring of the WHOLE shell (not just text direction)** — **OWNED & covered** (§4: shell +
  editor + views + overlays via logical properties; what mirrors vs not, §4.2; real-string test §4.4).
  This is the named §9 gloss-risk for this item.
- **Text expansion / fixed-width bug classes** — **OWNED & covered** (§2; the §8b.4 classes named §2.3).
- **Non-Latin clipping / tofu** — **OWNED & covered** (§3; self-hosted-coverage constraint §3.1).
- **Locale dates/numbers + SLA business calendar** — **OWNED & covered** (§5; the SLA correctness-not-
  cosmetic framing §5.2).
- **Machine strings leaking** — **covered by surfacing** §8b.5 as a G2 condition (§6); the *mechanism*
  is notifications/Refs (PROVEN, not re-derived).
- **Routed (depth elsewhere):** the **G1 audit** of these items (R-17 — keyboard/SR/contrast under
  i18n); the **per-surface full state catalogue** localized (R-21 multiplies states × locales); the
  **persona-adaptive vocabulary** *validation* (R-07 tree-test, now per-language) and *per-lens
  critique* (R-16) — §6.2/§10 name the cross-language fracturing risk, the resolvers own it.
- **Consciously deferred (with reason):** the *backend* `Intl`/CLDR data pipeline and the translation
  management workflow (engineering, not design); the choice of the specific shipped variable font
  family (Phase 8 look-fit, `[VERIFY]` coverage); non-Gregorian calendar *scope* (§5.2.5 deferred).

---

## 12. Sources (web-verified, 2024–2026 + surfaced contracts)

**Standards / patterns (PROVEN):**
- MDN — CSS Logical Properties & Values (floating/positioning; `inline-start/end`, Nov 2025):
  https://developer.mozilla.org/en-US/docs/Web/CSS/Guides/Logical_properties_and_values/Floating_and_positioning
- RTL Styling 101 (logical properties, bidi isolation, what flips): https://rtlstyling.com/posts/rtl-styling/
- Mozilla Firefox RTL guidelines (logical-properties-first, bidi): https://firefox-source-docs.mozilla.org/code-quality/coding-style/rtl_guidelines.html
- Material Design 3 — Bidirectionality & RTL (what mirrors / what doesn't; Hebrew exception): https://m3.material.io/foundations/layout/bidirectionality-rtl · https://m2.material.io/design/usability/bidirectionality.html
- MDN — `Intl` (DateTimeFormat/NumberFormat/RelativeTimeFormat/PluralRules; CLDR-backed): https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Intl
- Smashing Magazine — The Intl API definitive guide (formatter caching, dateStyle, 2025): https://www.smashingmagazine.com/2025/08/power-intl-api-guide-browser-native-internationalization/
- Google Fonts — Vertical metrics guide (120% too tight beyond basic Latin; diacritic headroom): https://googlefonts.github.io/gf-guide/metrics.html
- SIL — Font Development Best Practices, Line Metrics (diacritic clipping/clashing): https://silnrsi.github.io/FDBP/en-US/Line_Metrics.html

**Localization data / practice (PROVEN-as-reported):**
- SimpleLocalize — Why text expansion breaks your UI (German +30–40%, pseudo-loc): https://simplelocalize.io/blog/posts/text-expansion-ui-localization/
- SimpleLocalize — Localization as accessibility (truncation = a11y failure): https://simplelocalize.io/blog/posts/localization-and-accessibility/
- jem-products — Managing text expansion 2025 (20–35% avg, 2× short strings): https://jem-products.com/how-to-manage-text-expansion-in-translation-localization-2025/
- Wordpar — German grammar breaks UI: https://wordpar.com/german-grammar-ui-localization-fix/
- Deviniti — SLA business hours & holidays setup: https://deviniti.com/support/addon/cloud/sla-time-management/latest/use-cases-set-up-business-hours/
- Atlassian Community — SLA calendars done right (timezone/holiday/region): https://community.atlassian.com/forums/App-Central-articles/SLA-calendars-in-JSM-done-right-tips-tricks-and-time-savers/ba-p/3143788

**Surfaced Myelin contracts (PROVEN-as-existing, not invented):**
- design-language §4 (i18n/RTL baseline: externalised strings, locale-aware dates/numbers/calendars,
  RTL via logical properties, shell+editor+views mirror), §3.3 (Latin-ext+Greek+Cyrillic coverage,
  self-hosted no-CDN), §8b.4 (fixed-width/mobile bug classes), §8b.5 (humanise machine strings at the
  backend); ADR-13 (display-name resolution); ADR-10/14 (multilingual search).
- R-06 §6 (labels in tokens/config), R-08 §9.1/§11, R-09 §6.1/§8, R-10 §2.4/§3.5/§4.1/§5.

---

## 13. Self-check against R-18 acceptance criteria

| Criterion (prompt R-18) | Status | Evidence |
|---|---|---|
| **Text-expansion patterns concrete + reference logical/no-fixed-width** | ✅ Met | §2 (expansion budget table, 5 no-truncation rules, pseudo-loc), §2.3 (§8b.4 classes named) |
| **Non-Latin rendering: coverage, line-height, no clipping (Greek/Cyrillic)** | ✅ Met | §3 (coverage gate + self-hosted constraint §3.1; line-height/diacritic headroom §3.2; mono coverage) |
| **Full RTL via logical start/end; whole shell + editor + views + overlays mirrored** | ✅ Met | §4.1 (logical-properties mechanism), §4.3 (shell + 4 components each, what flips), §4.2 (mirror/not table) |
| **Tested with a REAL RTL string, not a flipped mockup** | ✅ Met | §4.4 (binding method: real Arabic/Hebrew + mixed-direction LTR-ref run, `<bdi>`); D-G2.3 §7.1 |
| **Locale-aware date/number/calendar; SLA/business-calendar load-bearing** | ✅ Met | §5 (`Intl` rules §5.1; SLA business-calendar correctness-not-cosmetic §5.2; calendar-system `[VERIFY]`) |
| **Humanised strings required (no raw machine strings)** | ✅ Met | §6 (surfaces §8b.5: backend-sourced, no frontend map; templates not concatenations; enum labels localized) |
| **The EXACT G2 demonstration set specified** | ✅ Met | §7.1 (D-G2.1–D-G2.6: German + Greek/Cyrillic + RTL-mirror + locale-date + no-machine-string + logical-CSS) |
| **Whole-shell mirroring required, not just text direction** | ✅ Met | §4.3 (shell + editor + views + overlays); §11 (the named §9 gloss-risk owned) |
| **The §8b.4 fixed-width bug classes named** | ✅ Met | §2.3 (5 named classes, each with the fix) |
| **Builds ON R-06/R-08/R-09/R-10, doesn't duplicate** | ✅ Met | §0 + inline cites; §7.2 per-component matrix references their sections; cashes out their i18n hooks |
| **Makes G2 checkable, not aspirational** | ✅ Met | §1 C1–C5 → §7.1 demo set + §7.2 obligation matrix = an inspectable checklist (rubric "show, not claim") |
| **PROVEN/HOUSE-STYLE tags + date + cited web sources** | ✅ Met | tagged throughout; dated 2026-06-20; §12 URLs (MDN/Material/Intl/GoogleFonts/SIL/SimpleLocalize) |
| **Deferred validation recorded as a plan, not faked** | ✅ Met | §10 (`[DEFERRED-UNTIL-USERS]`: translation quality, RTL comprehension, SLA-calendar/Hijri scope, each with falsifier) |
| **§9 gloss-risks addressed (whole-shell RTL mirror)** | ✅ Met | §11 (OWNED: whole-shell RTL, expansion, non-Latin, locale/SLA, machine strings; routed: G1 audit, state catalogue, vocabulary validation) |

**Top uncertainties (honest):**
1. **Copy quality & cross-language vocabulary fracturing** — layout/format/mirror correctness is
   PROVEN-by-construction, but whether the persona-adaptive vocabulary (R-06 §6.3) survives *translation*
   (does "work item"↔"deliverable" map cleanly into German/Arabic?) is HYPOTHESIS — §10.1, resolved by
   native-speaker review + R-07 per-language.
2. **The shipped variable font's actual EU-24 + Greek + Cyrillic + (RTL) Arabic/Hebrew coverage** is a
   `[VERIFY]` against a real family (Phase 8 look-fit, §3.1); the self-hosted no-CDN constraint makes
   coverage a *selection gate*, and an under-covering family would force a structural fallback.
3. **Non-Gregorian calendar scope (§5.2.5)** — whether EU-sovereign tenants need Hijri/other *display*
   is an undecided product/market question; the structural `Intl`-with-explicit-calendar rule ships
   regardless, but the scope decision is deferred.
4. **RTL of the diff/code surface specifically** — bidi-isolating LTR code inside an RTL shell is
   PROVEN-correct via `<bdi>`, but the *reading experience* of a mirrored review surface with LTR code
   islands is the genuine HOUSE-STYLE bet (§4.3 editor / §10.2), the hardest real-RTL comprehension case.

---

*End of R-18 deliverable. Date: 2026-06-20. i18n/RTL patterns HOUSE STYLE over PROVEN standards
(CSS logical properties, ECMA-402 `Intl`/CLDR, Material bidi, Google Fonts/SIL metrics, WCAG) and the
PROVEN Myelin contracts (design-language §4/§3.3/§8b.4/§8b.5, R-06 externalised labels, ADR-13);
correctness PROVEN-by-construction, copy/comprehension not user-validated — see §10. Builds on
R-06/R-08/R-09/R-10. Feeds rubric G2 (+ G1 cross-check) and Phase 6 (designed in from sketch #1).*
