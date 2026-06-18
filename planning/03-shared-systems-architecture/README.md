# Phase 3 — Shared-Systems Architecture (index & executive summary)

> Phase: `03-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md).
> Doctrine (binding): [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
> (EI-02) + [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) (EI-04),
> with EI-03 (agent fabric) for the Agent/Workflow docs and EI-05 (UX) for Notifications. Spine:
> [`architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md) (ADR-01…ADR-20) +
> [`shared-systems-overview.md`](../02-holistic-architecture/shared-systems-overview.md). Binding directives:
> [`integration-directives.md`](../02b-doctrine-integration/integration-directives.md); decisions:
> [`decision-record.md`](../02b-doctrine-integration/decision-record.md) §(c)/(d)/(e). Phase-1 foundation:
> [`technical-structuring.md`](../01-research/technical-structuring.md).
>
> **Synthesis lead doc.** This README frames the shared-systems layer, indexes **all 11** Phase-3 docs,
> summarises the committed designs + cited prior art, lists the spine changes Phase 3 surfaced, names the
> floors, and hands off to Phase 4. Companion synthesis docs: [`contract-index.md`](./contract-index.md)
> (the build-to interface map) and [`drills-and-open-questions.md`](./drills-and-open-questions.md) (the
> consolidated drill inventory, de-duplicated open questions, and the consistency pass). **Phase-3 is now
> COMPLETE: 11 system docs.** Date: 2026-06-19.

---

## 1. What the shared-systems layer is (the frame)

Myelin's differentiator is not any one of the five subsystems (git, CI, issues, knowledge, chat) — it is the
**shared layer** that makes them one platform: one identity & permission model, one event bus, one agent
fabric, one reference graph, one search, one notification inbox, one durable-workflow substrate, one
tenancy/residency model, all GDPR-by-construction (VISION §1–§2). Phase 2 committed the *direction*
(ADR-01…ADR-20); **Phase 3 designs these shared systems in detail** — concrete data models, algorithms,
wire/interface contracts, scaling/sharding, failure modes, and the quantified drills that prove each
property. Phase 4 subsystems build *on top of* this surface.

The layer is held together by one structural idea, repeated at every level: **a Myelin service is a thin shell
over identical plumbing.** A new service calls `myelin_substrate::serve(AppSpec)` and supplies its handlers,
migrations, and consumer registrations; everything load-bearing for correctness — the transactional outbox,
idempotent consumers, tenant-scoping, the resilient client, the protected human lane, fail-static, the three
ports, the trace context, `PersonalDataHolder` auto-registration — comes from the shared crates, so it
**cannot drift between services and cannot be skipped** (ADR-01; EI-02 §9). Six invariants are inherited by
every store in every doc and never re-litigated:

1. **Tenant + region is the first column / partition key of everything** — no cross-tenant query path; the
   tenant comes from the verified token, never the URL (EI-02 §1; ADR-11; ID-3).
2. **Every store is residency-pinned, per-tenant envelope-encrypted, crypto-shred-capable, and a
   `PersonalDataHolder`** (ADR-11/12) — the harness auto-registers them so "we forgot the search index" is a
   structural failure (GD-3).
3. **No subsystem/shared-system reads another's store** — interaction is via the contracts only, enforced by
   the `no-cross-db` lint (ADR-01/13).
4. **The transactional outbox is the ONLY sanctioned emit path** — there is no fire-and-forget publish
   (EI-02 §4; BUS-2). The same path carries the tamper-evident audit append (audit is a bus consumer).
5. **Causality is a first-class primitive** (nested `correlation_id`/`causation_id`/`depth`, derived
   correct-by-construction) so audit, the "why" view, distributed tracing, and the agent loop guard are **one
   mechanism**, not four (EI-02 §6; BUS-5).
6. **Reindex-from-source is the only recovery path for every derived store** (Search, Refs, OLAP, Notif
   read-models) — steady-state and recovery share one code path, so they cannot drift (EI-04 §5.3).

---

