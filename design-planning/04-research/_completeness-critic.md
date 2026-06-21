# Phase 4 Completeness-Critic Pass (the gloss-risk audit before Phase 5)

> Role: a **critic pass**, not a redo. Job: find what the autonomous pipeline GLOSSED — gaps,
> contradictions, orphaned surfaces, missed states — before the corpus is finalized and Phase 5 begins.
> Bar: roadmap [`README.md`](../02-research-roadmap/README.md) §7 (corpus definition-of-done, 9 criteria)
> + §9 (gloss-risk list); [`rubric.md`](../02-research-roadmap/rubric.md) (gates G1/G2, dims D1–D10);
> [`sketch-funnel.md`](../02-research-roadmap/sketch-funnel.md) (comparable screen set). Surface coverage
> cross-checked against [`design-language.md`](../../planning/02-holistic-architecture/design-language.md) §7.
> Tags: **PROVEN-gap** = verifiable omission; **JUDGEMENT** = critic's opinion. Status date: **2026-06-20**.
>
> Headline: the corpus is **strong** — all 22 files exist at their paths, each carries PROVEN/HOUSE-STYLE
> tags + a 2026-06-20 date, a self-check section, and an honest `[DEFERRED-UNTIL-USERS]` block. The
> unglamorous-states work (R-21) is genuinely exhaustive. This critic finds **no fully-orphaned surface**
> and **one materially-uncovered gloss-risk** (touch/mobile). The rest are seams Phase 5 must *resolve*,
> not holes Phase 4 must *fill*.

---

## 1. DoD scorecard (roadmap §7, the 9 corpus-definition-of-done criteria)

| # | Criterion | Verdict | One-line evidence |
|---|---|---|---|
| 1 | Every R-item has a file at its stated path, tagged + dated | **PASS** | All 22 files present (`find 04-research`); each has PROVEN/HOUSE-STYLE tags + `Status/File date: 2026-06-20` + a `Self-check against R-xx` section. |
| 2 | The three audiences are covered (JTBD + ≥1 flow each) | **PASS** | `jtbd-catalogue.md` names E1–E12 (eng), M1–M10 (PM), G1–G10 (gov), §2–§4. `cross-surface-flows.md` §1: F-ENG-1/2, F-PM-1/2, F-GOV-1, F-AGT-1 → ≥1 per audience + agent flagship. |
| 3 | "One product, five surfaces" answered concretely | **PASS** | `ia/platform-ia.md` §2 one-shell tree + §8 places every §7 group; `interaction/*` spec the shared chip/palette/views/editor; `ia/unification-study.md` §1.1 J1/J2/J3 names *where* unification yields to density (diff 0.85, roadmap 0.5) and why. |
| 4 | Every primary §7 view reachable from a flow OR pattern spec OR state matrix | **PASS** (see §3) | `craft/state-craft.md` §2a–§2g matrix is the backstop: every §7.1–§7.7 surface (incl. admin consoles + CLI) has a row; `platform-ia.md` §8 + `reference-unfurl.md` §7.1 corroborate. No orphans — but several surfaces are *matrix-only*, not flow-exercised (§3 caveat). |
| 5 | Lovability/craft items exist and are concrete | **PASS** | `visual/motion-microinteractions.md` (motion tokens + reduced-motion first-class), `visual/perceived-performance.md` (skeletons + optimistic rollback), `craft/onboarding-delight.md` (3 archetypes), `craft/state-craft.md` (14 states), `craft/wedge-moments.md` (W1–W7), `agent-ux/legibility-and-hitl.md` + `attribution-and-calm.md`. |
| 6 | Hard gates specified, not assumed (checkable a11y + i18n/RTL) | **PASS** | `accessibility/audit-method.md` §5 gives a per-hard-component keyboard+SR checklist (diff/board/views/editor/HITL/palette/overlays) the rubric G1 can point to; `accessibility/i18n-rtl-patterns.md` §2–§4 gives German expansion, Greek/Cyrillic, whole-shell RTL with real Arabic/Hebrew → G2. |
| 7 | Every deferred-until-users item recorded with substitute + trigger | **PASS** | Each of the 8 roadmap §6 deferrals has a `[DEFERRED-UNTIL-USERS]` home with method + falsifier: R-03 §6 (ODI ranking), R-05 §5 (real personas), R-07 Part 2 (card-sort), R-15 Part 2 (PAIR trust), R-16 §6 (both-audience), R-17 §8 (AT-user), R-19 §9 (regulated-buyer); RITE deferred at R-04 §11. |
| 8 | The completeness-critic list (§9) is addressed | **PARTIAL** | `state-craft.md` §3 explicitly OWNS the §9 list; nearly every named state/flow has a home (see §2 below). **One miss:** the device/form-factor → touch/mobile gloss is consciously *deferred* by R-13 but never actually *covered* anywhere (PROVEN-gap, §2). |
| 9 | Control artifacts usable as-is | **PASS** (out of Phase-4 scope to alter) | `rubric.md` is scoreable (gates + weighted dims + tie-break); `sketch-funnel.md` defines the comparable screen set + axes; corpus is authored *toward* them (rubric §5 contract is satisfied by R-21/R-17/R-18 demos). |

