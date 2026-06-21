# WS-H — Accessibility & i18n / l10n / RTL

> Workstream H (see [`README.md`](./README.md)). For an EU-sovereign product these are **requirements**,
> not enhancements (design-language §4; EN 301 549 / EAA is a legal procurement bar). This workstream
> produces the *method + checklist* that the rubric's **hard gates G1 (accessibility) and G2 (i18n/RTL)**
> point to. Build ON §4; do NOT re-derive the baseline. Phase-1 methods #21 (a11y audit, PROVEN-legal),
> #12 (measured tokens), #6 (IA for i18n).

---

## R-17 — Accessibility audit method & per-surface a11y checklist

**Questions answered.** Exactly what must a sketch/surface demonstrate to pass WCAG 2.1 AA / EN 301 549
(the hard floor) and the WCAG 2.2 AA house target? What is the manual-audit method (automated catches
only ~30–40%)? What is the per-surface checklist for the *hard* components (diff, board drag, views
inline-edit, editor, HITL card, palette, nested overlays)?

**Phase-1 methodology.** #21 accessibility audit (WCAG 2.2 AA / EN 301 549 / EAA; automated + manual
expert review of keyboard/focus/contrast/ARIA/reflow); #12 measured-not-claimed token QA (contrast math,
focus-token ≠ identity-token, status-not-by-colour-alone).

**Inputs.** design-language §4 (the full baseline: keyboard, focus, contrast-as-token-constraint,
screen-reader-as-component-contract, reduced-motion, 200% zoom), §8b.3 (the measured-token rules); the
rubric G1; R-10 (the components to audit); the cited standards (WCAG 2.2; EN 301 549; EAA enforceable
2025-06-28; note WCAG 2.2 ⊇ 2.1 except obsoleted 4.1.1).

**Deliverable.** `design-planning/04-research/accessibility/audit-method.md`. The audit method (automated
pass + manual expert pass per surface) and a **per-surface a11y checklist** the rubric's G1 references:
contrast-measured-not-claimed (incl. the focus-token-derivation rule); visible focus in light/dark/
high-contrast; full keyboard operability + no traps for each hard component; status-not-by-colour-alone;
correct semantics/ARIA per pattern; live-region announcement of event-driven updates without spamming;
200% zoom/reflow on dense surfaces; reduced-motion as first-class. Each item tagged PROVEN with its
WCAG/EN-301-549 criterion. Includes the **deferred** assistive-technology user-testing plan (the part
the audit can't cover).

**Sequencing & dependencies.** Seq #15. Depends on R-10 (components to audit). Feeds the rubric G1 (the
gate references this checklist) and Phase 5 (the a11y CI gate) and Phase 6 (every sketch audited).

**User-dependency.** none for the audit method/checklist; **AT user testing is deferred-until-users**
(carried from README §5.3).

**Effort.** M.

**Acceptance criteria.** The checklist is specific enough that G1 is *checkable* (not "be accessible");
every hard component has a keyboard + screen-reader entry; the focus-token-≠-identity-token rule and
measured-contrast rule are present; each item cites its WCAG/EN-301-549 criterion; the deferred AT user
test is recorded; WCAG 2.1-floor vs 2.2-target relationship is stated correctly.

---

## R-18 — i18n / l10n / RTL interaction-pattern research

**Questions answered.** What does the UI need to demonstrate multiple EU languages including a long-word
language (German) and a non-Latin script (Greek/Cyrillic), without truncation/overflow? How is RTL built
in via logical (start/end) properties so the *whole* shell — incl. editor, views component, overlays —
mirrors correctly (not a flipped mockup)? What are the locale-aware formatting needs (dates/numbers/
calendars — SLA/business-calendar load-bearing)? Where do fixed-width assumptions break (§8b.4)?

**Phase-1 methodology.** #21 a11y audit (i18n/RTL portion); #6 IA (labelling/vocabulary as i18n surface).

**Inputs.** design-language §4 (i18n-first, EU-language support, RTL via logical properties, locale-aware
dates/calendars), §3.3 (EU-multilingual type coverage as a selection criterion: Latin-extended, Greek,
Cyrillic), §8b.4 (fixed-width/mobile bug classes), §8b.5 (humanise machine strings); the rubric G2; R-06
(IA/labels); R-08–R-10 (the components that must survive expansion + RTL).

**Deliverable.** `design-planning/04-research/accessibility/i18n-rtl-patterns.md`. The i18n/l10n/RTL
pattern set the rubric's G2 references: the text-expansion handling (German ~30–40% longer — no
truncation/clipping; no fixed-width assumptions); the non-Latin rendering requirements (font coverage,
line-height, no clipping for Greek/Cyrillic); the **RTL pattern** (logical properties throughout; the
shell + editor + views + overlays mirrored; tested with a real RTL string); locale-aware date/number/
calendar formatting; the humanised-string requirement (no raw machine strings). Specifies the **exact
demonstration set** Phase 6 finalists must show for G2 (≥1 long-word language, ≥1 non-Latin script, ≥1
mirrored RTL state, locale-formatted dates).

**Sequencing & dependencies.** Seq #16. Depends on R-06, R-08–R-10. Feeds the rubric G2 (the gate
references this) and Phase 6 (the G2 demonstration set is designed in from sketch #1).

**User-dependency.** none.

**Effort.** M.

**Acceptance criteria.** The text-expansion + non-Latin + RTL patterns are concrete and reference logical
properties; the exact G2 demonstration set (languages + RTL + locale formatting) is specified; the
whole-shell mirroring (incl. editor/views/overlays) is required, not just text direction; humanised
strings are required; the fixed-width-assumption bug classes from §8b.4 are named as things to design
around.