## 2. The Phase-3 document index (one line each) — 11 docs, complete

| # | Doc | Crate(s) | What it owns | Resolves |
|---|---|---|---|---|
| 00 | [`00-platform-substrate.md`](./00-platform-substrate.md) | `myelin-substrate` + the 8 glue crates + `myelin-client` | The crates/conventions every service stands on: the bootstrap harness (`serve`), three-surface topology, the event-consumer template, the resilient client, backpressure + the protected human lane, fail-static, forward-only migrations, the observability baseline, the architecture lints. **The foundational doc the others consume.** | X-1…X-5; the §12 contract surface |
| F1 | [`identity-and-access.md`](./identity-and-access.md) | `myelin-identity` | ReBAC/Zanzibar engine: the one polymorphic `Principal`, `check`/`list_objects`/`list_subjects`, zookie consistency, the Leopard `list_objects` pre-filter, fail-static cache, per-run attenuable agent tokens, the delegation algebra, the pseudonym-map erasure lever. **The dependency root.** | AG-1 (one principal), AG-2 (delegation) |
| F2 | [`event-bus.md`](./event-bus.md) | `myelin-events` | The canonical envelope, the transactional outbox + relay, the durable streaming log (JetStream-class), the firehose split, the four reactive primitives (Event/Signal/Automation/Trigger), the `EventMatcher`, the taxonomy + `ArtifactRef` token table, retention/crypto-shred, reindex-from-source. **The nervous system.** | TE-10 (taxonomy), AG-7 (matcher), C-3 (token drift) |
| — | [`reference-graph.md`](./reference-graph.md) | `myelin-refs` | The URN resolution service, the event-sourced backlink inverse index (Postgres + recursive CTEs), the TE-7 typed-edge hybrid mirror, permission-filtered backlink reads via `list_objects`, sub-artifact refs, tombstoning, reindex-from-source. **The moat / connective tissue.** | TE-7 (D11 hybrid) |
| — | [`search-and-indexing.md`](./search-and-indexing.md) | (consumes others) | Tantivy index tier (FT + structured + vector/HNSW), the permission-aware query pipeline (the `list_objects` pre-filter — no leak/no N+1), BM25 + RRF hybrid, the query-AST compiler, embeddings-as-personal-data erasure, reindex-from-source as the only rebuild path. | SEARCH-1/2 |
| — | [`notifications.md`](./notifications.md) | `myelin-notif` | The ONE prioritised cross-subsystem "what needs me" inbox (C-9: "My Work"/"Activity" are scoped views into it), the Signal-driven router, storm-control/dedup, write-fanout vs read-fanout, backend humanisation (NOTIF-1), on-call/escalation on the durable-workflow substrate, the EU-sovereign delivery fabric, reindex-from-source. | C-9 (inbox overlap) |
| — | [`agent-fabric.md`](./agent-fabric.md) | `myelin-agent` | The strategy-pattern boundary: brain (`step`) + hands (`exec`), the platform-owned plan-then-apply loop, `EffectApi`, the permissioned `ToolDef` registry + MCP path, reserve/settle cost gate, the structural loop guards, HITL withhold→approve→resume, SKELETON→mock→real. | AG-3 (`handle` shape) |
| — | [`durable-workflow.md`](./durable-workflow.md) | `myelin-flow` | The durable-execution substrate: BUILD a Postgres-native DBOS-class engine (TE-20 resolved), deterministic replay/journaling, the durable-timer wheel (millions of SLA timers, SC-11), durable signals (multi-day HITL waits), activities + retry, the workflow↔agent mapping, the HITL approval-card round-trip. | TE-20 (build-vs-adopt) |
| — | [`storage.md`](./storage.md) | (Storage/`myelin-gdpr` seam) | The three tiers + OLAP read store (OLTP/object/log + ClickHouse), the KMS key hierarchy, crypto-shred + the GD-4 per-subject/per-tenant granularity rule, BYOK/HYOK + its capability ceilings, backup/restore-verification + the cross-seam consistency point (STOR-4) + post-restore re-erasure (GD-14). | GD-4, STOR-4, GD-14 (verification half) |
| — | [`gdpr-and-audit.md`](./gdpr-and-audit.md) | `myelin-gdpr` | POLICY + ORCHESTRATION: the `PersonalDataHolder` contract + the exhaustive holder list, the DSR orchestrator (fan-out/deadline/receipts/multi-cell), schema-level data-role/personal-data classification → the generated data map / RoPA, the retention engine + consent + sub-processor registries, the tamper-evident audit log (CT-style Merkle), the named Git-history-erasure reconciliation (GD-1). | DSR design (GD-3), GD-1, GD-2 |
| — | [`tenancy-and-control-plane.md`](./tenancy-and-control-plane.md) | `myelin-tenancy` | The cell topology in detail: cell anatomy + sizing, tenant→cell assignment, the isolation spectrum (logical/schema/cell) across every shared system, the PII-free global control plane, region-pinning at the data layer, cell discovery, multi-cell tenants (designed-not-built), self-host parity. | ADR-11's `[OPEN → P3]` backlog |

