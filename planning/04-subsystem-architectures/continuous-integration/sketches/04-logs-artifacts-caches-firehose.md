# Sketch 04 — Logs (firehose), artifacts & caches at scale; secrets inside the boundary

> Phase 4 — CI exploration. CI is the platform's heaviest storage consumer and the firehose's primary
> driver (CI-DD §5.4/5.5; Phase-2 §8.1). Decides: the **`ci.log.available` pointer-event model** (logs
> ride the firehose, NEVER the durable bus — the ownership line in the Phase-3 handoff), the
> artifact/cache content-addressed model, and **secrets resolved inside the boundary** (CI-1).
>
> Binding inputs: Bus §4.3 (firehose split; `firehose::publish/tail`; the durable bus carries only
> pointer events), Storage §3.3 (T3 log/firehose: append-mostly object-backed segments + a range index
> in OLTP), Storage §3.2 (T2 object/blob: content-addressed `BlobStore`, per-tenant dedup, crypto-shred),
> CI-1 (secrets by name, resolved inside, never forwarded via the agent runtime).

---

## Part 1 — Logs: the firehose, and the pointer-event contract CI OWNS

The hard rule (ADR-04.5; Bus §4.3): **the durable bus must not carry one event per log line** — an
agent is never woken per line. CI **owns** the `ci.log.*` side of this:

- **`ci.log.appended` frames ride the firehose transport** (`firehose::publish(stream, frame)`, Bus
  §5.5) — keyed by `(run, job, step)`, NOT the durable bus. Live-tail viewers `firehose::tail(stream,
  range)` for low-latency SSE/websocket fan-out to many viewers.
