# Sketch 09 — The wire contract + cross-language harness parity (if the gateway diverges)

> Exploration note. Substrate §13 Q1 hands Chat an explicit obligation: *"a subsystem that diverges from
> Rust (the chat connection tier, TE-21) needs an equivalent of `serve(AppSpec)` in its language that
> enforces the same non-negotiables. What is the minimum the wire contract + a thin per-language shim must
> provide?"* This sketch answers it, so the Sketch-01 divergence option (BEAM/Phoenix) is *costed honestly*.

---

## Why this sketch exists

Sketch 01 leans **Rust gateway** but keeps **BEAM/Phoenix as the written, open divergence**. The prompt's
standing rule (and ADR-02 §consequences) is unambiguous: *if you diverge it still speaks the Rust envelope
on the wire and implements `PersonalDataHolder`.* So the divergence is only admissible if the wire
contract + the per-language shim are *fully specifiable* — and the **size of that shim is itself a major
input to the Rust-vs-BEAM call** (a big shim is an argument for staying Rust). This sketch makes the cost
concrete.

**Scope clarity:** even if the *gateway process* is BEAM, the **Message Service, Unfurl Service,
Read-state, and the durable stores are Rust** (Phase-2 Chat §3 — only the connection tier is the
candidate divergence). So the shim covers the *gateway*, the thinnest, most-stateless tier, not the whole
subsystem. The gateway's job is: hold sockets, authenticate the handshake, route frames over the NATS
backplane, resync from the durable log. Most of its *correctness-critical* interactions (persist, emit,
authorize, unfurl) are **calls into the Rust services over the internal RPC** — which already speak the
contracts. That bounds the shim.

---

## Part A — The wire contract the gateway MUST speak (Rust-defined, language-agnostic)

These are consumed as **wire shapes (protobuf/JSON over the internal RPC), not linked crates** (ADR-02 §;
substrate §2 convention; X-5 names-and-units reconciliation):

| Contract | Shape | Source of truth |
|---|---|---|
| **`EventEnvelope`** | the canonical versioned envelope (event_id ULID, type, schema_ver, tenant, region, actor{principal,kind,on_behalf_of,session,run}, subject ArtifactRef, correlation/causation/depth, contains_personal_data/data_role/visibility/pii_key_ref, occurred/recorded_at, payload) | Bus §3.1 / substrate §2.10 — the canonical field list; **units pinned** (timestamps RFC-3339 UTC; budgets integer minor-units; TTLs/timers seconds) |
| **`ArtifactRef`** | `myelin://<tenant>/<subsystem>/<type>/<id>[#sub]`; the gateway never *parses* scope, it carries the string and calls Refs to resolve | Refs §3.1 / Bus §6.2 token table |
| **`OutboxTx::emit` semantics** | the gateway **does not emit directly** — it calls the Rust Message Service, which does the outbox-in-same-transaction emit. The gateway has **no publish path of its own** (BUS-2: outbox is the ONLY emit path; the gateway must not regress to fire-and-forget) | Bus §5.2 |
| **`authenticate` / `check` / `list_objects`** | the handshake calls Id `authenticate(credential) → Principal`; per-frame authorization is the Rust services' job (the gateway trusts the injected identity headers for *identity*, re-authorizes nothing it isn't responsible for) | Id §4/§8; substrate §4.1 |
| **Firehose `publish`/`tail`** | NATS-core subject frames for fan-out + presence (Sketch 01) | Bus §4.3 |
| **`resolve(ref, viewer, mode)`** | unfurl resolution is a call into Refs (or the Rust Unfurl Service in front of it) | Refs §4.2 |

**The gateway is mostly a frame router + a caller of Rust services**, so the wire contract it must
*originate* is small (the envelope it forwards, the `ArtifactRef`s it carries, the resync cursors). It
*consumes* the rest by RPC.

---

## Part B — The cross-language harness shim (the `serve(AppSpec)` equivalent)

If BEAM, the gateway needs an Elixir equivalent of the substrate harness enforcing the **same
non-negotiables** (substrate §3/§4/§13 Q1). The minimum the shim MUST provide:

1. **Three-surface topology** (substrate §4): public (the WS/SSE endpoint, gateway-fronted,
   identity-injected — the *tenant comes from the verified token, never the path*, ID-3) / internal RPC
   (to the Rust services, inside the trust boundary, mTLS/signed-internal-credential) / metrics-health.
