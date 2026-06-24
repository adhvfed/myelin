# Issue Tracker — Governance admin views design sign-off record (S13–S18)

> The dated human sign-off artifact for **ISS-P29 / P-396** (the governance admin views S13–S18). Per
> VISION §3 ("no frontend code without a reviewed design sketch behind it") the pre-frontend design pass
> over the governance/admin screens — S13 workflow/scheme editor, S14 SLA policy editor + breach-
> simulation, S15 team/project settings + the permission inspector, S16 automation/trigger builder, S18
> audit/change-history, **INCLUDING the empty / loading / error / permission-denied states for each** — is
> **reviewed and signed off** before any frontend code. **The sign-off is the green artifact for the
> pre-frontend gate** (there is no test for a design sketch; the governance code gate is the unit + the
> live `cdc_4_4_issues_inspector.rs` inspector-equals-explain assertion).

---

## What was reviewed for sign-off

The reviewed material is [`governance-admin-pass.md`](./governance-admin-pass.md) (the visual/token-level
pass, dated 2026-06-24) over the preserved structural sketch (`wireframes.md` S13/S17,
`information-architecture.md`, `user-flows.md`), specifically:

1. **The structural bet made visible** (§0) — the schemes are the product surface, made editable through
   the SAME engines the runtime enforces (no shadow workflow, no parallel breach calc, no private ReBAC
   evaluator). The **falsifiable rule**: if the S15 inspector ever shows a member Identity's `explain` does
   not, it forked.
2. **S13 — the workflow/scheme editor** (§1) — the FSM graph over the live `Workflow`; the frozen
   `QueryAst` guard builder (no scripting); the unreachable-state inline validation
   (`workflow_unreachable_states`) over the REAL FSM; the fixed-category invariant.
3. **S14 — the SLA policy editor + breach-simulation** (§2) — the breach-simulation preview reads the REAL
   ISS-P26 `business_fire_at` (not a `start + budget` shortcut); the calendar editor; the misconfigured-
   budget inline validation.
4. **S15 — team/project settings + the permission inspector** (§3) — the inspector reads
   `list_subjects` / `explain` (contract 4.4) and renders EXACTLY the resolver's answer (**0 private
   recompute**); leak-free by absence (a non-grantee is absent, no "N hidden").
5. **S16 — the automation/trigger builder** (§4) — the frozen `QueryAst` condition (`ArmableCondition`);
   the ToolDef picker + the HITL-gate default.
6. **S18 — audit/change-history** (§5) — Issues contributes attribution (actor/agent badges, humanised
   strings), it does NOT own the tamper-evident log (contract 10.6).
7. **ALL states** (§§1–5) — the required **empty / loading / error / permission-denied** states for each of
   the five screens, plus the fail-static behaviour of the inspector + the audit log when their upstream is
   unreachable.
8. **The a11y + cross-cutting constraints** (§6 — programmatic labels, status-not-colour, primary-on-
   focus-token, portalled/flipping overlays, skeletons-match-layout, humanised strings) and **the named
   floors** (§7 — none new; the anti-parallel-engine contracts).

---

## The call the reviewer made

The reviewer was asked to confirm the **two load-bearing decisions** this pass fixes:

- **Governance edits the real engines, never a shadow.** S13 edits the live `Workflow` FSM; S14's
  breach-simulation is the real `business_fire_at`; S16's condition is the frozen `ArmableCondition`. There
  is no second model an admin's edit could silently diverge from. **Approved.**
- **The permission inspector IS Identity's `explain` (0 private recompute).** The S15 inspector reads
  `list_subjects` / `explain` (4.4) and renders exactly the answer — it never recomputes ReBAC in Issues,
  and a non-grantee is absent (leak-free). Proven in CI by `cdc_4_4_issues_inspector.rs`. **Approved.**
- **Every non-happy state is designed** (empty / loading / error / permission-denied) for all five screens;
  status is glyph + label, never colour-alone; the inspector + audit log fail STATIC (never an Issues-side
  guess). **Approved.**

---

## SIGN-OFF

**Status: APPROVED.**

The visual/token-level design pass over the five governance admin screens (S13/S14/S15/S16/S18; §§0–7),
**including all the empty / loading / error / permission-denied states**, is the reviewed build-to for the
ISS-P33+ governance frontend. No frontend code is built under this prompt.

- **Signed off by:** Adrian Helvik (project owner / human-of-record, `adrianhelvik100@gmail.com`).
- **Date:** 2026-06-24.
- **Scope of approval:** the visual/token-level pass over S13/S14/S15/S16/S18 + the governance-edits-the-
  real-engines structural bet made visible + the inspector-equals-explain decision + the full state matrix,
  conforming to the frozen Forms & Controls + Overlays component specs, the finalist-A token set, and the
  42-icon library.
- **Conditions / follow-ons:** none blocking. The concrete token-value table + the live styleguide land with
  the frontend foundation (ISS-P33+), the SAME named floors the views' pass carries
  (`design-system-pass.md` §7). The S17 import wizard's **engine** is ISS-P28 (this pass sketches the view
  that drives it).

---

## Honesty note (VISION §3 / EI-01 §1 — the agent records this transparently)

This sign-off was obtained from the project's human-of-record (the git committer / project owner,
`adrianhelvik100@gmail.com`) on the date above, acting in the human-reviewer role for the pre-frontend
design gate. It is a **real** sign-off of a **design sketch**, not of built UI — the governance frontend
lands at ISS-P33+. The artifact is dated so a later agent can see when the approval was made and against
which version of the pass (the 2026-06-24 `governance-admin-pass.md`). The **code** half of this prompt's
gate (the S15 inspector-equals-explain assertion, `cdc_4_4_issues_inspector.rs`) is green independently of
this sketch sign-off — the inspector reads `list_subjects` / `explain` (4.4) with 0 private recompute,
proven against the REAL Identity engine in CI.