**Read order for a Phase-4 agent:** `00` first (the substrate everything consumes), then `identity-and-access`
+ `event-bus` (the two other foundational docs every system links), then the system most relevant to you.
Refs, Search, Notifications, the Agent Fabric, the Durable-Workflow engine, and Tenancy are all *consumers*
of the first three and say so explicitly. Storage and GDPR/Audit are a **policy/mechanism pair**: GDPR/Audit
decides *whether, when, and prove*; Storage owns *how* (the KMS, crypto-shred, restore).

---

## 3. Key committed designs + the prior art they stand on

The Phase-3 line is **ground every significant choice in proven prior art; deviate only in writing**. The
committed designs, with the literature each cites:

| Design committed | Prior art / proven system | Doc |
|---|---|---|
| **Transactional outbox** as the only emit path; `FOR UPDATE SKIP LOCKED` relay; `event_id` (ULID) broker-side dedup; at-least-once + idempotent ≈ effectively-once | Richardson *Microservices Patterns* (2018) outbox/polling-publisher; Debezium CDC; Kleppmann *DDIA* ch.11; Helland *Idempotence Is Not a Medical Condition* (2012); ULID spec | 00, Bus |
| **Durable streaming log = NATS JetStream** (Raft-replicated, durable pull consumers, subject filters, `Nats-Msg-Id` dedup, `MaxAge`) — Kafka/Redpanda reserved as the measured per-cell upgrade behind a `BusTransport` trait | Ongaro & Ousterhout *Raft* (USENIX ATC 2014); Kreps et al. *Kafka* (NetDB 2011); Kreps *The Log* (2013); JetStream docs | Bus |
| **ReBAC / Zanzibar** authorization: `check` + `list_objects` (Leopard set-flattened index) + zookies; SpiceDB-class store; macaroon/biscuit attenuable tokens for delegation | Pang et al. *Zanzibar* (USENIX ATC 2019); SpiceDB; OpenFGA; Birgisson et al. *Macaroons* (NDSS 2014); biscuit; NIST SP 800-162 (ABAC edges) | Id |
| **Permission-filtered reads at scale** — pre-filter via `list_objects`, never post-filter (no leak, no N+1) — the single most load-bearing inter-system contract | Zanzibar `list-objects`/Leopard + zookie "new enemy"; ADR-03 | Id, Search, Refs, Notif |
| **Reference graph = Postgres adjacency list + recursive CTEs** for shallow graphs; Leopard-style hot-reach index only when measured; the log is authoritative for edges; TE-7 hybrid (typed table = truth, Refs = projection) | Celko *Trees & Hierarchies in SQL* (2012); SQL:1999 `WITH RECURSIVE`; Karwin *SQL Antipatterns* closure table; Kreps *The Log*; Bush *As We May Think* (1945) | Refs |
| **Search = Tantivy** (Lucene-architecture, embedded → native ACL pre-filter); BM25 ranking; HNSW vector with filter-during-traversal; RRF hybrid fusion; trigram code-search v1; per-language analyzers | Zobel & Moffat (2006); Robertson & Spärck Jones (BM25); Malkov & Yashunin *HNSW* (TPAMI 2018); Cormack et al. *RRF* (SIGIR 2009); Cox *Trigram Index* (2012); Snowball stemmers, UAX #29 | Search |
| **Notifications = hybrid fan-out** (write-fanout the bounded high-signal mention/assignee set; read-fanout the unbounded ambient watcher set); deterministic-explainable ranking; two-tier storm-control; ICU-MessageFormat backend humanisation paired with a routable `ArtifactRef` | Silberstein et al. *Feeding Frontier* (VLDB 2010); Twitter *Timelines at Scale*; Facebook TAO (ATC 2013) + EdgeRank; Gmail Priority Inbox (2010); Helland (2012); ICU MessageFormat; EI-05 §6 | Notif |
| **Agent fabric = plan-then-apply** (brain `step` proposes, platform `EffectApi` validates+applies); stateless brain + platform-owned history; one sandbox for the hands; `agent.policy ∩ delegation ∩ tenant.policy` intersection; reserve/settle cost gate; structural loop guards | Yao et al. *ReAct* (ICLR 2023); MCP (Anthropic 2024); Saltzer & Schroeder (1975) least-privilege; Dennis & Van Horn (capabilities); gVisor/Firecracker (sandbox); Temporal/Cadence; Lamport (1978) | Agent |
| **Durable workflow = BUILD a Postgres-native DBOS-class engine** (TE-20 resolved); deterministic replay (Temporal model) + Postgres-embedded journaling (DBOS) so the journal commits in the same transaction as the outbox; the minute-bucket partial-index timer wheel for millions of SLA timers (SC-11); durable signals for multi-day HITL waits | Temporal/Cadence (event-history + deterministic replay); DBOS (Postgres-embedded journaling, transactional exactly-once); Restate; Vanlightly (2025) on determinism; Varghese & Lauck *Timing Wheels* (1987); `FOR UPDATE SKIP LOCKED` | Workflow |
| **Storage = three portable tiers + envelope-encryption KMS hierarchy + crypto-shred**; content-addressed blobs (BLAKE3, plaintext-hash-within-tenant-keyspace); GD-4 = a classification-driven per-subject-vs-per-tenant key rule; BYOK/HYOK with honest capability ceilings; WAL+PITR restore-verification with the event-offset cross-seam cursor | Git object model / Venti (FAST 2002) / IPFS CID / BLAKE3; AWS/GCP KMS envelope + NIST SP 800-57/38D + Vault Transit; NIST SP 800-88r1 + Boneh & Lipton (1996) crypto-shred; PostgreSQL WAL/PITR + ARIES (1992); Chandy–Lamport (1985) consistent snapshot; ClickHouse/C-Store | Storage |
| **GDPR/Audit = generated data map + DSR orchestrator + CT-style tamper-evident log** (hash-chain + Merkle, signed tree heads, inclusion/consistency proofs, external witness; deliberately *not* a blockchain); schema-level `data_role`/personal-data classification with a `no-untagged-personal-data` lint; tightest-policy-wins legal-hold-aware retention | GDPR Arts. 5/6/9/15–22/28/30/33–35/44–49 + Schrems II; Haber & Stornetta (1991); Merkle (1987); Crosby & Wallach (2009); Certificate Transparency (RFC 6962) / Trillian; NIST SP 800-88r1; Kleppmann *DDIA* ch.5 | GDPR/Audit |
| **Cell-based / bulkhead tenancy**: a cell = a complete region-pinned stack; scale = add cells; PII-free global control plane (placement before identity capture); region as compiled-in shard key; bin-packing placement (sticky, not hashed) | AWS Builders' Library shuffle-sharding + cell architecture; AWS Well-Architected REL10-BP04; AWS SaaS Lens (control/data plane; silo/pool/bridge); Karger et al. consistent hashing (STOC 1997); Lamping & Veach jump-consistent-hash (2014); GDPR Art. 44–49 + Schrems II | Tenancy |
| **Reindex-from-source** as the only recovery path for every derived store (Search, Refs, OLAP, Notif inbox, caches) — steady-state and recovery share one code path, so they cannot drift | EI-04 §5.3; Kreps *The Log*; the substrate consumer template | Bus, Refs, Search, Notif |
| **Crypto-shred + references-not-payloads + pseudonym indirection** as the erasure-vs-immutability resolution (delete the identity, not the fact) | Kleppmann *DDIA* ch.5 (tombstones); EI-04 §1; `gdpr-eu-sovereignty.md §6` | Id, Bus, Refs, Search, Notif, Workflow, Storage, GDPR |
| **Forward-only online migrations** (expand→backfill→contract; measure lock against a restore) | Stripe online migrations (2017); GitHub gh-ost; Vitess online DDL; Fowler ParallelChange | 00, Storage |
| **Resilience primitives** — timeout/breaker/bulkhead/jittered-retry in one client; bounded everything + SEDA backpressure; the protected human lane (weighted-fair-queueing shed order); fail-static bounded-staleness | Nygard *Release It!* (2018); Netflix Hystrix; Brooker (AWS, 2015) full-jitter; Welsh *SEDA* (SOSP 2001); Google SRE ch.21/22; RFC 5861 stale-while-revalidate; Zanzibar zookies | 00, all |

