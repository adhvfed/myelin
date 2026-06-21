# Phase-7 Judging — Visual-craft lens (D3, D4, D6)

> **Lens:** Visual-craft (owns **D3** visual craft & emotional tone, **D4** one-product coherence,
> **D6** agent legibility & trust). **Silent-first**, scored independently before panel discussion
> (rubric Part 3). Status date: **2026-06-20**.
> **Method:** read the rubric anchors (`rubric.md` Part 2), the craft bar (`visual-direction.md` §1
> shared-floor + §8b.3 anti-aesthetic), and the agent contract (`legibility-and-hitl.md` §1–§6); then
> inspected each finalist's `tokens.css`/`tokens.json` + EVERY `screens/*.html` for *inspectable*
> evidence. **Self-scores ignored.** Anchors: 0 absent/wrong · 1 weak · 2 incumbent bar · 3 North Star ·
> 4 beats North Star.
> **Tags:** **PROVEN** = read directly from the artifact (markup/token/measured number) ·
> **JUDGEMENT** = my taste call against the anchors. All comprehension/"loved" claims are
> `[DEFERRED-UNTIL-USERS]` (the artifacts prove *construction*, not reception).

---

## Gate pre-check (every lens must confirm before scoring)

All four ship a DTCG `tokens.json` + `tokens.css`, a derived AA-safe focus token **distinct from the
identity accent** (`--focus`/`--focus-ring` ≠ `--accent`, §8b.3), status carried by glyph+label+position
(never colour alone), logical properties throughout, and a mirrored real-Arabic RTL screen. **PROVEN**
from the token files + screens. G1/G2 are the Accessibility lens's gates; I record only that nothing in
D3/D4/D6 surfaces a gate breach. Detailed contrast/RTL adjudication is deferred to that lens.

---

## D3 — Visual craft & emotional tone (token discipline + coherent intentional tone + no amateur tells)

| Finalist | Score | Rationale (evidence) |
|---|:--:|---|
| **A — Instrument** | **3** | **PROVEN** strict 4/8 ramp (no off-ramp px found), borders-do-all-grouping with shadow used **only twice** total (palette modal + drag state), hierarchy via weight+glyph+colour, zero amateur tells (SVG/mono glyphs, no sparkle/emoji/gradient/saturated fill); diff `+/−` is a text sign not colour-alone. **JUDGEMENT:** tone is coherent + intentional (midnight command-deck, engineer-forward, unflinching copy) but *austere by design* — disciplined, not a reason-to-love. North Star, not beyond. |
| **B — Workshop** | **3.5** | **PROVEN** the most *distinctive* tone in the set: editorial-warm cream/ink ramp, Fraunces serif at weight 560/600 **confined to reading headings only** (verified it never leaks into chrome/buttons/status), shadow used **3×** only on floating `.card`/`.unfurl`, no amateur tells, weight-first hierarchy. **JUDGEMENT:** stays *crafted not cute* — warmth from tone/radius/serif, not decoration; does NOT tip toylike or into editorial cliché (serif is restrained, not oversized-display-as-only-hierarchy). One minor PROVEN tell: a few off-ramp paddings (`14px 15px`, `5px 11px`) on cards — a hair short of exemplary, but the most loved look here. |
| **C — Wayfinding** | **3** | **PROVEN** disciplined 4/8 (off-ramp only at the documented `sp-7`=48px ramp step), shadow **3×** (1 modal + 2 *inset-border* substitutions, not drop shadows), no amateur tells, weight/colour-before-size. **JUDGEMENT:** the Spiekermann transit-signage tone (channel▸topic addressable like a departures board) is coherent and intentional and *sustained* across surfaces — but reads competent-utilitarian rather than singular/loved; its distinctiveness is in *information structure*, less in surface emotion. North Star. |
| **D — Civic** | **3** | **PROVEN** consistent 4/8, borders-only with shadow **once per screen** (palette only), no amateur tells, glyph+label status, AAA-heavy token pairs (claimed + plausible from the ramp). **JUDGEMENT:** the sober Vignelli/Aicher institutional tone *avoids the named dull-grey-government-form trap* — it is crisp, dignified, consequence-first (the DSR erasure "what this does, irreversibly" framing), the always-on sovereignty band as a structural anchor not wallpaper. Intentional and competent; sober-by-choice means less "loved-at-first-glance" than B. North Star. |

**D3 winner: B — Workshop** (most distinctive/intentional/loved look while staying disciplined and
off the toylike line). A/C/D are a tight band of strong-but-deliberately-restrained tones.

---

