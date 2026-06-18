# Phase 3 — Contract Index (the consolidated interface/contract map)

> Phase: `03-shared-systems-architecture`. Companion to [`README.md`](./README.md) and
> [`drills-and-open-questions.md`](./drills-and-open-questions.md). Canonical brief:
> [`VISION.md`](../../VISION.md). **Complete across all 11 Phase-3 docs.** Date: 2026-06-19.
>
> **What this is.** The single consolidated map of **every cross-system contract** a Phase-4 subsystem (or a
> sibling shared system) must **implement or call**. This is the Phase-4 *build-to surface*. For each contract:
> **who owns it** (defines + is authoritative for the shape), **who consumes it**, and **where it is defined**
> (the doc + section that froze it). A contract here is **stable** (ADR-01): changing it is a single
> whole-workspace PR that breaks every consumer's build *now*, never silently in production.
>
> **Convention.** Signatures are Rust-shaped for concreteness (ADR-02: glue crates are Rust). Cross-language
> services consume the identical shape as a wire contract (protobuf/JSON over the internal RPC), with field
> **names and units** reconciled per X-5 against the canonical envelope field list (`00 §2.10`). "→ P4" marks
> the part still open; the *surrounding contract* is frozen.

---

## 0. How to read this map

The contracts cluster into twelve groups, each a section below:

1. **Bootstrap & service shell** — `serve(AppSpec)`, the three-surface topology, liveness/readiness, lints.
2. **Event envelope, outbox & consumer template** — the emit/consume surface + causality.
3. **Signals, Automations, Triggers & the firehose** — the four reactive primitives + the dedicated transport.
4. **Identity & access** — `authenticate`/`check`/`list_objects`/`list_subjects`/`delegation`/`mint`/`revoke`.
5. **`ArtifactRef`, refs & the projection API** — addressing, resolution, backlinks, traversal, `project`.
6. **Search** — `query`/`semantic`/`declare_indexable`/`reindex`.
7. **Notifications** — `list_inbox`/`humanise`/`prefs`/`on-call`/`define_notif_rule`.
8. **Agent fabric** — `register_tool`/`EffectApi`/`AgentRuntime`/`ToolHands`/`Agent.handle`.
9. **Durable workflow** — `DurableExecutor`/`WfCtx`/`signal`/the timer wheel.
10. **GDPR / Audit / `PersonalDataHolder`** — erasure, the DSR orchestrator, classify, the tamper-evident log.
11. **Storage — tiers / `BlobStore` / KMS / reserve-settle / backup-restore.**
12. **Tenancy & control plane** — `discover`/`place`/`placement_of`/`residency_verify` + the partition key.

The taxonomy/token table (`00`→Bus §6.2) and the canonical envelope field list (`00 §2.10`) are the two
**reconciliation anchors** every other contract aligns names/units against.

---

## 1. Bootstrap & service shell (substrate `00`)

| # | Contract | Owner | Consumed by | Defined in |
|---|---|---|---|---|
| 1.1 | **`serve(AppSpec)`** — boot → migrate → start outbox relay → start consumers → open the three ports → graceful drain. `AppSpec{ name, config, migrations, public, internal, consumers, holders, outbox }`. | substrate (`myelin-substrate`) | every service `main.rs` | `00 §3.1` |
| 1.2 | **Three-surface topology** — public (gateway-fronted, identity-injected) / internal RPC (trust boundary) / metrics-health. Public↔internal is a *security boundary*; tenant from token, not path. | convention + harness | every service | `00 §4` |
| 1.3 | **Liveness ≠ readiness** — liveness must not check deps; readiness gates on DB pool + broker + authz reachability; startup = not-ready-not-killed. | harness | every service | `00 §4.3` |
| 1.4 | **`PersonalDataHolder` auto-registration** — every store the harness opens is auto-registered with the DSR orchestrator. | harness | every store-opening service | `00 §3.4` |
| 1.5 | **Forward-only online migrations** — expand→backfill→contract; no rollback files; no blocking `ALTER` on a flagged-hot table; measure lock vs a restore. | migration runner | every schema owner | `00 §9` |
| 1.6 | **Architecture lints** (CI-committed) — `no-cross-db`, `no-raw-publish`, `tenant-predicate`, `no-host-exec`, `forward-only-migration`, `no-cross-sync-cycle`, **+ `residency-pin`, `control-plane-pii-free`, `search-requires-acl-filter`, `no-llm-in-platform`, `no-untagged-personal-data`, `flow-determinism`** (Phase-3 additions). | substrate/CI | every crate | `00 §2.11` + README §4 |
| 1.7 | **Cross-language harness parity** — a non-Rust subsystem needs an equivalent of `serve` enforcing the same non-negotiables (outbox, three ports, liveness/readiness, forward-only migrations). | the diverging subsystem | (Chat connection tier likely) | `00 §13 Q1` → **P4** |
| 1.8 | **Telemetry signal set** (RED/USE + consumer-lag + outbox-depth + breaker-state + fail-static ratios + shed-counts + causal-depth) on the metrics port — the Phase-5 drill survival signals. | harness | every shared system (X-1), every drill | `00 §10.2` |
| 1.9 | **Resilient client** — `ResilientClient::call(target, req, idem)`: per-call timeout + circuit breaker + bounded-concurrency bulkhead + jittered-retry-idempotent-only; honours `Retry-After`. The one place every outbound call goes. | substrate (`myelin-client`) | every inter-service caller, CLI, agent runtime | `00 §6` |
| 1.10 | **`FailStatic<T>`** — bounded-staleness cache around a critical dependency answer; `static_max ≤ revocation SLA` and ≥ agent-token TTL; serves coarse "actor active / coarse grants", never an escalation. | substrate (mechanism) | Id (primary), Notif, any critical-dependency caller | `00 §8` |
| 1.11 | **Protected-human-lane shed order** — speculative → batch/CI → agent → human-last; agents/CI get `429 + Retry-After`; per-tenant fairness. | harness + gateway | every public surface | `00 §7` |