- **`ci.log.available` is the ONLY log-related *durable* event** — a **pointer**: "lines N..M of
  `run/job/step` are ready at `<ArtifactRef>`" (Bus §4.3 table). This is what Search/Refs/agents
  consume; they pull the range they need, they don't get the firehose. **This pointer taxonomy is CI's
  to own and complete** (Phase-3 handoff: "owns the `ci.*` taxonomy + `ci.log.available` pointer
  events").
- **Durable archive** (Storage T3): frames append to a current segment; sealed segments flush to the
  T2 object tier as **content-addressed blobs** with a **range index** `(run/step, byte-range) →
  (segment-blob, offset)` in CI's OLTP for tail + range read (Storage §3.3). Standard "log as immutable
  segments + small index" (cites: Kreps *The Log*; the LSM/segment pattern).

```rust
// CI's log pipeline coordinator (the firehose seam — consumes Bus/Storage, owns the pointer taxonomy)
fn ship_line(run, job, step, line) {
    let redacted = secret_redact(line);                 // in-flight masking — best-effort defence (see Part 4)
    firehose::publish(stream_of(run,job,step), frame(redacted));   // live tail
    seal_and_flush_if_segment_full();                   // → T2 content-addressed blob + OLTP range index
    // coalesced, NOT per line:
    emit_pointer_if_threshold(ci.log.available { run, job, step, range });  // durable bus pointer
}
```

### Why this matters for UX (the jump-to-failure path)
The log model must be **structured around the step/job graph**, not a flat blob (CI-DD §6.6): the range
index is keyed by `(job, step)` so the live-log view is **collapsible per step** and a failed step
deep-links to its byte range (`ArtifactRef#step-3#L42-L88`, the sub-artifact `#sub` scheme, contract
5.7). "The system assembles context": a failing check → the failing step → the failing log lines is one
pre-fetched path (design-language §8b.6). This is the diff-anchored-log open item (CI-DD §9 / Phase-2
§9) resolved by **stable `(job, step, byte-range)` sub-anchors**.

## Part 2 — Artifacts & caches: content-addressed blobs

Both ride the **T2 `BlobStore { put, get, head, delete }`** (Storage §3.2): hash-on-write (BLAKE3),
**plaintext-hash-within-tenant-keyspace → per-tenant dedup** (cross-tenant dedup would be a residency
leak, Storage §3.2), residency-pinned, crypto-shred-capable.

| | Artifact | Cache |
|---|---|---|
| Semantics | retained job output (binaries, SBOMs, reports) | reconstructible perf optimization (deps, compiler cache) |
| Loss impact | **correctness** (it's an output) | **perf only** (rebuildable) |
| Addressing | content hash + `(run, name)` index | **key derivation** = `hash(lockfile + os + toolchain + ...)` — the subtle part (CI-DD §6.2) |
| Retention | explicit TTL/GC per project (Art. 5 storage-limitation) | TTL/LRU eviction |
| Referenceable | yes — `ArtifactRef`, linkable from chat/release (contract 5.x) | no (internal) |

### Poisoning resistance (a known exploit class — CI-DD §6.2; supply-chain, sketch 05)
**Cache writes from untrusted/fork runs must NOT poison the trusted cache.** Caches are **scoped by
trust tier / branch**: a PR-from-fork (`Untrusted`) run gets a **read-restricted or isolated** cache
namespace and **cannot write** the trusted (default-branch) cache. A restored cache is an *input* to a
build, so a poisoned cache is a build compromise — the scope boundary is the defence.

### Locality (CI-DD §6.2)
Cache/artifact blobs live **near the runner region** (residency *and* download-cost) — there is no
global blob pool, mirroring the no-global-runner-pool rule (sketch 03). Cross-region replication of a
cache is opt-in and residency-gated.

## Part 3 — GDPR: where PII hides, and crypto-shred (the CI-spicy holder)

CI is a GDPR-spicy `PersonalDataHolder` because **PII leaks incidentally** (CI-DD §9), not just into
obvious fields:
- **Direct:** commit/PR author, "triggered by", "approved by" — stored as **identity *references*, never
  copied PII** (the committed steer; resolves through Id, erasable there — CI-DD §9; the platform
  references-not-payloads rule).
- **Logs (worst offender):** emails, usernames, IPs, tokens, real fixtures — append-mostly + huge.
- **Artifacts/caches:** may embed personal data (seeded DBs, screenshots).

**Erasure = crypto-shred** (Storage §5.1; GD-4 granularity rule): log segments and artifact/cache blobs
are **per-tenant-DEK envelope-encrypted** (bulk pseudonym-referenced content), with **per-subject DEK**
for any free-text store that must carry inline PII — `erase(subject)` destroys the key, rendering the
immutable/append-only ciphertext (incl. backups) unrecoverable **without rewriting** (NIST SP 800-88r1;
Boneh & Lipton 1996). Plus **short default retention/TTL** (shrinks the erasure burden — Art. 5).
CI implements `locate/export/rectify/restrict/erase` over run-state, logs, artifacts, caches; the
harness auto-registers every CI store as a holder (GD-3 — "we forgot the cache table" is structurally
impossible). **Restriction flag honoured:** a restricted subject's data is not indexed / agent-used /
analytics-fed (Phase-3 handoff "honour the restriction flag").

## Part 4 — Secrets resolved INSIDE the boundary (CI-1) — non-negotiable

The hardening profile's secrets rule (EI-03 §3; CI-1): **secrets by name only, resolved inside the
sandbox boundary per run, scoped to exactly this job's references, never baked into images, and never
handed to the agent runtime to forward.**

- The `JobSpec` carries **secret *names*** (`SecretRef`), not values (sketch 01). An **in-boundary
  broker** resolves them after the sandbox is up, scoped to this one job's references, via the shared
  secret capability (placed under Id/GDPR per Phase-2 §8.7).
- **OIDC short-lived audience-scoped credentials over long-lived static keys** (CI-DD §6.4) — CI mints
  a short-lived federated token to talk to a registry/cloud target instead of storing static keys; a
  strong EU-sovereign + least-privilege fit. Minted via Id's token machinery.
- **Untrusted/fork runs get NO secrets by default** (the canonical "fork exfiltrates prod secrets" CVE
  class, CI-DD §7) — gated on `trust_tier`. Protected environments require explicit grants/approval.
- **Log masking is best-effort defence-in-depth, NOT a boundary** (CI-DD §6.4): in-flight redaction
  (Part 1) reduces leakage but is never trusted as the security control — egress default-deny is the
  control.

## Floors & follow-ons
- **FLOOR:** the log engine ships as object-backed segments + an OLTP range index (Storage T3); a
  measured volume promotion to a dedicated time-series/wide-column tier is the named follow-on (BUS-6
  analogue; Storage's seam).
- **FLOOR:** crypto-shred granularity for *free-text PII in logs* defaults to per-tenant-DEK; per-subject
  free-text shred is the named GD-6 / `[OPEN → LEGAL]` follow-on (CI-DD §9).
- **Drills owed (PROVE-IT):** (a) **erasure-reaches-every-holder** — erase a subject → logs/artifacts/
  caches/run-state PII unrecoverable, run attribution falls back to the opaque pseudonym (AG-D10
  analogue). (b) **fork-cannot-poison-trusted-cache** and **fork-gets-no-secrets** — adversarial fork
  run; gate: zero trusted-cache writes, zero secret reads. (c) **residency** — EU tenant's logs/
  artifacts/caches never leave region (residency-pin lint + a residency_verify attestation).