## D4 — One-product coherence (the five-surfaces-as-one test)

| Finalist | Score | Rationale (evidence) |
|---|:--:|---|
| **A — Instrument** | **4** | **PROVEN** indisputably one product: invariant brand mark, rail, status glyph grammar, agent mark+label, reference chip, and token set across all 6 screens (all link `../tokens.css`, no per-screen redefinition). The **board↔roadmap is literally ONE component density-tuned by a single toggle** (`.lens-on .board{display:none}` / `.roadmap{display:grid}`, same `.card`/`.sg`/`.pri` atoms) — the cleanest "dense and calm are the same system" proof in the set. Highly-unified one-skin delivered. |
| **B — Workshop** | **3** | **PROVEN** the *hardest* coherence case (distinct-per-surface bet) and it holds: invariant shell/topbar/rail, ONE `.chip`, ONE `.agent-badge` (identical markup across screens), ONE editor (serif body reused on knowledge + composer reused in chat/HITL), ONE token set; distinctness is purely density/measure/serif-emphasis. Engineer/PM/exec lens switch is over the *same* ISS-377/PR-412 rows. **JUDGEMENT:** the bet costs a point — by design surfaces *feel* different (roadmap-as-document vs diff vs book-set knowledge), so coherence is real but works *against* itself; not a fork, but a harder sell than one-skin. |
| **C — Wayfinding** | **3** | **PROVEN** invariant `.ref` chip, agent treatment, status grammar, button, shell skeleton and tokens across all 7 screens; CI(dense) vs roadmap(calm) are the same components + same ISS-377/CI#1894 data re-weighted, not forked. **JUDGEMENT:** also distinct-per-surface by design (each surface keeps its own working identity), so coherence is deliberately looser than A/D's one-skin; held together strongly by the addressing/wayfinding grammar but the same one-point cost as B's bet. |
| **D — Civic** | **3.5** | **PROVEN** highly-unified one-skin: ONE shell (rail merely collapses 200px→46px-icon on the dense roadmap, not redesigned), ONE brand monogram, ONE `.chip` (`.ty`+`.dot` reused on dashboard/HITL/palette), ONE status grammar, ONE token set; **the DSR console is folded into the SAME skin** (not a console-apart aesthetic — comment + markup confirm) and exec(comfortable)↔roadmap(dense) is visibly the same skin density-tuned (13px/1.5 → 12px/1.4, same chips/glyphs/band). **JUDGEMENT:** a half-step under A only because A's single-toggle board↔roadmap is a tighter same-component proof; D folds the *governance* surface into the product, which is the harder coherence win for the regulated story. |

**D4 winner: A — Instrument** (the single-toggle board↔roadmap is the most literal one-component
proof), with **D — Civic** a close second and the most *impressive* coherence (DSR console folded into
the one skin). B and C deliberately take the harder distinct-per-surface position and pay one point each.

---

## D6 — Agent legibility & trust (the §6 plan-then-apply / HITL contract)