---

## 2. Event envelope, outbox & consumer template (Bus `myelin-events`)

| # | Contract | Owner | Consumed by | Defined in |
|---|---|---|---|---|
| 2.1 | **`EventEnvelope`** — the canonical versioned envelope (event_id ULID, type, schema_ver, tenant, region, actor{principal,kind,on_behalf_of,session,run}, subject ArtifactRef, aggregate, correlation_id/causation_id/depth, contains_personal_data/data_role/visibility/pii_key_ref, occurred_at/recorded_at, payload). **References-not-payloads.** | Bus (`myelin-events`) | **every emitter + every consumer** | Bus §3.1; `00 §2.1`, §2.10 |
| 2.2 | **`OutboxTx::emit(draft, cause)`** — the ONLY sanctioned emit path; in the same tx as the state change; causality derived correct-by-construction. **No `publish_now`.** | Bus | every state-changing handler (incl. the audit append) | Bus §5.2; `00 §2.1` |
| 2.3 | **`outbox` table shape** — per-service, `(event_id UNIQUE, aggregate, seq, subject, envelope, …)`, `UNIQUE(aggregate, seq)` ordering invariant; drained by the relay (`FOR UPDATE SKIP LOCKED`). | Bus (schema convention) | every producing service | Bus §3.2, §4.1 |
| 2.4 | **`EventHandler` consumer template** — `subjects()` (whitelist, never `*`) + `handle(ev) → {Done \| NonRetryable \| Retry}`; the template owns durable-bind-by-name, ack-after-enqueue, the dedup ledger, bounded prefetch, lag metric. | Bus | every consumer (Search/Refs/Notif/OLAP/Agents/Audit/Workflow) | Bus §4.2; `00 §5` |
| 2.5 | **`consumer_dedup` ledger** — `(consumer, event_id) PK`; presence == already-handled; the idempotency check. | Bus (convention) | every consumer | Bus §3.3 |
| 2.6 | **Reindex-from-source** — `events::reindex(scope)` → each owner's `replay(scope, since)` emits `*.snapshot` via the outbox through the live consumer path. **The only recovery path** for derived stores. Must support **sub-artifact-granular** snapshots. | Bus + every subsystem (`replay`) | Search, Refs, OLAP, Notif read-models | Bus §4.9, §5.6 |
| 2.7 | **Crypto-shred / tombstone on the log** — `*.erased` tombstone events; inline-PII events envelope-encrypted with `pii_key_ref`; bus is a `PersonalDataHolder`. | Bus | DSR orchestrator, live consumers | Bus §4.8, §5.7 |
| 2.8 | **Schema evolution / upcasters** — `(type, from_ver) → to_ver` pure functions applied at consume; forward-only; un-upcastable → DLQ. | Bus | every consumer | Bus §4.10 |
| 2.9 | **Event taxonomy + `ArtifactRef` token table** — `<subsystem>.<artifact_type>.<event_name>` (singular, past-tense); the canonical subsystem/type tokens (`git`/`ci`/`issue`/`knowledge`/`chat`/`identity`/`refs`). **Each subsystem owns its complete list under this grammar.** | Bus (grammar + seed) | every subsystem (P4 completes its list) | Bus §6 |

---

## 3. Signals, Automations, Triggers & the firehose (Bus)

