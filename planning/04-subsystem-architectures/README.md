# Phase 4 — Subsystem Architectures (index)

> The five product subsystems of Myelin, each designed Stage-1 (UX/sketch) → Stage-2 (detailed
> architecture) **on top of the Phase-3 shared layer** ([`03-shared-systems-architecture`](../03-shared-systems-architecture/)).
> Canonical brief: [`VISION.md`](../../VISION.md). Each subsystem is a thin shell over `myelin-substrate`
> (`serve(AppSpec)`, the outbox, the three-surface topology), reads no other subsystem's DB (ADR-01), and
> reaches the shared systems only through the Phase-3 glue contracts. This index frames the five, records
> each one's **language/DB choice** (and any divergence from the Rust default + its justification), and the
> **headline resolved hard-problems**. It tees up **Phase 5 reconciliation**, whose primary input is the
> consolidated [`cross-subsystem-change-requests.md`](./cross-subsystem-change-requests.md). Date: 2026-06-19.

---

## The frame

Phase 4 is where the platform's bet — *one identity/permission/event/reference fabric, agent-native,
EU-sovereign, GDPR-by-construction* — is proven against five concrete products. The shared layer did the
heavy lifting in Phase 3 (the durable bus, the workflow engine, the firehose, the blob/log tiers,
reserve/settle, the ReBAC engine, `project`/`resolve`/`replay`). Each subsystem's genuine green-field work
is therefore small and named, and the rest is disciplined composition of frozen contracts. The five span
the platform's hardest surfaces: **Git** (the system-of-record + the worst erasure problem), **CI** (the
single hardest security surface — the one place untrusted code runs, and the unified sandbox the Agent
Fabric reuses), **Issues** (the most cross-subsystem-coupled — board/roadmap/governance over one model),
**Knowledge** (the heaviest reference-graph producer + the hardest real-time-collab problem), and **Chat**
(the most PII-dense holder + the most visible agent-native surface). Every "could-fail" property names a
quantified PROVE-IT drill; every partial answer is a named floor with a follow-on owner.

---

## The five subsystems

| Subsystem | Index README | Architecture (Stage 2) | Design / sketches (Stage 1) |
|---|---|---|---|
| **Git Hosting & Code Review** | [`git-hosting/README.md`](./git-hosting/README.md) | [`git-hosting/architecture/`](./git-hosting/architecture/) (`00`–`07`) | [`git-hosting/design/`](./git-hosting/design/) (IA/flows/wireframes) + [`git-hosting/sketches/`](./git-hosting/sketches/) (`00`–`09`; the visual/token pass is OQ-12) |
| **Continuous Integration / CD** | [`continuous-integration/README.md`](./continuous-integration/README.md) | [`continuous-integration/architecture/`](./continuous-integration/architecture/) (`00`–`07`) | [`continuous-integration/design/`](./continuous-integration/design/) + [`continuous-integration/sketches/`](./continuous-integration/sketches/) (`00`–`06`) |
| **Issue Tracker** | [`issue-tracker/README.md`](./issue-tracker/README.md) | [`issue-tracker/architecture/`](./issue-tracker/architecture/) (`00`–`07`) | [`issue-tracker/design/`](./issue-tracker/design/) + [`issue-tracker/sketches/`](./issue-tracker/sketches/) (`00`–`09`) |
| **Knowledge Platform (Notion-class)** | [`knowledge-platform/README.md`](./knowledge-platform/README.md) | [`knowledge-platform/architecture/`](./knowledge-platform/architecture/) (`00`–`08`; +`08` committed-resolutions) | [`knowledge-platform/design/`](./knowledge-platform/design/) + [`knowledge-platform/sketches/`](./knowledge-platform/sketches/) (`00`–`07`) |
| **Chat** | [`chat/README.md`](./chat/README.md) | [`chat/architecture/`](./chat/architecture/) (`00`–`07`) | [`chat/design/`](./chat/design/) + [`chat/sketches/`](./chat/sketches/) (`00`–`10`) |

Each `architecture/` folder follows the same eight-doc split: `00` overview/role · `01` tech & data model ·
`02` internals & algorithms · `03` events, contracts & glue · `04` views, CLI & API · `05` hard problems ·
`06` shared-system change requests · `07` drills & open questions. Knowledge-platform adds a ninth,
`08-committed-resolutions.md` (its nine Stage-1 open questions now committed, CR-A…CR-I).