| Finalist | Score | Rationale (evidence) |
|---|:--:|---|
| **A — Instrument** | **3** | **PROVEN** agent always labelled (`Agent` badge + plain square mark, no sparkle/emoji); plan-then-apply with **3 concrete numbered effects + per-effect target chips**; **gate marker only on the consequential effect** (open PR → protected `main`, `authority git.open_pr`), effects 2–3 explicit `no gate · reversible/advisory`; attribution `on behalf of Mara Ø. · triggered by ci.failed · correlation incident-9 → CI #1894 → ISS-377`; scope `delegation ∩ tenant (EU-West)`, budget `3/12 · est €0.04`, Why?/Audit links; "nothing applied yet / nothing runs until you choose". **JUDGEMENT:** Approve/**Edit plan…**/Reject all present but **Edit is a button only — no proposed→amended diff rendered**; ambient-by-design, so strong-and-correct rather than the showcase. |
| **B — Workshop** | **3.5** | **PROVEN** full card: `Agent` badge + plain geometric mark; 3 concrete effects with per-effect target chips; gate on the protected-`main` PR (`Consequential — opens PR against protected branch main`), others `no gate`; attribution + `correlation · incident-#9 → ISS-377 → run #1894`; scope `delegation ∩ tenant`, budget bar `3/12 · €0.04 · due …`, Why?/Audit; **plus** an *advisory* PR reviewer on screen 2 labelled `Agent · advisory` with "no effect runs without your action". **JUDGEMENT:** Approve/**Edit…**/Reject present; Edit labelled "Edit…" implying the amend modal but **the proposed→amended diff is not rendered** — so just under C. |
| **C — Wayfinding** | **4** | **PROVEN — best-in-set.** The HITL card renders the **actual Edit amend state with a proposed→amended DIFF** (`− branch fix/auto-377` / `+ branch fix/lru-weight-bound-377`, attributed `human-edited-agent-proposal`), **per-effect** Approve/Reject (effect 2 already `Approved by Mara Ø. · applied`; effect 3 still pending) proving partial approval, gate on the protected-`main` effect, scope `delegation ∩ tenant, never wider than @Mara`, live budget + `depth 2/12`, attribution + Why?/Audit + correlation walk. Foregrounded agents in chat are **labelled + contained by topic + chain-collapsible** (`expand 4 ▾`); `states.html` ships the **full 10-state set incl. the agent-storm**: human lane `holds`, agent lane `shedding · 429 Retry-After`, 34 actions collapse to one line, 7 gates group in the inbox — "the main stream never shows 30 rows". This *is* the R-14 contract, rendered, including the parts the others only gesture at. |
| **D — Civic** | **3** | **PROVEN** complete card: `agent` badge + plain square (no sparkle, comment cites §8b.3/R-14); plan-then-apply with concrete numbered effects + per-effect target chips (incl. an `EU-west` residency target chip — a sovereignty-native touch); gated effect marked `⛉ Gated effect` in restricted-ochre (not alarmist red) on the protected-`main` PR; Approve/**Edit plan…**/Reject with explicit note "Edit lets you amend the gated effect (branch/scope)"; authority `perms ∩ delegation ∩ tenant`, budget `2/5 · 0:42 wall-clock`, Audit link; **rendered one-`correlation_id` provenance walk** (`corr-9f2a CI#1894 failed → TriageAgent filed ISS-377 → FixAgent proposed → you ⟶ audit ✓`). Ambient agents on dashboard/roadmap are quiet labelled footnotes ("Schätzung — keine Aktion"). **JUDGEMENT:** the provenance walk is excellent and the residency-on-effect is distinctive, but Edit's amend is *described not rendered* and there's no partial-approval/storm state → a strong 3, under C's rendered-diff + storize depth. |

**D6 winner: C — Wayfinding** (the only finalist that *renders* the Edit proposed→amended diff,
per-effect partial approval, and the calm-under-30×-storm state — the foregrounded-agent bet pays off
without becoming a firehose). D's rendered provenance walk is the strongest single attribution treatment.

---

## Lens verdict (1 paragraph)

**PROVEN:** all four cleared the craft floor (4/8 ramp, derived focus≠identity, plain agent marks, no
amateur tells, glyph+label status) — there is **no median-in-disguise and no fork** in the set; every
"distinct-per-surface" finalist (B, C) demonstrably keeps shell/chip/identity/palette/agent-treatment
invariant in shared markup, so distinctness is genuinely *tuning*, not stitching. **JUDGEMENT:** the
**most distinctive/loved look is B — Workshop** (editorial-warm, serif disciplined to reading headings,
crafted-not-cute) — it owns D3. The **finalist that best resolves one-product coherence is A —
Instrument**, whose single-toggle board↔roadmap is the most literal "one component, density-tuned" proof
in the set (with **D — Civic** the most *impressive* coherence for folding the DSR/governance console
into the one skin). The **strongest agent-legibility treatment is C — Wayfinding**, the only finalist to
actually *render* the load-bearing parts of the R-14 contract — the Edit proposed→amended diff,
per-effect partial approval, and the agent-storm calm-under-surge state — proving foregrounded agents
need not become a firehose; **D — Civic** has the best single attribution artifact (the rendered
one-`correlation_id` provenance walk + a residency-on-effect chip). The honest spread: B leads on tone,
A on one-skin coherence, C on the agent contract, D on coherence-of-the-hard-surface + sovereignty-native
agent legibility — four real, non-overlapping bets, exactly as the funnel intended. All reception/"loved"
claims remain `[DEFERRED-UNTIL-USERS]`.

---

### Score grid (proposed by this lens; panel ratifies)

| Finalist | D3 | D4 | D6 |
|---|:--:|:--:|:--:|
| **A — Instrument** | 3 | **4** | 3 |
| **B — Workshop** | **3.5** | 3 | 3.5 |
| **C — Wayfinding** | 3 | 3 | **4** |
| **D — Civic** | 3 | 3.5 | 3 |

*Per-dimension winners: D3 → B · D4 → A · D6 → C. Scored only the three owned dimensions
(D1/D2/D5/D7/D8/D9/D10 belong to other lenses). Not committed.*
