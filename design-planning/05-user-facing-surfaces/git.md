# Surface group: Git hosting & code review (§7.1)

> Phase 5 surface map · group **G** · maps [`design-language §7.1`](../../planning/02-holistic-architecture/design-language.md)
> against the [§2 template](./README.md#2-the-per-surface-map-template). Pointer map, not a spec — the
> corpus owns depth. PROVEN / HOUSE STYLE tagged; date 2026-06-20. Cross-cutting obligations
> ([README §3](./README.md#3-cross-cutting-obligations-every-surface-inherits)) are inherited by all and
> not restated. **The diff (G-7) is the funnel's recommended dense-engineer surface** ([README §5](./README.md#5)).

---

## G-6 — PR overview / context pane *(the wedge flagship)*
1. **Jobs:** E1 (see the *why* without leaving flow), E2 (PR shows linked issue/run/doc inline). Flow **F-ENG-1** §2.
2. **IA + shell:** `Code → <repo> → PR → Overview`; the **context pane (R-06 §3.4) is where the wedge is felt** — the PR's linked issue/run/doc resolve into it as chips/unfurls.
3. **Components:** chip/unfurl (R-09, heavily), comment thread (R-10 §5.5), editor (R-10 §3 for description), views (embedded checks).
4. **Density:** 0.4. Single-audience (engineer); no dual-lens fork.
5. **Agent:** agent reviewers/authors render with the **four-channel treatment** (R-14 §1); an agent-proposed PR arrives as a plan-card (F-AGT-1).
6. **Sovereignty:** per-artifact **visibility chip** (R-19 §1.2) on the PR header; T0 residency token in scope indicator; cross-cell linked-issue → T3 provenance (R-19 §1.1).
7. **State set (R-21 §2b):** the **PR-context-pane skeleton** (R-13 §A.2 — labelled slots: diff/issue/CI/discussion fill as bundles resolve, CA-2); permission (linked item viewer can't see → no-access card, not leak); cross-cell; moved/outdated linked-issue chip.
8. **A11y/i18n:** landmark structure for the pane; **G2** — German expansion on field labels, RTL mirror (pane → inline-end), linked-chip humanised titles (no machine strings).
9. **Device:** **MOB-2** context pane → mobile drawer; pane content stacks below overview on narrow. Read-friendly on mobile (review-status, links); authoring (merge) desktop-mainly.
10. **Wedge/motion:** **W1 (PR context pane assembles itself)** — scaffold renders instantly, slots fill; checks flip red→green in place via `motion.liveUpdate` (B5).
11. **DoD + switch test:** all R-21 §2b states present; W1 demonstrable; **switch test** — a reviewer lands on a PR and sees issue + CI + doc + discussion live without opening GitHub's 4-tab dance (F-ENG-1 🔪).

## G-7 — Diff / files-changed *(densest engineer surface; funnel target)*
1. **Jobs:** E4 (review line-by-line, only-what-changed), E3 (fix at the failing line). Flow **F-ENG-1** §2.2.
2. **IA + shell:** `Code → <repo> → PR → Diff`; content region (the one region that earns density, R-06 §3.3).
3. **Components:** comment thread (R-10 §5.5, anchored to a line), chip/unfurl (links in comments), overlays (R-10 §5 for the line-comment popover).
4. **Density:** **0.85 (highest)** — earns it via J1 (2-D line-anchored two-column artifact) + J2 (compact monospace tier) + J3 (inline diff-comment), R-07 §2.1. Stays unified on: shell, chip, identity, palette, comment thread, editor (for comments).
5. **Agent:** inline **agent-suggested-fix** marker (R-04 §2.2); agent review comments carry the treatment (R-14 §6.1).
6. **Sovereignty:** part-of-diff restricted → "Part of this diff is restricted" (no-leak, R-04 §2.2); residency inherited from repo scope.
7. **State set (R-21 §2b — diff **owns** rebase-orphan, col 11):** structure skeleton (gutters + line numbers first, R-13 §A.2); file deleted in HEAD → tombstoned hunk; **🔪 diff-anchored comment relocates after rebase** → content-anchored line-range re-resolves; if content gone → **detach to "outdated, on former line N" pill, never silent wrong-line move** (R-09 §5.9, PROVEN mechanism).
8. **A11y/i18n (R-17 §5.1 — the diff hard component, specced here):** keyboard (F7/Shift-F7 next/prev change, expand/collapse hunks, comment on focused line, no trap); SR (changed lines announce **"added/removed/unchanged" as TEXT** not colour, linear SR-review mode, line numbers announced, rebase-orphan anchor announces "moved/outdated"). **G2:** monospace covers non-Latin in code/comments; RTL — code stays LTR, comment prose mirrors, mixed-direction bidi-isolated; 200%/320px reflow on the dense surface.
9. **Device (desktop-mainly, MOB-6):** the diff is **desktop-primary**; on mobile renders **unified-only (no side-by-side)**, line-comment **read** with authoring via tap-to-expand sheet; **MOB-5** name the gutter fixed-width.
10. **Wedge/motion:** **W4 (failing check → step → line → fix, one warm chain)** — CA-1 prefetch warms the diff line before the click; `motion.settle` on inline-comment commit.
11. **DoD + switch test:** R-17 §5.1 keyboard+SR rows pass; rebase-orphan honest-detach present; **switch test** — an engineer reviews a dense diff fully on the keyboard, faster than GitHub, without the line-comment-after-rebase landing on the wrong line.

## G-8 — Review surface (verdicts, batched, agent-aware)
1. **Jobs:** E4 (high-signal review, threads that resolve). 2. **IA:** `Code → <repo> → PR → Review`; content + context pane. 3. **Components:** comment thread (R-10 §5.5), chip/unfurl, editor.
4. **Density:** 0.6. 5. **Agent:** agent reviewer with stated scope/reliability; **review-comment advisory, merge gated** (R-14 §6.1, frozen §6.3 default). 6. **Sovereignty:** visibility inherited.
7. **State set (R-21 §2b):** agent-pending (agent reviewing); batched-review pending; permission. 8. **A11y/i18n:** **batched review = one coherent event** (R-01 §4.3, R-02 R-BATCH — never per-comment pings); verdict controls keyboard-reachable; humanised state strings.
9. **Device:** MOB-1 hover comment-actions must be touch-reachable; read on mobile. 10. **Wedge/motion:** suggested-changes one-click-apply; `motion.settle` on verdict submit (no confetti). 11. **DoD + switch:** "Start review → batch → submit one verdict" emits one inbox event (R-02 R-BATCH-1); team moves without the GitHub un-batched 14-pings regression.

## G-9 — Checks / CI integration panel
1. **Jobs:** E3 (red CI → jump in). Flow F-ENG-1 §2.1. 2. **IA:** `Code → <repo> → PR → Checks`; content / context pane. 3. **Components:** chip/unfurl (run/step refs), overlays.
4. **Density:** 0.6. 5. **Agent:** triage-agent badge "reviewing this failure" (R-04 §2.2). 6. **Sovereignty:** run hidden if viewer can't read it (no leaked name).
7. **State set (R-21 §2b):** check skeleton rows (never blank spinner); run crypto-shredded → tombstone; **stale/reconnecting** (firehose drop → "Reconnecting… last updated 12s ago", auto-resume on `ci.*`). 8. **A11y:** **status by glyph+label+position, never colour-alone** (R-02 R-COLOUR-1 / G1) — the canonical traffic-light trap. **G2:** humanised check names.
9. **Device:** read-friendly; required-checks summary legible on narrow. 10. **Wedge/motion:** W4 entry; PR-going-green `motion.liveUpdate` in place (B5, R-12 §4). 11. **DoD + switch:** check status never colour-only; one click check→step→line warm (CA-1).

## G-1 / G-2 / G-3 / G-4 / G-5 — Repo home · file/blame · history/commit · compare · code search
- **G-1 Repo home** (E-browse): `Code → <repo>`; README render (editor read-path, R-10 §3), branches/tags, activity. Density 0.3. State (R-21 §2b): empty = onboarding-forward "Let's get your code in" (R-20 startup rung 0). Device: read-first. Switch test: a new repo's empty state teaches the next action, not a blank page.
- **G-2 File tree & file view + blame** (E5): `Code → <repo> → Code @ref`; **W5 backlinks** trail from blame → commit → PR → issue → decision (F-ENG-2, all live unfurls). Density 0.4. MOB-1 blame hover-actions touch-reachable. SR: blame announces pseudonymised author after erasure (R-04 §3.2). LFS/binary graceful handling.
- **G-3 History / commit views** (E5): signature verification status as glyph+label (G1); commit detail = diff (reuses G-7). Density 0.4.
- **G-4 Compare view**: arbitrary ref/SHA diff — **reuses the G-7 diff component** (density 0.85, same R-17 §5.1 obligations). The same diff, different entry.
- **G-5 Code search** (E6): `Code → <repo> → Code search`; permission-pre-filtered (R-08, ADR-03 — "find only what you may see"). Density 0.4. Results = chips (R-09). State: no-results vs no-access distinct (R-21 col 14 vs 4).

## G-10 — Branch-protection / ruleset editor *(admin; flow-orphaned — job-linked)*
1. **Job link ([README §4.2](./README.md#42-critic-fix-2--flow-orphaned-admin-surfaces-give-each-an-explicit-job-link)):** **E12** (P5/P3 — accept fork contributions without leaking secrets needs required-reviewers/status-gates/fork-CI rules) + **G2** (P12/P15 least-privilege on protected refs). Used when a maintainer hardens `main`.
2. **IA + shell:** `Code → <repo> → [A] settings`; one layer down (R-06 §2 rule 4, P4 — reachable, never imposed). 3. **Components:** views/forms, overlays (confirm).
4. **Density:** 0.4 — progressive-disclosure admin (R-02 R-CFG-2: adding rules never changes the *default* repo surface). 5. **Agent:** n/a (or: which agents may bypass — links to S-9). 6. **Sovereignty:** RBAC-adjacent; effective-access view (R-19 §1.2).
7. **State set (R-21 §2b):** validation errors inline; permission (only maintainers). 8. **A11y/i18n:** form a11y; **G2** German expansion on long policy labels (R-18 §2 — branch-protection labels are verbose). 9. **Device:** **desktop-mainly (MOB-6)** — config authoring; read-only on mobile.
10. **Motion:** none decorative; `motion.settle` on save. 11. **DoD + switch:** anti-config-maze (R-02 R-CFG-1) — the editor is depth-on-demand, not a wall; a team configures protection without an "admin-as-a-job" requirement (R-CFG-3).

## G-11 — Repo settings (collaborators/teams, webhooks, keys)
1. **Jobs:** E12 (scoped permissions), G2 (inspectable access). 2. **IA:** `Code → <repo> → [A] settings`. 3. **Components:** views, overlays. 4. **Density:** 0.4. 5. **Agent:** n/a. 6. **Sovereignty:** collaborator authz = RBAC face (S-8); SSH keys under identity. 7. **State (R-21 §2b):** permission-denied graceful. 8. **A11y/i18n:** form a11y; humanised role labels. 9. **Device:** desktop-mainly. 10. **Motion:** settle-on-save. 11. **DoD + switch:** "who can see/do what" inspectable (G2), no Atlassian seam (one permission language, R-02 R-SEAM-2).

---

**Group invariants reminder:** every G-surface holds the eight invariants (R-07 §3) — the chip in the
diff is the *same* chip as in chat; the palette/inbox/identity badge are shell-owned. The diff's
`0.85` distinctness is **earned density, not a fork** (R-07 §2.1) — that is the D4 test this group most
stresses.