---

## Language / DB choices (and divergence from the Rust default)

The platform default is **Rust over PostgreSQL** (ADR-02 / ADR-01: thin shells over the substrate, one DB
per service, no cross-DB reads). **No subsystem requested a language divergence from Rust.** The only
*written-but-disfavoured* hatch is Chat's connection-tier gateway; the only *by-constraint* build is CI's
EU fleet autoscaler. Everything is EU-deployable / self-hostable.

| Subsystem | Language | Primary store(s) | Divergence from Rust + justification |
|---|---|---|---|
| **Git Hosting** | **Rust** end-to-end | PostgreSQL (OLTP, the outbox-in-one-tx) + the **object tier** (`BlobStore`: content-addressed packs/LFS/bundles); **reftable-on-OLTP** ref store | **None.** Layered **`GitCore`** seam (TE-8, web-verified): sandboxed canonical `git` serves the **wire + maintenance** in v1 (`gix` has **no** server-side `upload-pack`/`receive-pack`), with **`gix` preferred / `libgit2` fallback in-process** for read/diff/blame/projection; ops migrate gix-ward per-op behind the seam. No language boundary → simplest cross-language posture. (Mononoke proves Rust git-server feasibility.) |
| **Continuous Integration** | **Rust throughout** | Delegated to Phase-3 tiers: OLTP + `BlobStore` (T2) + log tier (T3) + OLAP | **None justified** (every component is a latency/correctness hot path or a trust boundary). One **by-constraint** build: CI builds the **EU fleet autoscaler** itself because ADR-11 forbids hyperscaler autoscaling. Sandbox = Firecracker microVM (default) / gVisor (named second) behind `SandboxBackend`. |
| **Issue Tracker** | **Rust** | **PostgreSQL** — typed-core columns + JSONB tail + a derived projection feeder (no per-tenant DDL, no JQL trap); OLAP read store (CQRS) for analytics | **None.** PG hybrid sharded by tenant is the floor; distributed-SQL is a *measured* follow-on, never premature. |
| **Knowledge Platform** | **Rust** services + the editor as a Rust `myelin-content` core compiled to **WASM** | **PostgreSQL** OLTP (block tree + rows + op-log) + **S3-compatible object store** (media + CRDT snapshots) + **Yrs** (Rust Yjs) as the eventual CRDT; Tantivy via shared Search | **None.** WASM is a compile *target* for the shared content crate (one editor render path), not a language divergence. |
| **Chat** | **Rust everywhere** incl. the gateway (the TE-21 call) | **PostgreSQL**-partitioned message store behind a `MessageStore` trait (**ScyllaDB** = measured floor) + object-store cold segments + **Valkey** (read-state/presence/unfurl cache) + **NATS core** (live delivery + presence) | **None committed.** A **BEAM/Phoenix** escape hatch for the connection-tier gateway is **written-but-closed** (TE-21): admissible only if D-C3/D-C4 (presence/tail-latency at scale) fail in Rust; the wire-envelope is Rust either way, so the hatch is a gateway-process swap, not a platform rewrite. |

**The one divergence-shaped item to watch in Phase 5:** Chat's BEAM hatch (gated on a drill, kept real by
CHG-C1's cross-language harness shim) and CI's EU autoscaler (a build forced by ADR-11, not a language
choice). Neither reverses the Rust default.

---

## Headline resolved hard-problems (per subsystem)

