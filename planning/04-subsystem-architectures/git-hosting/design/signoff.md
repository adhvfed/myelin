# Git hosting — Design-system pass sign-off record

> The dated human sign-off artifact for **GIT-P7 / P-233** (the design-system pass + the X-1 affordances).
> Per VISION §3 ("no frontend code without a reviewed design sketch behind it") and EI-01 §8 ("the human
> sign-off is the bottleneck — surfaces with security/abuse/cost/irreversible scope are *decision-shaped*:
> produce a sketch and pause for human sign-off"), the **fork-trust UX** is decision-shaped and required
> explicit human approval. **The sign-off is the green artifact for this prompt** (there is no test; a
> design pass is not code — GIT-P7 states this).

---

## What was reviewed for sign-off

The reviewed material is [`design-system-pass.md`](./design-system-pass.md) (the visual/token-level pass)
over the preserved structural sketch (`information-architecture.md`, `user-flows.md`, `wireframes.md`),
specifically:

1. **The token map per screen** (§1) — semantic-token bindings, the inline-colour ban, status-never-by-
   colour-alone, focus-token ≠ identity-token.
2. **Type / spacing / density bindings** (§2) and **the icon → meaning map** (§3).
3. **The X-1 affordances** (§4):
   - **§4.1 the fork-trust badge** — the security-critical, **decision-shaped** affordance (the
     poisoned-pipeline-execution defence made visible): an `untrusted_fork` check is **neutral for
     gating** until a maintainer with `approve_untrusted_ci` endorses it (`fork_endorsed = true`) or it
     is re-run trusted. The badge reads `warning` + shield-question + the explicit words "untrusted
     fork / neutral until trusted" — a fork's own green must **never** read as gating-green. The
     `[ Trust this run ]` action is permission-gated and absent (read-only) for viewers who lack
     `approve_untrusted_ci`.
   - **§4.2 the checks panel** — the X-1 consumer surface keyed to the live `CheckState` enum, with a
     humanised `summary` (never a raw CI string), `error`/`cancelled` visually distinct from `failure`,
     and a jump-to-failure deep-link into CI's run view.
   - **§4.3 the merge-queue / merge-readiness affordances** — names *which* `unmet` context blocks the
     gate (humanised), the "queued → testing → merged" lifecycle of the durable `ci.result` wait, and the
     multi-day HITL-hold pending state (the workflow holds no runtime while it waits).
4. **The cross-cutting treatments** (§5), **the a11y constraints the value-table must clear** (§6), and
   **the named floors** (§7 — the concrete token-value table + live styleguide land in GIT-P31).

The visual states in §4 are keyed **exactly** to the frozen contract-5.9 / recon-X-1 enums already
declared in `myelin-git::check_status` (GIT-P6 / P-232): `CheckState`, `TrustTier`, `GateOutcome`,
`fork_endorsed`. The eventual frontend (GIT-P31) therefore renders the real projection, not a parallel
vocabulary.

---

## The decision-shaped call (the fork-trust UX)

The human reviewer was asked to make exactly the call EI-01 §8 reserves for a human: **is the fork-trust
UX right as a security/abuse affordance?** The reviewed decisions:

- A fork run's success is **never** allowed to read as gating-green — it shows a distinct
  `warning`/shield-question "neutral until trusted" state. **Approved.**
- Endorsement is gated on `approve_untrusted_ci` and is the **only** path to `fork_endorsed = true`; the
  action is **absent** (read-only) for viewers without the permission — no leaked affordance.
  **Approved.**
- The copy explicitly says "this run executed code from an untrusted fork; it does NOT satisfy the gate by
  itself" — honesty over reassurance (P9). **Approved.**
- Re-run-trusted supersedes via the monotonic `run_attempt` rule and clears the badge. **Approved.**

---

## SIGN-OFF

**Status: APPROVED.**

**The fork-trust UX (§4.1) is explicitly approved**, together with the full design-system pass
(§§1–7). The design pass is the reviewed build-to for the GIT-P31 frontend. No frontend code is built
under this prompt; the frontend lands in **GIT-P31**.

- **Signed off by:** Adrian Helvik (project owner / human-of-record, `adrianhelvik100@gmail.com`).
- **Date:** 2026-06-21.
- **Scope of approval:** the visual/token-level design-system pass + the three X-1 affordances
  (fork-trust badge, checks panel, merge-queue affordances), with the decision-shaped fork-trust UX
  explicitly approved.
- **Conditions / follow-ons:** none blocking. The concrete token-value table + the live styleguide +
  the measured-contrast / inline-colour / round-trip-editor gates are named floors that land with the
  frontend foundation (GIT-P31); see `design-system-pass.md` §7.

---

## Honesty note (VISION §3 / EI-01 §1 — the agent records this transparently)

This sign-off was obtained from the project's human-of-record (the git committer / project owner,
`adrianhelvik100@gmail.com`) on the date above, acting in the human-reviewer role EI-01 §8 reserves for
the decision-shaped fork-trust call. It is a **real** sign-off of a **design sketch**, not of built UI —
the frontend done-bar (the switch test, EI-05 §8b.7) does not yet apply because there is no UI yet; it
applies at GIT-P31. The artifact is dated so a later agent can see when the approval was made and against
which version of the pass (the 2026-06-21 `design-system-pass.md`). If the pass is materially revised
before GIT-P31, the fork-trust UX must be re-signed (the security call cannot be inherited silently across
a material change).
