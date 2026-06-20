# Finalist D — "Civic" (highly-unified · sober · always-on sovereignty)

> Phase 6, stage 6c deepening. Carrier **C-03** (Vignelli/Aicher exec dashboard, sober grid,
> always-on residency band) absorbing **C-11** (the DSR console + inclusion-proof receipt — the
> mandated S-11 sovereignty flagship), **C-10** (small-multiples), **C-18** (Rams inbox restraint),
> **C-12** (dense-roadmap "density-is-tuning" proof). The trust/governance answer: the finalist that
> **carries the DSR / sovereignty console** and makes sovereignty *felt* without alarmist clutter.
>
> **Honesty (VISION §3):** these are **expert sketches, not user-validated** — `[DEFERRED-UNTIL-USERS]`.
> Tags: **PROVEN** = a cited GDPR article / WCAG / standard / surfaced Myelin mechanism;
> **HOUSE STYLE** = our design synthesis. The whole sovereignty-as-UX layer is **`[UNDER-EVIDENCED]`**
> per R-19 §0 (no external playbook exists) — see the D9 note below.

---

## Identity & persona

Massimo Vignelli / Otl Aicher institutional. One rigid modular grid; one type family (Inter UI sans
+ a mono for SHAs/locators/receipt hashes) at a few disciplined steps; a single low-chroma authority
blue; **status by glyph + label + position, never colour alone**; **plainness-as-authority**; an
**always-on residency / lawful-basis band near the data**. The surface a DPO *and* a VP both trust at
a glance — robust, legible, decoration-free, never alarming. Borders + surfaces over heavy shadow.

**Primary personas:** P13 (DPO) and P11/P6 (exec / PM corporate lens) — *not* the engineer's daily
diff. The engineer is served through the same skin density-tuned (screen 2), but Civic's home turf is
governance + portfolio.

## The six axis positions (PROVEN — read from the carriers / built into the screens)

| Axis | Position | How it shows |
|---|---|---|
| **1 Density** | **medium** (with a dense-tuned proof) | Exec dashboard is comfortable; screen 2 pushes the *same* skin to a dense transit-timetable roadmap — density is tuning, not a fork. |
| **2 Nav** | **rail** | Persistent primary rail (full on dashboard, collapsed-icon on the dense surface, structural on the DSR console). |
| **3 Unification** | **highly-unified (sober)** | One shell, one chip, one identity, one status grammar, one token set across all 7 screens — a *different corner* from Finalist A (sober/medium/rail vs utilitarian/dense/palette). |
| **4 Tone** | **sober** | Neutral-led warm-paper ramp + one authority blue; no saturated traffic-lights, no decoration. |
| **5 Agent** | **ambient** | FixAgent/ForecastAgent appear as labelled rows/footnotes (collapsed-by-default lane); foregrounded only at the HITL gate. Reserved violet agent treatment, plain square glyph, **no sparkle/emoji**. |
| **6 Sovereignty** | **ALWAYS-ON** | The residency/lawful-basis band is structural (in the rail on the console, across the top on the dashboard, compact on the dense surface) + the full DSR console + verifiable receipt. This is the finalist's reason to exist. |

---

## The screens (the comparable set + the mandated console)