| # | Contract | Owner | Consumed by | Defined in |
|---|---|---|---|---|
| 3.1 | **`define_signal_rule(SignalRule{ matcher, severity, dedup_key_tpl, dedup_window })`** — curated/deduped/severity-ranked subset; published to `sig.<tenant>.<severity>.<rule>`. **Product/reactive consumers subscribe to Signals, never `evt.*`.** | Bus | Notif (primary author of the default set), agents, reactive consumers | Bus §3.4, §4.4 |
| 3.2 | **`register_automation(AutomationRule{ matcher, action, run_as, delegation, budget, gates })`** — stateless per-event reflex; may invoke a durable workflow (`action.kind = workflow` → `DurableExecutor::start`). | Bus | project admins, subsystems | Bus §3.5 |
| 3.3 | **`arm_trigger / disarm_trigger(Trigger{ owner, condition, arms_subject, on_resolve, stale_after })`** — stateful per-person promise; armed→{resolved\|stale\|disarmed}, fires once per arming. `stale_after` is a `myelin-flow` durable timer. | Bus | Issues ("unblock/remind me when…"), users | Bus §3.6, §4.6 |
| 3.4 | **`EventMatcher`** — the predicate core of the shared query AST (JSON, bounded interpreter, no UDFs/loops/recursion, statically cost-bounded, permission-aware by construction). **Not CEL/JSONLogic.** | Bus + `myelin-query` | Signals, automations, triggers, saved views, Search, Notif prefs | Bus §4.5; ADR-07 |
| 3.5 | **Firehose transport** — `firehose::publish(stream, frame)` / `firehose::tail(stream, range)`; CI logs / presence / collab op-streams ride a *separate* transport; the durable bus carries only pointer events (`ci.log.available`, `doc.updated`). | Bus (seam) + Knowledge (collab transport, KN-1) | CI (logs), Chat (presence), Knowledge (collab) | Bus §4.3, §5.5 |
| 3.6 | **Reactive/dispatch tier** — Signal→`EventInbox` matching/guarding/rate-limiting; threads causality nested; runs the structural loop guards; bounded dispatch pool drops over-cap; reserve/settle gate before any run. (Separately-reviewed; the Agent Fabric is the *target handler*.) | Bus | Agent Fabric (consumes delivery), Notif | Bus §4.7 |

---

## 4. Identity & access (`myelin-identity`)

| # | Contract | Owner | Consumed by | Defined in |
|---|---|---|---|---|
| 4.1 | **`authenticate(credential) → Principal{ tenant, region, principal_id, kind, data_role, status }`** — resolves any credential (SSO/SCIM/passkey/SSH/PAT/CI/agent token); tenant from credential, never path. | Id (`myelin-identity`) | every gateway/entrypoint | Id §4, §12 |
| 4.2 | **`check(subject, permission, object, zookie?) → {Allow \| Deny \| Conditional}`** — the per-action gate; fail-closed on uncertainty. | Id | every write path, `EffectApi`, every gateway, Notif step-0 | Id §8.1, §12; `00 §2.2` |
| 4.3 | **`list_objects(subject, permission, type, zookie?) → {ids \| Filter{set_expr, zookie}}`** — **the leak-free pre-filter; the single most load-bearing inter-system contract.** Both modes; `Filter` must be **consumer-composable over an arbitrary id column** (push-down, facet-expressible). | Id | **Search, Refs** (+ every permission-aware read) | Id §8.2, §12; README §4 S-10 |
| 4.4 | **`list_subjects(object, permission, zookie?) → SubjectTree`** + **`explain(subject, perm, object) → RewriteTrace`** — the admin permission inspector + the ReBAC "why"; the HITL approver set; the Notif read-fanout `watch` resolution. | Id | admin inspector, `myelin policy show`, HITL approver set, Notif | Id §8.3, §12 |
| 4.5 | **`delegation(agent, trigger_actor) → EffectivePolicy`** — the composed `agent.policy ∩ delegation ∩ tenant.policy` (monotone intersection, macaroon caveats; attenuation never amplification). | Id | Agent Fabric `EffectApi`, workflow activities | Id §7, §12 |
| 4.6 | **`write_tuples([Δtuple], precondition?) → zookie`** — atomic tuple write; returns the zookie to stamp; emitted via outbox. | Id | subsystems (via their writes), role-compile | Id §6, §8.4, §12 |
| 4.7 | **`mint_run_token(agent_id, run_id, delegation_caveats, ttl) → token`** + **`revoke(jti \| principal_id)`** (idempotent even on crash) — per-run attenuated token, life == run life; auto-expiring tuples backstop. **Callable mid-workflow on resume** (S-11). | Id | Agent Fabric, CI dispatch, workflow activities | Id §4, §11, §12 |
| 4.8 | **`resolve_pseudonym(subject, tenant)`** + **`erase(subject)`** (the pseudonym-map shred — the erasure lever; DSR fan-out **step 1**). | Id | Git, Audit, GDPR DSR orchestrator | Id §11, §12 |
| 4.9 | **Per-subsystem ReBAC namespace declaration** — each subsystem contributes its namespace fragment (relations + permissions as union/intersect/exclude/TTU-rewrite). Compiled into one cell schema; Id owns the engine, never invents object IDs. Subsystems declare a `watcher` relation per watchable type (Notif). | Id (engine) + every subsystem (its fragment) | every subsystem (P4) | Id §5; Notif §8.3 |
| 4.10 | **`Consistency`/zookie semantics** — read-your-writes; zookie-stamped reads bypass the fail-static cache; security-sensitive transitions always carry a zookie. | Id | Search, Refs, Notif, every authz read | Id §8.4, §10; `00 §8` |
| 4.11 | **`FailStatic` bound (Id usage)** — `static_max ≤ revocation SLA` and ≥ agent-token TTL; coarse "actor active / coarse grants". DPO ratifies W; default-to-beat 5 min. | substrate (mechanism) + Id (primary user) | Id, Notif, any critical-dependency caller | `00 §8`; Id §10; GDPR §4.7 |