**Score: 8 / 9 PASS, 1 PARTIAL** (criterion 8, due to the single touch/mobile gloss-risk; criterion 4 PASS-with-caveat).

---

## 2. Gloss-risk coverage (roadmap §9, item by item)

Legend: **COVERED** (well, with a home) · **THIN** (named but lightweight) · **UNCOVERED** (gap).

### 2.1 Unglamorous UI states (route to R-21)
| §9 gloss-risk | Home | Verdict |
|---|---|---|
| Loading shows structure (skeletons matching final layout) | R-21 §1.2 + R-13 A.2 (per-surface skeleton catalogue, no-spinner-token) | **COVERED** |
| Empty states onboarding-forward (first repo/issue/doc/channel/agent) | R-21 §1.1 (3 kinds) + R-20 Archetype-1 zero-data shell | **COVERED** |
| Error blames system, one quiet line + path | R-21 §1.3 (system-blamed + correlation_id + path) | **COVERED** |
| Permission-denied as graceful no-access card, no leaked title | R-21 §1.4 + R-09 §5.4 (Restricted vs Absent, no-leak by construction) | **COVERED** |
| Erased / tombstoned (chip/unfurl/backlink/search each) | R-21 §1.5 + R-09 §5.7 (sub_gone/root_gone/erased) | **COVERED** |
| Agent-pending (working / awaiting approval) | R-21 §1.6 + R-14 §5 (gate-awaiting frontstage) | **COVERED** |
| Degraded-surface "temporarily unavailable" (fails static) | R-21 §1.7 (per-surface fails-static) | **COVERED** |
| Stale / offline / reconnecting (firehose drop+resume) | R-21 §1.8 (3 escalating cues, lossless resume) | **COVERED** |
| Optimistic-update rollback (honest failure) | R-21 §1.9 + R-13 A.3 (pending/settled/rolled-back contract) | **COVERED** |
| Conflict surfacing (CAS→CRDT, no silent overwrite) | R-21 §1.10 (CAS choose-yours/theirs; CRDT auto-merge) | **COVERED** |

### 2.2 Edge-case & cross-surface flows (route to R-04/R-22)
| §9 gloss-risk | Home | Verdict |
|---|---|---|
| Partial-failure agent branches (gate-rejected / mid-chain error / budget / loop-guard) | R-04 F-AGT-1 §7.2 + R-14 §5 table (all 5 branches enumerated) | **COVERED** |
| Cross-cell / cross-tenant ref → no-access or tombstone | R-09 §5.8 + R-04 state tables + R-22 W1/W5/W6 | **COVERED** |
| Diff-anchored comment relocates / orphans after rebase | R-09 §5.9 (4 states; content_gone → detach to file-level pill, never silent jump); R-04 F-ENG-1 §2.2; R-22 W4; R-21 §1.11 | **COVERED** (one seam, §4.3) |
| Storm / 30×-agent-surge notification experience | R-21 §1.13 (inbox owns) + R-15 §5.2 (agent lane sheds first) | **COVERED** |
| DSR/erasure flow from data-subject AND DPO side | R-19 §2 (DPO orchestrator) + §3 (subject own-scope, request-not-execute) | **COVERED** |

### 2.3 Accessibility cases (route to R-17, enforce in G1)
| §9 gloss-risk | Home | Verdict |
|---|---|---|
| Keyboard-only operability of the hard components | R-17 §5.1–§5.7 (diff/board/views-inline-edit/editor/HITL/palette/nested-overlays each) | **COVERED** |
| SR announcement of live/event-driven updates without spamming | R-17 §6.1 (politeness rules) | **COVERED** |
| Visible focus in every theme (light/dark/HC), focus≠identity token | R-17 M2 theme sweep + R-11 §2 focus-token rule | **COVERED** |
| Status-not-by-colour-alone | R-17 M4 + §6.2 greyscale test | **COVERED** |
| 200% zoom / reflow on dense surfaces; reduced-motion first-class | R-17 M7 zoom sweep + R-12 L4/§2.4 reduced-motion override table | **COVERED** |
| RTL mirroring of the *whole* shell incl. editor/views/overlays, real RTL string | R-18 §4.1–§4.4 (real Arabic/Hebrew + mixed-direction run) | **COVERED** |

