# Phase 6 — Roadmaps: the index & reconciled timeline

> Phase: `06-roadmaps`. **The single entry point** to the Myelin build sequence: it indexes the keystone
> master sequencing and all 16 per-system roadmaps (now complete), then reconciles them into one consolidated
> timeline, one critical path, one ordered set of drill gates, the cross-system sequencing dependencies, and
> the handoff to Phase 7.
> Canonical brief: [`VISION.md`](../../VISION.md) §6 (a roadmap is milestones with the work, the
> floor-then-full progression, the dependencies, and the quantified gates/drills that call a milestone done) —
> never contradicted. Binding doctrine:
> [`external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md)
> (§2 order-by-non-negotiability; §3 prove-it-or-it-isn't-real + the failure-injection harness; §5 the
> ratchet / committed gates; §1 code-wins-over-docs + name-your-floors) and
> [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md)
> (erasure-vs-immutability, CRDT-after-CAS, world-scale git, untrusted-code-execution, reindex-from-source).
> Frozen architecture (this index SEQUENCES + RECONCILES, it does not redesign):
> [`../05-refined-shared-systems-architecture/contract-index.md`](../05-refined-shared-systems-architecture/contract-index.md)
> + [`../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../05-refined-shared-systems-architecture/00-reconciliation-decisions.md)
> (X-1..X-7, OQ-A..OQ-L) + the 11 refined shared docs + the 5 rewritten subsystem `architecture/` folders.
> Testing strategy:
> [`../05-refined-shared-systems-architecture/testing-strategy/README.md`](../05-refined-shared-systems-architecture/testing-strategy/README.md)
> (the must-be-early gates §5) +
> [`../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> (the 178 proof obligations). Spine:
> [`../02-holistic-architecture/architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md)
> (ADR-01..ADR-20). Date: 2026-06-19.

---

## 1. The frame — what Phase 6 produced and how to read it

Phase 6's job (VISION §6) is to **sequence the build** — not to redesign it. The architecture is frozen
(Phase 5). What Phase 6 adds is the **order-of-operations**: which work happens in which band, what must exist
before it can start, which named floor ships first and when its full answer follows, and which quantified
drills must be green to call a milestone done. World-scale means world-scale: the hard problems (EI-04) are
scheduled *in their place*, never wished away.

There are three layers of document, and they nest:

1. **The keystone** — [`00-master-sequencing.md`](./00-master-sequencing.md). The global build sequence: the
   ordering thesis (order-by-non-negotiability, Tiers 0–6), the seven milestone bands **M0..M6**, the critical
   path, the dependency DAG across all 16 systems, the drill gates that bound each band, and the
   name-your-floors schedule. **Every per-system roadmap slots into its bands and must not contradict the band
   ordering or the gate invariant.**

2. **The per-system roadmaps** — one per system, all 16 now written (§2 index). Each refines the work *inside*
   each band for one system: the contracts it owns/consumes mapped to bands, its floor-then-full progressions,
   its upstream dependencies, and the specific drills that gate its slice. Each uses a **system-prefix + band**
   milestone naming (e.g. SUB-M0, B-M2, S-M2, ID-M1, GA-M1, CP-M1, R-M2, N-M2.0, FLOW-M2.1, KN-M3a, M4-C1);
   the convention is the same everywhere — a milestone names its band, its work, its floors, its upstreams, and
   its gate.

3. **This index** — the reconciliation layer. It places every system's milestones into the master bands (§3),
   states the one critical path across all 16 systems (§4), enumerates the cross-system sequencing dependencies
   (§5), gives the ordered drill-gate sequence that bounds the whole build (§6), flags any sequencing conflict
   between two roadmaps and recommends a resolution (§7), and hands off to Phase 7 (§8).

**The gate invariant binds everything (EI-01 §2, R-2):** no later milestone is done over a red earlier gate.
A band boundary is a *gate*, not a calendar date — a band is done when its drills emit dated green artifacts
(PROVEN, not CLAIMED), never when the code "looks done." Identifiers are plain text (no backticks-as-emphasis);
Markdown only; no commits.