---

## 5. `ArtifactRef`, refs & the projection API (`myelin-refs`)

| # | Contract | Owner | Consumed by | Defined in |
|---|---|---|---|---|
| 5.1 | **`ArtifactRef`** — `myelin://<tenant>/<subsystem>/<type>/<id>[#sub]`; the *type* lives in `myelin-events`. **`parse`/`format`** reject ambiguity, never guess scope; display keys (`#42`, `@alice`) are render-time only. | Refs (`myelin-refs`, parse/format/resolve) + Bus (the type + token table) | every service (the one URN library) | Refs §3.1, §5.1; Bus §6.2 |
| 5.2 | **`resolve(ref, viewer, mode) → Projection \| Tombstone`** — live per-viewer unfurl/embed; denied → tombstone (never leak); calls the owner's `project` API on cache miss. **`Display` mode returns the humanisation projection (Notif).** | Refs | Chat unfurl, PR context pane, Knowledge embeds, **Notif humanise** | Refs §4.2, §5.1 |
| 5.3 | **`backlinks(ref, viewer, page) → [Edge]`** + **`edges(ref, viewer)`** + **`traverse(root, rels, depth, viewer)`** — leak-free inverse index via `list_objects`; bounded cycle-safe recursive-CTE walk (depth ceiling 16). | Refs | "what references this", impact view, hierarchy, Notif affinity | Refs §4.4, §4.5, §5.1 |
| 5.4 | **The two edge events** — `refs.edge.created` / `refs.edge.removed` (= `ref.created`/`ref.removed`), emitted by producers via the outbox (`mention`/`artifact_ref`/`embed` nodes are the producers); **no standalone edge-write API.** | Refs (consumes) + every content-producing subsystem (emits) | Refs edge-builder | Refs §4.1, §5.1 |
| 5.5 | **TE-7 typed-edge mirror** — lifecycle edges (`closes`/`blocks`/`blocked_by`/`depends_on`/`parent`/`assigns`/`relates`) are dual-homed: the **typed relation table (Issues/Knowledge) is the source of truth**; Refs holds a rebuildable projection. Refs fixes the `rel` vocabulary + inverse pairing. | Refs (the mirror contract) + Issues/Knowledge (the typed table = truth) | cross-subsystem traversal | Refs §3.3 |
| 5.6 | **`project(ref, viewer) → { title, state, icon, render_hint, sub_anchor? }`** — **a REQUIRED contract on every subsystem** (ADR-13.1); per-viewer, pre-permission-checked; the *only* way Refs/Search/Notif read about another subsystem's artifact (no cross-DB). | each subsystem (P4) | Refs (resolve), Search (text projection), Notif (humanise) | Refs §5.2; Search §5.3 |
| 5.7 | **Sub-artifact `#sub` scheme** — each subsystem mints stable opaque sub-ids (`#comment-12`, `#b9`, `#step-3`, `#L42-L88`), **stable across edits** so embeds don't dangle. Refs fixes the grammar; stability is each subsystem's. | each subsystem (P4) | Refs, Search | Refs §3.5; `00 §13 Q4` → **P4** |
| 5.8 | **`reindex(scope)`** (Refs) — reindex-from-source for the edge index + projection cache; never reads owner DBs. | Refs | ops, GDPR re-erasure, new consumers | Refs §4.7, §5.1 |

---

## 6. Search (`myelin-search` — no glue crate; composes others')

