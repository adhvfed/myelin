# Git Hosting & Code Review — Subsystem Architecture (index)

> Phase: `04-subsystem-architectures/git-hosting`. Canonical brief:
> [`VISION.md`](../../../VISION.md). **The `architecture/` folder was rewritten from scratch in Phase 5-B**
> against the RECONCILED, FROZEN shared layer:
> [`05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md)
> + [`00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md)
> (X-1..X-7, OQ-A..OQ-L). Date: 2026-06-19.
>
> Git hosting is the **system of record for source code** and the gravitational centre of Myelin's
> engineering side: EU-sovereign, GDPR-by-construction, world-scale, agent-native, on the shared
> identity/permission/event/reference fabric. The differentiator is **not** the git server — it is that
> this one sits on that fabric and treats agents as first-class, legible, bounded actors.
>
> This subsystem is a **two-stage** deliverable: Stage-1 (exploration `sketches/` + design `design/` —
> **PRESERVED, the design record**) committed the direction; Stage-2 (`architecture/`) is the final
> detailed architecture, now conformed to the frozen Phase-5 contracts. **What changed in the 5-B rewrite is
> listed in [`architecture/00-overview.md` §0.1](./architecture/00-overview.md).**

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
| [`00-overview.md`](./architecture/00-overview.md) | **Changes vs the Phase-4 first pass (§0.1)**; role; owns-vs-delegates; the four-tier component map; the **floors register** (GF-1…GF-9, GF-2b); inherited non-negotiables (incl. the four uniform sandbox guarantees). |
| [`01-tech-and-data-model.md`](./architecture/01-tech-and-data-model.md) | **Language/DB (carried forward + confirmed)** (Rust + Postgres + object tier); the **git-core embed decision** (TE-8: layered `GitCore`); the **SHA decision** (TE-23: hash-agnostic, SHA-1+`sha1dc` default / SHA-256 opt-in); the data model now with the **frozen `check_status` projection (X-1)**, the **content-anchor fingerprint (OQ-D)**, **per-subject DEK** bodies, and the CDN clone / trust-scoped cache classes. |
| [`02-internals-and-algorithms.md`](./architecture/02-internals-and-algorithms.md) | Smart-transport; the sandboxed `receive-pack` + in-process Rust policy; the ref-store CAS; **replication** (TE-24); GC/repack; **diff-anchoring as the content-fingerprint 4-state resolver (OQ-D)**; **the merge gate + merge queue implementing the X-1 CheckStatus consumer + `run_attempt` supersession + fork-endorsement + the `ci.result` durable-signal wait**; forks; monorepo; code projection; scaling. |
| [`03-events-contracts-and-glue.md`](./architecture/03-events-contracts-and-glue.md) | The complete **`git.*` taxonomy** + consumed events (incl. **`ci.check.updated`/`ci.result`**); **every glue contract against the FROZEN shapes**: ArtifactRef + the unified `#sub` grammar, `project`, `replay`, the outbox envelope, Identity `check`/`list_objects` **SetExpr push-down** + the frozen ReBAC fragment (`approve_untrusted_ci`), `PersonalDataHolder` + the ONE erasure posture by reference, `ToolDef`s with the **frozen `requires_approval` defaults**, reserve/settle. |
| [`04-views-cli-and-api.md`](./architecture/04-views-cli-and-api.md) | The **views** (IA + flows + states, consuming `design/`; incl. the X-1 fork-trust + checks-panel + merge-queue affordances); the two **CLI** surfaces; the HTTP/RPC + **agent-tool** API. |
| [`05-hard-problems.md`](./architecture/05-hard-problems.md) | Each **hard problem resolved with cited prior art** (HP-0 the X-1 seam … HP-10) + named floors. |
| [`06-reconciliation-compliance.md`](./architecture/06-reconciliation-compliance.md) | **How this subsystem now IMPLEMENTS the frozen reconciled contracts** (CheckStatus, the `#sub` grammar, the `list_objects` Filter, the erasure posture, REF-3, trust-scoped cache, CDN clone, mirror gate) + the **residual requests for Phase 6** (R-1…R-9). |
| [`07-drills-and-open-questions.md`](./architecture/07-drills-and-open-questions.md) | The **quantified drills owed** (D-1…D-11, incl. the X-1 seam drill D-10 + the leak-free `list_objects` drill D-11) + **open questions** (OQ-1…OQ-12) + honesty notes. |

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
- **Diff-anchoring (TE-22 / OQ-D):** content anchor with a **BLAKE3 fingerprint**, resolving through the
  unified 4-state ladder **exact/rebased(moved)/partial(outdated)/tombstone** (never silently wrong).
- **The Git↔CI check seam (X-1, FROZEN contract 5.9):** Git is the **consumer + gate** — it owns the
  `check_status` projection, the **monotonic `run_attempt` supersession**, the branch-protection
  `required`-set policy, and the **fork-endorsement** flow (`untrusted_fork` is neutral for gating until
  endorsed via `approve_untrusted_ci` or re-run trusted). Git **reads `trust_tier` off the fact** and
  **never synchronously calls CI**. The merge queue is a durable workflow waking on the rollup `ci.result`.
- **Forks (TE-26):** shared object pool per network (residency-safe; cross-tenant forks copy). Merge
  queue = single-lane durable workflow; web edit = single-file edit (no 3-way conflict editor v1).
- **Code search (TE-27):** git emits a **symbol/path/literal/trigram projection**; Search owns the index;
  always conjoins the OQ-E `list_objects` `Filter`.
- **Erasure (the ONE platform posture, contract 10.9 / X-7):** **pseudonymous-commit-by-default** (GIT-1)
  makes erasure usually a pseudonym-map delete; per-subject DEK crypto-shred for self-authored bodies;
  **history-rewrite** (an audited op with fork/mirror/clone-cache invalidation) is the disruptive path. The
  residual is **instantiated by reference, not restated**; the Art. 17 reach is **`[OPEN — LEGAL]`**.

---

## Cross-references
- **Frozen build-to surface (Phase 5):** [`05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md)
  + [`00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md).
- Phase-2 high-level arch: [`02-holistic-architecture/subsystems/git-hosting.md`](../../02-holistic-architecture/subsystems/git-hosting.md).
- Phase-1 deep-dive: [`01-research/subsystem-deep-dives/git-hosting.md`](../../01-research/subsystem-deep-dives/git-hosting.md).
- Phase-3 handoff (superseded by Phase 5): [`03-shared-systems-architecture/README.md`](../../03-shared-systems-architecture/README.md) §5.
- Binding directives (GIT-1, STOR-5, GD-1): [`02b-doctrine-integration/integration-directives.md`](../../02b-doctrine-integration/integration-directives.md).
