# Surface group: CI/CD (§7.2)

> Phase 5 surface map · group **C** · maps [`design-language §7.2`](../../planning/02-holistic-architecture/design-language.md)
> against the [§2 template](./README.md#2-the-per-surface-map-template). Pointer map; PROVEN / HOUSE
> STYLE tagged; date 2026-06-20. Cross-cutting obligations ([README §3](./README.md#3)) inherited by
> all. The **live log (C-3)** sits at Axis-3 `0.8` — second only to the diff.

---

## C-2 — Single-run view (DAG / stages / jobs / steps)
1. **Jobs:** E3 (failing check → step → line). Flow **F-ENG-1** §2.1. 2. **IA + shell:** `CI → <run> → DAG`; content region. 3. **Components:** chip/unfurl (step/log-line refs, R-09 §5.2 `#job=<id>/step=<id>`), overlays.
4. **Density:** 0.8 — earns via J1 (DAG, a genuine graph shape) + J2 (compact). 5. **Agent:** agent-triage badge; proposed-fix-as-plan (C-9). 6. **Sovereignty:** in-region compute cue (E10 — "EU-controlled compute"); residency T0.
7. **State set (R-21 §2c):** run skeleton; step output redacted if secret-scoped; run past TTL → "Logs expired (90-day retention)" tombstone. 8. **A11y/i18n:** **DAG mirrors in RTL** (linear-time progression, R-18 §4.2); step status glyph+label not colour-alone (G1); SR-navigable graph. 9. **Device:** read-friendly DAG on mobile (zoom/pan); re-run actions read-then-act.
10. **Wedge/motion:** W4 chain entry; step status `motion.liveUpdate`. 11. **DoD + switch:** one click step→line (CA-1 prefetch); the run reads as one product, not an embedded foreign widget (R-02 R-SEAM-4).

## C-3 — Live log view *(streaming; Axis-3 0.8)*
1. **Jobs:** E3 (the exact line in opaque logs). Flow F-ENG-1 §2.1. 2. **IA:** `CI → <run> → Logs`; content. 3. **Components:** chip (log-line ref → diff-line, the W4 mechanism), overlays.
4. **Density:** 0.8 — J1 (time-ordered append-only stream, not rows) + J2 (compact monospace, virtualised). 5. **Agent:** agent-triage reads the log; failure formatted for triage (C-9). 6. **Sovereignty:** secret-masked output (PROVEN — never leaks secrets); in-region.
7. **State set (R-21 §2c — live log **owns** stream-drop/resume with chat timeline, col 8):** streamed log skeleton (tail follows); **"Log stream interrupted — resume"** (firehose drop, lossless resume on replay); log past TTL → "Logs expired". 8. **A11y/i18n:** streaming tail must not spam SR live region (R-17 §6.1 politeness); search-in-log keyboard; monospace covers non-Latin (R-18 §3.2). 9. **Device:** read on mobile; tail-follow + search; downloadable.
10. **Wedge/motion:** the **log line IS a ref to the diff line** (W4, R-22) — click → warm diff (CA-1); coalesced updates during burst (R-12 R2, no strobe). 11. **DoD + switch:** the failing line is one click from the log (not "scroll opaque logs → guess the file"); stream resumes losslessly after a drop.

## C-6 — Environments & deployments (+ approvals queue)
1. **Jobs:** E8 (observable automation), deploy-gate. Flow **F-AGT-1** (HITL). 2. **IA:** `CI → Environments`; content. 3. **Components:** **HITL approval card (R-10 §5 / R-14 §2)**, chip/unfurl, views.
4. **Density:** 0.5. 5. **Agent (R-14 §6.3):** deploy is a **frozen gated effect** — the **approvals queue is a HITL surface**; Approve/Edit/Reject; durable gate waits minutes/days. 6. **Sovereignty:** "what's deployed where" includes region; T2 cross-boundary warning if a deploy would cross residency.
7. **State set (R-21 §2c — approvals queue **owns** agent-pending col 6):** gate-awaiting; stale-approval ("base changed — re-propose?", R-04 §7.2); approval-card-storm collapses to "7 approvals awaiting you" (R-15 §5.2). 8. **A11y (R-17 §5.5 HITL card):** per-effect controls individually focusable; agent treatment as TEXT; arrival announced politely. **G2:** humanised deploy/env strings; locale-aware timestamps.
9. **Device:** read deploy state on mobile; **approve from inbox on mobile is allowed** (S-4) but Edit defers to desktop (MOB-6). 10. **Wedge/motion:** `motion.agentResolve` on Approve/Reject. 11. **DoD + switch:** a deploy gate is never silently lost (durable, surfaced in inbox); approval lives where the team is, not a separate ops console (F-AGT-1 🔪).

## C-9 — Agent-surfaced triage view
1. **Jobs:** E11/M9 plane (curated queue), E3. Flow **F-AGT-1**. 2. **IA:** `CI → <run> → triage`; content. 3. **Components:** plan card (R-14 §2), chip/unfurl, views.
4. **Density:** 0.5. 5. **Agent (R-14 §6.4):** failure formatted for agent/human triage; **agent's proposed fix = a plan (plan-then-apply)**, diagram-to-effect mapping; suggest-not-auto. 6. **Sovereignty:** agent scope/budget visible (R-15 §3).
7. **State set (R-21 §2c):** agent-pending/working; agent-error-mid-chain (saga — completed steps stand, R-04 §7.2); budget-exceeded ("Triage paused — budget reached"); loop-guard-tripped. 8. **A11y:** plan effects accessible name; never colour-alone. **G2:** humanised. 9. **Device:** read on mobile; act via inbox.
10. **Wedge/motion:** **W6 (one `correlation_id` across surfaces)** — the triage chain (CI→issue→chat→PR) reads as one story; reserved `motion.agentEnter` on proposal. 11. **DoD + switch:** every agent action attributed + audit-linked (R-15 §1); the proposed fix is shown *before* it happens (D6); a human inherits a partial-but-coherent state on agent failure, never a corrupt one.

## C-1 / C-4 / C-7 / C-8 — Run list · matrix · secrets · usage/billing
- **C-1 Run list / dashboard** (E10 "is main green?"): `CI → <pipeline|repo runs>`, filterable (branch/status/actor/trigger), live. Density 0.6. State (R-21 §2c): empty-filtered vs empty-first-use distinct; live status `motion.liveUpdate`. Status glyph+label (G1). Device: read-first, key triage surface on mobile.
- **C-4 Matrix view** (fan-out grid): `CI → <run> → Matrix`; partial-failure highlighting (glyph+label, not colour-alone). Density 0.8. **Desktop-mainly (MOB-6)** — wide grid; read-only/scroll on mobile.
- **C-7 Secrets management** (E12, P12): `CI → [A] Secrets`; scoped, audited. Density 0.4. Never displays secret values (masked by construction). State: permission-denied graceful. Desktop-mainly admin.
- **C-8 Usage / quota / billing** (E10/G1, P14): `CI → [A] Usage`; minutes/credits by repo/runner class. Density 0.45; charts (D5 charting language, §3.7). Locale-aware numbers (G2). Read-forward.

## C-5 — Pipeline / definition editor + validator *(admin; flow-orphaned — job-linked)*
1. **Job link ([README §4.2](./README.md#42)):** **E8** (P3/P15 — declare "when X do Y" as a first-class, *observable, validated* trigger; stop maintaining webhook glue that breaks silently). The **schema-validator is the anti-YAML-sprawl differentiator** (R-01/competitive-landscape §2). Used when a platform engineer authors/edits the paved-road pipeline.
2. **IA + shell:** `CI → <…> → Pipeline editor`; one layer down. 3. **Components:** block editor / code editor (R-10 §3), overlays (validation popover).
4. **Density:** 0.5. 5. **Agent:** n/a (config-as-code; agents *consume* the pipeline). 6. **Sovereignty:** runner-class region binding (E10 in-region compute).
7. **State set (R-21 §2c):** **inline schema validation errors** (the differentiator — typed/validated config); save-conflict; permission. 8. **A11y/i18n:** code-editor a11y (R-10 §3.5 contenteditable model); validation errors as text + ARIA; **G2** humanised validation messages (no raw schema-key errors leaking, R-18 §6). 9. **Device:** **desktop-mainly (MOB-6)** — config authoring; read-only on mobile.
10. **Motion:** none decorative; `motion.settle` on valid-save, error reverts visibly (OPT-1). 11. **DoD + switch:** schema-validated config catches errors *before* a broken run (anti-YAML-sprawl); a team adopts config-as-code without the GitLab/Jenkins YAML-debugging-in-prod wall (R-02 D10 switch test).

---

**Group invariants reminder:** a log-line, a step, a run are all `ArtifactRef`s rendered through the
*one* chip (R-09); the CI panel inside a PR (G-9) is the *same* status vocabulary as a CI unfurl in
chat (R-02 R-COLOUR-2, "red means trouble identically everywhere"). Earned density (run/log `0.8`)
never breaks the eight invariants.