| # | Contract | Owner | Consumed by | Defined in |
|---|---|---|---|---|
| 6.1 | **`query(ast, viewer, zookie?, page) → RankedResults`** — AST compiled to FT/structured/vector; **always conjoins `list_objects(viewer, read, type)` before scoring** (no path bypasses the ACL filter — the `search-requires-acl-filter` lint). | Search | every subsystem search UI, CLI, agents (RAG) | Search §4.2, §5.1 |
| 6.2 | **`semantic(text\|vec, viewer, k, filter_ast?) → k visible NN`** — ACL-filtered-during-traversal k-NN (k *visible* neighbours); agent RAG, dedup/triage. | Search | agent RAG, dedup | Search §4.5, §5.1 |
| 6.3 | **`declare_indexable(IndexSpec{ subsystem, type, projection, ft_fields, struct_fields, semantic, acl_object_type })`** — how an artifact projects to an index doc; Search indexes implicitly off the bus. | each subsystem (build-time) | Search | Search §5.1, §5.3 |
| 6.4 | **`reindex(scope) → job`** (Search) — the only rebuild path; invokes the bus re-emit protocol; needs **sub-artifact-granular** `*.snapshot` replay. | Search | admin/ops, post-restore | Search §4.9, §5.1 |
| 6.5 | **Code-search input** — Git emits an indexable `git.*` projection per blob/ref/symbol (path, symbols, literals, commit message) for code-search v1. Search does not parse repos. | Git (P4) | Search | Search §4.4, §9.3 → **P4 Git** |

---

## 7. Notifications (`myelin-notif`)

| # | Contract | Owner | Consumed by | Defined in |
|---|---|---|---|---|
| 7.1 | **`list_inbox(principal, filter?, page?) → [InboxItem]`** ranked by `priority DESC` — **the ONE inbox** (C-9); a "scoped view" (Issues "My Work", Chat "Activity", Git "Review requests") is a `filter` over `reason`/`subject`, never a second store. | Notif (`myelin-notif`) | inbox UI, Issues/Chat/Git scoped views, CLI `inbox list` | Notif §1.3, §4.1 |
| 7.2 | **`mark(item, state)` / `snooze(item, until)` / `mark_all_read(filter)`** — one read-state truth across all views. | Notif | inbox UI, CLI | Notif §4.1 |
| 7.3 | **`humanise(item \| (template_key, args), viewer, locale) → HumanisedString{text, links[], icon}`** — resolves each `ArtifactRef` arg per-viewer via Refs `resolve` (Display mode); permission-safe (tombstone on deny) + erasure-safe; ICU MessageFormat. **Every consumer and every agent-authored message inherits it (NOTIF-1).** | Notif | every channel renderer, **agent HITL cards + agent messages** | Notif §3.3, §4.1 |
| 7.4 | **`get_prefs/set_prefs(principal, routing, quiet_hours, digest)`** — per-principal routing + quiet-hours (recipient tz; critical/escalated pierce); the matcher reuses the safe query-AST predicate core (one predicate language). | Notif | settings UI, CLI `notify prefs` | Notif §2.2, §4.1 |
| 7.5 | **`oncall_now(schedule) → principal`; `page(target, reason)`** — resolves rotation; starts an escalation run (a durable workflow). | Notif | CLI `oncall show\|page`, SLA engine, Agent Fabric escalations | Notif §3.7, §4.1 |
| 7.6 | **`define_notif_rule(reason, dedup_tpl, default_class)`** — how a Signal class maps to an inbox reason/priority/class. | Notif (default set) + subsystems | admin, subsystem P4 | Notif §3.1, §4.1 |
| 7.7 | **`PersonalDataHolder` (Notif = "notification history")** + **`replay(scope, since)`** — references-not-payloads means erasing a person tombstones their appearance for free; the inbox is rebuilt by reindex-from-source. | Notif | DSR orchestrator, Bus reindex | Notif §3.8, §3.9 |
| 7.8 | **`DeliveryAdapter { channel, region, send(RedactedMessage, idem_key), receipts }`** — region-aware, EU-preferring, swappable; off-cell payloads are PII-minimised; at-least-once+idempotent delivery (`UNIQUE(idem_key)`). | Notif (trait) + provider adapters (P4) | email/push/web/mobile/desktop channels | Notif §2.3, §3.6 |

---

## 8. Agent fabric (`myelin-agent`)

