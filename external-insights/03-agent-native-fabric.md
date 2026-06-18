# The Agent-Native Fabric

Agents as first-class actors across every subsystem, with event propagation and triggers as
platform primitives. This is Myelin's core differentiator, and — importantly — it has been
shown to work end to end with **mock agents only**, which is exactly how Myelin should build
it. The thesis to internalise: **if the substrate is right, an agent needs almost no special
code** — it is an actor with `kind = agent` running through the same identity, gateway, and
event log as everyone else.

---

## 1. Two tiny strategy boundaries: the brain and the hands

Make the swap from mock to real agents a **single trait implementation**, by keeping the
abstraction surface minimal:

- **The brain — one method.** A provider trait with essentially `step(conversation) ->
  {use_tools | submit}`. The agent loop owns the conversation history; the provider is
  stateless. A **mock provider** replays a scripted queue of steps — deterministic, zero
  cost, used by both unit tests *and* a developer `--use-mock` runtime flag (the same path
  users hit). The real provider is the *only* place a model vendor is named, and it carries
  attribution fields (tenant, actor, run id, caused-by) so every call is traceable and
  metered.
- **The hands — one method.** A tool-execution trait with essentially `exec(command) ->
  result`, and **no host-execution path that bypasses it.** A real implementation runs the
  command inside the sandbox; a simulation implementation runs it in-process with an
  in-memory scratch space and a marker proving it went through the channel, not a host shell.
- Provide a **skeleton mode** (no model, no tools): authenticate, fetch the task, print a
  summary, exit — it verifies the whole gateway/identity path with zero model spend.
- The payoff, stated plainly: **build and prove the entire agent story on the mock brain
  first** (pick up work → act → caused-by chained → trace queryable), then *"replacing one
  trait implementation lights up the entire agent-first story."* The thing under test is the
  wiring and the sandbox, **not** model spend.

## 2. Four distinct primitives — don't collapse them

Keep these vocabulary items separate; each has a different author, lifetime, and contract:

- **Event** — a fact: "X happened." Every state change. Fire over the durable log (via the
  outbox).
- **Signal** — a curated, deduplicated, severity-ranked *subset* of events that actors should
  actually react to (errors, alerts, agent proposals). The trigger substrate. Don't make
  everything react to everything.
- **Automation rule** — a *reflex* the project owns: "when X, do Y." Stateless, per-event.
- **Trigger** — a *stateful promise* a person owns: "wait until condition C, then unblock
  this task." A small state machine (armed → resolved / stale / disarmed), fires once per
  arming.

A useful one-liner: *a trigger is a promise the system keeps for you; an automation rule is a
reflex the project has.*

## 3. One sandbox for CI **and** agents

CI steps and agent tool calls are the *same problem* — running untrusted code — so **build one
isolation primitive and harden it once.** A single job spec with a `kind` field (ci | agent)
feeds the same runner.

- Settled building block: a **userspace-kernel sandbox** (a gVisor-class runtime, or a
  microVM) — plain containers share the host kernel, and one kernel escape is a cross-tenant
  catastrophe. Keep the backend swappable behind a runtime-agnostic job spec.
- Defaults: **no host network (egress default-deny, allowlist opt-in), read-only root +
  tmpfs scratch, all capabilities dropped, no-new-privileges, seccomp, images pinned by
  digest** (reject a tag without a digest, fail-closed), whole-guest kill on teardown, cgroup
  limits including `pids.max` (fork-bomb ceiling) and zero swap.
- **Secrets by name only, resolved *inside* the boundary** per run, scoped to exactly this
  job's references, never baked into images and never handed to the agent runtime to forward.
- **Untrusted execution is a permanent target** (both CI and agents run arbitrary code by
  design); one escape is catastrophic. And a security property that has **not been drilled on
  a real kernel is a claim, not a fact** — the escape drill on a real host is the gate, and it
  is the single hard blocker before anything runs customer code.

## 4. Agents act through the same gateway as humans — no carve-out

An agent's write tools call the **same public endpoints a human uses**, carrying the run's
scoped token, so the existing authorization check runs unchanged.

- A `403`/`503` surfaces to the agent loop as an ordinary tool error — **never an escalation
  to a privileged path.** An agent can do nothing its identity is not permitted to do.
- Mint the per-run identity at dispatch, **unset any shared platform token in the child
  environment** so it can't leak in as the tool identity, and revoke on teardown even on
  crash (an idempotent cleanup hook).
- Reuse existing substrate for agent artifacts: an agent's execution **trace is just a
  document** in the knowledge subsystem (content-addressed, immutable) — reusing it saves an
  entire schema and projection.

## 5. Safety: approval, cost, loops, and storms

Four independent guardrails, none sufficient alone:

- **Human-in-the-loop approval lives in the tool layer.** A gated write tool whose name is in
  "requires approval" but not "approved" is **withheld — it returns an error and does not
  mutate.** Approval re-runs the step with the name added. The approval UI shows the pending
  action, its risk, and a live cost estimate. (Wire the approve→resume loop end to end — it's
  easy to ship the withhold logic and the card but forget the bridge between them.)
- **Cost pre-flight makes a runaway loop self-limiting.** Before a real-spend run, check a
  prepaid balance and any per-capability add-on; refuse to *start* a new run when exhausted
  (never interrupt one in flight). Meter one cost event per model call, keeping wholesale and
  markup separate so a pricing change never rewrites history. The principle: *a runaway agent
  spends down a wallet and stops — not a surprise infrastructure bill.* Put a universal
  reserve/settle gate in front of **every** kind of run (CI included), so "no balance → no
  execution" is uniformly true.
- **Loop prevention is structural** (see substrate §6): self-guard (skip the agent's own
  output), a reference gate (raw typed text must not be able to re-trigger — only a
  structured, picker-produced reference can), a **causal-depth ceiling**, and a shared-root
  tripwire.
- **Concurrency caps** bound a mention storm: a bounded worker pool drops over-cap dispatches
  rather than forking unboundedly.

## 6. Orchestrator gotchas worth knowing before you hit them

The reactive automation tier (the component that consumes events and dispatches
agents/automations) is a stateful exception — give it an explicit design, and avoid these
specific, expensive traps:

- **Over-broad subscription head-of-line-blocks everything.** A single durable consumer
  subscribed to *all* events, most of which it doesn't handle, can accumulate **tens of
  millions of unprocessed messages** behind the unhandled types and silently stall every
  real-time agent feature. **Whitelist the subjects you actually handle**, and monitor
  consumer lag (pending count) so this can never recur silently.
- **A durable consumer's start policy is immutable.** Re-asserting a consumer's start
  position on every reconnect can wedge the broker and stop delivering *all* events. **Bind to
  an existing durable consumer by name; never re-declare its start policy on reconnect.**
- **Acknowledge only after the work is enqueued** (at-least-once to subscribers). Terminate
  non-retryable messages (malformed bytes) immediately rather than burning the redelivery
  budget on them.
- Carry **causality through the dispatch path** so an agent action triggered by an event is
  attributable to the original human action — and thread it *nested*, not flat, or the "why"
  chain collapses to a single hop.

## 7. Product judgment the platform can't make for you

Some agent behaviours are **product/cost decisions, not engineering ones** — surface them for
a human rather than defaulting them:

- *Should a casual mention auto-spawn an autonomous, potentially costly run?* Getting it
  wrong is a real cost and UX regression. A reasonable path is to ship the **explicit** "run
  an agent here" action first and treat implicit auto-dispatch as a deliberate, separately
  decided feature with intent/cost detection.
- Keep these as **plan-and-sign-off** items (process doctrine §8), not 3am autonomous builds.
