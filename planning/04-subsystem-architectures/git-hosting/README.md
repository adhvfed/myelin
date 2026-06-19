# Git Hosting & Code Review — Subsystem Architecture (index)

> Phase: `04-subsystem-architectures/git-hosting`. Canonical brief:
> [`VISION.md`](../../../VISION.md). Build-to surface: the Phase-3 contracts
> ([`contract-index.md`](../../03-shared-systems-architecture/contract-index.md)). Date: 2026-06-19.
>
> Git hosting is the **system of record for source code** and the gravitational centre of Myelin's
> engineering side: EU-sovereign, GDPR-by-construction, world-scale, agent-native, on the shared
> identity/permission/event/reference fabric. The differentiator is **not** the git server — it is that
> this one sits on that fabric and treats agents as first-class, legible, bounded actors.
>
> This subsystem is a **two-stage** deliverable: Stage-1 (exploration `sketches/` + design `design/`)
> committed the direction; Stage-2 (`architecture/`) is the final detailed architecture built on it.

---

## Document index

### `sketches/` — Stage-1 exploration (the committed direction)

[`00-findings.md`](./sketches/00-findings.md) synthesises the committed per-hard-problem direction and
hands forward the open questions; the nine notes
([`01-storage-replication-backend`](./sketches/01-storage-replication-backend.md) …
[`09-erasure-pseudonymity-history-rewrite`](./sketches/09-erasure-pseudonymity-history-rewrite.md)) are
the exploration that grounds it. Two TE-8/reftable calls were **re-verified against the live web** here;
Stage-2 implements the verified position.

### `design/` — Stage-1 design (IA / flows / wireframes)

[`design/README.md`](./design/README.md) indexes:
[`information-architecture.md`](./design/information-architecture.md) (the one-shell IA + the wedge
substrate), [`user-flows.md`](./design/user-flows.md) (the eight key flows incl. agent/HITL +
cross-subsystem), and [`wireframes.md`](./design/wireframes.md) (eight primary screens, each with
happy/empty/loading/error/permission/erased/agent-pending states). This satisfies VISION §3 at structural
fidelity; the visual/token pass is the named follow-on (OQ-12).

### `architecture/` — the final detailed architecture (Stage 2)

| Doc | Covers |
|---|---|
| [`00-overview.md`](./architecture/00-overview.md) | Role; owns-vs-delegates; the four-tier component map; the **floors register** (GF-1…GF-9, GF-2b); inherited non-negotiables. |
| [`01-tech-and-data-model.md`](./architecture/01-tech-and-data-model.md) | **Language/DB choice + justification** (Rust + Postgres + object tier; no divergence); the **git-core embed decision** (TE-8: layered `GitCore` — canonical `git` for the wire + maintenance, `gix`/`libgit2` in-process for reads); the **SHA decision** (TE-23: SHA-1+`sha1dc` default / SHA-256 opt-in, hash-agnostic model); the full data model (git object tier + reftable-on-OLTP + hosting OLTP); the personal-data inventory. |
| [`02-internals-and-algorithms.md`](./architecture/02-internals-and-algorithms.md) | Smart-transport (protocol-v2/partial-clone/bundle-URI, canonical `git`); the **sandboxed `receive-pack` + in-process Rust push-policy engine**; the ref store CAS; **replication = the DB ref-store transaction as the linearisation point + primary/quorum-WAL pack replicas** (TE-24); GC/repack; **diff-anchoring** (TE-22); the **merge gate + merge queue**; **forks** (TE-26); **monorepo** serving (TE-25); the **code projection** (TE-27); scaling/hot-spots. |
| [`03-events-contracts-and-glue.md`](./architecture/03-events-contracts-and-glue.md) | The complete **`git.*` taxonomy** (owned) + consumed events; **every glue contract**: ArtifactRef + `#sub`, `project`, `replay`, the outbox envelope, Identity `check`/`list_objects` + the **ReBAC namespace fragment**, `PersonalDataHolder` (+ restriction flag), `ToolDef`s, reserve/settle. |
| [`04-views-cli-and-api.md`](./architecture/04-views-cli-and-api.md) | The **views** (IA + flows + empty/loading/error + agent-aware/per-viewer states, consuming `design/`); the two **CLI** surfaces (plain git + `myelin`); the HTTP/RPC + **agent-tool** API. |
| [`05-hard-problems.md`](./architecture/05-hard-problems.md) | Each **hard problem resolved with cited prior art** (HP-1…HP-10) + named floors. |
| [`06-shared-system-change-requests.md`](./architecture/06-shared-system-change-requests.md) | The **itemized Phase-5 reconciliation list** (Id/Bus/Refs/Search/Storage/Workflow/Agents/GDPR/Tenancy). |
| [`07-drills-and-open-questions.md`](./architecture/07-drills-and-open-questions.md) | The **quantified drills owed** (D-1…D-9) + **open questions** (OQ-1…OQ-12) + honesty notes. |