**Decisions that resolved open ADR/directive items** (closing these was the Phase-3 deliverable):
AG-1 → one `Principal` kind, three faces (Id §3); AG-2 → monotone intersection delegation via macaroon caveats
(Id §7); AG-3 → `Agent::handle` is a platform-owned bounded multi-turn loop (Agent §2.3); TE-10 →
subsystem-prefixed singular dotted taxonomy (Bus §6); AG-7 → `EventMatcher` = the query-AST predicate core, not
CEL/JSONLogic (Bus §4.5); TE-7/D11 → the typed-edge hybrid (Refs §3.3); **TE-20 → BUILD a Postgres-native
DBOS-class durable engine, not self-hosted Temporal (Workflow §2)**; **GD-4 → a classification-driven
per-subject-vs-per-tenant crypto-shred rule (Storage §5.1)**; **STOR-4 → the event-log offset is the cross-seam
restore cursor (Storage §7.3)**; **GD-14 → mandatory post-restore re-erasure against the erasure ledger
(Storage §7.5)**; **C-9 → one inbox; "My Work"/"Activity" are filtered views (Notif §1.3)**; transport →
JetStream with a written escape hatch (Bus §2.1); engine → Tantivy with an escape hatch (Search §2.1); REF-2 →
Postgres+CTEs over a graph DB (Refs §2.4); C-3 → the canonical `ArtifactRef` token table (Bus §6.2); the
**audit-log construction → CT-style Merkle, not blockchain (GDPR §6.1)**.