**The one structural pattern every roadmap follows (read this before the tables).** A system's *core build
band* is where its engine lands, but almost every system **contributes earlier and accretes later**:
- **M0 contributions** — its committed lint(s) with red+green fixtures, and its glue-crate skeleton (the
  contract carrier that makes a contract change break every consumer's build *now*).
- **M1 contributions** — its `PersonalDataHolder` registration (so the H1–H18 list is exhaustive before any
  real data) and its key-class reservation in the KMS hierarchy.
- **core band** — the engine.
- **later bands** — producer-fed corpora / per-subsystem fragments / floor follow-ons light up as their
  producers ship.

This is the *lint-before-the-path-it-guards* + *exhaustive-holder-before-data* + *freeze-before-consume*
discipline, not an early build. It is why Search/Refs/Notif (core M2) are *named* in M0/M1, why GDPR's spine
(M1) is *completed* across M1–M5 as holders come online, and why Knowledge (core M3) *freezes* `myelin-content`
in M2 and CI (core M4) *co-builds the sandbox runner* in M2. §7 reconciles the four tensions this pattern
creates; none moves a band.

---

## 2. The index — the master sequencing + all 16 per-system roadmaps

The 16 systems: **11 shared systems** (the substrate + the shared crates that every subsystem projects onto)
and **5 subsystems** (the product surfaces). Per-system roadmaps live under `shared/<system>.md` and
`subsystems/<system>.md`. **All are complete** as of 2026-06-19.

### 2.0 The keystone

- [`00-master-sequencing.md`](./00-master-sequencing.md) — **read this first.** The global band order, the
  critical path, the DAG, the gate invariant, the name-your-floors schedule. The spine every roadmap below
  refines.

### 2.1 The 11 shared-system roadmaps (`shared/`)

| # | System | Roadmap | Core band | Also lands in | Architecture |
|---|---|---|---|---|---|
| 1 | Platform substrate (`serve`, ports, resilient client, fail-static, lints, **harness**) | [`shared/00-platform-substrate.md`](./shared/00-platform-substrate.md) | **M0** | M1 (fail-static/restore half), M2 (firehose backpressure half), M4 (cross-lang shim), M5 (surge/migration) | [arch](../05-refined-shared-systems-architecture/00-platform-substrate.md) |
| 2 | Event bus + outbox + consumer template + signals/firehose | [`shared/event-bus.md`](./shared/event-bus.md) | **M0** (outbox) | M1 (crypto-shred holder), **M2** (signals/firehose/dispatch/check-seam carriage), M3/M4 (tokens + seam live), M5 (cross-cell + column-store) | [arch](../05-refined-shared-systems-architecture/event-bus.md) |
| 3 | Identity & access (`authenticate`/`check`/`list_objects`, ReBAC, fail-static) | [`shared/identity-and-access.md`](./shared/identity-and-access.md) | **M1** | M0 (4 lints + crate), M2 (first consumption), M3/M4 (ReBAC fragments), M5 (multi-cell authority) | [arch](../05-refined-shared-systems-architecture/identity-and-access.md) |
| 4 | Storage (OLTP/blob, KMS hierarchy, restore-verify, reserve/settle) | [`shared/storage.md`](./shared/storage.md) | **M1** | M0 (outbox table + fs-blob + 2 lints), M2 (OLAP fed), M3 (git packs/CDN), M4 (CI log tier), M5 (object-store/object-packs/multi-cell) | [arch](../05-refined-shared-systems-architecture/storage.md) |
| 5 | Tenancy & control plane (`(tenant,region)` partition, residency, cells) | [`shared/tenancy-and-control-plane.md`](./shared/tenancy-and-control-plane.md) | **M1** (single-cell) | M0 (2 lints + crate), M3 (repo-grain + mirror gate), M4 (CI no-global-pool attest), **M5** (multi-cell + live migration) | [arch](../05-refined-shared-systems-architecture/tenancy-and-control-plane.md) |
| 6 | GDPR / audit / `PersonalDataHolder` (data map, DSR, crypto-shred) | [`shared/gdpr-and-audit.md`](./shared/gdpr-and-audit.md) | **M1** (spine) | M0 (lint + holder hook), M2 (deadline timer + restriction), M3/M4 (holders light up), **M5** (full fan-out + GA-10/GA-11) | [arch](../05-refined-shared-systems-architecture/gdpr-and-audit.md) |
| 7 | Reference graph (`ArtifactRef`, `resolve`/`project`, `#sub`, tombstones) | [`shared/reference-graph.md`](./shared/reference-graph.md) | **M2** | M0 (value type + 4 lints), M1 (holder), M3/M4 (producer edges), M5 (hot-fanout/cross-cell) | [arch](../05-refined-shared-systems-architecture/reference-graph.md) |
| 8 | Search & indexing (leak-free pre-filter, reindex-from-source) | [`shared/search-and-indexing.md`](./shared/search-and-indexing.md) | **M2** | M0 (lint), M1 (holder + index DEK), M3/M4 (producer corpora), M5 (surge/filtered-ANN/federated) | [arch](../05-refined-shared-systems-architecture/search-and-indexing.md) |
| 9 | Notifications (the one inbox, `humanise`, escalation) | [`shared/notifications.md`](./shared/notifications.md) | **M2** | M3/M4 (per-subsystem notify-reasons accrete), M5 (surge + cross-cell) | [arch](../05-refined-shared-systems-architecture/notifications.md) |
| 10 | Agent fabric (`EffectApi` plan-then-apply, `ToolHands::exec` sandbox) | [`shared/agent-fabric.md`](./shared/agent-fabric.md) | **M2** (incl. **AG-D4 GATE**) | M3/M4 (per-subsystem `ToolDef`s), M5 (surge + E2E-2 + real LLM swap post-M5) | [arch](../05-refined-shared-systems-architecture/agent-fabric.md) |
| 11 | Durable workflow (`DurableExecutor`, timer wheel, durable signal) | [`shared/durable-workflow.md`](./shared/durable-workflow.md) | **M2** (M2.1–M2.4) | M4 (merge-queue `ci.result` wait goes live), M5 (1M+ timers + surge + E2E-2 long-park) | [arch](../05-refined-shared-systems-architecture/durable-workflow.md) |

> Note: the shared crates `myelin-content` (13.1) and `myelin-query`/`order_key` (13.3) are frozen *inside*
> the M2 reactive layer, **led/owned by Knowledge and co-owned with Issues** (byte-identical, X-2/X-3), and
> consumed by Knowledge / Issues / Chat / Search / the Bus `EventMatcher`. Signals + the firehose
> resume-cursor transport (contracts 3.1–3.6) are owned by the event-bus roadmap; its backpressure half is the
> substrate's.

### 2.2 The 5 subsystem roadmaps (`subsystems/`)

| # | Subsystem | Roadmap | Core band | Role | Earlier/later obligations | Architecture |
|---|---|---|---|---|---|---|
| 12 | Git hosting (repos, PRs, code review, pseudonymous commits) | [`subsystems/git-hosting.md`](./subsystems/git-hosting.md) | **M3** | producer | M1/M2 (ReBAC fragment + tokens frozen; X-1 consumer/projection); M5 (object-backed packs, history-rewrite) | [arch](../04-subsystem-architectures/git-hosting/architecture/) |
| 13 | Knowledge platform (block editor, in-doc DBs, CAS collab) | [`subsystems/knowledge-platform.md`](./subsystems/knowledge-platform.md) | **M3** | producer | **M2 (LEADS+FREEZES `myelin-content`/`myelin-query`)**; M5 (CRDT promotion, multi-cell collab) | [arch](../04-subsystem-architectures/knowledge-platform/architecture/) |
| 14 | Continuous integration (pipelines, unified sandbox, `CheckStatus`) | [`subsystems/continuous-integration.md`](./subsystems/continuous-integration.md) | **M4** | consumer/producer | **M2 (co-builds the unified runner + AG-D4/CI-T1 GATE)**; M5 (gVisor 2nd backend, surge) | [arch](../04-subsystem-architectures/continuous-integration/architecture/) |
| 15 | Issue tracker (boards/roadmaps/sprints, custom fields, SLAs) | [`subsystems/issue-tracker.md`](./subsystems/issue-tracker.md) | **M4** | consumer | M1/M2 (ReBAC + `initiative` token + `myelin-query` co-ownership frozen); M5 (move-CRDT, materialised rollup, cross-cell portfolio) | [arch](../04-subsystem-architectures/issue-tracker/architecture/) |
| 16 | Chat (conversation, unfurl-everything, explicit-first agents) | [`subsystems/chat.md`](./subsystems/chat.md) | **M4** | consumer (maximal) | M2 (ReBAC/tokens/humanise/index specs declared; firehose co-designed); M5 (mega-channel sharding, cross-org) | [arch](../04-subsystem-architectures/chat/architecture/) |

> Roadmap-file status (2026-06-19): **all 16 per-system roadmaps and the keystone are written and reconciled
> here.** Their band placement, critical-path position, cross-system dependencies, and gates are consistent
> with the master sequencing — no per-system roadmap proposes a band order that contradicts it (§7). A
> per-system roadmap refines the work inside its band; it does not move the band. Any future roadmap that needs
> to move a band boundary reconciles **here** first (the §7 rule).

---

## 3. The consolidated timeline — band → which systems do what

The master bands M0..M6 are the columns of the build. This table places every system's milestone into its
band. Read a row as "in this band, this system does this work to this exit gate"; read a column (a band) as
"everything that must be green to leave this band." A blank means no first-class work in that band (the system
may still be *depended on* — see §5). Per-system milestone names follow the system-prefix + band convention.

### M0 — Substrate, harness, and the committed gates (the floor under everything)

| System (milestone) | Work in this band |
|---|---|
| **Platform substrate** (SUB-M0) | `serve(AppSpec)` boot→migrate→relay→consumers→three ports→drain; liveness≠readiness; `ResilientClient`; `FailStatic<T>`; the protected-human-lane shed order; the **twelve committed lints** (red+green fixtures); the contract-coverage scanner; the shared overlay/state primitives; the thresholds file; **the Cargo workspace + 8 glue crates**; **the failure-injection harness** (1×/10×/30× load gen, scoped dependency-break injector, telemetry-assertion library — Tier 0, the unit of proof). |
| **Event bus** (B-M0) | The transactional outbox + idempotent-consumer template (the Tier-1 data-loss floor): `OutboxTx::emit`, the `outbox` table (`UNIQUE(aggregate,seq)` + `SKIP LOCKED` relay), `EventHandler` (`subjects()` never `*`, ack-after-enqueue), `consumer_dedup`. The `EventEnvelope` + `ArtifactRef` token table **frozen** as the names/units anchor; the seed taxonomy + new tokens (`ci.check.updated`/`ci.result`/`initiative`); the 3 Bus lints (`no-raw-publish`/`no-cross-sync-cycle`/tenant-predicate slice). |
| **Storage** (S-M0) | The OLTP tier client + RLS + **the outbox table physically lives here** (makes SUB-D1/BUS-D4 green); `BlobStore` content-addressed **fs-backed floor** + the narrow trait (never node-pinned); the `forward-only-migration` + `residency-pin` lints + the online-migration runner. |
| **Identity** (M0) | The `myelin-identity` glue-crate skeleton; its four lints (`tenant-predicate`, `no-untagged-personal-data`, `residency-pin`, `control-plane-pii-free`) wired into the ratchet; the envelope actor/subject fields anchored. No service yet. |
| **Tenancy** (CP-M0) | The `myelin-tenancy` partition-key crate (`TenantId`/`Region`/`ResidencyTag`); the `residency-pin` + `control-plane-pii-free` lints (red+green fixtures). |
| **GDPR** (GA-M0) | The `no-untagged-personal-data` lint (red+green fixtures); the `PersonalDataHolder` auto-registration **contract + harness hook** (frozen so M1 stores register on open); the `myelin-gdpr` crate; the `data_role` envelope field anchored. |
| **Refs** (R-M0) | The `myelin-refs` value type — `ArtifactRef` parse/format with ambiguity-rejection + the frozen `#sub` grammar vocabulary; validates (doesn't author) the token table; leans on 4 lints. |
| **Search** (S-M0) | The `search-requires-acl-filter` lint (red+green fixtures); index-doc names anchored to the envelope. |
| **Knowledge / Issues / CI / Chat / Workflow / Agent / Notif** | Each declares its hot-table flags (KN `block`/`db_row`/`doc_op`) and/or contributes its lint fixtures and anchors its contract names to the frozen envelope. No feature code yet. |

**M0 exit gate:** SUB-D1, SUB-D2, BUS-D4 (outbox exactly-once-in-effect, 0 ghost / 0 lost) + SUB-D5/D7/D8/D9
(resilience/IDOR/loop/readiness); **all twelve lints green with both fixtures**; the contract-coverage scanner
passes; the harness self-test (inject a fault, read a telemetry assertion).

### M1 — Identity + storage durability + tenancy (the dependency root + the data-loss floor)

| System (milestone) | Work in this band |
|---|---|
| **Identity** (ID-M1) | `authenticate` (all credential kinds incl. machine identity); `check` + `CaveatContext` (literal-only predicate floor); **`list_objects` with the `SetExpr` push-down + the S8 authz reverse index** (the single most load-bearing inter-system contract); `write_tuples`/zookie; `mint_run_token`/`revoke`; `delegation` (the `∩` algebra); `resolve_pseudonym`/`erase` + the frozen pseudonym grammar; the per-subsystem ReBAC **engine** + org/team/project core (fragments slot in later); **fail-static**. |
| **Storage** (S-M1) | KMS hierarchy + `KeyOrigin` (per-cell root → per-tenant KEK → per-subject DEK); crypto-shred + GD-4 granularity; OLTP envelope encryption; **backup/restore/cross-seam + restore-verify** (the headline silent-data-loss floor, CI-wired); the OLAP frame; reserve/settle; the structural GDPR floor. |
| **Tenancy** (CP-M1) | The PII-free control-plane registry; `discover`/`place`/`placement_of` two-phase signup; the four-layer region-pinning; `residency_verify`; the Pool isolation tier; the frozen (not-live) `CrossCellPointer` frame; provision-gating on restore-verify; self-host parity. |
| **GDPR** (GA-M1) | The `PersonalDataHolder` trait + M1-store holder impls; the `#[personal_data]` derive + the data-map/RoPA generator (CI-diffed); the DSR orchestrator state machine + certificate (coarse deadline floor); the structural erasure floor + **the ONE free-text/immutable posture (X-7) written once**; the erasure ledger; the tamper-evident audit log; retention/consent/sub-processor/transfer engine. |
| **Event bus** (B-M1) | Streams partitioned under `(tenant, region)` + residency-pinned; the Bus auto-registered as a holder; inline-PII crypto-shred wired to the KMS; the erasure-ledger re-erasure hook. |
| **Refs / Search / Notif / Agent / Workflow / each subsystem** | Register as a `PersonalDataHolder` (stub before data) and reserve their key class; confirm residency-pin. **Writes are blocked until STOR-D1 is green.** No engine yet. |

**M1 exit gate:** ID-D3 (cross-tenant 0); ID-D2 (fail-static); ID-D1 (disabled-user denied within N=5 min);
ID-D4/D7 (leak-free pre-filter incl. S8 JOIN + new-enemy watermark); CP-D2/CP-D3 (misroute 0 + residency-pin);
**STOR-D1/STOR-D2** (restore-verify, RPO ≤ 5 min / RTO ≤ 1h-tenant / 4h-cell, 0 loss — *the silent-data-loss
floor; M2 does not start over a red STOR-D1*); STOR-D4/GA-D5 (crypto-shred unrecoverable + `no-untagged-personal-data`
red on an untagged PII field).

### M2 — The reactive shared layer + the safety drills

| System (milestone) | Work in this band |
|---|---|
| **Reference graph** (R-M2) | The `edge` inverse index + 2 consumers; `resolve` per-viewer unfurl/embed → projection\|tombstone (the chokepoint); `backlinks/edges/traverse` leak-free depth-16 (lowers the `SetExpr` over `source_root`); the TE-7 typed-edge mirror discipline; **`project(ref, viewer)` shape**; the unified **`#sub` grammar + the 4-step tombstone ladder**; reindex-from-source; the R2 projection cache. |
| **Search** (S-M2) | The engine (Tantivy + 3 index shapes); the bus-fed incremental indexer; **the permission-aware query pipeline (conjoin the `SetExpr`/`Ids` ACL filter into every branch before scoring)**; the frozen-`QueryAst` compiler; RRF hybrid + filter-during-traversal; purge+reindex erasure; reindex-from-source; caches + telemetry. |
| **Notifications** (N-M2.x) | Holder + outbox + Signal-consumer skeleton; the ONE inbox `list_inbox` ranked; read-state truth; **`humanise` the ONE templating surface** (resolves each ref per-viewer); prefs/quiet-hours; `oncall_now`/`page` escalation on the durable wheel; the region-aware EU-preferring delivery adapter. |
| **Durable workflow** (FLOW-M2.1–2.4) | `DurableExecutor{start,signal,describe,cancel}` (idempotent `signal` per-effect `idem_key`); `WfCtx` deterministic surface + `flow-determinism` lint + divergence guard; the durable **timer wheel** (100k floor; 1M+ at M5); the durable **signal** (multi-day HITL); **the `SCHEDULE_AND_RUN_JOB` long-park idiom + the merge-queue `ci.result`-wait frame** (the durable-execution half of the X-1 seam). |
| **Agent fabric** (AG-M2) | `ToolSurface::register_tool` (frozen `requires_approval` defaults); **`EffectApi::apply` plan-then-apply** (schema→capability→delegation→tenant→budget→HITL→apply→meter); `AgentRuntime::step` strategy seam with `--use-mock`; **`ToolHands::exec` the unified sandbox = the CI runner's `kind=agent` job (co-built with CI)**; `Agent::handle` bounded loop; explicit-first dispatch; `run --dry-run`. |
| **CI** (M2 contribution) | **The unified sandbox runner (`SandboxBackend`/Firecracker) = `ToolHands::exec`** + the real-kernel escape-drill harness — co-owned with the agent fabric (ADR-20). This is CI's only M2 work; the rest of CI is M4. |
| **Event bus (signals/firehose)** (B-M2) | `define_signal_rule`; `register_automation`; `arm_trigger` (`QueryAst` condition); `EventMatcher` = frozen `QueryAst`; **the firehose transport + the resume-cursor subscription protocol** (`subscribe`/`resume`/`resync_required` — built FIRST, EI-04 §2.2); the check-seam carriage (`ci.check.updated`/`ci.result`); the dispatch tier (nested causality + loop guards + bounded pool + explicit-first + reserve/settle); reindex-from-source seam. |
| **Knowledge** (M2 freeze) | **LEADS + FREEZES `myelin-content` (block taxonomy, markdown-subset inline grammar, the 3 inline ref nodes, WASM `render(parse(md))===md`)** and **co-owns + freezes `myelin-query`/`order_key` (`QueryAst`/`ViewSpec`/field-type enum, LexoRank)** byte-identical so Issues/Chat/Search cannot drift. |
| **GDPR** (GA-M2) | The durable DSR deadline timer + nearing-deadline Signal (replacing the M1 coarse floor); `restrict` suppression into Search/Refs/Notif/OLAP; per-derivative erasure (Search purge+reindex of embeddings, Refs tombstone); the agent-trace holder seam. |
| **Identity** (M2) | First real consumption: Agent consumes `delegation`/`mint_run_token`; Search/Refs conjoin the `Filter`; the caveat evaluator promotes to the full `QueryAst` predicate core; the `watcher` relation begins to populate. |
| **Storage** (S-M2) | OLAP fed by the bus (reindex-from-source only); reserve/settle fronts the first agent runs; the T3 firehose-archive seam prepped. |
| **Chat / Issues** (M2 pre-work) | Declare ReBAC fragments, `chat.*`/`issue.*` event tokens (incl. `initiative`), `humanise` keys, `define_notif_rule` sets, `declare_indexable` specs; co-design the firehose; Issues co-owns the `myelin-query` freeze with Knowledge. |

**M2 exit gate (AG-D4 is the hard go/no-go for all untrusted code):** **AG-D4 / CI-T1** (real-kernel sandbox
escape = **0** — GATE); AG-D1/D2/D3/D5/D9; FLOW-D1/D2/D4/D5; REF-D1/D2/D6/D7/D8/D9; SRCH-D1/D2/D3; NOTIF-D4/D7;
BUS-D1/D3/D6 + D-10 (firehose) + D-11 (check-seam ordering); GA-D7 (restriction) + GA-D2/D4.

### M3 — The producer subsystems (Git hosting + Knowledge platform)

| System (milestone) | Work in this band |
|---|---|
| **Git hosting** (M3-G1..) | Repository hosting on the **local-disk floor** (content-addressed objects + Merkle history + pack/delta); code browsing; PRs; code review with content-anchored line ranges (the 5.7 tombstone ladder); the merge gate reads the Git-owned `check_status` projection (the **consumer half of X-1**); the merge queue is a durable workflow waking on `ci.result`; **pseudonymous-by-default commit identities** (decided before the git data model freezes); the Git ReBAC fragment (+`approve_untrusted_ci`); the `list_objects` `SetExpr` conjoin for PR/repo lists; `project(ref,viewer)`; the indexable `git.*` projection. |
| **Knowledge platform** (KN-M3a..e) | The collab transport (item 0, over the M2 resume-cursor seam); the block editor over frozen `myelin-content`; in-doc databases (rollups/formulas read-time, never stored); inline content as the markdown-subset string; **real-time collab — the CAS floor first** (per-block optimistic compare-and-swap, no *silent* overwrite, does not merge); the agent-trace holder (H17); the KN ReBAC fragment (page-tree inherit-with-overrides). |
| **Search** (S-M3) | Code search v1 (Git `git.*` projection: symbol/path/literal + trigram) + Knowledge indexing (blocks/pages multilingual + vector + JSONB facets); sub-artifact-granular + content-anchored projections. |
| **Refs** (R-M3) | Git-produced edges + content-anchored line-range `sub_anchor`; KN embeds + block/page/row anchors + the first `page_parent` lifecycle mirror; the `check-`/`step-` grammar used (Git's consumer half awaiting CI). |
| **Identity** (M3) | The Git + KN ReBAC fragments land; Git commits become pseudonymous-by-default (GIT-D2). |
| **Tenancy** (CP-M3) | `placement_of(repo)` goes live (repo-granular, relocatable, not node-pinned) + the outbound-mirror residency gate (deny-by-default). |
| **Storage** (S-M3) | Local-disk git packs behind the `BlobStore` trait; the within-EU CDN clone/bundle class (C3); the outbound-mirror residency gate seam (C6); git's crypto-shred reach. |
| **GDPR** (GA-M3) | The H1 (Git) + H4 (KN) + H17 (agent-trace) holders register; the Git + KN instances of the ONE posture (by reference, no restatement). |
| **Notif / Bus** | Notif accretes Git + KN notify-reasons; the Bus carries the new tokens + the heaviest firehose producers (KN collab op-streams) come online. |

**M3 exit gate:** GIT-D9 (push outbox emit-iff-committed); GIT-D8/D11 (cross-tenant 0 + `SetExpr` leak-free PR
list); GIT-D7 (force-push re-anchors inline threads, 0 mis-anchored); GIT-D2 (erase commit author →
pseudonymous residual); **KN-D3** (CAS floor: 0 silent overwrites); KN-D1 (resume 0 lost/dup, re-runs across
the future CRDT boundary); KN-D2 (`render(parse(md))===md` 100%); KN-D7/KN-D5/KN-D13; the M3 rows of
REF-D1/D2/D9/D4 + SRCH-D1/D3/D5 re-confirmed on the real Git+KN corpora.

### M4 — The consumer subsystems (CI + Issues + Chat)

| System (milestone) | Work in this band |
|---|---|
| **Continuous integration** (M4) | Pipelines as durable workflows (`flow-determinism` lint); the unified sandbox runner (**AG-D4 re-confirmed on the prod image**); the **`CheckStatus` producer half** (emit `ci.check.updated` per `(commit_oid,context)` with `run_attempt` supersession + `trust_tier`; emit the `ci.result` rollup the merge queue waits on; `details_ref` = `#step-<n>`; untrusted-fork success neutral-for-gating); reserve/settle on every run; the T3 log tier (per-subject-DEK segments + `(job,step,byte-range)` index); trust-tier/branch-scoped cache namespaces; residency-pinned runners. |
| **Issue tracker** (M4-I..) | Boards/roadmaps/sprints/hierarchies; custom fields (the `myelin-query` field-type enum); SLAs/reporting/audit; the board/backlog scan via the **`list_objects` `SetExpr` JOIN** (no N+1, <1s at 1M+ issues); co-equal board/roadmap over one `ViewSpec`; the human-key `<PROJECTKEY>-<seqno>`; drag-reorder via `order_key`/LexoRank; SLA timers + triggers on the durable wheel; the ADF→`myelin-content` import; descriptions/comments single-author CAS; the Issues ReBAC fragment; guard transitions ("can't mark Done while CI red" reads `CheckStatus`); the `issue_relation` typed-lifecycle events (2nd TE-7 mirror). |
| **Chat** (M4-C1..C9) | Conversation referencing any artifact; the gateway↔firehose resume-cursor transport (may diverge to a non-Rust connection tier per the 1.7 shim); per-conversation total order (ULID); idempotent send (`UNIQUE(conv,client_nonce)`); unfurls via Refs `resolve` (the 4-step tombstone ladder); the per-ref cache busting on `*.updated`; **explicit-first agent dispatch** (casual `@agent` notifies, only an explicit action dispatches a costed run; reserve/settle gates even the explicit run); batch HITL approval cards; the Chat ReBAC fragment (search-as-non-member → 0 results). |
| **Search** (S-M4) | Issues facets + the Tier-3 board-escalation valve (byte-identical ACL pre-filter); CI log search; Chat indexing (search-as-non-member = 0). |
| **Refs** (R-M4) | CI's `CheckStatus` producer half closes (X-1) — `details_ref`/`check`/`step` anchors resolve; Issues `issue_relation` lifecycle edges; Chat unfurls (the maximal consumer) — the full reference graph populated. |
| **Identity / Tenancy / Storage / GDPR / Notif / Workflow** | CI/Issues/Chat ReBAC fragments (ID); CI no-global-pool attestation + residency-pinned runners (CP-M4 / CI-R3); the CI log tier + trust-scoped cache + OLAP restriction gate (S-M4); the H2/H3/H5 holders + per-subject CI-log DEK + worklog classification (GA-M4); Notif humanises CI/Issues/Chat events; Workflow runs the CI pipelines + merge queue + SLA timers (the `ci.result` wait goes live). |

**M4 exit gate:** **CI-T1 / AG-D4 re-confirmed green on the production runner** (GATE); **GIT-D10 / CI-D8**
(the X-1 check seam end-to-end: `run_attempt` supersession, fork-self-green neutral, doubly-delivered
`ci.result` → merge-queue wakes exactly once, **0 double-merge**); CI-D1/D4/D6/D7 (effectively-once + supply
chain + fork isolation); CI-R3 (no-global-pool); ISS-D1/D2/D3 (board↔roadmap 0 drift; 50+ fields × 1M issues
board <1s; cross-tenant + IDOR 0); ISS-D5/D6/D12; CHAT-D1/D13/D14 (resume + co-commit + idempotent send);
CHAT-D5/D11/D17.

### M5 — World-scale hardening + the floor follow-ons + the cross-subsystem E2E wedge

| System (milestone) | Work in this band |
|---|---|
| **Knowledge platform** (M5) | **The CRDT, after the CAS floor** (Yrs/Automerge-class engine slotting into the M2 resume-cursor firehose transport; KN-D1 re-runs across the engine_promote boundary); cross-cell collab; per-facet/per-rollup materialisation; the all-hands-doc surge (KN-D8). |
| **Git hosting** (M5) | **Object-backed git packs, after the local-disk floor** (delta/pack/sharding/replication/smart-transport + the within-EU CDN clone/bundle class); concurrent-merge linearizability + failover (GIT-D4/D5); the audited history-rewrite erasure path. |
| **Tenancy / GDPR** (CP-M5 / GA-M5) | **Multi-cell, after single-cell** (the cross-cell PII-free pointer bridge live; live tenant migration / repo relocation; DSR fan-out iterates `member_cells`); the **full DSR / erasure fan-out across all H1–H18 holders** + GA-10 (history-rewrite invalidation) + GA-11 (outbound-mirror gate). FLOOR drills GA-D8/CP-D7/CP-D8 now owed. |
| **Storage / Event bus** (S-M5 / B-M5) | The fs-backed → object-store `BlobStore` swap; the **event-volume column-store seam** added only once volume is *measured*; the cross-cell bridge carriage; restore-verify at cell scale. |
| **Refs / Search / Identity / Workflow / CI / Notif** | The hot-fanout reach index (Refs R4, measured-trigger) + cross-cell backlink fan-out; the filtered-ANN strategy + cross-cell federated search (Search, designed-and-extends); multi-cell principal authority (Id); 1M+ timers + continue-as-new (Workflow); the gVisor 2nd backend + time-series log tier (CI); the cross-cell notif bridge (Notif). |
| **All owners** | The **F6 surge family** (30× surge: protected human lane holds, agent lane sheds 429+Retry-After, cross-tenant impact 0); the prod-scale benchmarks (1M+ timers, 100k-PR list, the monorepo ceiling); online-migration-under-load; restore-verify at cell scale; the cell bulkhead. |
| **The whole-system E2E wedge** | E2E-1 PR context pane (Git+CI+Issues+KN+Refs+Search+Id+Notif); **E2E-2 CI-fail → triage agent → issue → chat → fix-PR** (the agent-native flagship); E2E-3 spec-to-ship traceability (cold-reindex == live; audit tamper detected); E2E-4 DSAR fan-out (0 holders missed; 0 recoverable PII incl. vectors incl. backups). |

**M5 exit gate:** the full F6 surge family across all owners; GIT-D4/D5; KN-D1 re-green across the CRDT boundary
+ KN-D8; GA-D1/GA-D8/GA-10/GA-11 + CP-D5/CP-D7/CP-D8 (DSR fan-out 0-missed + multi-cell floors + cell bulkhead);
**the four E2E scenarios green**; STOR-D2 at cell scale.

### M6 — Dogfooding: Myelin hosts itself

| System | Work in this band |
|---|---|
| **All five subsystems** | Migrate the Myelin monorepo onto Myelin git hosting; the build/test/lint/mutation pipeline becomes a Myelin CI pipeline (the lints + the mandatory-core mutation gate run as Myelin CI jobs on every Myelin commit); the roadmap + gap report + scorecard live as Myelin issues + a Myelin Knowledge space; the team talks in Myelin Chat; the every-incident-adds-a-drill loop files a Myelin issue + a reproducing drill. The self-host cell is exactly one cell of identical artifacts (the degenerate control plane). |
| **The switch test** | Drive the real UI of all five subsystems in a browser (the frontend done-bar L5): could a GitHub/Jira/Linear/Notion/Slack user move to Myelin without hitting a wall the old tool didn't have? Measured contrast + latency budgets + `render(parse(md))===md` + overlays against the real anchor. |

**M6 done-bar:** ISS-D14 / CHAT-D19 / Git OQ-12 switch tests pass (driven in a browser, measured); the Myelin
self-hosting CI graph is green on the platform's own commits; **no later-band gate is red** (the truth-up pass
confirms every PROVEN row rests on a dated green artifact, never a doc claim).

---

## 4. The critical path across all 16 systems

The single longest chain of must-precede dependencies — the spine that fixes the minimum number of sequential
gates. Everything else branches off it.

> **harness + outbox + the twelve lints (M0, substrate + event-bus + storage's outbox table)**
> → **Identity `list_objects`/`check` + restore-verify + tenancy/residency (M1, identity + storage + tenancy)**
> → **agent fabric + durable workflow (the `SCHEDULE_AND_RUN_JOB` long-park) + the firehose resume-cursor
>    transport + the `AG-D4` sandbox-escape GATE (M2, agent-fabric + workflow + event-bus + CI's unified runner)**
> → **Git: pseudonymous commits + the merge gate + the `check_status` projection (M3, git-hosting)**
> → **CI: the `CheckStatus` producer closing the X-1 seam (M4, continuous-integration)**
> → **the X-1 check-seam end-to-end (GIT-D10 / CI-D8) + the E2E-2 flagship (M5)**
> → **dogfood the self-hosting CI graph (M6)**.

**The two hardest single seams on the path:**

1. **AG-D4 / CI-T1 — the real-kernel sandbox-escape GATE.** It blocks *all* untrusted execution, so it gates
   both CI (the runner) and any agent compute call (`ToolHands::exec` is the same runner, ADR-20). **CI owns
   the runner + the drill but delivers them in M2, not M4** — front-loaded out of band order precisely because
   everything downstream of untrusted execution waits on it. It is a permanent gate, re-run on every
   backend/image/kernel change. Until green on the production backend, nothing downstream may proceed.

2. **X-1 / contract 5.9 — the Git↔CI `CheckStatus` seam.** The most load-bearing cross-subsystem contract, and
   the one place the critical path crosses a band boundary *in the wrong direction*: Git builds the **consumer
   half** (the merge gate + the `check_status` projection table) in **M3**, but the **producer half** (CI
   emitting `ci.check.updated` / `ci.result`) lands in **M4**. The seam is *declared* in M2 (Refs 5.9 + the Bus
   carriage + the Workflow `ci.result`-wait frame), the consumer ships in M3, and it goes *live* in M4 — proven
   end-to-end by GIT-D10/CI-D8. The **durable-execution half** of the seam (the merge-queue long-park) is owned
   by `myelin-flow`, also on the critical path. See §7 for why this split is correct, not a conflict.

The acyclicity rule (`no-cross-sync-cycle` lint, M0) keeps the DAG acyclic by construction: Git never
synchronously asks CI "is it green," it reads its own `check_status` projection fed by CI's events. Every
cross-subsystem dependency is an async event/projection, never a synchronous call cycle.

---

## 5. The cross-system sequencing dependencies (X depends on Y's milestone Mn)

These are the inter-system "must exist first" edges, stated as *X depends on Y's milestone*. They are the
reconciliation of every per-system roadmap's upstream-dependency list into one DAG. Grouped by depended-on tier.

### 5.1 Everything depends on the substrate root (M0)

- **Every system depends on Platform-substrate M0** for `serve(AppSpec)`, the three-surface topology,
  liveness≠readiness, `ResilientClient`, `FailStatic`, the shed order, and the twelve lints.
- **Every state-changing handler + every consumer depends on Event-bus M0** for `OutboxTx::emit`, the `outbox`
  table, the `EventHandler` template, and `consumer_dedup`. The `EventEnvelope` + `ArtifactRef` token table
  are the names/units anchor every later contract aligns to. **The outbox table physically lives in Storage's
  M0 OLTP tier** — co-located so an outbox row commits in the same transaction as the state change (this is
  what makes the cross-seam restore cursor exist).
- **Every drill depends on the failure-injection harness M0** — nothing in the catalogue is drillable until it
  exists (the unit of proof). The substrate builds the harness too (it has no upstream).

### 5.2 The Identity / Storage / Tenancy root (M1) — the highest fan-in

- **Search, Refs, and every subsystem board/list depend on Identity M1 `list_objects` (4.3, the `SetExpr`
  push-down)** — the single highest-fan-in inter-system contract. No leak-free query/list path exists until it
  is frozen + green. Its dedicated authz reverse index (S8) is the proven first replica.
- **Every write path, `EffectApi`, and every gateway depend on Identity M1 `check` + `CaveatContext` (4.2)**
  (full predicate core promotes when `myelin-query` freezes in M2).
- **Agent runs, CI dispatch, and workflow activities depend on Identity M1 `mint_run_token` (4.7) + `delegation`
  (4.5).**
- **Notif and every critical-dep caller depend on Identity M1 fail-static (4.11)** to degrade rather than
  cascade.
- **Every subsystem that writes data depends on Storage M1 restore-verify (11.5)** — the silent-data-loss
  floor gates every later write. (Knowledge/Issues/Chat explicitly block their first real write on STOR-D1.)
- **Every erasable holder depends on Storage M1 KMS hierarchy + per-subject DEK (11.3/11.4)** for crypto-shred.
- **Every store, CI runner/log/cache depends on Tenancy M1 `(tenant,region)` partition + `residency_verify`
  (12.1/12.4).** Tenancy explicitly does **not** depend on Identity on the hot path (two-phase signup keeps the
  control plane PII-free).
- **Every agent run + every CI run depends on Storage M1 reserve/settle (11.7).**
- **GDPR's spine (M1) depends on the M0 holder hook + outbox + the M1 Identity pseudonym lever + the M1 Storage
  KMS/restore floor** — its only hard blockers; everything it orchestrates lands *after* it.

### 5.3 The reactive layer (M2) → its consumers

- **Knowledge collab (CRDT later), Chat presence/live, and CI log live-tail depend on Event-bus M2 the
  firehose resume-cursor transport (3.5)** — the durable real-time transport built *first*, into which the
  CRDT later slots. (Refs/Search consume the **durable bus, not the firehose** — noted so no one mis-wires them.)
- **Every agent-authoring subsystem (Issues/Knowledge/Chat tools) and CI depend on Agent-fabric M2 `EffectApi`
  plan-then-apply (8.2) + the `ToolHands::exec` unified sandbox (8.4)** — and therefore on **AG-D4 green**.
  **CI co-builds that sandbox in M2** (it is CI's runner).
- **Agent runs, CI pipelines, the merge queue, SLA timers, multi-day HITL, Notif escalation, and KN living-doc
  automations depend on Workflow M2 `DurableExecutor` + the timer wheel + the durable signal + the
  `SCHEDULE_AND_RUN_JOB` long-park (9.x).**
- **Every unfurl/embed/backlink in every subsystem depends on Refs M2 `project(ref, viewer)` (5.6) + the
  `#sub` grammar + the tombstone ladder (5.7).**
- **Every channel renderer, every agent HITL card, every status message depends on Notif M2 `humanise` (7.3).**
- **Knowledge, Issues, Chat, Search, and the Bus `EventMatcher` depend on the M2-frozen `myelin-content` +
  `myelin-query` crates (13.x), led/frozen by Knowledge and co-owned with Issues** so their content/query
  compilers are byte-identical (no drift, X-2/X-3).
- **GDPR's deadline timer (GA-M2) depends on the Workflow M2 timer wheel; its restriction-into-analytics
  depends on the OLAP read model + Search/Refs/Notif existing (M2).**

### 5.4 The producers (M3) → the consumers (M4)

- **CI M4, Issues M4, and Chat M4 depend on Git M3** for the commits/PRs/refs they check, reference, and
  unfurl.
- **Issues M4 (spec→issue lineage), Chat M4 (embeds), Search M3, and the agent-trace holder depend on
  Knowledge M3** for the docs/databases.
- **Git's merge gate (M3 consumer) depends on CI's `CheckStatus` producer (M4)** to go live — the X-1 seam,
  split producer/consumer across the band boundary (§4, §7). Git ships the projection + gate in M3 reading an
  empty projection until CI fills it.
- **Refs traversal + the spec-to-ship lineage depend on Issues M4** typed lifecycle edges (TE-7, 5.5) and on
  KN's `page_parent` (M3, the first lifecycle mirror).
- **Chat M4 is the maximal consumer** — it depends on Refs unfurl, the per-ref cache, and agent dispatch from
  every prior band.
- **Tenancy's repo-grain placement + mirror gate (CP-M3) go live with Git M3; its CI no-global-pool
  attestation (CP-M4) with CI M4.**
- **GDPR's holders light up per subsystem** (H1/H4/H17 with Git/KN M3; H2/H3/H5 with CI/Issues/Chat M4) — the
  precondition for the complete GA-D1 fan-out at M5.

### 5.5 The M5 floor follow-ons depend on their floors

- **The CRDT (KN M5) depends on the CAS floor (KN M3) + the resume-cursor transport (Event-bus M2).**
- **Object-backed packs (Git M5) depend on the local-disk floor (Git M3) + relocatable placement (Tenancy
  M1/M3).**
- **Multi-cell (Tenancy/GDPR M5) depends on single-cell (M1) + the cross-cell bridge frame (M1) + every
  subsystem's cross-cell consumers (ISS rollup / KN collab / CHAT cross-org) existing (M4).**
- **The full DSR fan-out (GDPR M5) depends on all H1–H18 holders existing (across M1–M4).**
- **The object-store `BlobStore` (Storage M5), object-backed packs, and Search's object-store backstop ride
  the same M5 storage promotion; the column-store event seam (Bus/Storage) is added only once volume is
  measured (post-M5).**

---

## 6. The ordered drill-gate sequence that bounds the whole build

The gate invariant restated as one ordered go/no-go list — the bands cannot be reordered around these, and a
later gate may not be claimed over a red earlier one. Each is a band boundary unless marked PERMANENT (re-run
forever, never "done").

| Order | Gate (must be green to proceed) | Band boundary | Family |
|---|---|---|---|
| 0 | The failure-injection harness **self-test** (inject a fault, read a telemetry assertion). | — (M0 internal) | the unit of proof |
| 1 | **SUB-D1, SUB-D2, BUS-D4** (outbox exactly-once-in-effect, 0 ghost / 0 lost) + SUB-D5/D7/D8/D9 + **all twelve lints green with both fixtures** + the contract-coverage scanner. | M0 → M1 | F5 + the ratchet |
| 2 | **STOR-D1 / STOR-D2** (restore-verify, RPO ≤ 5 min / RTO ≤ 1h-tenant / 4h-cell, 0 loss — *the silent-data-loss floor*). **PERMANENT** (re-runs on every store-touching change). | M1 → M2 | F3 |
| 3 | **ID-D3 / ID-D2 / ID-D1 / ID-D4 / ID-D7 + CP-D2 / CP-D3** (cross-tenant 0; fail-static; disabled-user denied in N=5 min; leak-free pre-filter incl. S8; new-enemy watermark; misroute 0 + residency-pin) + STOR-D4/GA-D5 (crypto-shred unrecoverable + untagged-PII lint). | M1 → M2 | F2, F7, F8, F3 |
| 4 | **AG-D4 / CI-T1** (real-kernel sandbox escape = **0**) — *the single hard go/no-go before any untrusted CI step or agent compute call*. **PERMANENT** (re-runs on every backend/image/kernel change). | M2 → M3 | escape |
| 5 | AG-D1/D2/D3/D5/D9; FLOW-D1/D2/D4/D5; REF-D1/D2/D6/D7/D8/D9; SRCH-D1/D2/D3; NOTIF-D4/D7; BUS-D1/D3/D6 + D-10 (firehose) + D-11 (check-seam order); GA-D2/D4/D7. | M2 → M3 | F9, F1, F5 |
| 6 | **KN-D3** (CAS floor: 0 silent overwrites) + KN-D1/D2/D5/D7/D13; GIT-D9 (push outbox emit-iff-committed); GIT-D8/D11 (cross-tenant 0 + `SetExpr` leak-free); GIT-D7 (re-anchor); GIT-D2 (pseudonymous residual). | M3 → M4 | CAS floor, F5, F2, F1 |
| 7 | **GIT-D10 / CI-D8** (the X-1 check seam end-to-end: 0 double-merge, fork-success-neutral) + CI-T1 re-green on the prod runner; CI-D1/D4/D6/D7 + CI-R3; ISS-D1/D2/D3/D5/D6/D12; CHAT-D1/D5/D11/D13/D14/D17. | M4 → M5 | X-1, escape, F1, F5 |
| 8 | The full **F6 surge family** (human lane holds, agent sheds, cross-tenant 0); GIT-D4/D5; KN-D1 re-green across CRDT + KN-D8; GA-D1/GA-D8/GA-10/GA-11 + CP-D5/CP-D7/CP-D8 (DSR fan-out 0-missed + multi-cell floors + cell bulkhead); **E2E-1..E2E-4 green**; STOR-D2 at cell scale. | M5 → M6 | F6, F3, the whole-system wedge |
| 9 | ISS-D14 / CHAT-D19 / Git OQ-12 switch tests; the self-hosting CI graph green; the truth-up pass confirms 0 red earlier gates. | M6 done-bar | the done-bar |

**The two permanent gates** — AG-D4/CI-T1 (one escape is catastrophic) and STOR-D1/STOR-D2 (silent data loss
outranks every feature) — ratchet across the whole build; they are not band-local.

---

## 7. Sequencing conflicts found, and the recommended resolutions

Reconciling the master sequencing against all 16 per-system roadmaps surfaced the following tensions. Each is
stated with the parties, the tension, and the recommended resolution. **No per-system roadmap proposes a band
ordering that contradicts the master sequencing** — every tension is resolved by *declaration-order within an
existing band* or the *named-floor discipline*; none moves a band boundary.

### 7.1 The X-1 check seam crosses the M3/M4 boundary "backwards" (Git ⟷ CI)

- **Parties:** Git hosting (M3, the merge-gate consumer), CI (M4, the `CheckStatus` producer), and Workflow
  (M2, the durable-execution half).
- **Tension:** Git's merge gate (M3) reads a `check_status` projection that *only CI (M4) produces*. Read
  naively, M3 depends on M4 — a band inversion.
- **Resolution (already taken in the master sequencing, ratified here):** split the contract across declaration
  points and keep the dependency a one-way async edge. (1) The `CheckStatus` seam (5.9) is **declared as a
  frozen contract in M2** — the Refs grammar half, the Bus per-aggregate-ordering carriage, and the Workflow
  `ci.result`-wait frame — so both sides build to the same shape. (2) Git ships the **consumer half** (the
  merge gate + the projection table) in **M3**, gating on a projection that is simply *empty* until a producer
  exists; the merge queue is a durable workflow that *waits on* `ci.result`, it does not call CI. (3) CI ships
  the **producer half** in **M4**; the seam goes live and is proven end-to-end by GIT-D10/CI-D8. There is **no
  synchronous call from Git to CI** (the `no-cross-sync-cycle` lint forbids it). **No conflict; the split is the
  correct design.** The Bus D-11 (check-seam ordering) and the Workflow long-park are drilled in M2 so the
  substrate under the seam is proven before either subsystem half exists.

### 7.2 The firehose transport must precede both its first consumer (KN collab) and the CRDT

- **Parties:** Event-bus (firehose resume-cursor transport, M2), the substrate (its bounded/shed backpressure
  half, M2), Knowledge (CAS collab M3 + CRDT M5), Chat (live transport M4).
- **Tension:** Knowledge's CAS collab floor (M3) and Chat's live tier (M4) both ride the firehose transport,
  and the M5 CRDT slots into the *same* transport. If the transport were sequenced with its first product
  consumer (KN, M3), Knowledge would have to build its own transport — the "features getting harder to add"
  anti-signal.
- **Resolution (ratified):** the firehose transport (contract 3.5) is **built in M2 as part of the reactive
  shared layer**, *before* any subsystem consumes it, precisely so KN collab (M3), Chat (M4), and the CRDT (M5)
  are all *projections* of one durable real-time transport. The Bus owns the zero-loss-replay half; the
  substrate owns the bounded/shed half (proven under hot streams in M2, connection-storm in M4). KN-D1 and Bus
  D-10 are deliberately written to **re-run green across the CAS→CRDT engine_promote boundary** so the floor's
  promotion is itself drilled on the same transport. **No conflict; M2 placement is load-bearing.**

### 7.3 Systems with a core band that contribute (or freeze) *before* it

- **Parties:** Search (core M2, lint M0 + holder M1); Refs (core M2, value-type M0 + holder M1); GDPR (spine
  M1, but completed across M1–M5 as holders come online); **Knowledge** (core M3, but **freezes
  `myelin-content`/`myelin-query` in M2**); **CI** (core M4, but **co-builds the unified runner + AG-D4 in
  M2**); Identity (core M1, ReBAC fragments M3/M4).
- **Tension:** a reader could see "Search/Refs/GDPR/Knowledge/CI named in an earlier band than their core" as
  building a system before its dependencies are green, or as a band inversion.
- **Resolution (the deliberate pattern, made explicit in §1):** distinguish **early contributions** (lint,
  holder registration, glue-crate/contract freeze, a shared sandbox co-build) from **the core build**. Every
  system contributes its committed lint in M0 and registers as a holder in M1 *before it has feature code* (the
  exhaustive-holder-list + lint-before-the-path rules). Knowledge freezing `myelin-content` in M2 is a
  *freeze-before-consume* obligation (Issues/Chat/Search consume the subset — it must freeze before they
  compile). CI delivering the sandbox runner in M2 is *front-loading the critical-path RCE gate* (the runner is
  the agent fabric's `ToolHands::exec`, ADR-20 — it cannot wait for CI's M4 bulk). None of these is an early
  *engine* build: Search's engine does not start over a red STOR-D1; Knowledge writes no row before M1 is
  green; CI runs no pipeline before M4. **No conflict; this is the deliberate structural pattern every roadmap
  follows.**

### 7.4 The mock→real agent-runtime swap (and the real embedding/LLM/KMS adapters) sit outside the M0..M6 bands

- **Parties:** Agent fabric (mock runtime M2, `--use-mock`) vs. the `LlmAgentRuntime`; Search (mock embedding
  adapter M2) vs. the real EU-hostable model; the real production KMS/LLM backends.
- **Tension:** every M2..M5 agent drill (AG-D4/D2/D3/D5/D9, E2E-2) and every Search vector drill runs against
  the **mock** adapter; the real adapters land **post-M5 / runtime**. A reader might expect the real adapter
  inside a band.
- **Resolution (ratified, VISION §3):** the real adapters are **named floor follow-ons, not bands** — config/
  impl swaps behind the `AgentRuntime::step` / embedding / `KeyOrigin` strategy seams, scheduled *after* the
  safety drills are green (AG-D4/D2/D3/D5; AG-D9 mock-determinism is itself a gate). The swap re-opens no gate.
  **No conflict; correctly sequenced post-M5.**

**Net:** four tensions, all resolved by declaration-order-within-a-band or the named-floor discipline — none
moves a band boundary. The standing rule: if a future per-system roadmap *does* require moving a boundary, it
reconciles **here** first, the master sequencing is amended, and the gate invariant is re-checked; a per-system
roadmap never silently overrides the band order.

---

## 8. The handoff to Phase 7 — the roadmaps become one sequence of implementation prompts

Phase 7's job is to turn this reconciled timeline into **one ordered sequence of implementation prompts**
spanning roughly **400k–700k tokens** of build instruction — the executable form of the M0..M6 bands. The
handoff contract:

1. **The prompt sequence follows the band order, not the system order.** Phase 7 walks M0 → M6; within each
   band it parallelises across systems exactly as the §3 timeline tables show. The keystone
   (`00-master-sequencing.md`) is the spine; each per-system roadmap supplies the *contents* of that system's
   prompt within each band.

2. **Each prompt carries its gate as its definition of done.** A Phase-7 prompt is not "done" when the code
   compiles — it is done when its band-and-system drill emits a dated green artifact (the §6 gate list, the
   per-system gate rows). The prompt must include the drill it has to make green, with the quantified threshold
   from the thresholds file (Q32 defaults). The gate invariant carries into Phase 7: **no prompt for a later
   band ships over a red earlier-band gate.**

3. **The floors are explicit prompt boundaries.** Each named floor (the §5 of the master sequencing, repeated
   in each per-system roadmap) is its own prompt that ships the floor *and registers its follow-on* as a later
   prompt — never a prompt that silently ships a floor as if it were the full answer. The honest-floor rule
   binds Phase 7: the gap report row is part of the prompt's output.

4. **The two permanent gates are standing prompts.** AG-D4/CI-T1 and STOR-D1/STOR-D2 generate re-run prompts on
   every backend/image/kernel change and every store-touching change respectively — they are not consumed once.

5. **The early-contribution pattern (§1, §7.3) is a Phase-7 ordering rule, not a band move.** Within each band's
   prompt set, emit the *lint + glue-crate + holder-registration + shared-crate-freeze + shared-sandbox-co-build*
   prompts before the prompts that depend on them — even when the system's *engine* prompt is in a later band.
   Concretely: the M0 prompt set includes Search/Refs/Tenancy/GDPR/Identity lint+crate prompts; the M2 prompt
   set includes Knowledge's `myelin-content`/`myelin-query` freeze and CI's unified-runner co-build before the
   prompts that consume them.

6. **The token budget maps to the bands roughly as:** M0+M1 (the substrate + the dependency root + the
   data-loss floor) and M2 (the reactive layer + the sandbox-escape gate + the frozen content/query crates)
   are the densest — they carry the most load-bearing contracts and the permanent gates. M3/M4 (the five
   subsystems) are each a *projection* of capabilities that already exist, so each subsystem's prompt set is
   smaller than the substrate's (the compounding-payoff test, EI-01 closing — if a subsystem prompt is *larger*
   than the substrate's, the substrate was wrong; stop and repair it). M5/M6 (hardening, the floor follow-ons,
   the E2E wedge, dogfood) are mostly drill + promotion prompts over an existing surface.

7. **Phase 7 reads this index as its table of contents.** The §3 timeline tells Phase 7 what to write in each
   band; the §4 critical path tells it what cannot be parallelised; the §5 dependency edges tell it the prompt
   ordering within a band; the §6 gate list tells it each prompt's done-bar; the §7 conflict resolutions tell
   it the declaration-order rules (X-1 split, firehose-first, lint/holder/freeze/runner-before-build,
   adapter-swap-post-M5).

---

## 9. Digest

**The band → systems timeline (consolidated):**

- **M0** — Substrate (`serve`, ports, resilient client, fail-static, shed order, the twelve lints + the
  failure-injection harness + the contract-coverage scanner + the 8 glue crates) + Event-bus (the
  transactional outbox + consumer template, the `EventEnvelope`/`ArtifactRef` anchor) + Storage (the outbox
  table physically lives here + the fs-blob floor). Every system contributes its lint fixtures + glue crate +
  envelope-name anchors. Gate: SUB-D1/D2, BUS-D4, all twelve lints, harness self-test.
- **M1** — Identity (`list_objects` `SetExpr` push-down + S8, `check`, `delegation`, fail-static) + Storage
  (KMS hierarchy, reserve/settle, **restore-verify = the data-loss floor**) + Tenancy (partition + residency +
  the cell-bridge frame) + GDPR structural spine + Bus crypto-shred holder. Every system registers as a holder.
  Gate: STOR-D1/D2, ID-D1/D2/D3/D4/D7, CP-D2/D3.
- **M2** — Refs + Search + Notif + Durable-workflow (+ the `SCHEDULE_AND_RUN_JOB` long-park) + Agent-fabric (+
  CI's unified runner) + the Event-bus signals/firehose resume-cursor transport + **the Knowledge-frozen
  `myelin-content`/`myelin-query` crates**. Gate: **AG-D4 sandbox-escape GATE** + the F1/F5/F9 correctness
  drills + the firehose/check-seam ordering (D-10/D-11).
- **M3** — Git (pseudonymous commits, local-disk floor, the merge gate + `check_status` projection) +
  Knowledge (the collab transport item-0, the CAS collab floor, in-doc DBs, agent-trace). Search lights up code
  + doc corpora; Refs lights up git + doc edges; the holders/fragments/residency light up. Gate: KN-D3 (CAS),
  GIT-D9/D7/D8/D11/D2, KN-D1/D2.
- **M4** — CI (the `CheckStatus` producer closing the X-1 seam, the unified runner re-confirmed) + Issues
  (board/SLA/roadmap) + Chat (unfurl-everything, explicit-first agents). Search/Refs/holders/attestations light
  up the consumer corpora. Gate: **GIT-D10/CI-D8 (X-1, 0 double-merge)** + AG-D4 re-green + ISS-D2/D3 +
  CHAT-D1/D13/D14.
- **M5** — World-scale hardening + the floor follow-ons (CRDT, object-backed packs, multi-cell, full DSR
  fan-out + GA-10/GA-11) + the four whole-system E2E scenarios + the F6 surge family. Gate: F6 family +
  E2E-1..E2E-4 + GA-D1/GA-D8 + CP-D5/D7/D8 + STOR-D2 at cell scale.
- **M6** — Dogfooding: Myelin hosts itself (one self-hosting CI graph, one self-host cell; the switch tests
  driven in a browser). Done-bar: switch tests + self-hosting CI green + 0 red earlier gates.

**The critical path (across all 16 systems):** harness + outbox + the twelve lints (M0) → Identity
`list_objects`/`check` + restore-verify + tenancy (M1) → agent fabric + workflow (the long-park) + the firehose
transport + **AG-D4 sandbox-escape GATE** (M2, CI co-builds the runner) → Git pseudonymous commits + the merge
gate + `check_status` projection (M3) → CI **`CheckStatus` producer closing the X-1 seam** (M4) → the X-1 seam
end-to-end (GIT-D10/CI-D8) + the E2E-2 flagship (M5) → dogfood the self-hosting CI graph (M6). The two hardest
single seams: **AG-D4** (blocks all untrusted execution; gates both CI and any agent compute; CI delivers the
runner in M2) and **X-1 / contract 5.9** (the Git↔CI check seam, split producer/consumer across M4/M3, with the
durable-execution half owned by `myelin-flow`).

**Sequencing conflicts found:** four tensions, all resolved without moving a band boundary — (1) the X-1 check
seam crossing M3/M4 "backwards" is *not* a real backward dependency (declared in M2 across Refs/Bus/Workflow,
consumer in M3 reads an empty projection, producer in M4 fills it via a one-way async edge —
`no-cross-sync-cycle` forbids the synchronous cycle); (2) the firehose transport is correctly built in M2
*before* its KN/Chat/CRDT consumers so they are projections of one transport; (3) the systems named earlier
than their core band (Search/Refs/GDPR lints+holders; Knowledge's M2 `myelin-content` freeze; CI's M2 unified
runner) are the deliberate lint/holder/freeze/runner-before-build pattern, not early engine builds or band
inversions; (4) the mock→real agent-runtime / embedding / KMS / LLM swaps are correctly post-M5 named-floor
follow-ons outside the bands. None moves a band; all are resolved by declaration-order or the named-floor
discipline. The standing rule: any future per-system roadmap needing to move a boundary reconciles **here**
first, never silently.