---

## The headline decisions (one-line each — the Stage-1-committed, Stage-2-built direction)

- **Language/DB:** **Rust** end-to-end (ADR-02 default; Mononoke proves Rust git-server feasibility) +
  **PostgreSQL** (OLTP, the outbox-in-one-transaction property) + the **object tier** (`BlobStore`,
  content-addressed packs/LFS/bundles). No divergence. No language boundary → simplest X-5 position.
- **Git core (TE-8, web-verified):** a layered **`GitCore`** strategy — **canonical `git` (sandboxed,
  streamed) serves the wire + maintenance** in v1 (`gix` has **no** server-side `upload-pack`/
  `receive-pack`, re-verified 2026-06), with **`gix` preferred / `libgit2` fallback in-process** for
  read/diff/blame/projection; ops migrate gix-ward per-op behind the seam.
- **SHA (TE-23):** **hash-agnostic** model; **SHA-1 + `sha1dc` default**, **SHA-256 opt-in per repo**
  (ecosystem/stock-client reality); flipping the default to SHA-256 is a named floor (GF-2b).
- **Replication (TE-24):** the **DB ref-store transaction is the linearisation point** (Postgres is the
  Praefect; "outbox order == ref-update order by construction", BUS-2) — **not** a bespoke per-repo
  consensus group — with **content-addressed packs made durable by a primary + quorum-ack WAL replica
  set** (consistency and durability decoupled). The DB ref index is the recovery tiebreaker.
- **Diff-anchoring (TE-22):** content anchor + blob-diff remap + **`outdated` fallback** (never silently
  wrong).
- **Forks (TE-26):** shared object pool per network (residency-safe; cross-tenant forks copy). Merge
  queue = single-lane durable workflow; web edit = single-file edit (no 3-way conflict editor v1).
- **Code search (TE-27):** git emits a **symbol/path/literal/trigram projection**; Search owns the index.
- **Erasure (GD-1):** **pseudonymous-commit-by-default** (commit-time prerequisite, GIT-1) makes erasure
  usually a pseudonym-map delete; **history-rewrite** is the supported disruptive path; the immutable-
  content residual is **`[OPEN — LEGAL]`**.

---

## Cross-references
- Phase-2 high-level arch: [`02-holistic-architecture/subsystems/git-hosting.md`](../../02-holistic-architecture/subsystems/git-hosting.md).
- Phase-1 deep-dive: [`01-research/subsystem-deep-dives/git-hosting.md`](../../01-research/subsystem-deep-dives/git-hosting.md).
- Phase-3 handoff (this subsystem's obligation list): [`03-shared-systems-architecture/README.md`](../../03-shared-systems-architecture/README.md) §5.
- Binding directives (GIT-1, STOR-5, GD-1): [`02b-doctrine-integration/integration-directives.md`](../../02b-doctrine-integration/integration-directives.md).