| # | Contract | Owner | Consumed by | Defined in |
|---|---|---|---|---|
| 8.1 | **`ToolSurface::register_tool(ToolDef{ name, input_schema, required_caps, effect_kind, side_effecting, requires_approval, exposed_over_mcp })`** + `resolve` — one permissioned catalogue, exposable over MCP. | Agent (`myelin-agent`) | every subsystem (contributes its actions) | Agent §6, §7.1; `00 §2.4` |
| 8.2 | **`EffectApi::apply(run, ProposedEffect) → Applied(event_id) \| Gated(gate_id) \| Denied(reason)`** — plan-then-apply: schema → capability → delegation → tenant → budget → HITL gate → apply via the **public endpoint** (no carve-out) → meter. Denied = ordinary tool error, no privileged fallback. | Agent | the loop, the external MCP path, workflow activities | Agent §5.2, §7.1 |
| 8.3 | **`AgentRuntime::step(&Conversation) → UseTools(Vec<ToolCall>) \| Submit(Submission)`** — the stateless brain; the strategy seam (mock/llm); platform owns history. | Agent (trait) + runtimes (impl) | the loop (an activity) | Agent §2.1, §7.1; `00 §2.4` |
| 8.4 | **`ToolHands::exec(Command) → ToolResult`** — sandboxed computation; **no host-exec bypass** (`no-host-exec` lint). Real impl = the CI runner's `kind=agent` job. **Routing (§5.0): only `compute`/`external` untrusted code goes here; side-effecting mutation goes through `EffectApi`.** | Agent (trait) + CI (runner) | the loop (compute/external tool calls) | Agent §2.2, §5.0, §7.1 |
| 8.5 | **`Agent::handle(InboxEvent, &dyn AgentRuntime) → RunOutcome`** — the platform-owned bounded multi-turn loop (AG-3); identical for mock/real; carries causality nested. A run *is* a durable workflow (§9). | Agent | the dispatch tier (Bus) | Agent §2.3, §5.1 |
| 8.6 | **`EventInbox::deliver(InboxEvent)`** — the platform delivers matched events (carries envelope + binding + token + budget); agents don't poll. | Bus (dispatch tier) → Agent (handler) | Agent Fabric | Agent §1.3; Bus §4.7 |
| 8.7 | **`run --dry-run(InboxEvent) → Vec<ProposedEffect>`** — plan-then-apply testability (stops before apply). | Agent | CLI, tests | Agent §7.1 |
| 8.8 | **Required of P4:** Knowledge accepts a content-addressed agent-trace write (AG-7) + registers it as an erasable holder; **CI owns the `kind=agent` job spec + the real-kernel escape drill**; the durable-workflow engine exposes open/signal/timer for runs (§9). | Knowledge / CI / Workflow (P4/P3) | Agent Fabric | Agent §11 |

---

## 9. Durable workflow (`myelin-flow`)

| # | Contract | Owner | Consumed by | Defined in |
|---|---|---|---|---|
| 9.1 | **`DurableExecutor { start(StartSpec, cause), signal(run, name, payload, idem_key), describe(run), cancel(run, reason) }`** — the engine-agnostic seam (a Temporal escape hatch sits behind the same trait); `start` returns a durable handle; `signal` is idempotent on `idem_key`. | Workflow (`myelin-flow`) | Bus automations (`action.kind=workflow`), Agent Fabric, CI, Notif escalation, Issues SLA | Workflow §5.1 |
| 9.2 | **`WfCtx { activity, sleep_until, sleep_for, wait_for_signal, now, rand, emit }`** — the deterministic surface a workflow *definition* is written against; every non-deterministic interaction journals; `emit` goes via the outbox (causality derived). Guarded by the `flow-determinism` lint. | Workflow | every workflow definition (agent run, CI pipeline, automation, SLA timer) | Workflow §5.1, §2.5 |
| 9.3 | **Durable timer (the SC-11 wheel)** — `sleep_until/sleep_for` backed by the minute-bucket partial index + `FOR UPDATE SKIP LOCKED`; millions of timers cost an indexed range read; effectively-once fire. **The Trigger `stale_after`, SLA timers, snooze re-surfacing, and HITL timeouts all ride it.** | Workflow | Bus (`Trigger.stale_after`), Issues (SLA), Notif (snooze/escalation) | Workflow §3.3, §4.2 |
| 9.4 | **Durable signal (multi-day HITL)** — a workflow `state=waiting` holds no runtime; an `approval`/`cancel`/`ci.result` signal arrives hours/days later (idempotent), re-leases + replays + consumes. The HITL approval-card round-trip. | Workflow | Agent Fabric (HITL gate), Chat (posts the `approval` signal) | Workflow §3.4, §4.3, §6.3 |
| 9.5 | **Workflow↔agent mapping** — a workflow owns `RunBudget`/gates/state; `AgentRuntime::step` and `ToolHands::exec` are **activities**; the reserve/settle gate is the workflow's bookends. Plan-then-apply survives. | Workflow + Agent Fabric | Agent runs, CI pipelines | Workflow §6.1; Agent §5.6 |
| 9.6 | **`PersonalDataHolder` (workflow history)** + **`replay`** — `input`/`result`/signal `payload` are references-not-payloads; inline-PII rows are per-subject-key crypto-shred; erasing a person rarely touches the workflow. | Workflow | DSR orchestrator | Workflow §4.8, §5.5 |

---

## 10. GDPR / Audit / `PersonalDataHolder` (`myelin-gdpr`)

