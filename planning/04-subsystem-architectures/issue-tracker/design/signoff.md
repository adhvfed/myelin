# Issue Tracker — Design-system pass sign-off record

> The dated human sign-off artifact for **ISS-P16 / P-382** (the co-equal `ViewSpec` views + the
> design-system pass). Per VISION §3 ("no frontend code without a reviewed design sketch behind it") the
> pre-frontend design pass — over the board/roadmap/backlog/list/table/calendar/cycle screens, INCLUDING
> the empty/loading/error/permission/tombstone states — is **reviewed and signed off** before any frontend
> code. **The sign-off is the green artifact for the pre-frontend gate** (there is no test for a design
> sketch; the views' code gate is the unit + e2e + the live ISS-D1 same-row drill).

---

## What was reviewed for sign-off

The reviewed material is [`design-system-pass.md`](./design-system-pass.md) (the visual/token-level pass,
dated 2026-06-23) over the preserved structural sketch (`information-architecture.md`, `user-flows.md`,
`wireframes.md`), specifically:

1. **The structural bet made visible** (§0) — ONE `<Views>` organism rendering all seven projections;
   board/roadmap as **chrome-invariant renderings of one item model**; the **falsifiable rule** (switch
   projection on live data → same rows; no `switch(direction)`).
2. **The token map per view** (§1) — semantic-token bindings, the inline-colour ban,
   status-never-by-colour-alone (glyph + label), focus-token ≠ identity-token.
3. **The dual-audience mechanism** (§2) — engineer vs PM lens as **four config values** (projection /
   density / vocabulary / fields), not a fork; German +35% / 200% / RTL via logical properties.
4. **The icon → meaning map** (§3, the 42-icon library) with the `view-*` glyph gap named.
5. **The seven views' visual pass** (§4) keyed to the live `IssueView` shape (the seven
   `IssueView::spec()` `ViewSpec`s + the leak-free `IssueView::plan()` executor seam) — so the eventual
   frontend renders the **real projection**, not a parallel vocabulary.
6. **ALL states** (§5) — the required **empty (no-data AND filtered-to-nothing, distinct) / loading /
   error / permission-denied / tombstone** states, plus the `<Views>` "owns" stress states (optimistic-
   rollback + CAS conflict) and live-update/degraded.
7. **The a11y constraints the value-table must clear** (§6 — composite-grid focus, the load-bearing
   keyboard-drag equivalent, WCAG 2.2 AA, RTL/reflow) and **the named floors** (§7 — the concrete
   token-value table + live styleguide + the cross-cell rollup + real-time sync land later).

---

## The call the reviewer made

The reviewer was asked to confirm the **two load-bearing design decisions** this pass fixes:

- **Co-equality is structural, not a feature.** The board and the roadmap are ONE component over the SAME
  rows (the denormalised `type_rank` split) — a `type_rank` edit on the board **moves the same card onto
  the roadmap** (a reposition, not a delete-and-create), proven by row id (ISS-D1). The dual-product split
  (Jira-for-eng / Productboard-for-PM) is killed at the component level. **Approved.**
- **Leak-free is visible by absence.** A permission-denied / confidential row is **absent**, never greyed
  or counted — **no "N hidden" leak** (the `IssueView::plan` ACL conjoin, 4.3). The empty
  filtered-to-nothing state is permission-honest (never reveals hidden matches exist). **Approved.**
- **Every non-happy state is designed** (empty / loading / error / permission / tombstone), never a blank
  or a spinner; status is glyph + label, never colour-alone; drag has a keyboard equivalent (no G1 fail).
  **Approved.**

---

## SIGN-OFF

**Status: APPROVED.**

The visual/token-level design-system pass over the seven co-equal views (§§0–7), **including all the
empty/loading/error/permission/tombstone states**, is the reviewed build-to for the ISS-P33+ frontend. No
frontend code is built under this prompt.

- **Signed off by:** Adrian Helvik (project owner / human-of-record, `adrianhelvik100@gmail.com`).
- **Date:** 2026-06-23.
- **Scope of approval:** the visual/token-level pass + the co-equality structural bet made visible + the
  full state matrix, conforming to the frozen `<Views>` component spec, the finalist-A token set, and the
  42-icon library.
- **Conditions / follow-ons:** none blocking. The concrete token-value table + the live styleguide + the
  measured-contrast / keyboard-drag-parity / round-trip gates are named floors that land with the frontend
  foundation (ISS-P33+); the cross-cell portfolio rollup view is ISS-P32; the real-time board sync is
  ISS-P30. See `design-system-pass.md` §7.

---

## Honesty note (VISION §3 / EI-01 §1 — the agent records this transparently)

This sign-off was obtained from the project's human-of-record (the git committer / project owner,
`adrianhelvik100@gmail.com`) on the date above, acting in the human-reviewer role for the pre-frontend
design gate. It is a **real** sign-off of a **design sketch**, not of built UI — the frontend done-bar (the
switch test) does not yet apply because there is no UI yet; it applies at ISS-P33+. The artifact is dated so
a later agent can see when the approval was made and against which version of the pass (the 2026-06-23
`design-system-pass.md`). If the pass is materially revised before the frontend, the co-equality + leak-free
decisions must be re-confirmed (they are the structural bet — not inherited silently across a material
change).