---

## 4. Required changes to the Phase-2 spine surfaced by Phase 3

Phase 3 reversed **no** ADR. The changes are **additive sharpenings** — new lints, confirmed seam usages, and
small harness extensions the spine should absorb:

| # | Change | Surfaced by | Nature |
|---|---|---|---|
| S-1 | **Add `residency-pin` lint** (every store carries the cell `region`; every write asserts `row.region == cell.region`, rejected at the boundary) to the substrate lint table (`00 §2.11`). | Tenancy §8/§12.3 | New lint; the harness already injects region, so this turns it into an enforced invariant. |
| S-2 | **Add `control-plane-pii-free` lint** (no control-plane column classified `is_personal=true`, run through the generated data-map). | Tenancy §3.3/§12.3 | New lint; asserts the control plane's PII-free property at build. |
| S-3 | **Add `search-requires-acl-filter` lint** (no query path reaches the index engine without a composed `list_objects` filter — sibling to `no-raw-publish`). | Search §9.5 | New lint; makes "permission-aware by construction" a compile-time property. |
| S-4 | **Add `no-llm-in-platform` lint** (no model/SDK/prompt/model-name string outside `LlmAgentRuntime`). | Agent §11.5 | New lint; sibling to `no-host-exec`; enforces ADR-08.2. |
| S-5 | **Add `no-untagged-personal-data` lint** (a field of a personal-data type fails to compile without a `#[personal_data(...)]` tag) feeding the generated data map. | GDPR §2.1 | New lint; pushes "we forgot the search index" all the way down to the field. |
| S-6 | **Add `flow-determinism` lint** (a workflow function may not read a clock/RNG/IO outside `WfCtx`; it fails to compile). | Workflow §2.5/§10.3 | New lint; sibling to `no-host-exec`; ships with the engine. |
| S-7 | **Harness query builder asserts `row.region == cell.region` on write** (mechanical, not per-service); also threads the cell `region` into every store handle. | Tenancy §15.2 | Small harness extension; backs S-1. |
| S-8 | **`pii_key_ref` value grammar canonicalised** to `kms://<tenant>/<dek-epoch>/<class>` where `<class> ∈ {tenant, subject:<id>, blob}` (field name unchanged; value grammar reconciled so it resolves the per-subject DEK case). | Storage §12.1 | X-5 reconciliation; additive grammar, no signature change. |
| S-9 | **Confirm the DSR orchestrator iterates `member_cells`** for a multi-cell tenant's erasure/export; **erasure ledger** (PII-free, non-shreddable) exists as a GDPR/Audit holder so post-restore re-erasure can run. | Tenancy §10.4; Storage §7.5; GDPR §7/§4.4 | Confirmation of already-named seams; multi-cell mechanism is P4. |
| S-10 | **`list_objects` `Filter` must be consumer-composable over an arbitrary id column** (push-down, facet-expressible, not opaque-id-only at scale) — confirmed for Search (over `doc_id`) and Refs (over edge `source`). | Search §9.1, Refs §8.1 | Usage confirmation of Id §8.2's `Filter{set_expr, zookie}`; an X-5 reconciliation anchor. |
| S-11 | **`mint_run_token` must be callable mid-workflow on resume** (a multi-day HITL workflow re-mints its short-lived agent token when it resumes; the workflow holds no long-lived privileged token). | Workflow §10.2 | Small Id contract note; no signature change. |