| # | File | Brief slot | What it is |
|---|---|---|---|
| 1 | `screens/01-shell-exec-dashboard.html` | **Shell** (#1) **+ PM/corporate** (#3) | One-product frame (primary nav + contextual sidebar + content + context pane) composing Issues + Govern. Q3 exec dashboard over the **same issue data** (ISS-377/PR #412). Always-on sovereignty band. Light/dark toggle. German content. |
| 2 | `screens/02-roadmap-dense.html` | **Dense engineer surface** (#2) | The Q3 roadmap pushed dense (transit-timetable Gantt) in the **same Civic skin** — the D4 "density-is-tuning" proof + D1/D7. Roving-tabindex keyboard nav. **G2 non-Latin (Greek + Cyrillic) lives here.** |
| 4 | `screens/04-hitl-card.html` | **Agent / HITL** (#4) | FixAgent plan-then-apply approval card in the unified inbox: proposed effects, per-effect target chips, the **gated** effect (open PR on protected `main`), **Approve / Edit / Reject**, attribution, budget, provenance walk (one `correlation_id`). |
| 5 | `screens/05-palette-wedge.html` | **Wedge** (#5) | Command palette (⌘K) over the universal graph — navigate/act/search modes, permission-pre-filtered rows ("2 results hidden — no access"), consequential verbs routing into the gate, a **live residency-aware unfurl**. |
| 6 | `screens/06-dsr-console.html` | **DSR / sovereignty console** (mandated) | The **Jürgen Vögel** erasure request (Art. 17) across all five surfaces (10 holders): five-tab holder ops, per-holder completeness + residency, **failure-isolated saga** (1 holder needs retry), consequence dialog, deadline clock, **verifiable Merkle receipt**, honest `[OPEN — LEGAL]` residual, linked subject-side view. The F-GOV-1 flow. |
| 7 | `screens/07-states.html` | **Unglamorous states** | empty · loading-skeleton · error · permission-denied (Restricted ≠ Absent, no leak) · **erased/tombstoned** (tombstone / pseudonymised / crypto-shredded / `sub_gone` / `root_gone`) · agent-pending (full HITL state set). |
| 8 | `screens/08-rtl-mirror.html` | **G2 RTL** | The DSR console fully mirrored in **real Arabic** (`dir="rtl"`, logical properties, no override sheet), with a **mixed-direction run**: LTR `myelin://` ref + SHA + `@handle` bidi-isolated inside RTL prose. |

Screen #3 (approachable PM/corporate) is folded into screen 1 per the brief's allowance (same shared
views/components tuned by role) — screen 1 (comfortable exec) and screen 2 (dense roadmap) are the
**dual-audience pair over the same issue data** (D5 proof).

## Token approach (DTCG)

`tokens.json` is **W3C DTCG** (`$type`/`$value`, `{group.ref}` aliases), three tiers
**primitive → semantic → component**, **light + dark**. `tokens.css` is the CSS-custom-property
projection every screen consumes — **no hardcoded colours/space in the HTML, vars only**; spacing on
a strict 4/8 ramp. Neutral-led warm-paper ramp + one low-chroma authority blue + functional
status/agent/restricted colours.

**The focus token ≠ identity token (PROVEN — §8b.3).** The identity accent is `--accent` (blue-600);
the **derived AA-safe focus/interactive-text token** is `--focus-ring` / `--accent-text` (blue-700),
a deeper step. One focus treatment, shared everywhere, **visible-by-offset**: the `outline` +
`outline-offset` leaves a surface-colour gap so the dark-blue ring is visible even on the blue accent
fill (the gap is 8.2:1 vs the fill in light, 6.1:1 in dark).

---

## How it meets **G1** (WCAG 2.1 AA / EN 301 549) — measured, not claimed

Contrast ratios **measured** with the WCAG relative-luminance formula (script in commit log).
Institutional surfaces pursue AAA where feasible — **AAA hits are marked**.

**Light theme (on `--surface` #ffffff):**

| Token pair | Ratio | Bar |
|---|---|---|
| `--text-primary` (neutral-900) | **17.4:1** | AAA ✔ |
| `--text-secondary` (neutral-600) | **7.7:1** | AAA ✔ |
| `--text-muted` (neutral-500, meta only) | **4.8:1** | AA ✔ |
| `--accent` identity (blue-600) | **8.2:1** | AAA ✔ |
| `--accent-text` / `--focus-ring` (blue-700) | **10.3:1** | AAA ✔ |
| white on `--accent` fill (buttons) | **8.2:1** | AAA ✔ |
| `--status-ok` (green-600) | **5.0:1** | AA ✔ |
| `--status-warn` (amber-700) | **5.7:1** | AA ✔ |
| `--status-bad` (red-700) | **6.8:1** | AA ✔ |
| `--status-restricted` (ochre-700) | **7.4:1** | AAA ✔ |
| `--agent` (violet-600) | **7.6:1** | AAA ✔ |
| focus ring visibility vs accent fill (via white offset gap) | **8.2:1** | > 3:1 ✔ |

**Dark theme (on `--surface` #1a1c1a):** text-primary **14.3:1** (AAA), text-secondary **7.3:1**
(AAA), accent **6.1:1** (AA), accent-text/focus **8.0:1** (AAA), agent **5.9:1** (AA), status-bad
**5.2:1** (AA); focus-ring visible vs dark accent fill via the **6.1:1** dark-surface offset gap.

**Other G1 properties demonstrated:**
- **Visible focus on every interactive element** — one `:focus-visible` token; the dense roadmap rows
  show an inset focus ring; the palette/overlay is focus-managed.
- **Status never by colour alone** — every status carries glyph + label + position (e.g. `✓ Gelöscht`,
  `✗ Re-Index fehlgeschl.`, `▣ Behalten, pseudonymisiert`). Verified on the dashboard, dense roadmap,
  DSR table, and states page.
- **Keyboard model noted for the hard components** — roadmap (roving-tabindex `j/k`/`↑↓`, `Enter`,
  `x`, `/`, `⌘K`); palette (`⌘K` open, `↑↓` move, `Tab` modes, `Enter` run, `Esc` close, focus-trap);
  HITL card (tab order Approve→Edit→Reject; Edit amends content).
- **Reflow** — every screen collapses the context pane → rail at breakpoints, `min-height:0` on flex
  scrollers (§8b.4), content-sized labels (no fixed-px text widths).
- **`prefers-reduced-motion`** honoured (tokens.css) — "pages render, they don't animate in."

## How it meets **G2** (i18n / l10n / RTL)

- **German (long compound labels):** screens 1, 2, 6 are in German with real strings incl.
  `EU-Datenresidenz-Steuerung`, `Offline-Sync-Warteschlange`, `Einschränkungs-Unterdrückung`,
  `Rechtsgrundlagen-Grenze`, `Personendaten-Halter` — no truncation/clipping; subject **Jürgen Vögel**
  (umlaut/expansion test).
- **Non-Latin (Greek + Cyrillic, with diacritics):** dense roadmap rows render `Άμεσες πληρωμές SEPA`
  / `Λίνα Ρ.` (Greek tonos) and `Засилване на надеждността` (Cyrillic) with correct `lang` attributes,
  no tofu, no diacritic clipping (line-height has headroom).
- **Mirrored RTL (real Arabic + mixed-direction run):** `screens/08-rtl-mirror.html` is the full DSR
  console in `dir="rtl"`, mirrored **by construction** via logical properties (no `[dir=rtl]` override
  sheet) — rail renders on the right, receipt on the left. A **mixed-direction line** isolates an LTR
  `myelin://acme/payments-api/ci/run/1894` ref + SHA `a94bcc7` + `@mara.o` handle inside Arabic prose
  with `<bdi>`/`unicode-bidi:isolate` so code/refs never visually reverse. Status glyphs and the agent
  square are non-directional and **intentionally not mirrored** (R-18 §4.2).
- **Locale-aware dates/numbers:** German `Intl`-style formatting on the dashboard (`Stand:
  20.06.2026, 14:02 MESZ`; `96,4 %`; targets `11.07.`) and on the SLA-bearing DSR deadline clock
  (`fällig 26.06.2026`) — the load-bearing surface.
- **No machine strings:** states/enums are humanised localized labels (`Im Plan`, `Gefährdet`,
  `Gelöscht`), never wire keys.
- **Logical properties inspectable** in the markup (the RTL screen is `left/right`-free; shared
  components use `inline-start/end`, `margin-inline-*`, `border-inline-*`).

## Unglamorous states (`screens/07-states.html`)

empty (onboarding-forward) · loading (per-holder structure skeleton, never a spinner) · error (quiet,
blames the system, offers a path; idempotent receipt regen) · permission-denied (**Restricted ≠
Absent**, counted-for-completeness, never a title leak) · **erased/tombstoned** (tombstone /
pseudonymised-author / crypto-shredded body / `sub_gone` / `root_gone` — central to the GDPR story) ·
agent-pending (pending → working → gate-awaiting → approved/edited/rejected + budget-exceeded /
stale-approval).

---

## Honest self-assessment vs rubric D1–D10 (0–4 each; weighted)

| Dim | Weight | Score | Rationale (honest) |
|---|---:|:---:|---|
| **D1** Power-user efficiency | 12% | **2** | Roadmap has a real keyboard model + dense grid, but Civic is medium-density-led; an engineer's daily diff isn't the home turf. Finalist A out-runs us here by design. |
| **D2** First-run / approachability | 10% | **3** | Plain-as-authority reads well for a DPO/VP; the exec dashboard + states-empty teach the next step. Not toylike; slightly cool for a first-time non-corporate user. |
| **D3** Visual craft & tone | 12% | **3** | Disciplined Vignelli/Aicher grid, one accent, borders-over-shadow, AAA-heavy palette, no amateur tells. Intentional but *sober by choice* — less "loved at first glance" than a warmer or darker direction. |
| **D4** One-product coherence | 14% | **3** | One shell/chip/identity/status-grammar/token-set across 7 screens; the DSR console folded into the *same* skin (not a console-apart aesthetic); density-is-tuning proven (screen 1 ↔ 2). Strong; a few surfaces (CI/knowledge) not drawn. |
| **D5** Dual-/tri-audience | 10% | **3** | Comfortable exec ↔ dense engineer roadmap over the same issue data, plus the DSR DPO ↔ subject pair. Both lenses real; the engineer lens is competent but not flagship. |
| **D6** Agent legibility & trust | 12% | **3** | Plan-then-apply card with gated effect, Approve/**Edit**/Reject, attribution, budget, provenance walk, full state set; reserved agent treatment, no sparkle. Solid; ambient-by-default means less foregrounded agent storytelling than Finalist C. |
| **D7** Density-made-calm | 8% | **3** | Dense roadmap stays calm (single-hue bars, hairlines, no traffic-lights); inbox collapses agent volume. Calm is the whole posture. |
| **D8** Perceived performance | 6% | **2** | Loading skeletons designed (states page) + "pages render, don't animate in"; but optimistic-update/rollback states are described more than shown. |
| **D9** Sovereignty / GDPR-as-UX | 8% | **4** | **Our strength.** Always-on residency/lawful-basis band; full DSR console with per-holder completeness across all five surfaces, failure-isolated saga, residency per holder, consequence-first erasure, **verifiable receipt**, honest `[OPEN — LEGAL]` residual; tombstone/no-access states first-class; both DPO + subject lenses. **FLAGGED: D9 is `[UNDER-EVIDENCED]` (R-19 §0/§9) — there is no external playbook for sovereignty-as-UX, and "a DPO trusts it at a glance" is the unproven keystone, falsifiable only by the deferred regulated-buyer (P13/P14) review. The score reflects coverage + craft against the blueprint, NOT validated trust.** |
| **D10** Switch test | 8% | **3** | A regulated buyer could move their DSR/governance workflow here and gain a one-subject-one-receipt-with-proof flow they don't have today; the engineer daily-driver would feel calmer-but-cooler than Linear. |

**Weighted total = 2.90 / 4** (≈ 72.5%).
Computation: .12·2 + .10·3 + .12·3 + .14·3 + .10·3 + .12·3 + .08·3 + .06·2 + .08·4 + .08·3 = **2.90**.

**Weakest dimension: D1 (power-user efficiency) and D8 (perceived performance), both 2/4** — by
deliberate positioning (Civic optimises governance/corporate trust, not the engineer's keyboard
sprint) rather than oversight. The honest trade is: Civic wins D9 outright and is competitive on D4,
but cedes D1 to Finalist A and foregrounded-agent storytelling to Finalist C.

## Known gaps

- **D9 is the strength but it is `[UNDER-EVIDENCED]`** (flagged above) — the single most important
  caveat. Validated only by the deferred DPO/procurement review (R-19 §9): three falsification bars
  (DPO can't answer the P9 three questions in ≤1 console each; receipt not trusted as proof; ambient
  cues read as noise) would each lower this.
- CI run, knowledge editor, and chat timeline are not drawn (out of the comparable-set scope here);
  coherence across *those* seams is asserted, not shown.
- Optimistic-update/rollback (D8) is described, not interactively demonstrated.
- The dense roadmap's inline Gantt offsets now use `inset-inline-start` (logical), but the bar
  *widths* remain physical (acceptable — width is direction-neutral).
- Sketch uses a single CDN font link (Inter); **production self-hosts** a variable family with
  Latin-ext + Greek + Cyrillic + Arabic coverage (design-language §3.3, no font CDN — a sovereignty
  constraint that intersects i18n). Coverage is a `[VERIFY]` against the EU-24 + RTL set.
- `[DEFERRED-UNTIL-USERS]` throughout: these are expert sketches, not user-validated; copy quality
  and real-RTL comprehension are translator/locale-user questions (R-18 §10).