| # | Contract | Owner | Consumed by | Defined in |
|---|---|---|---|---|
| 10.1 | **`PersonalDataHolder { locate, export, rectify, restrict, erase }(subject\|tenant) → receipt`** — every store implements it; the holder list is **exhaustive (H1–H18)**; the harness auto-registers. Erasure is **purge/crypto-shred/pseudonymise, never hide** (Search purges+reindexes incl. embeddings; Refs/Notif rely on pseudonym shred; Bus/Workflow crypto-shred inline-PII keys). Each op returns a receipt hash-linked into the audit log. | `myelin-gdpr` (trait) + every store (impl) | DSR orchestrator | GDPR §3.1; `00 §2.5`, §3.4 |
| 10.2 | **`#[personal_data(category, role, basis, retention, erasure, subject_locator)]`** classify derive — the schema-level tag every personal-data field carries; the `no-untagged-personal-data` lint fails the build otherwise. Feeds the generated data map. | GDPR (`myelin-gdpr` derive) | every schema owner | GDPR §2.1 |
| 10.3 | **`data_map() → Inventory` / `ropa(tenant) → ProcessingActivities`** — generated from the schema tags + registered holders, diffed in CI; drives the DSR fan-out, breach scoping, RoPA, DPIA gate. | GDPR/Audit | DPO, breach-scoping, DSR fan-out | GDPR §2.2 |
| 10.4 | **`dsr_submit(kind, subject, scope, posture) → dsr_id`; `dsr_status`; `dsr_certificate(dsr_id) → MerkleProvenBundle`** — the DSR state machine (validate → fan-out by the data map → legal-hold gate → collect receipts → seal); the 1-month deadline is a durable timer; **iterates `member_cells`** for multi-cell. Operable by Myelin and by/for tenants (Art. 28). | GDPR/Audit | Myelin ops, tenant admins, auditors | GDPR §4 |
| 10.5 | **Retention / consent / sub-processor / legal-hold** — `effective_retention` (tightest-policy-wins, legal-hold-aware); `consent_record/withdraw`; `subprocessors`/`transfer_allowed` (deny extra-EU by default); `legal_hold_set` (suspends retention + erasure). | GDPR/Audit | retention engine, controller-posture activities, adapters, ops/legal | GDPR §5 |
| 10.6 | **Tamper-evident audit log** — per-tenant hash-chain + Merkle (CT-style: signed tree heads, `inclusion_proof`/`consistency_proof`, external witness); minimised (pseudonyms/`ArtifactRef`s, never payload); **written via the outbox only** (audit consumer appends). Distinct from telemetry and from the agent trace (three holders). | GDPR/Audit | every action-taking service (emits), auditors/eDiscovery (proofs) | GDPR §6; `00 §10.3` |
| 10.7 | **eDiscovery / legal-hold export** — `ediscovery_export(scope) → MerkleProvenBundle`; content-addressed, inclusion-proof-bearing, legal-hold-frozen (GD-2). | GDPR/Audit | legal/auditors | GDPR §5.4 |
| 10.8 | **Erasure ledger** (PII-free, **non-crypto-shred-erasable**) — opaque subject id + timestamp + holders/keys shredded; survives to drive **post-restore re-erasure** (GD-14). | GDPR/Audit (owns) | Storage restore (consumes) | GDPR §4.4; Storage §7.5 |

---

## 11. Storage — tiers / `BlobStore` / KMS / reserve-settle / backup-restore

| # | Contract | Owner | Consumed by | Defined in |
|---|---|---|---|---|
| 11.1 | **OLTP tier client** — the harness pool + thin query layer (tenant-scoped, RLS, encrypted columns); one DB per service; no cross-DB; forward-only migrations; the outbox lives here (cross-seam anchor). | Storage | every subsystem | Storage §3.1; `00 §3.3` |
| 11.2 | **`BlobStore { put, get, head, delete }`** — content-addressed (hash-on-write, BLAKE3; plaintext-hash-within-tenant-keyspace → per-tenant dedup); fs↔object is a one-line swap; erasure for immutable tiers is crypto-shred, not `delete`. | Storage (`myelin-gdpr`/Storage seam) | every blob-holding service; git pack tier (STOR-5 seam) | `00 §2.7`; Storage §3.2 |
| 11.3 | **KMS key hierarchy + `KeyOrigin` trait** — per-cell root → per-tenant KEK → per-tenant/per-subject DEK; `key_ref`/`pii_key_ref` = `kms://<tenant>/<dek-epoch>/<class>`; `KeyOrigin{ wrap, unwrap, can_derive_plaintext_index, destroy }` (platform/BYOK/HYOK). `can_derive_plaintext_index()=false` (HYOK) **structurally** skips Search/Agent indexing. | Storage/GDPR | every encrypted store, Search, Agent Fabric | Storage §4, §6.2 |
| 11.4 | **Crypto-shred + GD-4 granularity rule** — free-text/profile/chat-body/agent-memory = per-subject DEK; bulk pseudonym-referenced content = per-tenant DEK; tenant offboarding = the KEK. The `erasure` classification tag drives the key choice automatically. | Storage | DSR orchestrator (calls `erase`) | Storage §5.1 |
| 11.5 | **Backup / restore / cross-seam** — WAL+PITR (RPO ≤ 5 min default), `restore(to_offset)`, `restore-verify` (CI-gated), the **event-log offset is the cross-seam cursor** (OLTP↔blob↔index↔offset), `post_restore_reerase` (GD-14). Derived stores are **rebuilt, not restored** (reindex-from-source). | Storage | ops/DSR; CI durability gate | Storage §7 (STOR-4) |
| 11.6 | **OLAP read store** — CQRS read model fed by the bus (consumer template); reindex-from-source only; a holder. | Storage | Issues/analytics | Storage §3.4 |
| 11.7 | **Reserve/settle cost gate** — `reserve` at dispatch (no balance → no start), `settle` on completion, never interrupt in-flight; meter integer minor-units; wholesale ≠ markup. The universal gate fronts **every agent run and every CI run** (D8/CI-2). | Agent Fabric (the gate) + Commercial (the wallet, C-1) | Agent Fabric, CI, spend-bearing workflow activities | Agent §5.4; Workflow §6.2; Bus §4.7 |

