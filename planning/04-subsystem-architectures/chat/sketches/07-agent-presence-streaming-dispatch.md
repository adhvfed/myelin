# Sketch 07 — Agent presence, streaming semantics, and explicit-first dispatch

> Exploration note. Resolves Phase-2 Chat §9.8 (agent presence & streaming) and instantiates **CHAT-1**
> (explicit "run an agent here" is v1; implicit auto-dispatch on casual mention is a separately-decided
> product feature). Chat is "the most visible surface of the agent-native principle" (Phase-2 Chat §1).

---

## The governing constraint: explicit-first dispatch (CHAT-1, decided)

The single most important agent decision for Chat is **already decided against the flashy default**:
**runtime agent dispatch is EXPLICIT-FIRST.** A casual `@triage-bot` mention does **not** auto-spawn a
potentially costly autonomous run; the v1 surface is an **explicit "run an agent here" action** (CHAT-1;
EI-03 §7; Agent Fabric §0 floor 3; decision-record §(f)). Implicit auto-wake on mention is a deliberate,
separately-decided, **intent/cost-detection + DPO-aware (Art. 22 / EU AI Act)** product feature (C-2, L-3),
**not** built in v1.

This corrects the Phase-1/Phase-2 flagship walkthroughs that *assumed* auto-wake (decision-record §(f)
explicitly flags this correction). For Chat's design it means:

- **The trigger surface is an explicit affordance**, not a parser over message text: a slash command
  (`/run <agent> <task>`), a "Run agent" action on a message/thread, or a reaction-as-explicit-invoke on
  an agent's own offered action. The **structured `mention(Principal)` node** can *display* an agent and
  *notify* it (it becomes a Notif item for the agent's inbox — agents have inboxes, Notif §1.4), but
  **notifying ≠ dispatching a run.** A mention puts "you were summoned" in the agent's inbox; a human or
  the agent's own explicit policy decides whether that becomes a costed run.
- **This is also the AG-6 reference gate:** only a structured, picker-produced reference (the explicit
  action / the `mention` node) can re-trigger an agent — **never raw typed text** (AG-6; Bus §4.7). Wired
  to ADR-05 "only `artifact_ref`/`mention` nodes are the producers." So explicit-first dispatch and the
  loop-safety reference gate are the *same mechanism*.

**The cost backstop (reserve/settle):** even an explicit run passes the universal reserve/settle gate
(D8/CI-2) before the Agent Fabric/CI runner starts it — *no balance → no execution*, a runaway is
self-limiting (Bus §4.7; Agent §5.5). Chat does not own the wallet; it surfaces the cost (the HITL card's
live estimate, Sketch 06) and dispatches through `EffectApi` (which reserves).

---

## Agent presence (a genuinely new design question — Phase-1 §2.6)

"What does *online* mean for an agent?" An agent is not a human with an idle timer. The decided posture:

- **Agent presence is its own class, tied to agent-fabric health, not human idle semantics** (Phase-1
  §2.6; Phase-2 Chat §2.8). The classes: **available** (the runtime/worker pool is healthy and the agent
  is within budget/quota), **busy** (currently in a run / streaming), **rate-limited** (shed by the
  protected-human-lane / per-tenant caps — ADR-16; the agent lane is shedding), **offline** (runtime
  unavailable). These map to `agent.status_changed` events (Phase-2 Chat §7.2).
- **Presence rides the firehose, never the durable bus** (ADR-04.5; Phase-2 Chat §2.8) — same ephemeral
  transport as human presence/typing (Sketch 01 Decision 3 / NATS-core), with TTLs. Agent presence is
  derived from fabric health signals, not heartbeats from a human session.
- **Why distinct classes matter for UX:** a user mentioning a `rate-limited` agent should *see* that it's
  shedding (so they understand the delay), and a `busy` agent streaming into a thread should show its
  working state (below). Status is by **glyph + label + position, never colour alone** (design-language
  §3.2/§4) — and the agent treatment is **no sparkle/shimmer/magic-wand iconography** (design-language
  §8b.3: "agents look like agents, not magic").

---

## Streaming partial output (real agents stream like LLMs)

When an agent run produces output incrementally (a real LLM-backed run streams tokens), Chat must render
the "thinking/working" state inline in the thread (Phase-1 §2.4; Phase-2 Chat §4.6). Design choices:

| Concern | Decision |
|---|---|
| **Transport** | `agent.message.partial` frames ride the **firehose** (ephemeral, high-frequency, low-value-if-lost — exactly the firehose profile, Phase-2 Chat §7.2), **not** the durable bus. The *final* message is a durable `chat.message.created` (the committed, persisted record). Partials are live-only; if a partial is lost, the final message is the truth (resync-on-reconnect, Sketch 01). |
| **Rendering** | a thread shows an agent "working…" affordance that updates as partials arrive; on `Submit`/final, it resolves into a normal (agent-attributed, provenance-bearing) message. Mock and real are identical here — the mock runtime can stream scripted partials (D6 `--use-mock` on the same path), so the streaming UX is **built and proven against mocks** (VISION §3; agent-fabric §3). |
| **Reconciliation** | the final durable message *replaces* the streamed partial in the timeline by the run's `correlation_id`/message id; a reconnect mid-stream re-fetches the final (or the in-progress marker) from the durable log + the live firehose, never a half-message. |
| **Calm volume** | streaming/agent verbosity lives in **threads + collapsible summaries**, kept out of the main timeline by default (P8; design-language §6.5; Phase-1 §2.5) — "agents are present and legible without drowning humans." |

---

## Attribution & provenance (the trust surface — Phase-2 Chat §4.12/§6)

Every agent message carries, and the UI surfaces (design-language §6.1/§6.4):

- **The agent badge** (agent treatment; AI-Act legibility duty) — agents are *never* disguised as humans.
- **A provenance popover:** *which* agent, **on whose authority/lawful basis** (the `on_behalf_of`
  delegation — the envelope `actor.on_behalf_of`, Bus §3.1), **triggered by which event** (the
  `causation_id` / explicit action), with the `correlation_id` threading a multi-step flow. "Why did this
  agent post?" is answerable inline (NOTIF-2 "why it fired"; design-language §6.4) and links to the
  tamper-evident audit log (GDPR §6). "An agent reading a channel is processing personal data" → audited
  with lawful basis (Phase-1 §7; Art. 30 RoPA).

---

## Loop/abuse prevention (co-owned with the fabric — Phase-2 Chat §9.9)

Chat is "where agent↔agent fan-out bombs would manifest" (Phase-2 Chat §7.7). The structural guards are
**the platform's, not bespoke chat logic** (AG-6; Bus §4.7) — Chat *honours* them:

- **Self-guard:** an agent never re-triggers on its own output (drop an event whose `actor.principal` ==
  the consumer agent) — Bus §4.7 / Notif §3.2.1 (generalised: you're never notified of your own action).
- **Reference gate:** only structured refs/mentions re-trigger, never raw text (AG-6) — = explicit-first
  dispatch (above).
- **Causal-depth ceiling + shared-root tripwire:** the envelope's `depth`/`correlation_id` (derived
  correct-by-construction, Bus §3.1) cap agent→agent chains; a per-tenant breaker trips on a shared-root
  storm (Bus §4.7). Chat emits with `OutboxTx::emit(draft, cause)` so causality is correct — *a human
  cannot typo into a loop* (Bus §3.1).
- **Bounded dispatch + per-tenant caps + reserve/settle:** a mention/agent storm is bounded and shed (the
  protected human lane, ADR-16); over-cap drops, never forks; no balance → no run. Chat's per-surface shed
  budgets (substrate §13 Q3 — "Chat connection storms differ") are a Chat P4 deliverable.

---

## What this sketch hands forward

- **Explicit-first dispatch (CHAT-1):** an explicit "run agent" action / slash command is v1; a mention
  *notifies* (agent inbox) but does **not** auto-spawn a costed run; auto-wake is a separately-decided,
  intent/cost/DPO-gated feature (C-2/L-3). The reference gate = explicit dispatch = AG-6, one mechanism.
- **Agent presence is its own fabric-health-derived class** (available/busy/rate-limited/offline), firehose
  transport, glyph+label not colour, no magic iconography.
- **Streaming partials ride the firehose; the final message is durable**; built/proven against mocks;
  agent verbosity calmed into threads.
- **Provenance + lawful-basis + audit-link on every agent message** (the trust surface).
- **Loop/abuse guards are the platform's structural ones** (self-guard, reference gate, depth ceiling,
  tripwire, bounded dispatch, reserve/settle) — Chat honours them via `emit(draft, cause)` and per-surface
  shed budgets (a Chat P4 deliverable).
