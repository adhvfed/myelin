# Sketch 01 — The connection tier: transport, language, and backplane (TE-21)

> Exploration note. Weighs candidate approaches for **the single biggest Chat decision** (Phase-2
> Chat §9.1; the Phase-3 handoff names this "the most likely Rust divergence (TE-21)"). This is a
> *sketch*: I enumerate, weigh, and lean; the commit is in `00-findings.md`, the binding write-up is
> the architecture stage's. Prior art is cited inline.

---

## The problem, precisely

The connection tier holds **millions of concurrent, long-lived, soft-real-time client connections**
(WebSocket primary; SSE/long-poll fallback for restrictive networks/CLI `chat tail`). When a message
is posted to a channel with K online members spread across many gateway nodes, it must reach **exactly
those K connections, fast**, across a node-to-node backplane. The tier is also where the worst load
manifests: connection storms (a deploy reconnect-thundering-herd), mega-channels (a 100k-member
announcement), and agent-generated fan-out (EI-03 §5.4). Phase-1 §5.1 calls this "one of the hardest
systems to scale."

Three sub-decisions, separable:
1. **Language/runtime** of the gateway process (the Rust-vs-BEAM divergence, TE-21).
2. **The node-to-node backplane** that routes a posted message to the gateways holding the recipients.
3. **The presence/typing/read-state ephemeral path** (firehose-class; must never touch the durable bus).

The platform constraint that dominates all three (Phase-3 handoff; ADR-02 §consequences): **whatever the
gateway is written in, it must (a) speak the Rust-defined `EventEnvelope` over the wire, (b) implement
`PersonalDataHolder` for any personal data it holds, and (c) stay EU-deployable/self-hostable.** A
divergence is a *wire-contract* relationship, not a linked-crate one.

---

## Decision 1 — Gateway language/runtime

### Candidate A — Rust (tokio + an actor/connection crate), the platform default

