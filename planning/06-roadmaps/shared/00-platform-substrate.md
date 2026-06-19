# Phase 6 — Roadmap: Platform Substrate & Foundations (`myelin-substrate` + the glue crates)

> Phase: `06-roadmaps/shared`. The detailed sequenced roadmap for the **00-platform-substrate** shared system.
> Slots into the master sequencing bands M0..M6:
> [`../00-master-sequencing.md`](../00-master-sequencing.md) (§1 ordering thesis / Tier 0–6, §2 bands, §3
> critical-path/DAG, §4 the gate invariant, §5 name-your-floors). Frozen architecture (this roadmap
> SEQUENCES, it does not redesign):
> [`../../05-refined-shared-systems-architecture/00-platform-substrate.md`](../../05-refined-shared-systems-architecture/00-platform-substrate.md)
> (the refined substrate architecture) + the refined
> [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md)
> §1 (the contracts the substrate owns) + §2/§3 (the seams it carries). Drills owed:
> [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> §4.2 (SUB-D1..SUB-D10) + the substrate-half of SUB-D6/restore (with Storage) + the L1 lint ratchet. Doctrine:
> [`../../../external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> (§2 order-by-non-negotiability: silent-data-loss + RCE floors first; §3 prove-it-or-it-isn't-real + the
> failure-injection harness; §5 the committed ratchet; §1 name-your-floors) and
> [`../../../external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md) (§2 build the
> durable resume-cursor transport FIRST; §5 untrusted-code-execution). Spine: ADR-01 (glue crates that cannot
> drift), ADR-04 (bus), ADR-16 (backpressure), ADR-17 (fail-static), ADR-18 (restore-verify), ADR-20 (the
> unified runner). Date: 2026-06-19.
>
> **The shape of this system, and what that means for sequencing.** The substrate is **the root of the
> dependency DAG**: every other shared system and every subsystem is a `main.rs` that calls
> `myelin_substrate::serve(AppSpec)`. It is the layer the doctrine calls "cheap to get right at the start,
> brutal to retrofit" (EI-02 preamble). Two consequences for the roadmap, and they are the inverse of a
> consumer system like Search: (1) the substrate **builds first and almost entirely in M0** — it cannot wait
> for upstreams because it *has* no upstreams (its only "dependency" is the failure-injection harness, which it
> also builds, so the harness can drill it). (2) Its correctness properties are not features layered on later;
> they are the **floors the order-by-non-negotiability thesis front-loads** — the transactional outbox (silent
> data loss), the twelve committed lints (the ratchet), and the failure-injection harness itself (Tier 0, the
> unit of proof). The substrate therefore does not have a "first useful" that arrives late; its first useful is
> the moment another service can boot from it. What it *does* carry across later bands is a short, named set of
> seams whose owning property cannot be proven until the systems above exist: fail-static (proven against a real
> Identity in M1), restore-verify cross-seam integrity (proven with Storage in M1), the firehose backpressure
> half (proven with Bus/KN/Chat collab streams in M2/M4), and the 30× surge family (proven at scale in M5).

---

## 0. Where the substrate lands in the master bands (the one-paragraph map)

The substrate's **core build is M0** — it *is* M0's substrate half: the Cargo workspace + the eight glue
crates, `serve(AppSpec)`, the transactional outbox + idempotent-consumer template, the failure-injection
harness, the twelve committed lints (red+green fixtures), the contract-coverage scanner, the shared
overlay/state primitives, and the versioned thresholds file (master §2 M0). Three of the substrate's properties
are then **completed-and-proven in later bands because their proof needs a system that does not yet exist in
M0**: **fail-static** (the mechanism ships M0, but SUB-D4 is proven against a real Identity hiccup in **M1**),
**restore + cross-seam integrity** (the substrate owns the harness half of SUB-D6; the full restore-verify gate
is proven with Storage in **M1**), and the **firehose resume-cursor backpressure discipline** (the bounded /
shed half ships M0 as part of bounded-everything, but is exercised against real hot streams in **M2** Bus and
re-confirmed under Chat/KN load in **M4**). The **30× surge family** (SUB-D3) and **online-migration-under-load**
(SUB-D10) are scheduled-frequency drills that prove at **M5** world scale. The substrate also participates in
the **M5 E2E wedge** (every scenario boots on `serve`, emits via the outbox, reads the telemetry signal set) and
the **M6 dogfood** (the lints and the harness run as Myelin CI jobs on Myelin's own commits).

The honest progression: **first runnable** = M0 (a hello-world service boots from `serve`, emits one event
through the outbox, a consumer dedups it, the metrics port answers, the harness injects one fault and reads one
telemetry assertion); **first useful** = end of M0 (another shared system — Identity — can be built *on* the
substrate because the outbox, the lints, the harness, and the contract-coverage scanner are green: SUB-D1 /
SUB-D2 / BUS-D4 + all twelve lints); **production-hardened** = M5 (the 30× surge holds the protected human lane,
online-migration-under-load holds its lock budget, restore-verify holds at cell scale, and the firehose
backpressure half holds under real connection-storm load).

---

## 1. The contracts the substrate owns / carries, mapped to the milestone they land in

From contract-index §1 (the substrate's own cluster) + the substrate-relevant seams in §2/§3 it carries. "Lands"
= the milestone by which the contract must be implemented or callable. The substrate owns the largest single
contract cluster in the index because it is the floor every other cluster stands on.

### 1.1 Owned by the substrate (contract-index §1 — the bootstrap & service shell)

| # | Contract | Lands | Notes / floor |
|---|---|---|---|
| 1.1 | `serve(AppSpec)` — boot → migrate → outbox relay → consumers → three ports → graceful drain | **M0** | the one call every service wires by hand. The whole DAG roots here. |
| 1.2 | Three-surface topology (public / internal RPC / metrics-health); public↔internal is a security boundary; tenant from token | **M0** | the public↔internal split is a security boundary, not a convenience; tenant-from-token (never URL) feeds SUB-D7. |
| 1.3 | Liveness ≠ readiness | **M0** | the SUB-D9 property; composes with fail-static (§1.10). |
| 1.4 | `PersonalDataHolder` auto-registration — every store the harness opens | **M0** mechanism; **exhaustive holder list M1** | the harness wires the mechanism in M0; the **exhaustive H1–H18 list** is GDPR's M1 deliverable that this mechanism guarantees nothing is missed (makes "we forgot a store" structurally impossible). |
| 1.5 | Forward-only online migrations + hot-table declaration | **M0** runner + lint; **per-subsystem flags M1+** | the runner + `forward-only-migration` lint ship M0; each subsystem declares its hot tables (KN `block`/`db_row`/`doc_op`; all high-write) as it lands. SUB-D10 (under-load) proves at **M5**. |
| 1.6 | The twelve architecture lints (the ratchet) | **M0** | each ships with a **red-fixture** (proves it rejects) + a **green-fixture** (proves it admits), wired into CI loud-never-swallowed (no `\|\| true`). The cheapest gate; comes first, stays green forever. |
| 1.7 | Cross-language harness shim (the frozen divergence contract) | **M0** spec; **enforced if/when a subsystem diverges (Chat M4)** | a no-op if Chat stays Rust; the seven non-negotiables (three-surface, liveness≠readiness, no fire-and-forget emit, holder registration, resilient-client, shed order, forward-only migrations) are *specified* now so a divergent subsystem cannot quietly drop them. |
| 1.8 | Telemetry signal set (the drill survival signals) | **M0** | RED/USE per principal-kind + consumer-lag + outbox-depth + breaker-state + fail-static ratios + shed-counts + causal-depth + firehose frame-lag. **Every drill in the catalogue asserts against this set — no signal = no provable drill, so it must exist before anything is drilled.** |
| 1.9 | `ResilientClient::call(target, req, idem)` — timeout + breaker + bulkhead + jittered-retry-idempotent-only; honours `Retry-After` | **M0** | one client every outbound inter-service call goes through. Proven by SUB-D5 (M0) and re-exercised by the surge family (M5). Per-target *values* are each consumer's tuning call (named floor, §6). |
| 1.10 | `FailStatic<T>` — bounded-staleness cache; `static_max ≤ revocation SLA` and ≥ agent-token TTL | **M0** mechanism; **SUB-D4 proven against real Id in M1** | the *mechanism* + the `≤ revocation-SLA ≥ agent-token-TTL` constraint ship M0; the staleness **value W** is `[OPEN — LEGAL]` (DPO-ratified, L-1, §6); the property is only *proven* once a real Identity dependency exists to hiccup (M1). |
| 1.11 | Protected-human-lane shed order + per-surface shed budgets | **M0** mechanism + v1 floor table; **numbers tuned by drills M5** | speculative → batch/CI → agent → human-last; `429 + Retry-After`. The v1 budget *floors* (§7.6 of the architecture) ship M0; the **numbers** are named floors tuned by SUB-D3 / connection-storm drills at **M5**. |

### 1.2 Carried by the substrate — seams it owns a half of, by milestone

These are contracts whose *home* is another cluster but whose **substrate stake** (the bounded/shedding/holder
half) the substrate must implement. Listed so the gate ownership is unambiguous.

| # | Seam (substrate's half) | Home owner | Substrate stake lands |
|---|---|---|---|
| 2.1 | `EventEnvelope` — the canonical field list (names/units anchor, X-5) | Bus (`myelin-events`) | **M0** — the envelope **type** lives in `myelin-events` (a substrate glue crate); the substrate freezes the field list as the anchor every later contract aligns to. |
| 2.2 / 2.3 / 2.4 / 2.5 | `OutboxTx::emit` (the ONLY emit path) + `outbox` table + `EventHandler` template + `consumer_dedup` ledger | Bus | **M0** — these live in `myelin-events`, a substrate-owned glue crate; the substrate ships them as the Tier-1 silent-data-loss floor (SUB-D1/SUB-D2/BUS-D4). |
| 3.5 | Firehose resume-cursor transport — **the bounded / `resync_required`-shed half** (per-connection in-flight caps, slow-consumer drop, scope-bounded selector) | Bus (protocol) | **M0** discipline (bounded-everything generalised to streaming); **exercised M2** (Bus seam) and **re-confirmed M4** (Chat/KN hot streams). The Bus owns the zero-loss-replay half. |
| 11.5 | Backup / restore / **cross-seam integrity** — the harness's failure-injection + telemetry half of SUB-D6 | Storage (restore-verify CI job) | **M0** injector + assertion library; **full gate proven M1** with Storage. The substrate makes the drill *possible*; Storage makes restore-verify *green*. |
| 10.1 | `PersonalDataHolder` auto-registration mechanism (see 1.4) | GDPR (the trait + the H1–H18 list) | **M0** mechanism; **M1** exhaustive list. |

**The plainest statement of the substrate's place in the DAG:** it has **no upstream**. The failure-injection
harness is the only thing that must exist before the substrate can be *proven*, and the substrate builds the
harness too. Everything else in Myelin is downstream of this cluster — which is exactly why it is sequenced
first and why a red substrate gate blocks every later band by the gate invariant (master §4).

---

## 2. The milestones (mapped to master bands, with the work)

The substrate's milestones are **SUB-M0 (its core, = master M0)** plus the **follow-on slices** it owes in M1,
M2, M4, M5, M6 where a property's proof needs a higher system. Each milestone names its work, its entry
dependency, and its exit gate (the drills that must emit a green artifact to call it done).

### SUB-M0 — The substrate core, the harness, and the committed gates (master band M0)

**Thesis (master §2 M0).** Build the machine that proves things, the lints that forbid whole bug-classes, and
the service shell every system boots from. Nothing here is a feature; everything is a precondition for honestly
claiming any feature later. This milestone *is* the substrate half of master M0 — there is no earlier substrate
milestone because the substrate is the root.

**Work — in the order-by-non-negotiability sequence (build the unit of proof, then the data-loss floor, then the
ratchet, then the shell):**

1. **Tier 0 — the failure-injection harness (FIRST, before the systems it drills; R-3, testing-strategy §3).**
   - The **1×/10×/30× load generator** with mixed principal kinds (human / agent / service / CI / external-MCP)
     and per-surface storm profiles (OQ-K: CI-surge, collab op-stream, connection-storm, agent-mention-storm).
   - The **scoped-reversible dependency-break injector** (break one named dependency for one named scope, then
     restore — so a drill can sever Identity, the broker, a downstream, without taking the test rig down).
   - The **telemetry-assertion library** reading the survival-signal set (contract 1.8): RED/USE per
     principal-kind, consumer-lag, outbox-depth, breaker-state, fail-static fresh/stale/closed ratios,
     shed-counts per lane, causal-depth, firehose frame-lag. A property is PROVEN only when a drill forces the
     failure and an assertion over these signals reads green.
   - The **every-incident-adds-a-drill loop** wired (T-3): an incident files a reproducing drill that joins the
     catalogue and re-runs forever.

2. **Tier 1 — the transactional outbox + idempotent-consumer template (the silent-data-loss floor; lives in the
   `myelin-events` glue crate).**
   - `OutboxTx::emit(draft, cause)` (2.2) as the **only** sanctioned emit path — same transaction as the state
     change, causality correct-by-construction (root carries, parent = cause, depth = cause.depth + 1). **No
     `publish_now`** exists in the API (a shortcut that exists will be used and will lose data).
   - The `outbox` table (2.3): `(event_id UNIQUE, aggregate, seq, subject, envelope)`, `UNIQUE(aggregate, seq)`
     per-aggregate ordering, relay claims with `FOR UPDATE SKIP LOCKED` (safe across replicas), stamps the stable
     ULID for broker-side dedup, marks sent, dead-letters after bounded retries.
   - The `EventHandler` template (2.4) with the seven encoded rules (idempotent on `event_id` via the
     `consumer_dedup` ledger 2.5; ack-after-enqueue; whitelist subjects never `*`; bind-durable-by-name;
     terminate poison; bounded prefetch; lag as a first-class metric).
   - The `EventEnvelope` (2.1) **frozen here as the names/units anchor** every later contract aligns to (X-5):
     timestamps RFC-3339 UTC, costs integer minor-units, TTLs/timers seconds, client timeouts ms,
     `pii_key_ref = kms://<tenant>/<dek-epoch>/<class>`.

3. **Tier 3 — the twelve committed architecture lints (the ratchet; cheapest gate, comes early).**
   - `no-cross-db`, `no-raw-publish`, `tenant-predicate`, `no-host-exec`, `forward-only-migration`,
     `no-cross-sync-cycle`, `residency-pin`, `control-plane-pii-free`, `search-requires-acl-filter`,
     `no-llm-in-platform`, `no-untagged-personal-data`, `flow-determinism`.
   - **Each ships with a red-fixture** (a code sample the lint must reject) **+ a green-fixture** (a sample it
     must admit), wired into CI **loud, never swallowed** (no `|| true`). The four most load-bearing —
     `tenant-predicate` (no cross-tenant leak, F2), `no-raw-publish` (no event escapes the outbox, F5),
     `no-host-exec` (no sandbox bypass), `no-untagged-personal-data` (no PII escapes the data map) — are the ones
     that make whole bug-classes impossible to *compile*.

4. **Tier 6-precondition — the service shell + the resilience primitives.**
   - `serve(AppSpec)` (1.1): boot → migrate → outbox relay → consumers → three ports (1.2) → graceful drain;
     liveness ≠ readiness (1.3); forward-only online migrations runner (1.5).
   - `ResilientClient` (1.9): timeout + breaker + bulkhead + jittered-retry-idempotent-only, honours
     `Retry-After`.
   - `FailStatic<T>` (1.10): bounded-staleness cache, mechanism + the `static_max ≤ revocation-SLA ≥
     agent-token-TTL` constraint (the value W is `[OPEN — LEGAL]`, §6).
   - The protected-human-lane shed order + the per-surface v1 budget floor table (1.11, ADR-16).
   - The cross-language harness shim **specified** (1.7) — contract only, no implementation (Chat may diverge,
     TE-21).

5. **The committed-gate machinery (the rest of the ratchet).**
   - The **contract-coverage scanner**: CI fails the workspace if any contract-index row lacks provider +
     consumer CDC coverage (an uncommitted contract test is no contract test).
   - The **shared overlay/state primitives** (testing-strategy §5): built before any feature consumes them so the
     off-screen-picker / clipped-dialog / focus-leak bug-classes are foreclosed at the design-system layer.
   - The **versioned thresholds file**: one file holds every Q32 default-to-beat (N=5min revocation, 30× surge,
     W=5min fail-static, RPO ≤ 5min, RTO ≤ 1h/tenant ≤ 4h/cell, depth ceilings 12/16, etc.). A red gate becomes a
     "claimed, not proven" scorecard row — **never edited green**.

**Entry dependency:** **none** (this is the root of the DAG).

**Exit gate (must be green to start M1 — master §2 M0 exit):**
- **SUB-D1** (kill service between commit and publish → exactly-once-in-effect, **0 ghost, 0 lost**; outbox-depth
  drains, dedup ledger absorbs) — CI.
- **SUB-D2** (drop broker mid-stream → **0 lost** across reconnect by bind-by-name + dedup; a slow subject does
  not head-of-line-block others) — CI.
- **BUS-D4** (crash producer between state-commit and publish → event delivered, never without state; outbox
  **emit-iff-committed**) — CI.
- **SUB-D5** (trip a downstream breaker → callers fail fast, no retry through the tripped breaker, honour
  `Retry-After`, no amplification) — CI.
- **SUB-D7** (cross-tenant read via path-tenant ≠ token-tenant → **0**; the `tenant-predicate` lint catches a
  tenant-less query at compile time) — CI.
- **SUB-D8** (adversarial agent→agent loop → depth ceiling + shared-root tripwire + bounded pool halt it) — CI.
  *(The substrate ships the depth-ceiling / tripwire / bounded-dispatch machinery; the full agent-loop proof
  re-runs in M2 with the agent fabric.)*
- **SUB-D9** (kill a critical dependency → instance reports not-ready + sheds; liveness does not restart-storm) —
  CI.
- **All twelve lints green** with both fixtures; the **contract-coverage scanner** passes on the (still-small)
  contract set; the **harness self-test** (inject one fault, read one telemetry assertion — the unit-of-proof
  drilling itself).

### SUB-M1 — Fail-static proven + the restore-verify cross-seam half (master band M1)

**Thesis.** Two substrate properties cannot be *proven* in M0 because the system they degrade against does not
yet exist: **fail-static needs a real Identity to hiccup**, and **restore-verify cross-seam integrity needs
Storage's backup/restore + the blob/index/offset seams**. M1 is where Identity, Storage, and Tenancy land
(master §2 M1), so the substrate's two M0-built-but-unproven mechanisms get drilled green here.

**Work:**
- **Fail-static proven against Identity** (1.10/4.11): the `FailStatic<T>` mechanism (M0) is now wired into the
  Identity authz client's read path; SUB-D4 injects a transient Identity hiccup and asserts already-authenticated
  traffic survives on the coarse cache within W, a revoked actor is denied once the window closes, and the zookie
  bypass forces a security-sensitive read past the cache. (Identity's own ID-D2 is the mirror property; the
  substrate owns the `FailStatic` primitive, Identity owns the policy.)
- **The restore-verify cross-seam half** (11.5, SUB-D6, with Storage): the substrate supplies the
  failure-injection + telemetry-assertion machinery; Storage supplies the WAL+PITR restore and the
  `restore-verify` CI job. Together they prove the rebuild lands at **one consistent cross-seam point**
  (OLTP rows ↔ blob ↔ search index ↔ event-log offsets — no row → missing blob). **This is the silent-data-loss
  floor; it is the half the substrate owes.**
- **The `PersonalDataHolder` exhaustive-list confirmation** (1.4): the M0 auto-registration mechanism is now
  exercised against the real H1–H18 holder set as Identity/Storage/GDPR stores come online; the
  `no-untagged-personal-data` lint goes red on any untagged PII field (GA-D5 mirror).

**Entry dependency:** SUB-M0 green (the `FailStatic` primitive, the harness, the telemetry library) **and** the
M1 systems landing (Identity authz client to hiccup, Storage backup/restore to rebuild). Per the band order, the
substrate's M0 gate is green before M1 starts.

**Exit gate (contributes to the master M1 → M2 boundary):**
- **SUB-D4** (Id-hiccup → already-authenticated survives within W; revoked denied when the window closes;
  fail-static fresh/stale/closed ratios read green) — CI.
- **SUB-D6 / STOR-D1 / STOR-D2** (rebuild from backups → **0 loss**; OLTP↔blob↔index↔offsets one consistent
  point; **RPO ≤ 5 min, RTO ≤ 1h/tenant ≤ 4h/cell**) — SCHED. *(Substrate owns the injection/assertion half;
  Storage owns the gate. M2 does not start over a red STOR-D1.)*

### SUB-M2 — The firehose backpressure half goes live (master band M2)

**Thesis.** The firehose resume-cursor transport (contract 3.5) is **Bus-owned**, but it **rides the substrate's
bounded-everything + shed discipline**. M2 is where the Bus ships the resume-cursor protocol and the first hot
streams (CI logs, KN collab, Chat presence) appear, so the substrate's bounded/shed half is exercised here for
the first time.

**Work:**
- **Per-connection in-flight frame caps** (§7.1 bounded-everything generalised to streaming): a subscription's
  frame buffer is bounded; over-cap sheds in the firehose's own bounded queue.
- **Slow-consumer drop to `resync_required`** (never buffer unboundedly): the slow consumer falls back to a full
  `*.snapshot` replay (the cold-rebuild path, named not silent) rather than the transport growing memory.
- **Scope as a bounded selector, never `*`** (the whitelist-not-`*` rule generalised): a 50k-row board paginates
  its scope; the firehose delivers only that slice's frames.
- **The per-surface shed budgets (1.11) apply to firehose frames**: presence/speculative frames shed before
  message delivery; agents shed before humans.

**Entry dependency:** SUB-M0 green (bounded-everything, the shed order, the telemetry library) **and** the M2 Bus
firehose protocol (3.5) landing.

**Exit gate (the substrate's half of the M2 firehose property; the full zero-loss-replay is the Bus's):**
- **The substrate's firehose survival signals read green** (per-`(stream, scope)` frame lag, `resync_required`
  count) under a hot-stream drill — the bounded/shed half of the D-11 reconnect-loses-zero-ops drill (the Bus
  owns the zero-loss-replay assertion; the substrate owns "bounded-and-sheds, never unbounded memory"). Proven
  under real load in **SUB-M4**.

### SUB-M4 — Cross-language shim enforced (if Chat diverges) + firehose under connection-storm (master band M4)

**Thesis.** Two substrate seams reach their real test in M4. **(1)** Chat is the named candidate to diverge to a
non-Rust connection tier (TE-21); if it does, the frozen cross-language shim (1.7) is **enforced** here — the
seven non-negotiables must hold in the divergent language. **(2)** The firehose backpressure half (SUB-M2) meets
its hardest real load: Chat's connection-storm and KN's hot-doc collab op-stream.

**Work:**
- **Enforce the cross-language harness shim** (1.7) **if and only if** Chat diverges: assert the divergent tier
  provides three-surface topology, liveness ≠ readiness, no fire-and-forget emit (the outbox pattern in the
  divergent language too), `PersonalDataHolder` registration, the resilient-client behaviour + `Retry-After`
  honouring, the principal-aware shed order, and forward-only online migrations. A no-op if Chat stays Rust — but
  the shim cannot be quietly dropped at the language boundary.
- **Firehose backpressure under connection-storm**: re-confirm the SUB-M2 bounded/shed half under Chat's
  connection-storm budget and KN's hot-doc op-stream budget (1.11 §7.6 floors).

**Entry dependency:** SUB-M2 (the firehose half live) + the M4 subsystems (Chat connection tier, KN collab).

**Exit gate (contributes to the master M4 → M5 boundary):**
- **CHAT-D1 / CHAT-D13 / CHAT-D14** firehose path (resume 0 lost/0 dup; co-commit; idempotent send) — the
  substrate's bounded/shed half reads green under connection-storm (Chat owns the end-to-end drill; the substrate
  asserts its survival signals hold) — CI.
- **The cross-language shim's seven non-negotiables green** in the divergent tier (only if Chat diverged) — CI.

### SUB-M5 — World-scale hardening: the surge family + online-migration-under-load (master band M5)

**Thesis (master §2 M5).** With all systems on one substrate and the deterministic correctness drills green,
prove the substrate **as a whole** under world-scale load. The substrate's two scheduled-frequency drills
(SUB-D3 surge, SUB-D10 migration) and its participation in the F6 surge family land here.

**Work:**
- **The 30× surge family** (SUB-D3, part of the F6 family across all owners): drive a 30× agent/CI surge on one
  tenant; assert the **protected human lane holds**, the **agent lane sheds** (429 + Retry-After, clients honour
  it), and **cross-tenant impact is 0** — against the §7.6 per-surface budgets, which are now **tuned** by the
  drill (the named-floor numbers become measured numbers).
- **Online-migration-under-load** (SUB-D10): run an expand→backfill→contract migration on a **restored
  production-scale copy under load**; assert no blocking lock beyond budget (lock-wait p99) and 0 errored writes /
  0 downtime. This ties the migration runner (1.5) to the restore-verify machinery (the lock-time-against-a-
  restore rule, §9.2 of the architecture).
- **Restore-verify re-confirmed at cell scale** (SUB-D6 / STOR-D2): RPO/RTO held under world-scale load.
- **The resilient-client per-target values tuned by the surge drills** (1.9): the auth hot path gets a tighter
  timeout than a batch indexer — measured, not predicted.

**Entry dependency:** M4 green (all five subsystems on the substrate; the deterministic correctness drills green;
the named floors in place to be tuned).

**Exit gate (contributes to the master M5 → M6 boundary):**
- **SUB-D3** (30× surge: human lane within budget, agent sheds, cross-tenant impact 0) — SCHED.
- **SUB-D10** (online-migration-under-load: lock-wait p99 within budget, 0 errored writes, 0 downtime) — SCHED.
- **SUB-D6 / STOR-D2 at cell scale re-confirmed** (RPO/RTO under world-scale load) — SCHED.
- The substrate participates in **all four E2E scenarios** (each boots on `serve`, emits via the outbox, reads
  the telemetry signal set; the substrate's job is to be invisible — no scenario fails on a substrate signal).

### SUB-M6 — Dogfooding: the substrate runs Myelin's own development (master band M6)

**Thesis (master §2 M6).** The cheapest, most honest load generator is the platform's own development. The
substrate's lints and harness now run as **Myelin CI jobs on Myelin's own commits** — the ratchet ratchets on
the builders' own work.

**Work:**
- The twelve lints + the contract-coverage scanner + the mandatory-core mutation gate run as **Myelin CI jobs**
  on every Myelin commit (the dogfood loop is live).
- The every-incident-adds-a-drill loop files a **Myelin issue + a reproducing drill** for any substrate incident.
- The harness drives the substrate's own surge/restore/migration drills as part of the self-hosting CI graph.

**Entry dependency:** M5 green (the substrate is world-scale-ready; you do not dogfood real team data onto a
substrate whose restore-verify and fail-static are not green — Tier 1 of the thesis).

**Exit gate (contributes to the master M6 done-bar):**
- **The Myelin self-hosting CI graph is green** on the platform's own commits (the lints + the contract-coverage
  scanner run there).
- **No earlier substrate gate is red** (the truth-up pass confirms every substrate PROVEN row rests on a dated
  green artifact — code-wins-over-docs, EI-01 §1).

---

## 3. The floor-then-full progression (name each floor + its follow-on)

The discipline (VISION §3, EI-04 §4): **name the floor and name the follow-on.** A floor masquerading as done is
the failure. The substrate carries a small set of named floors — most are mechanism-now / value-or-scale-later,
not full subsystem swaps (because the substrate's *correctness* floors are absolute, not staged).

| Floor (shipped) | Band | The full answer (follow-on) | Band | The trigger |
|---|---|---|---|---|
| **fs-backed `BlobStore`** (content-addressed, hash-on-write) | M0/M1 | **Object-store `BlobStore`** (one-line swap — the narrow trait keeps fs↔S3 a single change) | M5 | with object-backed git packs (the single-node ceiling measured); never any code reads through `head`/`get` differently. |
| **The per-surface shed-budget v1 *floor table*** (every surface bounded + reserved human lane + shed order) | M0 | **Tuned per-surface numbers** (the floor *discipline* is the contract; the numbers are tuned) | M5 | SUB-D3 / connection-storm drills measure the real budgets (EI-02 §8 measured-not-predicted). |
| **Resilient-client default per-target values** (one default set) | M0 | **Per-target tuned values** (auth hot path tighter than a batch indexer) | M5 | the surge/latency drills (SUB-D5, the F6 family) measure each target. |
| **Hot-table seed set named** (KN `block`/`db_row`/`doc_op`; high-write subsystems) | M0 mechanism | **Measured hot-table flags** (a table flagged hot on *measured* write rate, not predicted) | M1+ per subsystem | the subsystem's write-rate measurement gate (declare → `forward-only-migration` lint enforces). |
| **`FailStatic` mechanism + constraint** (`static_max ≤ revocation-SLA ≥ agent-token-TTL`) | M0 | **The ratified value W** (one DPO-ratified number) | parallel (legal) | `[OPEN — LEGAL]` L-1; the mechanism ships regardless, the value is one ratified statement, not five. |
| **Single-region event log** (general-purpose DB) | M0 | **Column-store / time-series seam** for the highest-volume streams | post-M5 | event volume **measured** to outgrow the DB (EI-04 §5) — added only once volume is measured, never before. |
| **Cross-language shim *specified*** (contract only, 1.7) | M0 | **Shim *enforced*** (the seven non-negotiables proven in the divergent language) | M4 | if and only if Chat diverges to a non-Rust connection tier (TE-21). |

**The honest-floor rule binds all of these:** each is tracked in the gap report with its claimed/proven status
and its linked follow-on; the gap being *invisible* is the only failure (EI-04 §4). Note the asymmetry vs a
consumer system: the substrate's **correctness floors are not staged** — the outbox is not a floor for a "better
outbox," the lints are not a floor for "better lints." They are absolute from M0. The floors above are
*scale/value/legal* deferrals (which blob backend, which budget number, which staleness value), never correctness
deferrals.

---

## 4. The world-scale / hard-problem work, scheduled explicitly

The substrate owns the *substrate half* of three of the hard problems (EI-04). Each is scheduled, with the floor
named:

- **The durable resume-cursor real-time transport (EI-04 §2.2 — "build it FIRST").** The substrate ships the
  **bounded-everything + shed discipline** half of the firehose in **M0** (it is generalised
  bounded-everything), so the transport's backpressure is correct from the start. The Bus ships the
  zero-loss-replay protocol; the CRDT slots into *that same transport* in M5. The substrate's stake is proven
  under hot-stream load in **M2** and under connection-storm in **M4**. **Floor:** the bounded/shed half ships
  M0; **follow-on:** exercised against real streams M2/M4, tuned at scale M5.
- **Untrusted-code execution (EI-04 §5).** The substrate does **not** own the sandbox (that is the Agent
  Fabric's `ToolHands::exec` / the unified runner, AG-D4, M2). The substrate's stake is the **`no-host-exec`
  lint** (1.6, M0) that makes a host-execution bypass *impossible to compile*, and the **resilient-client +
  shed-lane + reserve-gate substrate** every untrusted run rides. **Floor:** the lint ships M0; **follow-on:**
  the real-kernel escape drill is the Agent Fabric's M2 GATE (the substrate provides the ratchet under it, not
  the sandbox).
- **Restore-verify at world scale (EI-04 implicit; ADR-18, the silent-data-loss floor).** The substrate owns the
  **failure-injection + telemetry half** of SUB-D6. **Floor:** single-tenant restore-verify proven M1;
  **follow-on:** re-confirmed at **cell scale under world-scale load in M5** (SUB-D6 / STOR-D2), and
  online-migration-under-load (SUB-D10) measured against a restored prod-scale copy in M5.
- **The 30× surge family (F6, world-scale load).** SUB-D3 is scheduled in **M5** as part of the cross-owner F6
  surge family; the per-surface budget *numbers* are tuned by it (the v1 floor table is the M0 floor). This is
  the substrate's headline world-scale proof: the protected human lane holds while the agent lane sheds and
  cross-tenant impact stays 0.

---

## 5. The gates / drills (quantified) that call each milestone done

The gate invariant (master §4): **no later band is done over a red earlier substrate gate.** The substrate owns
**two of the platform's permanent gates** (re-run forever, never "done"): the **outbox 0-loss/0-ghost floor**
(SUB-D1/SUB-D2/BUS-D4, re-run on every change touching the emit path) and — as co-owner with Storage — the
**restore-verify gate** (SUB-D6, re-run on every change touching a store). The full substrate drill set, by
band:

| Milestone | Drill | Quantified threshold | Green artifact | Freq |
|---|---|---|---|---|
| SUB-M0 | **SUB-D1** | kill service between commit & publish → exactly-once-in-effect (**0 ghost, 0 lost**) | outbox-depth drains; dedup ledger | CI |
| SUB-M0 | **SUB-D2** | drop broker mid-stream → **0 lost** across reconnect; slow subject doesn't block others | consumer-lag; no HoL stall | CI |
| SUB-M0 | **BUS-D4** | crash producer between state-commit and publish → delivered, never without state | outbox emit-iff-committed | CI |
| SUB-M0 | **SUB-D5** | trip a downstream breaker → fail fast, honour `Retry-After`, no amplification | breaker-state; Retry-After issuance | CI |
| SUB-M0 | **SUB-D7** | cross-tenant read via path≠token → **0**; lint catches tenant-less query at compile | misroute-count 0; lint green | CI |
| SUB-M0 | **SUB-D8** | adversarial agent→agent loop → depth ceiling (12/16) + tripwire + bounded pool halt it | causal-depth histogram; tripwire | CI |
| SUB-M0 | **SUB-D9** | kill a critical dependency → not-ready + sheds; no restart-storm | readiness flips; no liveness churn | CI |
| SUB-M0 | **All 12 lints + harness self-test** | every lint red-fixture rejects + green-fixture admits; contract-coverage scanner passes; harness injects a fault and reads one assertion | lint green ×12; scanner pass; assertion green | CI |
| SUB-M1 | **SUB-D4** | Id-hiccup → already-authenticated survives within W; revoked denied when window closes | fail-static fresh/stale/closed | CI |
| SUB-M1 | **SUB-D6** (w/ Storage) | rebuild from backups → **0 loss**; OLTP↔blob↔index↔offsets one consistent point; **RPO ≤ 5 min, RTO ≤ 1h/tenant ≤ 4h/cell** | restore-verify-pass | SCHED |
| SUB-M2 | **firehose bounded/shed half** | hot-stream drill → per-(stream,scope) frame lag bounded; over-retention gap → `resync_required` (named, not silent); slow consumer dropped (not buffered) | frame-lag; resync_required count | CI |
| SUB-M4 | **cross-language shim** (if Chat diverges) | the seven non-negotiables hold in the divergent tier | shim-conformance green | CI |
| SUB-M4 | **firehose under connection-storm** | the bounded/shed half holds under Chat connection-storm + KN hot-doc budgets | shed-counts/lane; frame-lag | CI |
| SUB-M5 | **SUB-D3** | 30× agent surge one tenant → human lane holds, agent sheds, **others unaffected** | shed-counts/lane; per-tenant RED | SCHED |
| SUB-M5 | **SUB-D10** | expand→backfill→contract on a restored prod-scale copy under load → no blocking lock beyond budget; **0 downtime** | lock-wait p99; 0 errored writes | SCHED |
| SUB-M5 | **SUB-D6 / STOR-D2 at cell scale** | RPO/RTO held under world-scale load | restore-verify-pass at scale | SCHED |

**The green-artifact rule (testing-strategy §4):** a milestone is done only when its drills emit a dated green
artifact — the named telemetry assertion (contract 1.8) reading green. Until then the property is **claimed, not
proven**, and lives as a scorecard row, never edited green (the thresholds-file discipline).

---

## 6. The honesty register — open items the substrate carries into the build

These are named floors / decision-shaped calls, not silent gaps (each names its follow-on + owner):

- **`[OPEN — LEGAL]` — the fail-static staleness bound value W (L-1).** The mechanism + the `≤ revocation-SLA ≥
  agent-token-TTL` constraint ship in M0 regardless; the **value** is a DPO-ratified call (architecture §8.2,
  index 4.11). *This is the one substrate-owned legal flag.* The broader `[OPEN — LEGAL]` posture (free-text /
  immutable erasure) is GDPR/Audit's deliverable; the substrate provides the structural floor it instantiates.
- **Concrete resilient-client per-target values** remain each consuming system's call (M5-tuned via drills); the
  shape + on-by-default posture are fixed in M0. Not a blocker.
- **The per-surface shed-budget numbers** are named v1 floors tuned by the drills (SUB-D3, the connection-storm
  drill) in M5; the floor *discipline* (bounded + reserved human lane + shed order) is the M0 contract.
- **Hot-table flags are measured, not predicted** (architecture §9.4): the seed set is named in M0, but a table
  is flagged hot on measured write rate per subsystem.

---

## 7. Digest

**Milestones (substrate slices, mapped to master bands):**
- **SUB-M0 (= master M0)** — the core: the workspace + eight glue crates, `serve(AppSpec)`, the transactional
  outbox + idempotent-consumer template, the failure-injection harness (Tier 0, the unit of proof), the twelve
  committed lints (red+green fixtures), the contract-coverage scanner, the overlay/state primitives, the
  thresholds file. **This is the root of the whole DAG — it has no upstream.**
- **SUB-M1 (master M1)** — fail-static proven against a real Identity hiccup (SUB-D4); the restore-verify
  cross-seam half proven with Storage (SUB-D6, the silent-data-loss floor); the exhaustive-holder mechanism
  confirmed.
- **SUB-M2 (master M2)** — the firehose resume-cursor **backpressure half** (bounded frame caps,
  slow-consumer→`resync_required`, scope-bounded selector) goes live against real hot streams.
- **SUB-M4 (master M4)** — the cross-language shim **enforced** if Chat diverges (the seven non-negotiables); the
  firehose half re-confirmed under connection-storm.
- **SUB-M5 (master M5)** — world-scale: the 30× surge family (SUB-D3), online-migration-under-load (SUB-D10),
  restore-verify at cell scale; the budget/per-target floors tuned to measured numbers.
- **SUB-M6 (master M6)** — the lints + harness run as Myelin CI jobs on Myelin's own commits (the dogfood loop).

**Floors + follow-ons:**
- fs-backed `BlobStore` (M0) → object-store `BlobStore` one-line swap (M5).
- per-surface shed-budget v1 floor table (M0) → tuned numbers (M5).
- resilient-client default per-target values (M0) → per-target tuned values (M5).
- hot-table seed set named (M0) → measured hot-table flags per subsystem (M1+).
- `FailStatic` mechanism + constraint (M0) → the ratified value W (parallel/legal, L-1).
- single-region event log (M0) → column-store seam, only when volume is measured (post-M5).
- cross-language shim specified (M0) → enforced iff Chat diverges (M4).
- *The correctness floors (outbox, the twelve lints, the harness) are NOT staged — absolute from M0.*

**Critical upstream dependencies:** **none.** The substrate is the dependency root (master §3.2). The only thing
that must exist before the substrate is *proven* is the failure-injection harness — which the substrate builds in
M0 (Tier 0, the unit of proof). Every other shared system and subsystem is **downstream** of this cluster:
Identity, Storage, Tenancy (M1) build on `serve` + the outbox + the holder mechanism + the lints; the reactive
layer (M2) emits via the outbox and rides the resilient client + shed lane + fail-static; the subsystems (M3/M4)
boot from `serve`. By the gate invariant, **a red substrate gate (SUB-D1/D2, BUS-D4, any of the twelve lints)
blocks every later band** — which is precisely why it is sequenced first. The two substrate-owned **permanent
gates**: the **outbox 0-loss/0-ghost floor** (re-run on every emit-path change) and the **restore-verify gate**
(co-owned with Storage, re-run on every store change).
