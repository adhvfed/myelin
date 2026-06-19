# Phase 4 → Phase 5 — Consolidated Cross-Subsystem Change Requests

> The **de-duplicated, grouped** list of every required shared-system change the five Phase-4 subsystems
> asked for, plus the cross-subsystem **conflicts** and **open questions**. This is the **primary input to
> Phase 5 reconciliation** (the pass that refines the shared layer + rewrites the Phase-4 docs against it).
> Sources: each subsystem's `architecture/06-shared-system-change-requests.md` + `07-drills-and-open-questions.md`.
> Subsystem keys: **GIT** (git-hosting), **CI** (continuous-integration), **ISS** (issue-tracker),
> **KN** (knowledge-platform), **CHAT** (chat). Date: 2026-06-19.
>
> **Headline:** No subsystem requests an ADR reversal. The overwhelming pattern is *confirmation* or *small
> additive sharpening* of an already-frozen Phase-3 seam; a handful are genuinely **NEW**, and several are
> `[OPEN → LEGAL]` policy deliverables. The single most-repeated ask across all five is the **`list_objects`
> `Filter{set_expr}` push-down composable over an arbitrary subsystem id column** (the S-10 family).

---

## 1. Identity & Access

| Ask | Requested by | One-line | Nature |
|---|---|---|---|
| **`list_objects` push-down over an arbitrary id column** (S-10) — `Filter{set_expr, zookie}` consumer-composable / facet-expressible, no N+1, no post-filter | **GIT** (repo/PR ids), **CI** (`run_id`), **ISS** (`issue.id`, *blocking*), **KN** (`database_row`), **CHAT** (channel/artifact + `message` ids) | The leak-free, ACL-pre-filtered list/board/search scan all five depend on; conjoin the ACL filter into the native query | CONFIRM (the most-repeated ask) |
| **Subsystem ReBAC namespace fragment compiled into the cell schema** | **GIT** (ref-glob-scoped relations + CODEOWNERS-as-relations), **CI** (`ci_project/environment/secret/run` + the `read & !is_untrusted_fork` ABAC edge), **ISS** (`issue` namespace), **KN** (page-tree inheritance-with-overrides + row-level + field-level caveat), **CHAT** (`channel.read = member + parent_project->read`) | Each declares its fragment; Id owns the engine | NEW fragment over an existing seam (Id §5) |
| **Field/transition-level ABAC caveat at `check`-time, kept off the hot `list_objects` path** | **ISS** (field.view + transition approver), **KN** (field-level column hiding per db) | Field-level hiding degrades to all-or-nothing without it; needs the caveat-context shape | RECONCILE (Id §9) |
| **`resolve_pseudonym` / `erase` keyed on the subsystem's author/subject pseudonym** + pin the pseudonym grammar (`<pseudonym>@<tenant>.noreply`) | **GIT** (git author pseudonym, the DSR step-1 lever; GIT-1) | Delete the map ⇒ commit bytes hold only the pseudonym | CONFIRM + **NEW** grammar pin |
| **Machine-identity resolution: SSH-pubkey / deploy-key / PAT / per-job token → Principal** | **GIT** (SSH + smart-HTTP front door; deploy keys = repo-scoped machine principals), **CI** (per-job attenuated token mintable mid-workflow on resume; self-hosted runner token scoped to one tenant's `SelfHosted` jobs) | The auth front door + the scheduler→runner handoff | CONFIRM (Id §4 / S-11) + **NEW** self-hosted-scope specificity |
| **A zookie returned from `write_tuples`, stamped on the object** (the "new enemy" guard) | **KN** (`page.acl_zookie`), **CHAT** (membership writes) | A just-revoked grant must not read stale on the next collab/read authz | CONFIRM (Id §6/§8.4) |
| **`mint_run_token` callable mid-workflow on resume** | **CHAT** (a days-later approval resumes under a fresh attenuated token), **CI** (re-mint on stage resume) | A multi-day HITL workflow holds no long-lived privileged token | CONFIRM (S-11) |
| **`list_subjects(channel, watcher)` performant for read-fanout at chat/large-channel density** | **CHAT** (the ambient unread set on a 50k-member channel) | A slow `list_subjects` defeats the read-fanout half of the fanout boundary | CONFIRM (Notif §8.3 usage, Id-served) |

## 2. Event Bus (incl. firehose)

| Ask | Requested by | One-line | Nature |
|---|---|---|---|
| **`<subsystem>.*` taxonomy registered under the §6 grammar** | **GIT**, **CI**, **ISS**, **KN**, **CHAT** (all) | Each subsystem owns + completes its dotted-name list; Bus validates | CONFIRM (the P4 taxonomy completion the Bus seeded) |
| **New type tokens added to the §6.2 token table** | **ISS** (`initiative`) | A ranked `issue`-family type alongside the seeded set | NEW token (sanctioned extension) |
| **Per-aggregate ordering at production QPS, no lost/ghost events** | **GIT** (per-**ref** aggregate at push QPS), **CHAT** (per-**conversation** total order at scale) | The aggregate is the ref / the conversation; the outbox order must == the state-change order | CONFIRM (Bus §2.3; the shared D-9 drill) |
| **Firehose sized for the heaviest producers + `tail(stream, range)` fan-out** | **CI** (`ci.log.appended` — heaviest firehose producer), **KN** (collab op-stream + presence), **CHAT** (live delivery + presence + agent streaming partials) | Per-line / per-op volume rides the firehose; the durable bus carries only pointer/summary events | CONFIRM + sizing (Bus §4.3) |
| **`replay(scope, since)` supports sub-artifact-granular `*.snapshot`** | **CI** (one run/deployment/project scope), **KN** (page-subtree at block granularity), and all (cold rebuild + post-restore re-erasure) | Search re-indexes / Refs re-derives at sub-artifact granularity | CONFIRM (Bus §4.9) |
| **`EventMatcher` cost-bound suffices for trigger predicates (no per-subsystem CEL); relational/projection-state conditions** | **CI** (`on: pull_request`/`issue.transitioned` filters), **ISS** (`arm_trigger` over `issue_relation` projection state — "all `blocked_by` resolved") | Subsystems must not invent a trigger DSL; the matcher must express a relational condition | CONFIRM / RECONCILE (Bus §3.6/§4.5) |
| **Firehose presence/typing/read-state/partial subject grammar + TTLs + durable-vs-firehose split** | **CHAT** (`fan.<tenant>.<channel>`, presence/typing/partial subjects) | The live tier + agent streaming ride NATS core as an agreed seam, not a Chat-private transport | CONFIRM (Bus §4.3 usage) |

## 3. Reference Graph

| Ask | Requested by | One-line | Nature |
|---|---|---|---|
| **`#sub` sub-artifact grammar + outdated/tombstoned-anchor semantics** | **GIT** (`#comment-`/`#thread-`/`#L42-L88` content-anchored line range → partial/tombstone when content gone), **KN** (`#b<block>`/`#h<block>`/`#comment`/`#row` stable across edits/moves), **CHAT** (`#message-<id>`/`#thread-<root>`) | Refs stores the full sub-URN + the `#sub`-stripped root and degrades gracefully; the *outdated-line-range* case is git's new specificity | CONFIRM (Refs §3.5) |
| **TE-7 typed-edge mirror carries each subsystem's rel vocabulary** | **ISS** (`issue_relation` = source of truth; `parent`/`blocks`/`closes`/...), **KN** (`db_relation`/`page_parent`; `parent`/`relates`/`rollup_source`) | Subsystem owns the edge truth; Refs holds the rebuildable projection + fixes the inverse pairing | CONFIRM (Refs §3.3 / REF-1) |
| **Edge production from commit trailers / PR links / two-way db relations; best-effort eventual inverse** | **GIT** (`Closes ISSUE-412`, `Co-authored-by` → `refs.edge.created`), **KN** (forward edge transactional, inverse projection lags) | The context pane / backlinks; reindex-from-source is the drift correction | CONFIRM (Refs §4.1) |
| **Reconcile "the human key IS the ArtifactRef id" with REF-3 (display keys render-time)** | **ISS** (*blocking*: `ENG-1421` is the canonical `<id>` segment; `#1421` is the render-time projection) | The ArtifactRef id grammar must be agreed before keys are minted | RECONCILE (REF-3 vs ISS doc 01 §7) |

## 4. Search

| Ask | Requested by | One-line | Nature |
|---|---|---|---|
| **Code-/content-/struct-shaped `IndexSpec`, incremental, ACL-aware via `list_objects`** | **GIT** (code projection: path/symbols/literals/commit-message + trigram, camel/snake tokenizer), **ISS** (Tier-3 escalation: a board query over budget compiles to a Search query, ACL-pre-filtered — *blocking for Tier-3*), **KN** (block-level + page-level docs, multilingual, **vector-in-v1** for RAG; struct queries over flexible JSONB fields) | The escalation valve / code search / RAG; per-subsystem projection shapes | CONFIRM (Search §4.4/§3.2/§2.2) |
| **Embeddings purged with their source on `*.erased`** | **KN** (Knowledge content vectors), **CHAT** (message vectors; HYOK `can_derive_plaintext_index()=false` skips indexing) | Embeddings are personal data; an erasure leak via the index otherwise | CONFIRM (Search §4.8 / §erasure) |
| **Projection-feeder frequency signal to drive measured promotion** | **ISS** (which custom facet is filtered often → generated index) | The GIN index serves until measured promotion | CONFIRM + a feeder signal |
| **Consume CI-produced SCIP/LSIF** for "find usages" | **GIT** (GF-3 follow-on, jointly with CI) | Semantic code search as a later index input | NEW (future; named) |

## 5. Notifications

| Ask | Requested by | One-line | Nature |
|---|---|---|---|
| **"My Work" / "Activity/Mentions" is a `list_inbox(principal, filter)` view over the ONE inbox (C-9), never a second store; shared read-state** | **ISS** (*blocking*: assigned/blocked/needs-approval/overdue = reason/subject filters), **CHAT** (Activity/Mentions is a filter, not a store) | Mark-once-consistent-everywhere; no parallel inbox | CONFIRM (Notif §1.3/§3.1) |
| **`define_notif_rule` set + `humanise` templates each subsystem registers** | **ISS** (SLA at-risk/unblocked/approval-requested → reasons/priorities), **KN** (mentions/comments/shares/watched-page changes), **CHAT** (mentioned/replied/thread_watched/approval_requested + agent-message strings) | Map Signal classes to inbox reasons/priorities; backend humanisation paired with a routable `ArtifactRef`, never a frontend string map | CONFIRM (Notif §3.1/§9, NOTIF-1) |
| **`watcher` relation declaration + read-fanout** | **ISS**, **KN**, **CHAT** (all declare their watcher set) | Watched pages/channels/issues feed the one inbox | CONFIRM (Notif §8.3) |
| **Escalation-chain config shape (`oncall_now`/`page`)** | **ISS** (an SLA breach starts a durable escalation workflow) | Issues passes the chain-definition shape to Notif | CO-DESIGN (Notif §3.7) |

## 6. Agent Fabric

| Ask | Requested by | One-line | Nature |
|---|---|---|---|
| **`ToolHands::exec` realised by CI's runner as the `kind=agent` job on the same hardened sandbox; the escape drill gates both kinds** | **CI** (the deepest unification, HP-5), implicitly relied on by all agent-authoring subsystems | Untrusted execution built + drilled once; metered into the same wallet | CONFIRM (Agent §11.2 / contract 8.4) |
| **Subsystem `ToolDef` set + `requires_approval` defaults registered into the one `ToolSurface`, MCP-exposable** | **CI** (deploy/approve/secret-write gated), **ISS** (forecast/triage/SLA-draft tools), **KN** (publish/confidential-edit gated + approver set), **CHAT** (chat ToolDefs via `EffectApi`) | The agent-tool surface; the gated defaults are each subsystem's product call jointly with the fabric | CONFIRM (Agent §6 / contract 8.1) |
| **`agent_needs_human` HITL gate enforced; agent legibility metadata carried** | **GIT** (an agent cannot bypass required human approval on `git.merge`/`open_pr`; `is_agent`/`agent_run` rendered distinctly) | The agent-vs-human merge policy + the AI-Act labelling | CONFIRM (ADR-08 / AG-8) |
| **Accept a content-addressed agent-trace write (AG-7) + register it as an erasable holder** | **KN** (`write_agent_trace` reuses the block model) | The Fabric fixes the seam; Knowledge is the deliverable | CONFIRM (Agent §11) |
| **Explicit-first dispatch honoured** (a mention notifies, does not auto-spawn a costed run) | **CHAT** (CHAT-1; reserve/settle gates even the explicit run) | No bespoke loop logic; honours the platform's structural guards | CONFIRM (CHAT-1 / AG-6) |

## 7. Durable Workflow

| Ask | Requested by | One-line | Nature |
|---|---|---|---|
| **`SCHEDULE_AND_RUN_JOB` activity + `ci.result:<job_id>` durable-signal handshake** — a long-parked activity whose completion is a signal (hours later), idempotent on `idem_token` | **CI** (the seam between CI's scheduler and the engine) | The pipeline-as-workflow mapping | **NEW** (a concrete activity-vs-signal pattern over Workflow §4.4) |
| **`DurableExecutor::signal` idempotency + per-effect `idem_key` for batch/partial approval** | **CHAT** (`idem_key=card_id`; `card_id:<effect_idx>` for a multi-effect card) | A double-click is one approval; a partial approval is well-defined | CONFIRM + small joint decision (Workflow §6.3) |
| **Multi-day HITL gate via `wait_for_signal` holding no runtime, woken by an `approval` signal** | **CI** (protected-env deploys), **CHAT** (approval card), **KN** (approval-card resume), **ISS** (escalation as a durable workflow) | The HITL bridge across four subsystems | CONFIRM (Workflow §3.4 / contract 9.4) |
| **Reserve/settle as the workflow's bookends (refuse-on-exhaustion, never interrupt in flight)** | **CI** (the one metering path), **ISS**/**KN**/**CHAT** (via `EffectApi` reserves) | The universal cost gate | CONFIRM (Workflow §6.2 / CI-2) |
| **Maintenance ops (large repack / bundle gen / history-rewrite) as resumable activities; SLA timer re-arm on pause/resume** | **GIT** (GC/maintenance at fleet scale), **ISS** (*blocking*: cheap disarm/re-arm of the precomputed `fire_at` without polluting the wheel with calendar logic) | Resumable activities + cheap timer re-arm | CONFIRM (Workflow §4.2 / §9.5) |
| **Durable timers + signals for scheduled/living-doc automations** | **KN** (daily-notes, living-doc maintenance ride the timer wheel) | `DurableExecutor::start` + the timer wheel | CONFIRM (Workflow §9) |

## 8. Storage (BlobStore / KMS / log tier / OLAP)

| Ask | Requested by | One-line | Nature |
|---|---|---|---|
| **Per-subject DEK for free-text / op-log / body columns (GD-4) — the crypto-shred lever** | **GIT** (PR/review/comment bodies; reflogs/bitmaps/pack backups), **CI** (free-text PII in log segments — GD-6 floor), **ISS** (`issue` row free-text + change-log), **KN** (free-text blocks + PII-bearing ops + agent trace), **CHAT** (message bodies/drafts — the canonical GD-4 case) | An individual's erasure crypto-shreds exactly their content reachable in immutable storage + backups | CONFIRM (Storage §5.1) + **NEW** per-subject granularity for CI logs |
| **Object-backed pack/delta over `BlobStore` + smart-transport read path from object blobs** | **GIT** (the STOR-5 follow-on, GF-1) | Pack chunking + delta-base selection + serving from object tier | CONFIRM the seam; the **impl is the git P4 deliverable** (TE-24) |
| **Within-EU CDN-distributable clone/bundle blob class** | **GIT** (hot-repo/clone-storm acceleration) | A named blob class + within-EU CDN posture beyond base `BlobStore` | **NEW** |
| **A T3 log tier sealing firehose frames into T2 content-addressed segments + an OLTP `(job,step,byte-range)` range index, at CI volume** | **CI** (the durable log archive + jump-to-failure; heaviest consumer) | Append-mostly, per-tenant-DEK, byte-range index keyed by `(job, step)` | CONFIRM (Storage §3.3) + CI specificity |
| **Trust-tier/branch-scoped cache namespaces in `BlobStore`** | **CI** (an `UntrustedFork` write cannot reach the trusted cache scope) | A scope-key convention over per-tenant `BlobStore` | **NEW** |
| **Content-addressed snapshot/media blobs, crypto-shred on erase; restore-consistency cross-seam (rows ↔ blobs ↔ op-log ↔ index ↔ offsets); Scylla hot-tier promotion seam** | **KN** (CRDT snapshots + media, BLAKE3; row↔blob↔offset consistency), **CHAT** (`MessageStore` Scylla promotion residency-pinned + crypto-shred per cell) | Immutable-blob erasure = destroy the key; the restore-verify drill asserts cross-seam consistency | CONFIRM (STOR-1/STOR-4 / measured follow-on) |
| **OLAP read store accepts the subsystem's event stream + honours the restriction flag** | **ISS** (*partially blocking*: `issue.*`/`sla.*`/`cycle.*` for CFD/cycle-time/velocity; no analytics for a restricted subject) | Reports depend on OLAP; restriction-flag honouring is a compliance gate | CONFIRM (Storage §3.4) + restriction-flag propagation |

## 9. GDPR / Audit + Legal `[OPEN → LEGAL]`

| Ask | Requested by | One-line | Nature |
|---|---|---|---|
| **Harness auto-registration of every store as an erasable `PersonalDataHolder`; `restrict` suppresses indexing/agent-use/analytics/notif** | **CI** ("we forgot the cache table" must be structurally impossible), **ISS**/**KN**/**CHAT** (every store) | The structural erasure guarantee | CONFIRM (contract 1.4 / 10.1) |
| **The free-text / immutable-content erasure residual — documented lawful-basis limit, co-owned with Legal/DPO** (the GD-1 / GD-6 family) | **GIT** (Art. 17 reach into immutable commit bytes — `[OPEN — LEGAL]`), **CI** (per-subject vs per-tenant DEK for inline log PII), **ISS** (third-party free-text mentions — GD-6), **KN** (free-text PII residual — GD-6), **CHAT** (a name typed into another user's un-erased message body) | Structured PII erases reliably; the free-text / immutable-byte residual is a ratified-posture deliverable, not a checkbox | **NEW** policy artifact (5× the same legal seam) |
| **History-rewrite as an audited, tamper-evident, rate-limited tenant op with fork/mirror/clone-cache invalidation fan-out** | **GIT** (the erasure-admin tool) | A git-specific audited-op + invalidation surface | **NEW** |
| **Worklog/productivity/estimate field sensitivity classification** (works-council / labour-law — GD-13) | **ISS** (are these special-category / works-council-consultable in EU jurisdictions?) | Drives whether they are `#[personal_data]`-tagged with a restricted `data_role` | **NEW** legal classification |
| **Build-data-as-LLM-training lawful basis (AG-8); CD-as-PaaS product scope (PR-5)** | **CI** (flagged, not foreclosed) | Future agent-on-CI-data + CD product scope | **NEW** `[OPEN → LEGAL]` |

## 10. Tenancy / Control plane (residency, multi-cell)

| Ask | Requested by | One-line | Nature |
|---|---|---|---|
| **`residency_verify(tenant)` covers the subsystem's stores; `placement_of`/`discover` usable by the front door; repos relocatable** | **GIT** (`placement_of(repo)` → cell + group, region-pinned, relocatable; `discover` for the git wire), **CI** (runner pool + log/artifact/cache region; the no-global-pool property attestable) | Front-door routing + residency reject; the EU-sovereign pitch is attestable | CONFIRM (Tenancy contract 12.2/12.4) + repo-granular specificity |
| **Cross-cell PII-free pointer bridge (carries only `subject`/`type`/`correlation_id`; per-viewer resolution always cell-local)** | **ISS** (cross-cell portfolio rollup — named floor), **KN** (cross-cell collab — designed-not-built), **CHAT** (single-home-cell floor + cross-org channels) | The seam the multi-cell + cross-org follow-ons ride | CONFIRM (Tenancy §10 — the named multi-cell floor) |
| **Outbound push-mirror to a foreign host = residency boundary crossing → policy-gated at the control plane** | **GIT** (mirror config) | A residency policy gate on outbound mirror | **NEW** |

## 11. Substrate / shared crates / cross-cutting

| Ask | Requested by | One-line | Nature |
|---|---|---|---|
| **Cross-language harness shim frozen** (three-surface topology, liveness≠readiness, no fire-and-forget emit, `PersonalDataHolder`, resilient-client, shed order, forward-only migrations) | **CHAT** (the BEAM/TE-21 escape hatch is only admissible if this is frozen) | Keeps the hatch real even if never used; a no-op if Chat stays Rust | CONFIRM (substrate §13 Q1) |
| **Hot-table flags for the `forward-only-migration` lint (expand→backfill→contract, no blocking `ALTER`)** | **KN** (`block`/`db_row`/`doc_op`), and implicitly all high-write subsystems | Every subsystem declares its hot tables | CONFIRM (substrate §9) |
| **Per-surface shed budgets (in-flight caps + protected-human-lane reservation) tuned to the subsystem's load profile** | **CI** (CI-surge), **KN** (collab op-stream + hot-doc read storms), **CHAT** (connection-storm + agent-mention-storm) | The 30×-agent-surge / connection-storm drills assert against these | CONFIRM (substrate §7) + the subsystem's P4 budget call |
| **WASM compile target for `myelin-content`** | **KN** (the one editor render path reuses the Rust core client-side) | `render(parse(md)) === md` on identical code | CONFIRM (build-system) |
| **`myelin-query` primitive parity (field-type enum, view-model, AST grammar, `order_key`/LexoRank encoding); ADF→`myelin-content` converter fidelity** | **ISS** (co-owns `myelin-query` with KN per ADR-06; needs the lossy-node map for import) | Encoding drift would block a future shared CRDT/render path | CONFIRM (ADR-05/06) + co-design |

---

## 12. Cross-subsystem CONFLICTS noticed

These are the places two subsystem designs touch the *same* shared model and must be reconciled in Phase 5
before the contracts are frozen. None is a contradiction of an ADR; each is a **seam two subsystems
approached from different sides**.

| # | Conflict / divergent assumption | The two sides | Resolution lever for Phase 5 |
|---|---|---|---|
| **X-1** | **The Git↔CI checks / merge-gate contract is the tightest seam and is *not yet jointly specified*.** Git assumes a `check_status` per `(commit_oid, context)` driving its merge gate; CI assumes it emits `ci.status.updated` keyed `(commit_oid, context)` last-writer-wins per context and the merge-queue workflow wakes on a `ci.result` signal. Both name it as a top open question (GIT OQ-3, CI OQ-7, CI D-8). | **GIT** (merge gate, `02 §6`) vs **CI** (`ci.status.updated`, `03`/`07`) | **Jointly design the `check_status` shape + fork/trust-tier gating signals + re-run supersession + the merge-queue signal in Phase 5** (the most load-bearing cross-subsystem seam). |
| **X-2** | **The shared content model `myelin-content` is led by Knowledge but consumed by Chat and Issues with different fidelity needs.** Chat reuses the block/inline AST ("share the AST, not the editor"); Issues needs an ADF→`myelin-content` lossy-node map for import; Knowledge owns the taxonomy + the concurrency engine. Risk: Chat/Issues assume nodes Knowledge hasn't committed to the taxonomy, or the converter is lossier than Issues' import assumes. | **KN** (leads, ADR-05) vs **CHAT** (consumes AST) vs **ISS** (import converter, CR-9) | **Knowledge fixes the canonical block/inline node taxonomy + the lossy-map; Chat/Issues confirm their consumed subset** in Phase 5. |
| **X-3** | **`myelin-query` co-ownership encoding must be byte-identical or a future shared CRDT/render path breaks.** Issues owns its AST→store compiler; Knowledge owns its flexible-DB execution; both share the field-type enum, view-model, AST grammar, and the `order_key`/LexoRank encoding (base, jitter). Risk: encoding drift between "a row dragged in a Knowledge db" and "an issue dragged in a backlog." | **ISS** (CR-10) vs **KN** (ADR-06 co-owner) | **Confirm primitive + `order_key` parity** (ISS CR-10 / KN's ADR-06 clause); the `order_key` family is already aligned by intent. |
| **X-4** | **The sub-artifact `#sub` grammar + tombstone semantics are defined three times for three content shapes.** Git mints content-anchored line ranges (`#L42-L88` → partial/tombstone on content loss); Knowledge mints block/heading/row anchors stable across edits; Chat mints message/thread anchors. Refs must hold *one* `#sub` URN grammar + one graceful-degradation rule that covers all three. | **GIT** vs **KN** vs **CHAT** (all CONFIRM Refs §3.5 separately) | **Refs unifies the `#sub` grammar + the outdated/tombstone resolution** so the three mints share one scheme. |
| **X-5** | **Chat comments vs Knowledge comment threads — possible duplicate primitive.** Knowledge v1 ships KB-native comment threads but flags "reuse the Chat threading primitive?" as an open question (KN KQ-2); Chat owns threads-first conversation. Risk: two threading/comment primitives diverge before consolidation. | **KN** (KQ-2, KB-native v1) vs **CHAT** (threads) | **Decide threading-primitive consolidation** (KN+Chat P5); v1 ships separate, consolidation is a named follow-on. |
| **X-6** | **The unified sandbox is CI-owned but Agent-Fabric-shared; every agent-authoring subsystem depends on it transitively.** CI owns the runner + the escape drill (T-1); `ToolHands::exec` is CI's `kind=agent` job. Issues/Knowledge/Chat all register gated `ToolDef`s that ultimately execute there. Risk: a subsystem assumes agent execution semantics (cost gate, attribution, HITL withhold) the unified runner must guarantee uniformly. | **CI** (owns sandbox + drill) vs **ISS/KN/CHAT** (register tools that run on it) | **CI's T-1 escape drill + the UNIFY contract (HP-5) gate all agent execution**; the per-subsystem `requires_approval` defaults are confirmed jointly with the Fabric. |
| **X-7** | **The free-text / immutable-content erasure residual is the same legal seam, named five times.** Git (immutable commit bytes), CI (inline log PII), Issues (third-party mentions), Knowledge (free-text blocks), Chat (a name in another's message body) all ship the structural floor + crypto-shred and hand the same residual to Legal/DPO. Risk: five different residual statements instead of one ratified platform posture. | **GIT/CI/ISS/KN/CHAT** (all `[OPEN → LEGAL]`) | **Legal/DPO ratifies ONE platform-wide lawful-basis-limit posture** for immutable/third-party free-text PII, instantiated per subsystem. |

---

## 13. Consolidated cross-subsystem open questions for Phase 5

| # | Question | Resolvers |
|---|---|---|
| **OQ-A** | **The Git↔CI checks/merge-gate contract** — `check_status` shape, `(commit_oid, context)` keying, fork/trust-tier gating, re-run supersession, the merge-queue `ci.result` signal. *The most load-bearing seam (X-1).* | GIT P4 + CI P4 |
| **OQ-B** | **The canonical `myelin-content` node taxonomy + the ADF→content lossy-map + the Chat/Issues consumed subset** (X-2). | KN (leads) + CHAT + ISS |
| **OQ-C** | **`myelin-query` primitive + `order_key`/LexoRank encoding parity** (X-3) and the projection-feeder/materialisation promotion thresholds (ISS CR-11, KN KQ-4). | ISS + KN + Search |
| **OQ-D** | **The unified `#sub` sub-artifact grammar + outdated/tombstone resolution** across line-range / block / message anchors (X-4). | Refs + GIT + KN + CHAT |
| **OQ-E** | **The `list_objects` `Filter{set_expr}` push-down encoding** over each subsystem's id column — the one shape that serves git repo/PR, CI `run_id`, issue `issue.id`, KN `database_row`, chat `message`/channel ids without N+1. | Identity + all five |
| **OQ-F** | **The `SCHEDULE_AND_RUN_JOB` long-parked-activity-completes-by-signal pattern** + the `DurableExecutor::signal` per-effect `idem_key` scheme for batch HITL approval (CI CR-WF-1 + CHAT CHG-C4). | Workflow + CI + CHAT |
| **OQ-G** | **The ratified free-text / immutable-content erasure lawful-basis posture** — one platform statement instantiated per subsystem (X-7). | Legal/DPO + GIT + CI + ISS + KN + CHAT |
| **OQ-H** | **Worklog/productivity-field sensitivity** (works-council / labour-law, GD-13) + build-data-as-LLM-training basis (AG-8) + CD-as-PaaS scope (PR-5). | Legal/DPO + ISS + CI |
| **OQ-I** | **The cross-cell PII-free pointer bridge shape** (carries `subject`/`type`/`correlation_id`, per-viewer cell-local resolution) — the seam ISS cross-cell rollup, KN cross-cell collab, and CHAT cross-org channels all ride. | Control plane / Tenancy + ISS + KN + CHAT |
| **OQ-J** | **The reconnect/resync protocol + per-view subscription scope bounding** over the shared firehose — a huge board (ISS), a hot doc (KN KD-8), a hot channel (CHAT). Co-design the resume-cursor discipline once. | CHAT (connection tier) + KN (KN-1) + ISS |
| **OQ-K** | **Per-surface shed budgets + the protected-human-lane reservation sizes** for the storm profiles (CI-surge, collab op-stream, connection-storm, agent-mention-storm). | substrate + CI + KN + CHAT |
| **OQ-L** | **Threading/comment-primitive consolidation** (KN KQ-2, X-5) and shared templating (KN KQ-3, with ISS/CI). | KN + CHAT + ISS + CI |

---

## 14. The five-line digest for Phase 5

1. **No ADR reversal requested by any subsystem.** Almost every ask is a *confirmation* or *small additive
   sharpening* of a frozen Phase-3 seam; the genuinely-new asks are itemized (NEW) above.
2. **The single most-repeated ask is the `list_objects` `Filter{set_expr}` push-down** (S-10) — all five
   need it, three name it *blocking* (ISS CR-1, plus the leak-free scans of GIT/CI/KN/CHAT depend on it).
3. **The hardest unresolved seam is Git↔CI checks/merge-gate** (X-1 / OQ-A) — top open question for both,
   must be jointly designed before contracts freeze.
4. **The same erasure residual is named five times** (X-7 / OQ-G) — Legal/DPO must ratify ONE platform
   lawful-basis-limit posture, not five.
5. **The shared content + query + ref primitives** (`myelin-content`, `myelin-query`, the `#sub` grammar)
   are the cross-subsystem reconciliation core (X-2/X-3/X-4 / OQ-B/C/D): Knowledge leads, Chat/Issues
   consume, Refs unifies — confirm the canonical taxonomies and the encoding parity in Phase 5.