---

## 12. Tenancy & control plane (`myelin-tenancy`)

| # | Contract | Owner | Consumed by | Defined in |
|---|---|---|---|---|
| 12.1 | **`TenantId` / `Region` / `ResidencyTag`** — `(tenant, region)` is the first-class partition key everywhere; injected by the harness from config. | `myelin-tenancy` | **every service** | Tenancy §12.1; `00 §2.5` |
| 12.2 | **`discover(slug \| tenant_id) → { cell_id, region, cell_endpoint, ttl }`** — PII-free routing only; cacheable with bounded staleness; off the per-request hot path. | control plane | CLI, SDKs, gateways, git wire | Tenancy §9, §12.1 |
| 12.3 | **`place(region, requested_tier) → { tenant_id, home_cell, isolation_tier, cell_endpoint }`** + **`placement_of(tenant_id) → { region, home_cell, member_cells, isolation_tier, status }`** — region-first, sticky, PII-free; placement happens *before* identity capture; returns the multi-cell fan-out list. | control plane | signup edge, cell gateways, DSR orchestrator | Tenancy §6, §12.1 |
| 12.4 | **`residency_verify(tenant_id) → SignedAttestation`** — every store reports the tenant's region; the §8 proof. | control plane | `myelin tenant residency verify`, auditors | Tenancy §8, §12.1 |
| 12.5 | **Isolation-tier contract** — `logical \| schema \| db \| cell`; the partition key is **identical at every tier** (the tier changes physical separation, never the logical key); the matrix holds across every shared system's partition surface. | Tenancy (the spectrum) | every shared system | Tenancy §7 |
| 12.6 | **Cross-cell PII-free pointer bridge** (FLOOR) — carries only `subject`/`type`/`correlation_id`; per-viewer resolution always local; never payload/PII. | control plane (Bus §7.4 bridge) | multi-cell event/ref/search/inbox/workflow/DSR fan-out | Tenancy §10; Bus §7.4 → **P4** |

---

## 13. The reconciliation anchors (X-5) — where names & units are frozen

Two sections are the canonical anchors every other contract reconciles against (directive X-5: reconcile
**names AND units** before either side ships):

- **Canonical envelope field list** — `00 §2.10` + Bus §3.1. Units pinned platform-wide: **timestamps =
  RFC-3339 UTC**; **budgets/costs = integer minor-units (never floats)**; **TTLs/staleness windows/timers =
  seconds**; resilient-client **timeouts = milliseconds**; **`pii_key_ref` = `kms://<tenant>/<dek-epoch>/<class>`**
  (`<class> ∈ {tenant, subject:<id>, blob}`, Storage §4.2).
- **`ArtifactRef` subsystem/type token table** — Bus §6.2 (canonical singular tokens `git`/`ci`/`issue`/
  `knowledge`/`chat`/`identity`/`refs`; CLI noun aliases are render-time only). Refs is the validator, not a
  second authority.

The two contracts that are **load-bearing and shared by the most consumers**, and therefore the highest-risk
to drift, are **2.1 `EventEnvelope`** (every emitter + consumer) and **4.3 `list_objects`** (Search + Refs +
Notif + every permission-aware read). Both are frozen here; their open *encodings* (`list_objects` push-down
shape; the per-subsystem taxonomy completion) are the named P4 items in
[`drills-and-open-questions.md`](./drills-and-open-questions.md).

---

## 14. Cross-references
- [`README.md`](./README.md) — the Phase-3 index, committed designs, spine changes, Phase-4 handoff.
- [`drills-and-open-questions.md`](./drills-and-open-questions.md) — the consolidated drill inventory, open
  questions by resolver, and the consistency pass.
- The 11 Phase-3 system docs (`00-platform-substrate`, `identity-and-access`, `event-bus`, `reference-graph`,
  `search-and-indexing`, `notifications`, `agent-fabric`, `durable-workflow`, `storage`, `gdpr-and-audit`,
  `tenancy-and-control-plane`).
- Spine: [`architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md);
  [`integration-directives.md`](../02b-doctrine-integration/integration-directives.md).