2. **Liveness ≠ readiness** (substrate §4.3): liveness must not check deps; readiness gates on the NATS
   backplane + the Rust-service reachability + (if it opens any store) the DB. A dead critical dep →
   not-ready → shed new connections; liveness does not restart-storm.
3. **No fire-and-forget emit** (BUS-2): the shim exposes **no** bus-publish; all durable emit is via the
   Rust Message Service's outbox. (If the gateway holds *any* durable store — it largely shouldn't — that
   store needs an outbox too. Lean: the gateway is **stateless**, holding only live connection state +
   ephemeral resync cursors, so it owns *no* outbox. This is a strong argument for keeping the gateway
   stateless regardless of language.)
4. **`PersonalDataHolder`** (prompt requirement; ADR-12): the gateway holds **connection state +
   in-memory presence + resync cursors**, which can include a `principal_id` (pseudonymous) and last-read
   cursors (personal data). It must implement `locate/export/restrict/erase` over that ephemeral state —
   *small surface* because the durable PII lives in the Rust stores (Sketch 05). Mostly: on `erase(P)`,
   drop P's live connections + presence + cursors (ephemeral; TTL'd anyway). Auto-registration is the
   substrate's job; the shim registers the gateway's ephemeral holder.
5. **The resilient-client posture + `Retry-After` honouring** (substrate §6; ADR-16): the gateway's RPC
   calls to Rust services use timeout/breaker/bulkhead and **honour `Retry-After`** (else shedding becomes
   a retry storm — the protected-human-lane defeat). This must be re-implemented in Elixir (BEAM has good
   libraries, but it is owed).
6. **The telemetry signal set** (substrate §10.2; X-1): the gateway exports the **Phase-5 drill survival
   signals** — connection count, per-tenant in-flight, shed counts per lane, NATS-subject lag, resync-gap
   size, breaker state. The drills *assert against these*, so omitting them fails X-1.
7. **The protected-human-lane shed order** (substrate §7; ADR-16): the gateway is *the* edge where the
   shed lane applies (a human connection in the protected lane; an agent/CI connection in the shed-able
   lane, getting `429 + Retry-After`). This is **the most load-bearing reason the gateway can't be a dumb
   proxy** — and re-implementing weighted-fair shedding in Elixir is real work (though BEAM's scheduler
   *helps*, it doesn't give the per-tenant fairness for free).
8. **Forward-only online migrations** (substrate §9; STOR-2): only if the gateway owns a schema (lean: it
   doesn't — stateless). If it does, expand→backfill→contract, forward-only.

### The honest cost verdict (feeds the Sketch-01 call)

The shim is **bounded but non-trivial**: items 2, 5, 6, 7 are the substantive re-implementations
(liveness/readiness, resilient-client + Retry-After, the telemetry survival signals, the protected-human-
lane shed order). Items 1, 3, 4, 8 are small *because the gateway is stateless and emits via the Rust
services*. **This shim cost is a real argument in Sketch 01's lean toward Rust** — in Rust, items 2/5/6/7
come from the substrate harness *for free*; in BEAM they are owed and must each pass their Phase-5 drill.
Phoenix gives us Decisions 2+3 (PubSub+Presence) in exchange; the trade is "free fan-out/presence" vs
"owed substrate shim." Sketch 01 leans Rust partly *because of this sketch* — the shim is enough work that
the BEAM presence/PubSub win doesn't clearly clear it, and getting items 5/6/7 subtly wrong is a
correctness/availability risk the drills would have to catch in a second runtime.

---

## What this sketch hands forward

- **The wire contract the gateway must speak is small and Rust-defined** (envelope, `ArtifactRef`, resync
  cursors it originates; everything else consumed by RPC into Rust services) — so a divergence *is*
  admissible.
- **The cross-language shim minimum is specified** (three surfaces, liveness≠readiness, no-fire-and-forget,
  `PersonalDataHolder` over ephemeral state, resilient-client + `Retry-After`, the telemetry survival
  signals, the protected-human-lane shed order, forward-only migrations) — items 2/5/6/7 are the
  substantive owed re-implementations.
- **Keeping the gateway stateless** (no durable store, no outbox of its own) shrinks the shim to its
  minimum and is decided regardless of language.
- **This cost is an input to Sketch 01's Rust lean:** Rust gets the shim for free; BEAM must earn the
  presence/PubSub win against the owed-shim cost + the second-runtime drill burden. The divergence stays
  *written and open* but *disfavoured*, exactly as ADR-02 honesty requires.