### 2.4 Device / form-factor glosses (route to R-13/R-21)
| §9 gloss-risk | Home | Verdict |
|---|---|---|
| **Touch / mobile** (hover-only row actions invisible; full-width panels clip; popovers under bottom-pinned composer must flip) | R-13 explicitly routes it *away* ("out of scope for a perceived-performance file… routed to R-21/R-13/R-04"); R-21's per-surface matrix has **no device dimension**; no file actually specifies the touch behaviours | **UNCOVERED** — PROVEN-gap |
| CLI as a peer surface (error/ref vocabulary) | R-21 §2g CLI row + note; R-06 §7.7; R-09 §7.1 | **COVERED** |

### 2.5 Process gloss-risk
| §9 gloss-risk | Home | Verdict |
|---|---|---|
| Funnel converging early on one instinct | `sketch-funnel.md` axes + cull-for-spread rule (Phase-6 control, not a corpus file) | **COVERED** (deferred to Phase 6 by design) |

**Uncovered gloss-risks: 1** (touch/mobile). All others COVERED. **0 THIN-only** risks of consequence.

---

## 3. Orphaned surfaces (primary §7 views with no research behind them)

**Result: ZERO fully-orphaned surfaces.** Cross-checking the design-language §7 catalogue against the
three reachability paths (R-04 flow / R-08·R-09·R-10 pattern spec / R-21 §2 state matrix):

- The R-21 §2 per-surface matrix (§2a–§2g) is the backstop and it is **exhaustive** — it has a row for
  every §7.1–§7.7 surface, *including* the easily-glossed governance/admin consoles: branch-protection
  editor, secrets, workflow/SLA admin, dashboards, RBAC, **agent governance + kill-switch**, **audit-log
  explorer**, **GDPR/DSR console**, **data-map/RoPA & residency console**, and the **CLI**.
- IA placement is confirmed for all of them in `platform-ia.md` §8; the chip/unfurl recurrence is confirmed
  in `reference-unfurl.md` §7.1.

**JUDGEMENT caveat for Phase 5/6 (not an orphan, but a thinness to watch):** several heavy admin surfaces
are reachable *only* via the state matrix + IA tree, **not exercised by any R-04 task flow** — notably the
**pipeline/definition editor + validator** (§7.2), **branch-protection/ruleset editor** (§7.1),
**templates UI** (§7.4), **billing/usage/export-&-exit** (§7.6), and the **incident/"canvas" view** (§7.5,
itself `[UNCERTAIN/DEFER]` in the source catalogue). They have a *placement* and *states* but no
*narrative of use*. Phase 5's per-surface DoD should author at least a thin flow or job-link for these so
they don't get sketched as decoration. The comparable-screen-set (`sketch-funnel.md`) does not force any of
them, so they will be skipped unless Phase 5 names them.

---

## 4. Contradictions / seams (name them; Phase 5 resolves — critic does NOT resolve)

### 4.1 Chat threading: ruled, but lightly — confirm against the Zulip-topic question
`platform-ia.md` §2 normalises **thread** as a first-class navigable L2 node (`Chat → <channel> → <thread>`)
and makes it `ArtifactRef`-able (`#thread-<id>`). This is a *ruling* (threads-as-artifacts) but it does **not
explicitly adjudicate** the Zulip-topic-stream model vs Slack-style ephemeral threads, and the source
catalogue still flags the **incident/"canvas" view** as `[UNCERTAIN/DEFER]`. **Seam for Phase 5:** confirm
whether topics-within-channels (Zulip) is in or out, because the thread-pane density (R-21 §2f) and the
incident surface depend on the answer. **JUDGEMENT.**

### 4.2 Vocabulary-fracturing handling across R-06 / R-07 / R-16 — converged but UNVALIDATED
All three agree on the *direction* (vocabulary is presentation, never a schema fork): R-16 §5 proposes the
T1-frozen-schema / T2-curated-synonyms / T3-audited-rename bound; R-06 §6.3 + R-07 Part-2 flag the
two-label card-sort as the **decisive** test and **defer it to users**. **No contradiction** — but the
load-bearing fracturing question is *deferred, not answered*. **Seam for Phase 5:** treat T2 curated
synonyms as a HOUSE-STYLE bet a finalist must demonstrate, and carry the card-sort as a live deferral; do
not let a sketch treat vocabulary-adaptation as *settled*. **PROVEN-gap (validation), not a contradiction.**