- **For.** ADR-02 Rust default; no GC pauses and low, predictable memory-per-connection are a *real*
  edge at millions of connections (Phase-2 Chat §3; the canonical reason the Rust steer exists). The
  hot message/unfurl/read-state services are already Rust — one language, one build, one set of glue
  crates linked directly (not over the wire). Mature ecosystem: `tokio-tungstenite` (WebSocket),
  `axum`/`tower` (HTTP+WS upgrade, SSE), `tokio` tasks-as-lightweight-actors. The substrate harness
  (`serve(AppSpec)`, the resilient client, fail-static, the telemetry signal set, the consumer
  template) is *native* — no cross-language shim to build (substrate §13 Q1 is answered "no shim
  needed").
- **Against.** Rust has no built-in supervision/soft-real-time fabric the way BEAM does; we hand-build
  connection supervision, backpressure, graceful drain, and the per-node subscription registry. tokio's
  cooperative scheduler can suffer tail-latency under a CPU-bound task on a shared runtime (mitigated by
  isolating the gateway's CPU work). The "one slow connection stalls others" failure is on us to prevent
  (per-connection bounded queues, drop-and-resync — Phase-2 §2).
- **Prior art.** Discord's switch *from* Go *to* Rust for read-states/and latency-sensitive services
  (Discord eng blog, 2020) — GC pauses were the named enemy, exactly our concern. Cloudflare's
  Rust-based edge proxies (millions of connections). `tokio` design docs (work-stealing scheduler).

### Candidate B — BEAM / Elixir + Phoenix Channels (the justified divergence, TE-21)

- **For.** Phoenix Channels is *best-in-class* for exactly this workload: the BEAM scheduler gives
  per-connection lightweight processes with preemptive scheduling (no head-of-line blocking from one
  busy connection — the property Rust must hand-build), built-in supervision trees (a crashed
  connection process is isolated and restarted), and **Phoenix.PubSub + Presence** which solve
  Decision 2 and Decision 3 *out of the box*. The famous public result: the Phoenix team demonstrated
  **2 million concurrent WebSocket connections on a single box** (Phoenix "2M connections" benchmark,
  2015) and the WhatsApp/Ejabberd lineage proves Erlang at messaging scale. Presence (a CRDT-based
  distributed presence tracker, Phoenix.Presence) is the single hardest ephemeral problem (Phase-1 §2.6
  "O(N×M) fan-out nightmare") and BEAM ships a proven answer.
- **Against.** A *second language and runtime* in the platform (ADR-02 divergence cost). It cannot link
  the Rust glue crates — it must consume the envelope/`ArtifactRef`/authz contracts **over the wire**,
  and the substrate's `serve(AppSpec)` non-negotiables (outbox, three ports, liveness/readiness,
  forward-only migrations, the telemetry signal set, `FailStatic`, `Retry-After` honouring) must be
  **re-implemented as a thin Elixir shim** (substrate §13 Q1 — *this becomes Chat's owed deliverable*).
  BEAM's per-process memory is heavier than a Rust task (~hundreds of bytes/process vs a tokio task), but
  the *scheduling* model is the win, not raw memory. Operationally: a second observability/release/build
  story per cell.
- **Prior art.** Phoenix Channels + Phoenix.Presence (Chris McCord et al.); the 2M-connection benchmark;
  Discord *kept* Elixir for its real-time gateway/guilds fan-out for years (Discord eng, "How Discord
  Scaled Elixir to 5M Concurrent Users", 2017) even while moving hot data services to Rust — a precedent
  for **exactly the split this sketch leans toward**.

### Candidate C — Go (gobwas/ws, nbio) or a managed/off-the-shelf realtime service

- **Go**: strong WS ecosystem, goroutine-per-connection is ergonomic, but GC pauses are the named enemy
  (the Discord lesson) and it is a third language with no platform presence. **Rejected** — buys nothing
  Rust or BEAM don't, adds a language.
- **Managed realtime (Ably/Pusher/PubNub)**: **rejected outright** — non-EU-sovereign, not self-hostable,
  violates VISION §1. Not admissible.

### Leaning (Decision 1)

**Lean: Rust gateway by default, with BEAM/Phoenix held as the *justified, written* divergence the
architecture stage may exercise — and the decision gated on one thing: whether we are willing to own a
hand-built distributed presence tracker.** The Discord precedent (Rust for hot data, Elixir for the
real-time fan-out gateway) is the strongest external signal that *this specific split* is sound. The
honest read:

- If we adopt BEAM, we get Phoenix.PubSub (Decision 2) and Phoenix.Presence (Decision 3) essentially
  for free, at the cost of a second runtime + the substrate shim. The shim is bounded and one-time.
- If we stay Rust, we keep one runtime and link the glue crates natively, at the cost of **building the
  subscription registry + a distributed presence tracker ourselves** — which is real work but is *also*
  work that benefits from the platform's existing NATS-in-cell deployment (Bus §2.1).

**My lean for the commit: Rust, with the divergence kept open-but-disfavoured.** Reasoning: (1) the
substrate non-negotiables are a *lot* to re-implement in Elixir and getting them subtly wrong (the
outbox-only emit, `Retry-After` honouring, the telemetry survival signals the Phase-5 drills read) is a
correctness risk; (2) the platform already runs **NATS JetStream in every cell** (Bus §2.1) and **NATS
core** is the firehose transport seam (Bus §4.3) — NATS core gives us a proven, EU-sovereign,
self-hostable pub/sub backplane *and* a presence-suitable ephemeral channel without a new runtime; (3)
"one language, glue crates linked not wired" is a strong coherence/correctness win that the BEAM
divergence forfeits. The BEAM option stays **written and not foreclosed** (ADR-02 honesty) precisely
because if presence-at-scale or scheduler tail-latency proves intractable in Rust during the build, the
escape hatch is real and the wire contract makes it a gateway-process swap, not a platform rewrite.

This honours the prompt's instruction: *make the call in writing; if you diverge it still speaks the
Rust envelope on the wire and implements PersonalDataHolder.* The call is **Rust-default, BEAM-escape-
hatch**, and the wire-contract discipline is specified either way (Sketch 09).

---

## Decision 2 — The node-to-node fan-out backplane

The gateway is horizontally sharded; a connection can land on any node (Phase-2 Chat §2). A posted
message must reach every node holding a subscribed connection. Candidates:

| Candidate | Mechanism | Verdict |
|---|---|---|
| **NATS core (in-cell, the firehose transport)** | Subject-per-channel (`fan.<tenant>.<channel>`); gateways subscribe to the subjects of channels they hold a connection for; the fan-out tier `publish`es the rendered message frame to the subject; NATS routes to exactly the subscribed gateway nodes. | **CHOSEN lean.** Reuses the Bus's already-deployed NATS (Bus §4.3 firehose seam — "presence frames ride NATS core"); EU-sovereign, self-hostable, one-binary; subject-based routing is the literal mechanism we need; non-durable at-most-once is *correct* here (the durable log is the source of truth, this is just live delivery — a missed frame is recovered by resync-on-reconnect, Decision below). |
| **Redis pub/sub cluster** | `SUBSCRIBE channel:*`; publish to channel. | Viable, same-class; **but** adds a Redis deployment whose pub/sub is fire-and-forget with weaker delivery semantics than NATS, and Redis is already constrained to "never a source of truth" (STOR-3) — using it for fan-out is fine but it's a *second* ephemeral transport beside NATS. Rejected for operational-minimalism (EI-02 §8) when NATS is already in-cell. |
| **Channel-sharded actor model (consistent-hash a channel to an owning node)** | Each channel has one "home" node that owns its fan-out; gateways forward to the home node. | The Phoenix/Discord guild model. **Strong** if we go BEAM (it *is* the BEAM model). In Rust it's a build-it-ourselves consistent-hash ring + rebalancing on node churn — more moving parts than NATS-subject routing for the same outcome. Kept as the **escape hatch for mega-channels** (a 100k-member announcement channel may want a dedicated home-node fan-out rather than 100k NATS subject-subscribers — see Sketch 04). |

**Leaning:** **NATS-core subject-per-channel fan-out**, reusing the cell's existing NATS; the
channel-sharded-home-node model is the named escalation for *measured* mega-channels (Sketch 04). This
mirrors the platform's pattern everywhere: a default that reuses existing infra + a measured-trigger
escalation.

---

## Decision 3 — Resync-on-reconnect (the correctness backbone)

The fan-out backplane is **at-most-once / non-durable by design** (it is ephemeral live delivery). The
correctness guarantee — *per-conversation total order, no lost messages* (Phase-1 §5.7) — comes **not**
from the backplane but from the **durable per-conversation log + a resume cursor**:

- Every connection tracks "the last message id I have for each subscribed conversation." On
  (re)connect, the client sends its cursors; the gateway streams the **gap from the durable message log**
  (Sketch 02) before resuming live delivery. A frame missed during a NATS hiccup is recovered by the next
  resync — so the ephemeral backplane *is allowed to drop*, which is what makes it cheap.
- This is the **same resume-cursor discipline** EI-04 §2.2 mandates for the collab transport ("a
  real-time relay *without* resume cursors is a floor that will silently lose the gap on a reconnect") —
  applied to chat delivery. We adopt it as a **first-class correctness property**, not an afterthought,
  and it gets a quantified drill (Sketch 08 / `00-findings`): *zero messages lost across a reconnect*.

This decision is what lets Decision 2 be a non-durable transport without violating Phase-1 §5.7.

---

## What this sketch commits to handing forward

- **Gateway language: Rust default; BEAM/Phoenix is the written, open-but-disfavoured divergence**
  (TE-21), gated on presence-at-scale and scheduler tail-latency proving tractable in Rust during build.
- **Backplane: NATS-core subject-per-channel**, reusing the in-cell NATS; channel-sharded home-node is
  the measured escalation for mega-channels.
- **Correctness backbone: resume-cursor resync from the durable log** — the ephemeral backplane may drop;
  the cursor recovers. Owed drill: zero-loss-across-reconnect.
- **Either way: the gateway speaks the Rust `EventEnvelope` on the wire and implements
  `PersonalDataHolder`** (Sketch 09 specifies the wire/shim contract).
