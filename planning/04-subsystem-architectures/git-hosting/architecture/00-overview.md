# Git Hosting & Code Review — Detailed Architecture: Overview

> Phase: `04-subsystem-architectures/git-hosting/architecture`. Stage 2 of 2 (detailed).
> Canonical brief: [`VISION.md`](../../../../VISION.md) (never contradicted). Builds on the
> Phase-2 high-level arch ([`02-holistic-architecture/subsystems/git-hosting.md`](../../../02-holistic-architecture/subsystems/git-hosting.md))
> and the Phase-1 deep-dive ([`01-research/subsystem-deep-dives/git-hosting.md`](../../../01-research/subsystem-deep-dives/git-hosting.md)).
> Build-to surface: the Phase-3 contracts ([`contract-index.md`](../../../03-shared-systems-architecture/contract-index.md)).
> Binding directives: [`integration-directives.md`](../../../02b-doctrine-integration/integration-directives.md)
> (GIT-1, STOR-5, GD-1, SUB-X, the X-/T-/E- families). Date: 2026-06-19.
>
> **Built on Stage-1.** This Stage-2 architecture builds directly on the committed Stage-1 direction:
> the findings ([`../sketches/00-findings.md`](../sketches/00-findings.md)), the nine exploration notes
> ([`../sketches/01`](../sketches/01-storage-replication-backend.md)…`09`), and the design folder
> ([`../design/`](../design/) — `information-architecture.md`, `user-flows.md`, `wireframes.md`, with
> happy/empty/loading/error/permission/erased/agent-pending states for every primary screen). Where
> Stage-1 re-verified a hard-problem call against the live web (TE-8, reftable), this architecture
> implements the *verified* position; the per-hard-problem committed direction in
> [`00-findings.md`](../sketches/00-findings.md) is the build-to here.

---

## 0. Document map