### 4.3 Rebase diff-anchor relocation — consistent, but R-10 is silent (ownership ambiguity)
R-09 §5.9 OWNS the resolver (4 states; orphan → detach to file-level pill); R-04 F-ENG-1, R-22 W4, and
R-21 §1.11/§2b all cite R-09 and agree. **But** `shared-patterns.md` (R-10) — which owns the views/editor
substrate where *non-diff* anchored comments could also relocate — does **not** address content-anchored
relocation at all. **Seam for Phase 5:** confirm whether anchored-comment relocation is diff-only (R-09's
scope) or a general content-anchor pattern the editor/views also owe. **JUDGEMENT.**

### 4.4 Token AA-contrast values are principle-level, not numeric (G1 dependency)
`visual-direction.md` states the focus≠identity + measured-contrast *rules* but leaves concrete token values
`[OPEN → Phase 4/8]`. The rubric G1 requires **measured** contrast in the sketch artifact. **Seam:** Phase 6
finalists must author DTCG tokens with real AA-passing values from sketch #1 (rubric §5 already mandates
this) — the corpus gives the rules, not the numbers, which is correct *but* means G1 is only assertable, not
yet provable, until sketches exist. **JUDGEMENT (expected handoff, flag so it isn't forgotten).**

---

## 5. Top gaps to fix before sketches (prioritized)

**FIX NOW (before Phase 6 sketches lock the screen set):**
1. **[HIGH · PROVEN-gap] Touch/mobile form-factor is uncovered.** No file specifies the §9 touch behaviours
   (hover-only actions invisible on touch; full-width panels clipping beside a present column; popovers under
   a bottom-pinned composer flipping). R-13 punted it and R-21's matrix has no device axis. Either author a
   short device/responsive-behaviour note (extend R-13 or R-21 §2 with a device column) **or** make it an
   explicit, reasoned Phase-6 deferral in the funnel — but it must not stay silently dropped (it is the one
   §7-criterion-8 miss).
2. **[MED · JUDGEMENT] Name thin flows for the flow-orphaned admin surfaces** (pipeline editor,
   branch-protection editor, templates, billing/export-&-exit). They have states + IA placement but no
   narrative of use, and the comparable-screen-set won't force them. Phase 5's per-surface DoD should give
   each a one-line job link so they aren't sketched as decoration.
3. **[MED · JUDGEMENT] Resolve the chat-threading model explicitly** (§4.1) before the chat/incident screens
   are sketched — Zulip-topics in or out; settle the `[UNCERTAIN/DEFER]` incident/canvas view.

**CONSCIOUSLY DEFER (with reason — already correctly flagged, just confirm they stay flagged):**
4. **Vocabulary-fracturing card-sort** (§4.2) — genuinely needs users (roadmap §6). Reason to defer: the
   two-label findability test is participant-driven. *Action:* carry T2 synonyms as a HOUSE-STYLE bet a
   finalist demonstrates; do not let Phase 6 treat it as validated.
5. **All 8 roadmap-§6 deferrals** (real personas, ODI ranking, PAIR trust, both-audience, AT-user testing,
   regulated-buyer review, RITE on sketches) — correctly homed with substitutes + falsifiers; keep them
   visible so Phase 6/7 never treat the no-user substitute as validation.
6. **Numeric AA token values** (§4.4) — correctly left to per-finalist DTCG token sets; rubric §5 already
   enforces this at 6c. No corpus action; just ensure Phase 6 authors them from sketch #1.

**The single most important contradiction Phase 5 must resolve:** the **vocabulary-fracturing handling**
(§4.2) — R-06/R-07/R-16 *agree on the bound* but *defer the decisive test*; Phase 5 must explicitly carry it
as an unvalidated HOUSE-STYLE bet (the central dual-audience risk), not let a sketch present persona-adaptive
vocabulary as settled. (Runner-up: confirm the chat-threading model, §4.1.)

---

## 6. Bottom line

The Phase-4 corpus clears its own definition-of-done at **8/9 PASS (1 PARTIAL)**, with **zero fully-orphaned
surfaces** and **exactly one materially-uncovered gloss-risk (touch/mobile)**. The unglamorous-state work
(R-21) is the corpus's strongest asset and genuinely defeats the happy-path bias §9 warned about. The
remaining issues are **seams to resolve, not research to redo** — chiefly: cover/defer touch explicitly,
give the flow-orphaned admin surfaces a narrative, and carry vocabulary-fracturing forward as an
*unvalidated* bet. Phase 5 may proceed once gap #1 (touch) is either covered or consciously deferred in the
funnel.
