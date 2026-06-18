# Process & Quality Doctrine

How to build a system of this size without it rotting under you. Each item is a default;
deviate only with a written reason.

---

## 1. The code wins over the docs

Plans and specifications **lag reality** the moment implementation starts. Treat the
running code and its observable behaviour as the source of truth; treat planning documents
as intent that must be re-verified against the code.

- When a doc and the code disagree, **the code wins** — fix the doc, then proceed.
- Schedule periodic **truth-up passes** whose only job is to re-sync the docs to what the
  code actually does. Without them, the docs drift far enough that agents act on fiction.
- **Failure mode if ignored:** an agent reads a stale capability note ("X is a stub") and
  builds around a limitation that no longer exists, or skips work it thinks is done. Stale
  capability docs *actively mislead* the next worker. Date your status docs; never let a
  claim outlive its verification.

## 2. Order work by non-negotiability, not by size

Sequence the roadmap by **what kills you first**, not by architectural layer or convenience.

- **Stop-the-bleeding first:** silent data loss and remote code execution outrank every
  feature. A platform that loses data or can be breached has no features worth discussing.
- Then the keystones (the load-bearing capabilities everything else depends on), then
  breadth, then polish and scale.
- Enforce a **gate invariant:** no later phase may be claimed done while an earlier phase's
  gate is still red. The ordering is enforced, not aspirational.
- **Failure mode if ignored:** you build a beautiful feature surface on top of a substrate
  that silently corrupts, and discover it the day a real tenant loses real data.

## 3. Prove it or it isn't real

**A property does not exist until a test forces the failure and observability watches the
system survive it.** This is the heart of the discipline.

- Gates resolve to **quantified thresholds** (recovery-point/recovery-time objectives,
  "zero sandbox escapes", "zero messages lost across a reconnect", "a disabled user has zero
  working access paths within N minutes"). A target you cannot measure is not a gate.
- **Never weaken a threshold or invert an assertion to make a check pass.** A red gate is
  *information*. Record honest "needs human verification" items rather than softening a check
  into a green it didn't earn.
- **Observability is part of the pass condition.** A system that survives a drill but emits
  no signal that it survived has *failed* the drill — you cannot operate what you cannot see.
- Build the failure-injection harness early: a load generator that can multiply traffic
  (1×/10×/30×) and mix principal types, a scoped reversible way to break a dependency, and
  assertions read from the production telemetry. Cheap drills run in CI on every change;
  expensive ones run scheduled. Every real incident ends by adding a drill that reproduces it.
- **Failure mode if ignored:** "looks done" gets marked done; the first forced failure in
  production is the first time anyone learns the property was never real.

## 4. Actually try it — exercise the real thing before claiming it

Automated tests prove the parts; they routinely miss what only appears when a real user (or
agent) drives the whole thing end to end.

- For any user-facing surface, **drive the real UI in a browser** before claiming it works.
  The "switch test": a surface is done only when someone could move to it without hitting a
  wall the old tool didn't have — and that verdict is reached by *driving it*, not by reading
  the feature list.
- Integration tests typically use a fresh database, call a single handler, and render once
  with final state. **Real sessions chain mutations and update state mid-flight** — which is
  exactly where the bugs live. Write end-to-end tests that *chain* operations, not just
  exercise handlers in isolation.
- **Untested is acceptable if you name it untested.** Every piece of work should honestly
  record whether it was exercised (yes / no / partial). Silent skipping is the failure mode.
- **Failure mode if ignored:** a feature passes every unit test and is unusable the first
  time a human opens it — a modal that renders in the wrong place, a control unreachable on a
  phone, a picker that opens off-screen.

## 5. The ratchet — turn discipline into committed, loud gates

Every quality habit that lives only in an agent's good intentions will eventually be skipped.

- Convert each assumed discipline into a **committed, mechanical gate**: CI jobs,
  pre-commit hooks, and small custom scanners built from the *fingerprint of a recurring
  failure*. When the same class of bug recurs a few times, write the check that makes it
  impossible, and commit it.
- **An uncommitted gate is no gate.** A linter config or CI workflow that exists on disk but
  was never wired in lets drift accumulate quietly while everyone assumes it's covered.
- Make violations **loud, never silently swallowed.** Replace `... || true` and silent
  filters with explicit, noisy failures. A contract violation you drop silently is a
  multi-day misdiagnosis waiting to happen.
- **Failure mode if ignored:** formatting/lint/contract drift piles up across hundreds of
  files; a swallowed error sends three debugging sessions chasing the wrong cause.

## 6. Investigate before you build

The answer is often smaller than the question.

- Before writing a fix, **test the hypothesis** — introspect the database, replay the
  events, reproduce the symptom. The obvious cause is frequently wrong.
- Follow the chain to **root cause** (surface → API → data/architecture). Treat "it looks
  right but doesn't fire" as a signal to dig, not to patch.
- Not every signal warrants a dedicated work item; triage. But every fix needs a confirmed cause.
- **Failure mode if ignored:** you fix the wrong thing, ship it, and the real bug resurfaces
  wearing a different hat.

## 7. Keep the architecture coherent as it grows

Emergent systems drift toward incoherence unless coherence is actively maintained.

- **Abstract at the third copy.** The moment a pattern is about to be hand-rolled a third
  time, hoist it into one primitive. Earlier is premature; later is load-bearing duplication
  with divergent bugs in every copy.
- **Spawn a cleanup pass the moment a workaround threatens to go load-bearing.** The trigger
  test: "would building more on top of this gap make it harder to fix later?" The cost of a
  workaround compounds linearly with every consumer added on top of it.
- **Reconcile cross-component contracts at the plan layer, before either side ships.** Two
  components that will exchange data must agree on field names *and units* up front — a unit
  mismatch (e.g. a 100× scale difference) that ships on one side calcifies and is brutal to
  unwind later.
- **Failure mode if ignored:** the same overlay/menu/field is implemented five times with
  five subtly different behaviours, and a schema "fact" means two incompatible things in two
  services.

## 8. The human sign-off is the bottleneck — design around it

When most of the building is autonomous, **human approval, not agent capacity, is the scarce
resource.** Spend it well.

- Surfaces with **security, abuse, cost, or irreversible-scope** implications are
  *decision-shaped*: produce a sketch and pause for human sign-off rather than building
  autonomously. Tag each next step as "just build it" vs "needs a decision first".
- Don't churn a document while a human is reading it for sign-off.
- Let autonomous work that is genuinely safe proceed without gating it on a human; reserve
  the human for the calls only a human should make.
- **Failure mode if ignored:** either an agent autonomously ships a costly/irreversible
  decision that should have been a human call, or every trivial step waits on a human and
  throughput collapses.

---

### The compounding payoff

When the substrate and contracts are right, each new surface is *smaller* than the last,
because it is a projection of capabilities that already exist. Watch for the inverse signal:
if features keep getting **harder** to add, the substrate is wrong and no amount of feature
work will fix it — stop and repair the foundation.