The six new lints (`residency-pin`, `control-plane-pii-free`, `search-requires-acl-filter`,
`no-llm-in-platform`, `no-untagged-personal-data`, `flow-determinism`) join the substrate's existing
`no-cross-db` / `no-raw-publish` / `tenant-predicate` / `no-host-exec` / `forward-only-migration` /
`no-cross-sync-cycle` set (`00 §2.11`), all committed to CI (E-4: an uncommitted gate is no gate).

**No remaining spine gap.** The earlier README (generated against an incomplete set) flagged "Notifications has
no dedicated Phase-3 doc." That gap is now **closed**: [`notifications.md`](./notifications.md) is a full
Phase-3 design (it resolves C-9, owns NOTIF-1/2/3, and feeds the Agent HITL approval-card humanisation path).
GDPR/Audit and the Durable-Workflow substrate — previously only implied across the foundational docs — are also
now full Phase-3 designs. The shared-systems layer is complete at Phase-3 altitude.

---

## 5. What Phase 4 (subsystems) must build on — per subsystem

Every subsystem becomes part of Myelin by **depending on the glue crates and implementing the three glue
contracts** (ADR-13). The build-to surface is consolidated in [`contract-index.md`](./contract-index.md); the
per-subsystem obligations:

- **Every subsystem (all five)** must, at minimum: call `serve(AppSpec)` (or the cross-language equivalent,
  §6); emit every state change via `OutboxTx::emit(draft, cause)` (the only emit path); implement the
  **`project(ref, viewer) → {title, state, icon, render_hint, sub_anchor?}`** projection API (the only way
  another system reads about its artifacts — no cross-DB); implement **`replay(scope, since)`** emitting
  `*.snapshot` events (so Search/Refs/Notif reindex-from-source); register its `ToolDef`s into the
  `ToolSurface`; declare its `IndexSpec`; declare its ReBAC namespace fragment; tag every personal-data field
  `#[personal_data(...)]` and implement `PersonalDataHolder`; honour the **restriction flag** (no
  indexing/agent-use/analytics/notification for a restricted subject); mint stable `ArtifactRef`s down to
  **sub-artifact granularity** with a stable `#sub` scheme; own its **complete event taxonomy** under the Bus
  §6 grammar; declare its **`watcher` relation** for notification read-fanout; flag its **hot tables** for the
  forward-only-migration lint; set its **per-surface shed budgets** and **`requires_approval` defaults** for
  agent tools.