| Subsystem | The headline resolutions |
|---|---|
| **Git Hosting** | **Replication = the DB ref-store transaction IS the linearisation point** (Postgres is the Praefect; "outbox order == ref-update order by construction" — *not* a bespoke per-repo consensus group) + content-addressed packs made durable by a primary + quorum-ack WAL replica set (TE-24, the linearizable protected-ref merge point); **layered `GitCore`** — canonical `git` serves the wire/maintenance, `gix`/`libgit2` in-process for reads (TE-8); **SHA-1 + `sha1dc` default / SHA-256 opt-in per repo, hash-agnostic model** (TE-23; flipping the default is named floor GF-2b); **diff-anchoring** = content-anchor + blob-diff remap + `outdated` fallback, never silently wrong (TE-22); **forks** = shared object pool per network, residency-safe (cross-tenant forks copy) (TE-26); **code search** = git emits a symbol/path/literal/trigram projection, Search owns the index (TE-27); **erasure** = pseudonymous-commit-by-default (GIT-1) makes erasure usually a pseudonym-map delete, history-rewrite is the disruptive path, immutable residual is `[OPEN — LEGAL]`. |
| **Continuous Integration** | **T-1 the real-kernel escape drill gates everything** that runs untrusted code; **isolation** = Firecracker microVM default + mandatory hardening (HP-1); **scheduler** = pull-leasing + DRR fair-share + priority lanes + concurrency groups (HP-2); **pipeline = a `myelin-flow` durable workflow**, the seam is the `SCHEDULE_AND_RUN_JOB` activity; **UNIFY** (HP-5) — `ToolHands::exec` *is* CI's runner, untrusted execution built+drilled once; **metering** = resource-seconds wholesale → credits markup; **supply-chain** = digest-pin-or-fail-closed + sigstore + SLSA/SBOM; **logs** ride the firehose, only `ci.log.available` pointers on the durable bus. |
| **Issue Tracker** | **Board and roadmap are co-equal `myelin-query` AST views over one `issue` table** — they structurally cannot drift; **three independent axes** (containment / time / org-scope) never collapsed into one tree; **flexible-field query** = typed core + JSONB tail + cost-bounded planner + Search escalation valve (no JQL trap); **governance** = layered schemes interpreted as config (Linear-simple = empty; Jira-powerful = more schemes; one product, no fork); **keys** = Hi/Lo, **rank** = LexoRank + CAS; **SLA** = business-calendar arithmetic over the SC-11 timer wheel; **stateful trigger** ("remind me when unblocked") fires-once-after-restart. |
| **Knowledge Platform** | **Collaboration ladder** = resume-cursor durable transport built FIRST (KN-1, reconnect-loses-zero-ops is its drill) → per-block CAS floor (no silent overwrite) → Yrs CRDT promoted on the first true concurrent conflict; **block tree** = adjacency list + fractional ordering key, inline = markdown-subset + structured mention/ref/embed nodes; **one editor render path** `render(parse(md)) === md` as a hard gate (KN-4); **databases** = JSONB bag + derived projection, **formulas/rollups read-time, never stored** (KN-3); **permissions** = page-tree inheritance-with-overrides compiled to ReBAC tuples; **erasure** = per-subject crypto-shred reaching the immutable op-log. |
| **Chat** | **Connection tier** = Rust gateway + NATS-core backplane + **resume-cursor resync** that makes the backplane allowed-to-drop (zero-loss-across-reconnect); **message store** = per-conversation append log, body is per-subject-DEK-encrypted (the body *is* the PII), PG-partitioned hot + object cold behind `MessageStore` (Scylla floor); **read-state** = Valkey hot + PG durable, eventually-consistent, never authoritative in cache; **cheap per-viewer unfurls** = shared-per-ref projection cache gated by a per-viewer `check`, lazy-on-viewport, calling Refs `resolve` (never re-implements permission-aware resolution); **HITL bridge** = `Id.check(approve)` → `DurableExecutor::signal(idem=card_id)`; **Activity/Mentions is a view into the one Notif inbox** (C-9); **explicit-first agent dispatch** (CHAT-1 — a mention notifies, never auto-spawns a costed run). |

---

## Cross-references

- **Primary Phase-5 input:** [`cross-subsystem-change-requests.md`](./cross-subsystem-change-requests.md)
  — the consolidated, de-duplicated shared-system change requests, conflicts, and open questions.
- Phase-3 shared layer (the build-to surface): [`../03-shared-systems-architecture/contract-index.md`](../03-shared-systems-architecture/contract-index.md)
  + [`../03-shared-systems-architecture/README.md`](../03-shared-systems-architecture/README.md) §5 (the handoff).
- Phase-2b binding directives: [`../02b-doctrine-integration/integration-directives.md`](../02b-doctrine-integration/integration-directives.md).
- Phase-2 holistic architecture: [`../02-holistic-architecture/`](../02-holistic-architecture/).