| Doc | Covers |
|---|---|
| `00-overview.md` (this) | Role, what it owns vs delegates, the component map, the floors register. |
| [`01-tech-and-data-model.md`](./01-tech-and-data-model.md) | Language/DB choice + written justification; the full data model (git object tier + hosting OLTP); git-core embed decision (TE-8); SHA decision (TE-23). |
| [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md) | Smart-transport, receive-pack policy, ref store, replication (TE-24), GC/repack, diff-anchoring (TE-22), merge gate, forks (TE-26), monorepo (TE-25), code projection (TE-27). |
| [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md) | The complete `git.*` taxonomy; every glue contract (ArtifactRef, `project`, `replay`, outbox envelope, Identity `check`/`list_objects` + the ReBAC fragment, `PersonalDataHolder`, `ToolDef`s, reserve/settle). |
| [`04-views-cli-and-api.md`](./04-views-cli-and-api.md) | The views (IA + flows + states), the git wire + Myelin CLI, the HTTP/RPC API, the agent-tool surface. |
| [`05-hard-problems.md`](./05-hard-problems.md) | Each subsystem-specific hard problem resolved, with cited prior art and named floors. |
| [`06-shared-system-change-requests.md`](./06-shared-system-change-requests.md) | The itemized Phase-5 reconciliation list (what each shared system must add beyond Phase-3). |
| [`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md) | The quantified drills owed + open questions for Phase 5. |

---

## 1. Role & responsibilities

Git hosting is the **system of record for source code and its history** and the gravitational centre of
Myelin's engineering side (Phase-1 §1, Phase-2 §1). Its differentiator is **not the git server** —
every competitor has a competent one — but that this one sits on Myelin's unified
identity/permission/event/reference fabric and is **agent-native** (agents are first-class
authors/reviewers, legible and bounded), all **EU-sovereign and GDPR-by-construction**.

### 1.1 What it OWNS (core competency)

- **The git object store + serving core**: blobs/trees/commits/tags, refs, packfiles + delta
  compression, reachability acceleration (commit-graph, reachability bitmaps, multi-pack-index),
  GC/repack, partial-clone/sparse/shallow serving, and the **git wire protocol** (smart-HTTP
  protocol-v2 + SSH).
- **Per-ref ordering at push QPS** — the aggregate for `git.ref.updated` is the **ref**, not the repo
  (Bus §2.3); the ref-update transaction is the linearisation point.
- **Hosting-layer domain entities** not in git itself: Repository (visibility/default-branch/tenant
  binding), Fork/network, the **Pull Request** lifecycle, **Reviews + inline comment threads** (with
  diff-anchoring), **Branch-protection rulesets**, CODEOWNERS, deploy-key bindings, commit-status/check
  aggregation.
- **The merge gate** — *the place "what is allowed to land" is decided*: branch-protection evaluation,
  required-review/required-check enforcement, the merge queue, and the agent-vs-human merge policy.
- **The indexable CODE PROJECTION** (path/symbols/literals/commit-message per blob/ref) emitted for
  Search code-search v1 (Search §4.4; contract-index 6.5) — git hosting owns *what* to index.
- **`git`-namespace `ArtifactRef`s** down to sub-artifact granularity, and the complete `git.*` event
  taxonomy under the Bus §6 grammar.
- **Its erasure obligations** as a `PersonalDataHolder` — the hardest in the platform — including being
  the engineering co-owner of the **GD-1 git-history-erasure reconciliation** and **pseudonymous-commit
  by default (GIT-1)**, which is a *commit-time prerequisite* that gates this very data model.
- **The pack tier kept object-backable/relocatable** (STOR-5): repos are never node-pinned.

### 1.2 What it DELEGATES to shared systems (ADR-13; no cross-DB, ADR-01)

| Concern | Delegated to | Git hosting still owns |
|---|---|---|
| Who a principal is; SSH-key/token/OAuth auth; org/team model; user lifecycle; **the pseudonym map** | **Identity** (`authenticate`/`check`/`list_objects`/`resolve_pseudonym`/`erase`) | git-specific authz decisions as a ReBAC namespace fragment; the SSH/HTTPS front door |
| Emitting/consuming events | **Event Bus** (`OutboxTx::emit`) | the receive-pack → outbox path; per-ref ordering |
| Commit↔issue↔doc↔run edges, backlinks, unfurls | **Reference Graph** (`ArtifactRef`/`resolve`/`backlinks`) | producing edges from trailers/PR links; the `project` API; sub-artifact `#sub` minting |
| Code/PR/comment index + query | **Search** (`query`/`declare_indexable`) | the code projection + incremental update on push |
| Durable bytes (LFS, packs-as-blobs, bundles, backups) | **Storage** (`BlobStore`, KMS) | the LFS batch protocol, pack/delta management, residency tags |
| Notification delivery | **Notifications** (`humanise`/inbox) | which events are notifiable + targets (via Signals) |
| Agent authors/reviewers; trigger dispatch; plan-then-apply | **Agent Fabric** (`ToolSurface`/`EffectApi`) | its `ToolDef`s + the events that drive triggers |
| Long-running / human-gated flows (auto-merge-when-green, HITL gate, merge-queue waits) | **Durable Workflow** (`DurableExecutor`/signals/timers) | the merge-queue state-machine semantics |
| DSR fan-out, KMS/crypto-shred, the tamper-evident audit log | **GDPR/Audit** (`PersonalDataHolder` orchestration) | implementing `locate/export/rectify/restrict/erase` over git+metadata; the history-rewrite path |
| Cell placement, region-pinning, discovery | **Tenancy/control plane** (`discover`/`place`/`residency_verify`) | repo→cell placement honoring residency; rejecting any route that leaves region |

**Hard rule (ADR-01/13):** no subsystem reads git hosting's DB; git hosting reads no other subsystem's
DB. All cross-subsystem reads go through `ArtifactRef` resolution + the owning subsystem's `project`
API, permission-filtered per viewer.

---

## 2. Internal component architecture

The subsystem keeps the Phase-2 four-tier shape — a **stateless front door**, a **stateful serving
tier**, a **metadata control plane**, and an **async event/index path** — now with the Phase-3 substrate
(`serve(AppSpec)`, the outbox, the three-surface topology, the resilient client) wired in. Every box is
a thin shell over `myelin-substrate` (ADR-01).

```
            ┌──────────────────────── CLIENTS ────────────────────────┐
            │  git wire (SSH / smart-HTTP v2) · Web UI · myelin CLI ·  │
            │  internal RPC · MCP (external agents, later)             │
            └───────────────┬─────────────────────────────────────────┘
                            ▼
 ┌──────────────────────────────────────────────────────────────────────────┐
 │ (A) GIT FRONT DOOR / ROUTER  (stateless, per-cell, region-pinned)         │
 │   Id.authenticate → Principal · Id.check per-action gate · tenancy.place  │
 │   → backend node(s) · residency reject-if-leaving-region · streams packs  │
 │   (no full buffering) · ADR-16 protected-human-lane shed · SSH + smart-   │
 │   HTTP-v2 endpoints · liveness≠readiness                                   │
 └───────────────┬───────────────────────────────────┬──────────────────────┘
                 ▼ (git transactions)                 ▼ (PR/review/API/UI/RPC)
 ┌───────────────────────────────────┐  ┌─────────────────────────────────────┐
 │ (B) REPO SERVING TIER (stateful)  │  │ (C) HOSTING CONTROL PLANE (OLTP)    │
 │  git-core engine: upload-pack /   │  │  PR / review / comment-thread /     │
 │  receive-pack · pack/delta · GC/  │  │  repo / fork / ruleset / merge-queue│
 │  repack · commit-graph + bitmaps  │  │  rows · branch-protection evaluator │
 │  + MIDX · reftable-on-OLTP ref    │  │  · CODEOWNERS resolver · DIFF-ANCHOR│
 │  store · partial-clone / sparse   │  │  service · check/status aggregator  │
 │  · IN-PROCESS receive-pack policy │  │  · CODE-PROJECTION emitter · the    │
 │  engine (pre-receive) + outbox    │  │  project(ref,viewer) API · outbox   │
 │  emit (post-receive, same tx)     │  │  emit                               │
 └───────┬───────────────┬───────────┘  └───────┬───────────────────┬─────────┘
         │ packs/LFS/     │ outbox events        │ outbox events     │ reads
         ▼ bundles (Blob) ▼                      ▼                   │ (project/
 ┌──────────────┐   ┌────────────────────────────────────────┐       │  resolve)
 │ STORAGE      │   │        EVENT BUS (JetStream-class)      │◄──────┘
 │ BlobStore +  │   │  git.ref.updated · git.pr.* · git.review│
 │ KMS (T2)     │   │  .* (envelope + outbox + per-ref order) │
 └──────────────┘   └──┬──────┬──────┬──────┬──────┬──────────┘
                       ▼      ▼      ▼      ▼      ▼
                     REFS  SEARCH  AGENTS  NOTIF  CI / OLAP / AUDIT
```

- **(A) Front door.** SSH + smart-HTTP-v2 endpoints. Authenticates a `Principal` via Id (SSH pubkey,
  PAT, or OAuth/device token), runs the per-action `Id.check`, resolves `repo_id → cell + backend
  node(s)` via the control plane, **rejects any route that would leave the region** (ADR-11), and
  *streams* the transaction to the serving tier without buffering whole packs. Rate-limiting and the
  protected-human-lane shed order (ADR-16: speculative→CI→agent→human-last) live here. Stateless;
  scales horizontally; liveness must not check deps, readiness gates on backend reachability.
- **(B) Serving tier (stateful).** The git core via the layered **`GitCore`** seam: **wire serving
  (`upload-pack`/`receive-pack`/`ls-refs`) + maintenance run as sandboxed canonical `git`** in v1 (the
  Stage-1-web-verified TE-8 position — `gix` has no server-side serving; `01 §2`), while **`gix` (libgit2
  fallback) runs in-process for read/diff/blame + the code projection**. Pack/delta storage, reachability
  acceleration, GC/repack, the **reftable-on-OLTP ref store**, partial-clone/sparse serving all live here.
  **Push policy runs in-process in Rust** (not as shell `pre-receive` hooks): the sandboxed `git
  receive-pack` ingests the pack into a quarantine, *our* Rust evaluates branch protection / secret-scan /
  size / agent / pseudonymity rules — *reject before the ref moves* — and *our* code does the ref CAS +
  the outbox insert **in the same DB transaction** (BUS-2; the DB CAS is the linearisation point, not a
  native `post-receive` emit). This tier's scale hot-spots are repo placement, the ref store, and
  replication (`02 §3-4`).
- **(C) Control plane (OLTP).** Postgres for everything that is not a git object: PR/review/comment/
  repo/fork/ruleset/merge-queue rows, the branch-protection evaluator, CODEOWNERS resolver, the
  **diff-anchor service** (the hard correctness battleground), the check/status aggregator (consumes
  CI), the **code-projection emitter**, and the **`project(ref, viewer)`** API. One DB per service, RLS
  tenant-scoped, per-tenant envelope-encrypted, forward-only migrations, auto-registered as a
  `PersonalDataHolder`.
- **(D) Async path.** Off the bus: Search code-projection indexing, Refs edge creation, Notification
  routing, the OLAP feed, the agent trigger/dispatch tier, the audit consumer. All idempotent on
  `event_id`. Never in the synchronous push/PR write path.

**Two-transport discipline (ADR-04).** Durable control/domain events (`git.ref.updated`, `git.pr.*`,
`git.review.*`) ride the durable bus. Git hosting has no per-line firehose of its own (unlike CI logs or
chat presence); its high-volume path is **streaming clone/fetch byte transfer**, which stays on the git
wire/object tier and **never** touches the durable bus.

---

## 3. The floors register (named partials + their follow-ons — VISION §3, E-3)

These are carried into the gap report; each is dated 2026-06-19 and named with its follow-on owner.

| # | Floor (what v1 ships) | Follow-on | Owner |
|---|---|---|---|
| GF-1 | **Object-backed packs** — v1 runs packs on local NVMe behind the `BlobStore` trait; repos are **relocatable, never node-pinned** (STOR-5 constraint DECIDED). | Object-store-backed pack/delta management + smart-transport over `BlobStore` (TE-24). | Git P4 / Storage |
| GF-2 | **Replication** — the **DB ref-store transaction is the linearisation point** (per-ref CAS); pack durability via a **primary + quorum-ack WAL-streamed replica set** (no bespoke per-repo consensus group). Single-cell in v1. | Cross-cell active replica sets; geo read-replicas within-EU; object-store-backed pack relocation (with GF-1). | Git P4 / control plane |
| GF-2b | **SHA-256** — default new repos are **SHA-1 + `sha1dc`** (ecosystem/stock-client reality); SHA-256 is **opt-in per repo**; the data model is **hash-agnostic**. | Flip the default to SHA-256 once the client/tooling ecosystem matures (post-Git-3.0); audited `migrate --to sha256`. | Git P4 (measured) |
| GF-3 | **Code search** — symbol/path/literal/trigram-grade projection (Search §4.4). | AST-aware / cross-reference / "find usages" semantic code search (SCIP from CI). | Git P4 + Search + CI |
| GF-4 | **Monorepo** — large-but-normal monorepos via partial-clone/sparse/commit-graph/bitmaps; **not** a Mononoke-class virtual FS. | A Mononoke-class scalable backend if a tenant exceeds the benchmarked ceiling (`05 §HP-3`). | Git P4 (measured) |
| GF-5 | **Diff-anchoring** — interval-tree blob-diff remap with `outdated` fallback (TE-22). | Rebase-aware "changes since you last reviewed" position carry-over via patch-id chains. | Git P4 |
| GF-6 | **In-UI conflict resolution / web editing** — view + single-file web edit + commit; **no** 3-way merge conflict editor in v1. | In-browser conflict resolution for simple cases (TE-26). | Git P4 (measured) |
| GF-7 | **Git-history author/email erasure** — pseudonymous-commit-by-default (GIT-1) makes erasure usually a pseudonym-map delete; history-rewrite is the supported disruptive path for PII-in-content. | The Art. 17 reach into immutable commit bytes is `[OPEN — LEGAL]` (GD-1/L-2). | Git P4 + Legal/DPO |
| GF-8 | **Merge queue** — single-lane serialised merge queue in v1 (correctness first). | Speculative/parallel batched merge-queue (GitHub-merge-queue-class). | Git P4 (measured) |
| GF-9 | **External MCP** — `exposed_over_mcp` flags set; external endpoint deferred to the platform's shared MCP work. | Platform MCP server + threat model. | P4/P6 + Legal |

---

## 4. The non-negotiables this subsystem inherits (substrate, never re-litigated)

1. `(tenant, region)` is the first column / partition key of every table and every git object placement;
   tenant comes from the verified token, never the URL (ID-3).
2. Every store is residency-pinned, per-tenant envelope-encrypted, crypto-shred-capable, and a
   `PersonalDataHolder` (auto-registered by `serve`).
3. No cross-DB reads (`no-cross-db` lint); interaction via contracts only.
4. The transactional **outbox is the only emit path** (`no-raw-publish` lint); no fire-and-forget.
5. Causality is nested + derived correct-by-construction (`correlation_id`/`causation_id`/`depth`).
6. Reindex-from-source is the only recovery path for the derived stores fed by git (Search code index,
   Refs edges) — `replay(scope, since)` re-emits `git.*.snapshot`.
7. The three-surface topology (public gateway / internal RPC / metrics-health); public↔internal is a
   security boundary; liveness ≠ readiness.

See [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md) for the concrete
implementation of every glue contract.