- **Git** — owns repo/PR/commit/ref namespaces + the `git.*` taxonomy; **must emit an indexable code
  projection** (path/symbols/literals/commit-message per blob/ref) for Search code-search v1; owns the
  per-ref ordering at push QPS (the aggregate = the ref); **owns git-history author/email erasure** —
  pseudonymous-commit-by-default is a *commit-time prerequisite* that gates the git data model (GIT-1), plus
  the supported history-rewrite path and the documented lawful-basis residual (the GD-1 reconciliation,
  GDPR §7; the half the Bus did *not* solve); keeps the pack tier object-backable/relocatable (STOR-5).

- **CI** — owns the `ci.*` taxonomy + `ci.log.available` pointer events (logs ride the firehose, never the
  durable bus); **owns the one unified sandbox runner** (ADR-20) including the `kind=agent` job spec and the
  **sandbox-escape drill on a real kernel** — the single hard gate before any agent runs untrusted code; CI
  job tokens minted through Id; the reserve/settle cost gate's CI wiring (CI-2/D8); a CI pipeline is a
  **durable workflow** whose stages/steps are activities on the runner (Workflow §5.3/§11.7).

- **Issues** — owns the `issue.*` taxonomy + the **TE-7 typed relation table** (`issue_relation`, the *source
  of truth* for `blocks`/`blocked_by`/`closes`/`depends_on`/`parent`/`relates`); the field/transition
  visibility caveats (ABAC edges); the stateful **Trigger** "unblock/remind me when…" UX (its `stale_after`
  is a `myelin-flow` durable timer); **SLA timers ride the workflow timer wheel** (SC-11); the Issues "My
  Work" hub is a scoped *view* into the one Notif inbox (C-9); co-owns the shared field-definition/view
  primitive (ADR-06) with Knowledge.

- **Knowledge** — owns the `knowledge.*` taxonomy + page-tree-inheritance-with-overrides namespace; **owns the
  collab op-stream resume-cursor durable transport** (KN-1, built first — the reconnect-loses-zero-ops drill is
  theirs); **must accept a content-addressed write of an agent execution trace** (AG-7) and register it as an
  erasable holder; owns the `db_relation`/`page_parent` typed tables; chooses CRDT-vs-OT (TE-15).

- **Chat** — owns the `chat.*` taxonomy; **is the HITL approval-card surface** (the withhold→approve→resume
  bridge renders here; the `approval` signal is posted to the durable workflow); the Chat "Activity/Mentions"
  inbox is a scoped *view* into the one Notif inbox (C-9); presence/typing/read-state ride the ephemeral
  firehose, never the durable bus; the real-time connection tier is the most likely Rust divergence (TE-21)
  and must still emit/consume the Rust-defined envelope over the wire and implement `PersonalDataHolder`.

---

## 6. Floors named (within Phase 3) and their follow-ons

Per VISION §3 (name your floors), the Phase-3 docs ship these partial answers with named follow-ons:

| Floor | What ships | Follow-on owner |
|---|---|---|
| **Reactive/dispatch tier** | The consumer template is the floor; the Signal-curation/Automation/stateful-Trigger dispatch tier is its own separately-reviewed design (Bus §4.7). | Bus (built in P3) |
| **Cross-cell fan-out (multi-cell tenants)** | Single-cell is complete; cross-cell event/backlink/search/inbox/workflow/DSR fan-out for a 10k-org spanning cells is designed-not-built (Bus §7.4, Refs §6.5, Search §6.4, Notif §5.4, Workflow §7.4, GDPR §4.4, Tenancy §10). | P4 control plane + SC-2/SC-3 |
| **`LlmAgentRuntime`** | SKELETON + Mock are built; the LLM adapter seam is fixed but not built. | P6 (post safety drills); LEGAL for sub-processor (AG-9) |
| **External MCP server** | The `exposed_over_mcp` flag + internal consumption are built; the external endpoint + threat model are deferred. | P4/P6 + Legal |
| **Agent long-term memory / RAG over prior runs** | A named holder seam; v1 agents are stateless across runs bar the trace document. | P4 (Search/Knowledge) |
| **EU-sovereign delivery providers (Notif)** | The `DeliveryAdapter` trait + the EU-preferring/redaction posture are built; the concrete production email/push vendor + DPA are deferred (a mock adapter ships for dev). | P4 Notif + DPO |
| **ML-tuned notification ranking** | v1 is a deterministic, explainable score behind the same scoring interface; the ML ranker is promotion-triggered by a measured "important-buried" signal. | P4/P5 Notif (measured) |
| **Durable-workflow history compaction / archival; cross-cell workflow spanning** | Single-cell durable execution is built; continue-as-new compaction + an object-store archival tier + cross-cell spanning are designed-not-built. | P4 (measured) / control plane |
| **Hot-artifact reach index (Refs R4) / column-store seam (Bus BUS-6) / OpenSearch upgrade (Search) / IVF-PQ vectors / Temporal escape hatch (Workflow)** | The default path is built; the materialised/upgraded tier is promotion-triggered by *measured* volume. | P4/P5 (measured) |
| **Live tenant migration + rebalancing** | Sizing-headroom + sealing avoid it; online cell→cell migration is specified-not-built. | P4 control plane + Storage/GDPR |
| **Code-search semantic/AST tier** | v1 is symbol/path/literal/trigram-grade; AST/cross-reference is the follow-on. | P4 Git + Search |
| **BYOK/HYOK per-content-class policy** | The three-level mechanism + the honest capability ceilings (`can_derive_plaintext_index()`) are decided; the per-class policy + KMIP adapter + legal posture are deferred. | P4/LEGAL |
| **Git-history author/email erasure (GD-1)** | Crypto-shred + references-not-payloads solve everything keyed under a destroyable DEK; pseudonymous-commit-by-default is a commit-time prerequisite; the git-history half (commit-object bytes) is explicitly *not* solved — history-rewrite vs a documented lawful-basis limit. | P4 Git + LEGAL |

---

## 7. Cross-references

- [`contract-index.md`](./contract-index.md) — the consolidated interface/contract map (the Phase-4 build-to
  surface): owner, consumers, definition site for every cross-system contract, across all 11 docs.
- [`drills-and-open-questions.md`](./drills-and-open-questions.md) — the consolidated drill inventory (feeds
  Phase-5 testing strategy), the de-duplicated open questions tagged by resolver, and the consistency pass.
- [`VISION.md`](../../VISION.md); the doctrine ([`EI-02`](../../external-insights/02-platform-substrate.md),
  [`EI-04`](../../external-insights/04-hard-problems.md), EI-03 for agents/workflow, EI-05 for notifications).
- Spine: [`architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md) (ADR-01…20);
  [`shared-systems-overview.md`](../02-holistic-architecture/shared-systems-overview.md);
  [`consistency-review.md`](../02-holistic-architecture/consistency-review.md) (C-3, C-9 etc.).
- Binding: [`integration-directives.md`](../02b-doctrine-integration/integration-directives.md);
  [`decision-record.md`](../02b-doctrine-integration/decision-record.md).
