# Phase 5 — Refined Contract Index (the frozen build-to surface)

> Phase: `05-refined-shared-systems-architecture`. **Supersedes**
> [`planning/03-shared-systems-architecture/contract-index.md`](../03-shared-systems-architecture/contract-index.md).
> Canonical brief: [`VISION.md`](../../VISION.md). Companion + rationale:
> [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) (resolves X-1..X-7, OQ-A..OQ-L; read
> it for *why* each shape is what it is). Date: 2026-06-19.
>
> **What this is.** The single consolidated map of **every cross-system contract** Phase 6 (roadmaps) and
> Phase 7/8 (build) must implement or call, now incorporating the Phase-5 reconciliation. For each contract:
> **owner**, **consumers**, **definition site**, and a **status vs Phase 3**:
> - **CONFIRMED** — unchanged from Phase 3; ratified.
> - **SHARPENED** — the contract stood, but its open encoding/shape is now **frozen concrete** (was "→ P4").
> - **NEW** — a contract or sub-shape named for the first time in Phase 5.
>
> A contract here is **stable** (ADR-01): changing it is one whole-workspace PR that breaks every consumer's
> build *now*, never silently in production. The two reconciliation anchors (the `EventEnvelope` field list +
> units `00 §2.10`; the `ArtifactRef` token table Bus §6.2) are **CONFIRMED unchanged** and remain the
> names/units authority every contract aligns to.
>
> **Units (frozen):** timestamps = RFC-3339 UTC; budgets/costs = integer minor-units; TTLs/staleness/timers =
> seconds; resilient-client timeouts = milliseconds; `pii_key_ref = kms://<tenant>/<dek-epoch>/<class>`,
> `<class> ∈ {tenant, subject:<id>, blob}`. Signatures are Rust-shaped (ADR-02); cross-language services
> consume the identical wire shape.

---

## 0. The clusters

1. Bootstrap & service shell · 2. Event envelope, outbox & consumer template · 3. Signals/Automations/
Triggers & the firehose · 4. Identity & access · 5. `ArtifactRef`, refs & projection (incl. the Git↔CI check
seam) · 6. Search · 7. Notifications · 8. Agent fabric · 9. Durable workflow · 10. GDPR/Audit/
`PersonalDataHolder` · 11. Storage · 12. Tenancy & control plane. The shared crates (`myelin-content`,
`myelin-query`) get their refined shapes called out in §13.

---

## 1. Bootstrap & service shell (substrate `00`)

| # | Contract | Owner | Consumed by | Status | Site |
|---|---|---|---|---|---|
| 1.1 | **`serve(AppSpec)`** — boot → migrate → outbox relay → consumers → three ports → graceful drain. | substrate | every service `main.rs` | CONFIRMED | `00 §3.1` |
| 1.2 | **Three-surface topology** — public (gateway-fronted, identity-injected) / internal RPC / metrics-health; public↔internal is a security boundary; tenant from token. | convention + harness | every service | CONFIRMED | `00 §4` |
| 1.3 | **Liveness ≠ readiness.** | harness | every service | CONFIRMED | `00 §4.3` |
| 1.4 | **`PersonalDataHolder` auto-registration** — every store the harness opens. | harness | every store-opening service | CONFIRMED | `00 §3.4` |
| 1.5 | **Forward-only online migrations** + **hot-table flags** each subsystem declares (KN `block`/`db_row`/`doc_op`; all high-write). | migration runner | every schema owner | SHARPENED (hot-table declaration frozen) | `00 §9` |
| 1.6 | **Architecture lints** — `no-cross-db`, `no-raw-publish`, `tenant-predicate`, `no-host-exec`, `forward-only-migration`, `no-cross-sync-cycle`, `residency-pin`, `control-plane-pii-free`, `search-requires-acl-filter`, `no-llm-in-platform`, `no-untagged-personal-data`, `flow-determinism`. | substrate/CI | every crate | CONFIRMED | `00 §2.11` |
| 1.7 | **Cross-language harness shim (frozen)** — three-surface, liveness≠readiness, no fire-and-forget emit, `PersonalDataHolder`, resilient-client, shed order, forward-only migrations: the contract a non-Rust subsystem must satisfy. | the diverging subsystem (Chat connection tier likely, TE-21) | Chat (if it diverges) | SHARPENED (frozen as the divergence contract) | `00 §13 Q1` |
| 1.8 | **Telemetry signal set** — RED/USE + consumer-lag + outbox-depth + breaker-state + fail-static ratios + shed-counts + causal-depth; the Phase-5 drill survival signals. | harness | every shared system, every drill | CONFIRMED | `00 §10.2` |
| 1.9 | **`ResilientClient::call(target, req, idem)`** — timeout + breaker + bulkhead + jittered-retry-idempotent-only; honours `Retry-After`. | substrate (`myelin-client`) | every inter-service caller, CLI, agent runtime | CONFIRMED | `00 §6`; ADR-16 |
| 1.10 | **`FailStatic<T>`** — bounded-staleness cache; `static_max ≤ revocation SLA` and ≥ agent-token TTL. | substrate | Id (primary), Notif, any critical-dep caller | CONFIRMED | `00 §8`; ADR-17 |
| 1.11 | **Protected-human-lane shed order** — speculative → batch/CI → agent → human-last; `429 + Retry-After`; **+ per-surface shed budgets** (CI-surge / collab op-stream / connection-storm / agent-mention-storm) as named v1 floors tuned by drills. | harness + gateway + each subsystem (its budget) | every public surface | SHARPENED (per-surface budget floors named, OQ-K) | `00 §7`; recon §OQ-K |

---

## 2. Event envelope, outbox & consumer template (Bus `myelin-events`)

| # | Contract | Owner | Consumed by | Status | Site |
|---|---|---|---|---|---|
| 2.1 | **`EventEnvelope`** — the canonical versioned envelope (event_id ULID, type, schema_ver, tenant, region, actor, subject ArtifactRef, aggregate, correlation/causation/depth, contains_personal_data/data_role/visibility/pii_key_ref, occurred_at/recorded_at, payload). References-not-payloads. **The names/units anchor.** | Bus | every emitter + consumer | CONFIRMED | Bus §3.1; `00 §2.10` |
| 2.2 | **`OutboxTx::emit(draft, cause)`** — the ONLY sanctioned emit path; same tx; causality correct-by-construction. No `publish_now`. | Bus | every state-changing handler | CONFIRMED | Bus §5.2; BUS-2 |
| 2.3 | **`outbox` table** — `(event_id UNIQUE, aggregate, seq, subject, envelope)`, `UNIQUE(aggregate, seq)` ordering; relay `FOR UPDATE SKIP LOCKED`. Per-**ref**/per-**conversation** aggregate ordering at production QPS (D-9 drill). | Bus (convention) | every producing service | CONFIRMED | Bus §3.2, §4.1 |
| 2.4 | **`EventHandler` consumer template** — `subjects()` whitelist (never `*`) + `handle → {Done\|NonRetryable\|Retry}`; durable-bind-by-name, ack-after-enqueue, dedup ledger, bounded prefetch, lag metric. | Bus | every consumer | CONFIRMED | Bus §4.2; BUS-3 |
| 2.5 | **`consumer_dedup` ledger** — `(consumer, event_id)` PK. | Bus | every consumer | CONFIRMED | Bus §3.3 |
| 2.6 | **Reindex-from-source** — `events::reindex(scope)` → owner `replay(scope, since)` emits `*.snapshot` via outbox through the live consumer; **sub-artifact-granular** (CI one-run, KN page-subtree at block granularity). The only recovery path for derived stores. | Bus + every subsystem | Search, Refs, OLAP, Notif | CONFIRMED | Bus §4.9; REF-4/SEARCH-1 |
| 2.7 | **Crypto-shred / tombstone on the log** — `*.erased` tombstones; inline-PII events envelope-encrypted with `pii_key_ref`; bus is a holder. | Bus | DSR orchestrator, consumers | CONFIRMED | Bus §4.8 |
| 2.8 | **Schema evolution / upcasters** — `(type, from_ver) → to_ver` pure fns at consume; forward-only. | Bus | every consumer | CONFIRMED | Bus §4.10 |
| 2.9 | **Event taxonomy + token table** — `<subsystem>.<artifact_type>.<event_name>`. **+ new tokens:** `ci.check.updated`, `ci.result` (X-1); type token `initiative` (issue-family). Each subsystem completes its list. | Bus (grammar + seed) | every subsystem | SHARPENED (new tokens registered) | Bus §6; recon §2 |

---

## 3. Signals, Automations, Triggers & the firehose (Bus)

| # | Contract | Owner | Consumed by | Status | Site |
|---|---|---|---|---|---|
| 3.1 | **`define_signal_rule(SignalRule{matcher, severity, dedup_key_tpl, dedup_window})`** — curated/deduped/ranked subset; `sig.<tenant>.<severity>.<rule>`. Consumers subscribe to Signals, never `evt.*`. | Bus | Notif, agents, reactive consumers | CONFIRMED | Bus §3.4; ADR-19 |
| 3.2 | **`register_automation(AutomationRule{matcher, action, run_as, delegation, budget, gates})`** — stateless per-event reflex; may invoke a durable workflow. | Bus | project admins, subsystems | CONFIRMED | Bus §3.5; ADR-19 |
| 3.3 | **`arm_trigger/disarm_trigger(Trigger{owner, condition, arms_subject, on_resolve, stale_after})`** — stateful per-person promise; armed→{resolved\|stale\|disarmed}; `stale_after` is a `myelin-flow` timer. `condition` is a `QueryAst` over projection state ("all `blocked_by` resolved"). | Bus | Issues, users | SHARPENED (condition = the frozen `QueryAst`) | Bus §3.6; recon §2/OQ-C |
| 3.4 | **`EventMatcher`** — the predicate core; **= the frozen `myelin-query` `QueryAst`** (OQ-C): bounded interpreter, no UDFs/loops/recursion, statically cost-bounded, permission-aware. Not CEL/JSONLogic. No per-subsystem trigger DSL. | Bus + `myelin-query` | Signals, automations, triggers, saved views, Search, Notif prefs | SHARPENED (= the frozen `QueryAst`) | Bus §4.5; ADR-07; recon §X-3 |
| 3.5 | **Firehose transport + the resume-cursor subscription protocol (NEW)** — `firehose::publish(stream, frame)` / `tail(stream, range)`; **`subscribe(stream, scope, cursor?) → SubStream`**, frames carry per-`(stream,scope)` monotonic `seq`; **`resume(stream, scope, last_seq)`** backfills `(last_seq, now]` then live (reconnect loses zero ops); `resync_required` → `*.snapshot` fallback; **scope is a bounded selector, never `*`** (board:/doc:/channel:). CI logs / presence / collab op-streams ride this; the durable bus carries only pointer events. | Bus (seam) + KN (collab transport) | CI (logs), Chat (presence/live), Knowledge (collab) | SHARPENED → **NEW protocol** (OQ-J) | Bus §4.3; recon §OQ-J |
| 3.6 | **Reactive/dispatch tier** — Signal→`EventInbox` matching/guarding/rate-limiting; nested causality; structural loop guards; bounded dispatch (drop over-cap); reserve/settle before any run. | Bus | Agent Fabric, Notif | CONFIRMED | Bus §4.7; AG-6 |

---

## 4. Identity & access (`myelin-identity`)

| # | Contract | Owner | Consumed by | Status | Site |
|---|---|---|---|---|---|
| 4.1 | **`authenticate(credential) → Principal{tenant, region, principal_id, kind, data_role, status}`** — any credential (SSO/SCIM/passkey/SSH/PAT/CI/agent/deploy-key); tenant from credential. **+ machine-identity:** SSH-pubkey/deploy-key (repo-scoped machine principal)/PAT/per-job token → Principal. | Id | every gateway/entrypoint | SHARPENED (machine-identity resolution pinned) | Id §4; recon §1 |
| 4.2 | **`check(subject, permission, object, zookie?, caveat?: CaveatContext) → {Allow\|Deny\|Conditional}`** — per-action gate, fail-closed. **+ `CaveatContext{object, field?, transition?, attrs}`** for field/transition ABAC, evaluated here (off the hot `list_objects` path). | Id | every write path, `EffectApi`, gateways, Notif | SHARPENED (`CaveatContext` shape, OQ-E) | Id §8.1; recon §OQ-E |
| 4.3 | **`list_objects(subject, permission, type, zookie?) → Ids{ids, zookie} \| Filter{set_expr, zookie}`** — the leak-free pre-filter; **`SetExpr`** is the consumer-composable set algebra (All/None/Ids/NotIds/`InRelation{relation, via_column}`/Union/Intersect/Difference/`TupleSet{index}`) **lowered to a SQL predicate / JOIN over the consumer's own id column** via the per-tenant **authz reverse index**. No N+1, no post-filter. **The single most load-bearing inter-system contract.** | Id | Search, Refs, Git/CI/Issues/KN/Chat (every permission-aware read) | **SHARPENED → frozen** (the `SetExpr` push-down, OQ-E) | Id §8.2; recon §OQ-E |
| 4.4 | **`list_subjects(object, permission, zookie?) → SubjectTree`** + **`explain(...)→RewriteTrace`** — admin inspector, HITL approver set, Notif read-fanout `watch` resolution; **performant at 50k-member channel density** (served by the same authz reverse index). | Id | admin inspector, HITL, Notif | SHARPENED (read-fanout density pinned) | Id §8.3; recon §1 |
| 4.5 | **`delegation(agent, trigger_actor) → EffectivePolicy`** — `agent.policy ∩ delegation ∩ tenant.policy` (monotone intersection, macaroon caveats). | Id | Agent `EffectApi`, workflow activities | CONFIRMED | Id §7 |
| 4.6 | **`write_tuples([Δtuple], precondition?) → zookie`** — atomic tuple write; returns the zookie to stamp on the object (`page.acl_zookie`, Chat membership). Emitted via outbox. | Id | subsystems, role-compile | CONFIRMED | Id §6, §8.4 |
| 4.7 | **`mint_run_token(agent_id, run_id, delegation_caveats, ttl) → token`** + **`revoke(jti\|principal_id)`** — per-run attenuated token, life == run life; callable mid-workflow on resume; **self-hosted runner token scoped to one tenant's `SelfHosted` jobs**. | Id | Agent Fabric, CI dispatch, workflow | SHARPENED (self-hosted scope) | Id §4, §11; recon §1 |
| 4.8 | **`resolve_pseudonym(subject, tenant)`** + **`erase(subject)`** — the pseudonym-map shred (DSR step 1); **pseudonym grammar `<pseudonym>@<tenant>.noreply` (frozen)**; Git commits pseudonymous-by-default. | Id | Git, Audit, DSR orchestrator | SHARPENED (grammar pinned) | Id §11; recon §1/§X-7 |
| 4.9 | **Per-subsystem ReBAC namespace fragment** — each declares relations + permissions; compiled into one cell schema. **Frozen fragments:** Git (ref-glob + CODEOWNERS-as-relations + `approve_untrusted_ci`); CI (`ci_project/environment/secret/run` + `read & !is_untrusted_fork`); Issues (`issue` + field/transition caveats); KN (page-tree inherit-with-overrides + row + field caveat); Chat (`channel.read = member + parent_project->read`). `watcher` relation per watchable type. | Id (engine) + each subsystem (fragment) | every subsystem | SHARPENED (fragments frozen) | Id §5; recon §1 |
| 4.10 | **`Consistency`/zookie semantics** — read-your-writes; zookie-stamped reads bypass the fail-static cache; security-sensitive transitions carry a zookie; the authz reverse index honours the zookie revision watermark. | Id | Search, Refs, Notif, every authz read | CONFIRMED | Id §8.4 |
| 4.11 | **`FailStatic` bound (Id usage)** — `static_max ≤ revocation SLA` ≥ agent-token TTL; coarse "actor active / coarse grants". **DPO ratifies the bound (L-1).** | substrate + Id | Id, Notif, any critical-dep caller | CONFIRMED (`[OPEN — LEGAL]` ratification) | Id §10; ADR-17 |

---

## 5. `ArtifactRef`, refs & projection (`myelin-refs`) — incl. the Git↔CI check seam

| # | Contract | Owner | Consumed by | Status | Site |
|---|---|---|---|---|---|
| 5.1 | **`ArtifactRef`** — `myelin://<tenant>/<subsystem>/<type>/<id>[#sub]`; `parse`/`format` reject ambiguity. **Issues id grammar frozen `<PROJECTKEY>-<seqno>`** as the stored canonical key; `#1421` is the render-time display projection (REF-3 reconciliation). | Refs (parse/format/resolve) + Bus (type table) | every service | SHARPENED (Issues key grammar; REF-3 reconciled) | Refs §3.1; recon §3 |
| 5.2 | **`resolve(ref, viewer, mode) → Projection \| Tombstone`** — live per-viewer unfurl/embed; denied → tombstone; `Display` mode = the Notif humanisation projection. **Cross-cell: resolution is always cell-local** (OQ-I). | Refs | Chat unfurl, PR context pane, KN embeds, Notif | SHARPENED (cell-local resolution pinned) | Refs §4.2; recon §OQ-I |
| 5.3 | **`backlinks/edges/traverse`** — leak-free inverse via `list_objects`; bounded cycle-safe recursive-CTE walk (depth 16). | Refs | "what references this", impact, hierarchy, Notif | CONFIRMED | Refs §4.4 |
| 5.4 | **`refs.edge.created` / `refs.edge.removed`** — emitted by producers via outbox; the `mention`/`artifact_ref`/`embed` content nodes are the producers; **no standalone edge-write API**. Commit trailers / PR links / two-way db relations produce edges; best-effort eventual inverse. | Refs (consumes) + content-producing subsystems (emit) | Refs edge-builder | CONFIRMED | Refs §4.1 |
| 5.5 | **TE-7 typed-edge mirror** — lifecycle edges (`closes/blocks/blocked_by/depends_on/parent/assigns/relates`) dual-homed: the typed relation table (Issues `issue_relation` / Knowledge `db_relation`,`page_parent`) is source of truth; Refs holds the rebuildable projection + fixes inverse pairing. | Refs + Issues/Knowledge | cross-subsystem traversal | CONFIRMED | Refs §3.3; REF-1 |
| 5.6 | **`project(ref, viewer) → {title, state, icon, render_hint, sub_anchor?}`** — REQUIRED on every subsystem (ADR-13.1); per-viewer, pre-permission-checked; the only way Refs/Search/Notif read another subsystem's artifact. | each subsystem | Refs, Search, Notif | CONFIRMED | Refs §5.2 |
| 5.7 | **Unified `#sub` sub-artifact scheme (frozen)** — one grammar (`comment-`/`thread-`/`message-`/`b`/`h`/`row-`/`field-`/`L<a>-L<b>`/`check-`/`step-`), stable opaque ids minted by each owner; Refs stores full sub-URN + stripped root. **One 4-step tombstone ladder** (permission → root → sub-resolve {live/moved/outdated/gone} → erased). Git line-ranges are **content-anchored** (BLAKE3 fingerprint + 3-way context match → exact/rebased/partial/tombstone). | Refs (grammar + ladder) + each subsystem (stable mint) | Refs, Search | **SHARPENED → frozen** (X-4/OQ-D) | Refs §3.5; recon §X-4 |
| 5.8 | **`reindex(scope)`** (Refs) — reindex-from-source for the edge index + projection cache; never reads owner DBs. | Refs | ops, GDPR re-erasure, new consumers | CONFIRMED | Refs §4.7 |
| 5.9 | **The Git↔CI `CheckStatus` seam (NEW)** — CI-owned `CheckStatus{repo, commit_oid, context, state, required, run, run_attempt, trust_tier, details_ref, summary, ...}` keyed `(commit_oid, context)`, **last-writer-wins by `run_attempt`** (monotonic supersession). Emitted as `ci.check.updated` via outbox; Git maintains the `check_status` **projection table** + the branch-protection `required`-set policy + fork-endorsement (`approve_untrusted_ci`). The merge queue is a durable workflow waking on the rollup `ci.result` signal (9.4); an `untrusted_fork` success is **neutral for gating** until endorsed/re-run-trusted. | **CI** (producer) + **Git** (gate) | Git merge gate, PR checks UI | **NEW** (X-1/OQ-A) | recon §X-1 |

---

## 6. Search (`myelin-search`)

| # | Contract | Owner | Consumed by | Status | Site |
|---|---|---|---|---|---|
| 6.1 | **`query(ast, viewer, zookie?, page) → RankedResults`** — AST compiled to FT/structured/vector; **always conjoins the OQ-E `list_objects` `Filter`** before scoring (`search-requires-acl-filter` lint). The Issues Tier-3 board-escalation valve compiles the board query to Search with the same `Filter` (now unblocked). | Search | every search UI, CLI, agents (RAG) | SHARPENED (the OQ-E `Filter` conjoin frozen; Tier-3 unblocked) | Search §4.2; recon §OQ-E/§4 |
| 6.2 | **`semantic(text\|vec, viewer, k, filter_ast?) → k visible NN`** — ACL-filtered-during-traversal k-NN; agent RAG, dedup. | Search | agent RAG, dedup | CONFIRMED | Search §4.5 |
| 6.3 | **`declare_indexable(IndexSpec{subsystem, type, projection, ft_fields, struct_fields, semantic, acl_object_type})`** — per-subsystem projection (Git code path/symbols/literals/commit-msg + trigram, camel/snake; KN block+page multilingual + vector-in-v1 + JSONB struct; Issues facets). **Measured projection-feeder promotion** (facet > threshold → generated index). | each subsystem | Search | SHARPENED (measured promotion threshold, OQ-C) | Search §5.1; recon §4 |
| 6.4 | **`reindex(scope) → job`** — the only rebuild path; sub-artifact-granular `*.snapshot` replay. | Search | admin/ops, post-restore | CONFIRMED | Search §4.9 |
| 6.5 | **Code-search input** — Git emits an indexable `git.*` projection per blob/ref/symbol. **Named follow-on:** consume CI-produced SCIP/LSIF for "find usages" (future). | Git + CI (future) | Search | SHARPENED (SCIP/LSIF follow-on named) | Search §4.4; recon §4 |

---

## 7. Notifications (`myelin-notif`)

| # | Contract | Owner | Consumed by | Status | Site |
|---|---|---|---|---|---|
| 7.1 | **`list_inbox(principal, filter?, page?) → [InboxItem]`** ranked by priority — the ONE inbox (C-9); scoped views (Issues "My Work", Chat "Activity", Git "Review requests") are `filter`s over `reason`/`subject`, never a second store. | Notif | inbox UI, subsystem scoped views, CLI | CONFIRMED | Notif §1.3 |
| 7.2 | **`mark/snooze/mark_all_read`** — one read-state truth across all views. | Notif | inbox UI, CLI | CONFIRMED | Notif §4.1 |
| 7.3 | **`humanise(item\|(template_key, args), viewer, locale) → HumanisedString`** — resolves each `ArtifactRef` per-viewer via Refs `resolve` (Display); permission/erasure-safe; ICU MessageFormat. **The ONE templating surface** — every consumer, every agent-authored message, KN living-doc/Issues SLA/CI status all register here (OQ-L). | Notif | every channel renderer, agent HITL cards + messages | SHARPENED (sole templating surface, OQ-L) | Notif §3.3; NOTIF-1 |
| 7.4 | **`get_prefs/set_prefs`** — routing + quiet-hours; matcher reuses the `QueryAst` predicate core. | Notif | settings UI, CLI | CONFIRMED | Notif §2.2 |
| 7.5 | **`oncall_now(schedule) → principal`; `page(target, reason)`** — resolves rotation; starts an escalation durable workflow. **Escalation-chain config shape frozen** (Issues passes the chain definition: `page → oncall_now → escalate-after-timer` on the wheel). | Notif | CLI, SLA engine, Agent escalations | SHARPENED (chain shape frozen) | Notif §3.7; recon §5 |
| 7.6 | **`define_notif_rule(reason, dedup_tpl, default_class)`** — Signal class → inbox reason/priority. Each subsystem registers its set (Issues SLA/unblocked/approval; KN mentions/comments/shares/watched; Chat mentioned/replied/thread_watched/approval). | Notif + subsystems | admin, subsystems | CONFIRMED | Notif §3.1 |
| 7.7 | **`PersonalDataHolder` (Notif history) + `replay`** — references-not-payloads; erasing a person tombstones their appearance; inbox rebuilt by reindex-from-source. | Notif | DSR orchestrator, Bus reindex | CONFIRMED | Notif §3.8 |
| 7.8 | **`DeliveryAdapter{channel, region, send(RedactedMessage, idem_key), receipts}`** — region-aware, EU-preferring, swappable; PII-minimised off-cell; at-least-once+idempotent. | Notif + provider adapters | email/push/web/mobile/desktop | CONFIRMED | Notif §2.3 |

---

## 8. Agent fabric (`myelin-agent`)

| # | Contract | Owner | Consumed by | Status | Site |
|---|---|---|---|---|---|
| 8.1 | **`ToolSurface::register_tool(ToolDef{name, input_schema, required_caps, effect_kind, side_effecting, requires_approval, exposed_over_mcp})`** + `resolve` — one permissioned catalogue, MCP-exposable. **Per-subsystem `requires_approval` defaults frozen** (CI deploy/secret = yes; Git merge = yes, open_pr = no; Issues forecast/triage = no, SLA transition = caveat-gated; KN publish/confidential = yes; Chat post = no; cross-subsystem effect inherits the target's default). | Agent + every subsystem | every subsystem | SHARPENED (defaults table frozen, X-6) | Agent §6; recon §X-6 |
| 8.2 | **`EffectApi::apply(run, ProposedEffect) → Applied(event_id) \| Gated(gate_id) \| Denied(reason)`** — plan-then-apply: schema → capability → delegation → tenant → budget → HITL gate → apply via the public endpoint → meter. Denied = ordinary tool error. A withheld gated tool does not mutate (AG-8). | Agent | the loop, external MCP, workflow activities | CONFIRMED | Agent §5.2 |
| 8.3 | **`AgentRuntime::step(&Conversation) → UseTools \| Submit`** — the stateless brain; strategy seam (skeleton/mock/llm); platform owns history; `--use-mock` is a real runtime flag. | Agent + runtimes | the loop (an activity) | CONFIRMED | Agent §2.1; AG-1/AG-3/AG-4 |
| 8.4 | **`ToolHands::exec(Command) → ToolResult`** — sandboxed computation; **no host-exec bypass**; **= the CI runner's `kind=agent` job** on the unified sandbox; the real-kernel escape drill gates both kinds. Only `compute`/`external` untrusted code here; mutation goes through `EffectApi`. **Four uniform guarantees** (cost gate, per-run token attribution, HITL withhold, isolation floor+drill). | Agent + CI (runner) | the loop | SHARPENED (the four uniform guarantees pinned, X-6) | Agent §2.2, §5.0; recon §X-6; ADR-20 |
| 8.5 | **`Agent::handle(InboxEvent, &dyn AgentRuntime) → RunOutcome`** — platform-owned bounded multi-turn loop; nested causality; a run is a durable workflow. | Agent | dispatch tier | CONFIRMED | Agent §2.3 |
| 8.6 | **`EventInbox::deliver(InboxEvent)`** — platform delivers matched events (envelope + binding + token + budget); agents don't poll. **Explicit-first dispatch** (CHAT-1): a mention notifies, does not auto-spawn a costed run; implicit auto-dispatch is L-3 (counsel-gated). | Bus → Agent | Agent Fabric | SHARPENED (explicit-first pinned) | Agent §1.3; recon §6 |
| 8.7 | **`run --dry-run(InboxEvent) → Vec<ProposedEffect>`** — plan-then-apply testability. | Agent | CLI, tests | CONFIRMED | Agent §7.1 |
| 8.8 | **AG-7 agent trace** — Knowledge accepts a content-addressed agent-trace write (reusing the block model) + registers it as an erasable holder; distinct from the audit log. | Knowledge (deliverable) + Agent (seam) | Agent Fabric | CONFIRMED | Agent §11; AG-7 |

---

## 9. Durable workflow (`myelin-flow`)

| # | Contract | Owner | Consumed by | Status | Site |
|---|---|---|---|---|---|
| 9.1 | **`DurableExecutor{start, signal, describe, cancel}`** — engine-agnostic; `start` returns a durable handle; **`signal` idempotent on `idem_key`**, with the **per-effect `idem_key` rule** (`card_id` single, `card_id:<effect_idx>` multi/partial approval — a double-click is one approval, a partial approval is well-defined). | Workflow | Bus automations, Agent, CI, Notif escalation, Issues SLA | SHARPENED (per-effect `idem_key`, OQ-F) | Workflow §5.1; recon §OQ-F |
| 9.2 | **`WfCtx{activity, sleep_until, sleep_for, wait_for_signal, now, rand, emit}`** — the deterministic surface; non-determinism journals; `emit` via outbox; `flow-determinism` lint. **The `SCHEDULE_AND_RUN_JOB` long-park idiom**: an activity dispatches a job and returns; completion arrives as a durable signal keyed by `idem_token` (the workflow holds no runtime). | Workflow | every workflow definition (agent run, CI pipeline, merge queue, automation, SLA timer) | SHARPENED (`SCHEDULE_AND_RUN_JOB` idiom, OQ-F) | Workflow §5.1; recon §OQ-F |
| 9.3 | **Durable timer wheel** — `sleep_until/sleep_for` on the minute-bucket partial index + `FOR UPDATE SKIP LOCKED`; millions of timers = an indexed range read; effectively-once. Trigger `stale_after`, SLA timers (cheap disarm/re-arm of a precomputed `fire_at` without calendar logic), snooze re-surfacing, HITL timeouts, KN living-doc automations all ride it. | Workflow | Bus, Issues, Notif, KN | CONFIRMED | Workflow §3.3 |
| 9.4 | **Durable signal (multi-day HITL)** — `state=waiting` holds no runtime; an `approval`/`cancel`/`ci.result`/`job.done` signal arrives hours/days later (idempotent), re-leases + replays + consumes. **The merge-queue `ci.result` wait** (X-1) and CI protected-env / Chat approval-card / KN approval-card / Issues escalation all use it. | Workflow | Agent HITL, Chat, CI merge queue | SHARPENED (`ci.result`/`job.done` waits pinned) | Workflow §3.4; recon §X-1/OQ-F |
| 9.5 | **Workflow↔agent mapping** — workflow owns `RunBudget`/gates/state; `step`/`exec` are activities; reserve/settle = the bookends. | Workflow + Agent | agent runs, CI pipelines | CONFIRMED | Workflow §6.1 |
| 9.6 | **`PersonalDataHolder` (workflow history) + `replay`** — references-not-payloads; inline-PII rows per-subject crypto-shred. | Workflow | DSR orchestrator | CONFIRMED | Workflow §4.8 |

---

## 10. GDPR / Audit / `PersonalDataHolder` (`myelin-gdpr`)

| # | Contract | Owner | Consumed by | Status | Site |
|---|---|---|---|---|---|
| 10.1 | **`PersonalDataHolder{locate, export, rectify, restrict, erase}`** — every store; exhaustive holder list (H1–H18); harness auto-registers. Erasure = purge/crypto-shred/pseudonymise, never hide. `restrict` suppresses indexing/agent-use/analytics/notif for a subject. | `myelin-gdpr` + every store | DSR orchestrator | CONFIRMED | GDPR §3.1 |
| 10.2 | **`#[personal_data(category, role, basis, retention, erasure, subject_locator)]`** classify derive — the `no-untagged-personal-data` lint. **+ worklog/productivity/estimate fields tagged `category=behavioural, role=tenant-content, restricted by default`** (OQ-H, `[OPEN — LEGAL]`). | GDPR derive | every schema owner | SHARPENED (worklog tags, OQ-H) | GDPR §2.1; recon §OQ-H |
| 10.3 | **`data_map()/ropa(tenant)`** — generated from tags + holders; drives DSR fan-out, breach scoping, RoPA, DPIA. | GDPR/Audit | DPO, breach-scoping, DSR | CONFIRMED | GDPR §2.2 |
| 10.4 | **`dsr_submit/dsr_status/dsr_certificate → MerkleProvenBundle`** — the DSR state machine; 1-month durable timer; **iterates `member_cells`** for multi-cell (over the OQ-I bridge). Art. 28 operable by/for tenants. | GDPR/Audit | ops, tenant admins, auditors | CONFIRMED | GDPR §4 |
| 10.5 | **Retention / consent / sub-processor / legal-hold** — `effective_retention` (tightest-wins, legal-hold-aware); `consent_record/withdraw`; `subprocessors`/`transfer_allowed` (deny extra-EU by default). **+ outbound push-mirror to a foreign host is gated here** (NEW residency gate). | GDPR/Audit | retention engine, adapters, ops/legal, Git mirror | SHARPENED (outbound-mirror gate, §10) | GDPR §5; recon §10 |
| 10.6 | **Tamper-evident audit log** — per-tenant hash-chain + Merkle (CT-style proofs, external witness); minimised; written via outbox only. **History-rewrite is an audited op here** (rate-limited, with fork/mirror/clone-cache invalidation fan-out). | GDPR/Audit | every action-taking service, auditors, Git erasure-admin | SHARPENED (history-rewrite audited op, §9) | GDPR §6; recon §9 |
| 10.7 | **eDiscovery / legal-hold export** — `ediscovery_export(scope) → MerkleProvenBundle`; content-addressed, inclusion-proof-bearing, legal-hold-frozen. | GDPR/Audit | legal/auditors | CONFIRMED | GDPR §5.4 |
| 10.8 | **Erasure ledger** (PII-free, non-shred-erasable) — opaque subject id + holders/keys shredded; drives post-restore re-erasure (GD-14). | GDPR/Audit | Storage restore | CONFIRMED | GDPR §4.4 |
| 10.9 | **The ONE free-text / immutable-content erasure posture (NEW)** — structural floor: per-subject DEK crypto-shred (self-authored) + pseudonym-map shred (identity) + `restrict` suppression. Residual: third-party/immutable free-text PII authored by others → documented lawful-basis limit + best-effort `rectify`/tombstone + (git) pseudonymous-by-default or audited history-rewrite. **Instantiated per subsystem by reference, not restated.** `[OPEN — LEGAL]`: counsel/DPO ratify the residual basis (one statement). | GDPR/Audit (owns the posture) + every subsystem (by reference) | DSR, Legal/DPO, all subsystems | **NEW** (X-7/OQ-G), `[OPEN — LEGAL]` | recon §X-7 |

---

## 11. Storage — tiers / `BlobStore` / KMS / reserve-settle / backup-restore

| # | Contract | Owner | Consumed by | Status | Site |
|---|---|---|---|---|---|
| 11.1 | **OLTP tier client** — harness pool + thin visible SQL (tenant-scoped, RLS, encrypted columns); one DB per service; no cross-DB; the outbox lives here. | Storage | every subsystem | CONFIRMED | Storage §3.1 |
| 11.2 | **`BlobStore{put, get, head, delete}`** — content-addressed (BLAKE3, per-tenant dedup); fs↔object one-line swap; immutable-tier erasure = crypto-shred. **+ object-backed pack/delta seam** (Git STOR-5, impl = Git P4); **+ within-EU CDN clone/bundle blob class** (NEW); **+ trust-tier/branch-scoped cache namespaces** (NEW — an UntrustedFork write cannot reach the trusted cache scope). | Storage | every blob-holding service; git pack tier | SHARPENED (CDN class + trust-scoped cache, §8) | Storage §3.2; recon §8 |
| 11.3 | **KMS hierarchy + `KeyOrigin` trait** — per-cell root → per-tenant KEK → per-tenant/per-subject DEK; `KeyOrigin{wrap, unwrap, can_derive_plaintext_index, destroy}` (platform/BYOK/HYOK); `can_derive_plaintext_index()=false` structurally skips Search/Agent indexing. | Storage/GDPR | every encrypted store, Search, Agent | CONFIRMED | Storage §4 |
| 11.4 | **Crypto-shred + GD-4 granularity** — free-text/profile/body/agent-memory/op-log = per-subject DEK (**incl. CI log segments** — NEW per-subject granularity); bulk pseudonym-referenced = per-tenant DEK; tenant offboarding = the KEK. The `erasure` tag drives the key choice. | Storage | DSR orchestrator | SHARPENED (per-subject CI-log DEK, §8) | Storage §5.1; recon §8 |
| 11.5 | **Backup / restore / cross-seam** — WAL+PITR (RPO ≤ 5 min); `restore(to_offset)`; `restore-verify` (CI-gated, ADR-18); event-log offset = the cross-seam cursor (OLTP↔blob↔index↔offset); `post_restore_reerase`. Derived stores rebuilt, not restored. | Storage | ops/DSR, CI durability gate | CONFIRMED | Storage §7 |
| 11.6 | **OLAP read store** — CQRS read model fed by the bus; reindex-from-source only; a holder. **+ honours the restriction flag** (no analytics for a restricted subject); worklog fields analytics-eligibility per OQ-H. | Storage | Issues/analytics | SHARPENED (restriction-flag propagation, §8) | Storage §3.4; recon §8 |
| 11.7 | **Reserve/settle cost gate** — reserve at dispatch (no balance → no start), settle on completion, never interrupt in-flight; integer minor-units; wholesale ≠ markup. Fronts **every agent run and every CI run** + every `SCHEDULE_AND_RUN_JOB`. | Agent (gate) + Commercial (wallet, C-1) | Agent, CI, spend-bearing workflows | CONFIRMED | Agent §5.4; CI-2 |
| 11.8 | **T3 log tier (CI)** — sealing firehose frames into T2 content-addressed segments + an OLTP **`(job, step, byte-range)` index**, per-tenant-DEK; the jump-to-failure `details_ref` (5.9) resolves through it. | Storage + CI (heaviest consumer) | CI logs, the check seam | SHARPENED (CI `(job,step,byte-range)` index frozen, §8) | Storage §3.3; recon §8 |

---

## 12. Tenancy & control plane (`myelin-tenancy`)

| # | Contract | Owner | Consumed by | Status | Site |
|---|---|---|---|---|---|
| 12.1 | **`TenantId`/`Region`/`ResidencyTag`** — `(tenant, region)` is the first-class partition key; injected by the harness. | `myelin-tenancy` | every service | CONFIRMED | Tenancy §12.1 |
| 12.2 | **`discover(slug\|tenant_id) → {cell_id, region, cell_endpoint, ttl}`** — PII-free routing; off the hot path; usable by the git wire. **+ repo-granular `placement_of(repo) → cell + group`, region-pinned, relocatable** (no node-pinning). | control plane | CLI, SDKs, gateways, git wire | SHARPENED (repo-granular placement, §10) | Tenancy §9; recon §10 |
| 12.3 | **`place/placement_of`** — region-first, sticky, PII-free; returns `member_cells` for multi-cell fan-out. | control plane | signup edge, gateways, DSR | CONFIRMED | Tenancy §6 |
| 12.4 | **`residency_verify(tenant_id) → SignedAttestation`** — every store reports the tenant's region; the no-global-pool property attestable (CI runner/log/artifact/cache region included). | control plane | `myelin tenant residency verify`, auditors | SHARPENED (CI no-global-pool attestation, §10) | Tenancy §8; recon §10 |
| 12.5 | **Isolation-tier contract** — `logical\|schema\|db\|cell`; the partition key is identical at every tier. | Tenancy | every shared system | CONFIRMED | Tenancy §7 |
| 12.6 | **Cross-cell PII-free pointer bridge** — `CrossCellPointer{subject (opaque), type, correlation_id, home_cell}`; resolution always cell-local (the home cell renders + permission-checks; only the projection crosses). The named multi-cell floor ISS rollup / KN collab / CHAT cross-org channels ride. | control plane | multi-cell event/ref/search/inbox/workflow/DSR fan-out | SHARPENED → frozen (frame pinned, OQ-I) | Tenancy §10; recon §OQ-I |

---

## 13. The shared crates' refined shapes (`myelin-content`, `myelin-query`)

| # | Contract | Owner | Consumers | Status | Site |
|---|---|---|---|---|---|
| 13.1 | **`myelin-content` taxonomy (frozen)** — the canonical block set (paragraph/heading/lists/task_list/blockquote/code_block/callout/table/divider/image/embed/db_view/toggle/sync_block) + inline = markdown-subset string with three structured nodes (`mention`/`artifact_ref`/`embed`). Chat & Issues consume strict subsets. **WASM compile target** (one editor render path; `render(parse(md)) === md`). The three inline ref nodes produce `refs.edge.created` uniformly. | Knowledge (leads) + Chat/Issues (consume subset) | Chat, Issues, Knowledge, Refs, Search | **SHARPENED → frozen** (X-2/OQ-B) | recon §X-2; ADR-05 |
| 13.2 | **ADF → `myelin-content` lossy-map (frozen)** — the Issues import conversion table; lossy nodes named + recorded in the import report. | Issues (import) | Issues | **SHARPENED → frozen** (X-2) | recon §X-2 |
| 13.3 | **`myelin-query` primitive (frozen byte-identical)** — the field-type enum, the view-model (`ViewSpec`), the `QueryAst` grammar (= the `EventMatcher` core, 3.4), and the **`order_key`/LexoRank fractional-index encoding** (base-62 `0-9A-Za-z`, lexicographic compare, midpoint bisection, 2-char jitter, 48-char rebalance trigger, `created_at`+ULID tiebreak). Issues & Knowledge each own their compiler/executor; the definitions are identical. | Issues + Knowledge (co-own) + Search (compile target) | Issues, Knowledge, Search, Bus (matcher) | **SHARPENED → frozen** (X-3/OQ-C) | recon §X-3; ADR-06/07 |

---

## 14. The reconciliation anchors (CONFIRMED unchanged)

The names/units authority is unchanged from Phase 3 and remains binding (directive X-5):
- **Canonical envelope field list + units** — `00 §2.10` + Bus §3.1 (timestamps RFC-3339 UTC; costs integer
  minor-units; TTLs/timers seconds; client timeouts ms; `pii_key_ref` shape).
- **`ArtifactRef` subsystem/type token table** — Bus §6.2 (canonical singular tokens
  `git`/`ci`/`issue`/`knowledge`/`chat`/`notif`/`signal`/`identity`/`agent`/`refs`; CLI aliases
  render-time only; **+ `initiative`** type token). Refs is the validator, not a second authority.

The two highest-fan-in contracts (and thus highest drift-risk), now both **frozen with their concrete
shapes**: **2.1 `EventEnvelope`** (every emitter + consumer) and **4.3 `list_objects`** with the `SetExpr`
push-down (Search + Refs + Notif + every permission-aware read). The previously-open encodings are now
closed.

---

## 15. Change summary vs Phase 3 (what moved shape)

**NEW contracts:** 5.9 (Git↔CI `CheckStatus` seam, X-1) · 10.9 (the one free-text/immutable erasure posture,
X-7, `[OPEN — LEGAL]`) · the trust-tier/branch-scoped cache namespaces + within-EU CDN clone class (11.2) ·
the outbound-mirror residency gate (10.5) · history-rewrite audited op (10.6).

**SHARPENED → frozen (was open "→ P4"):** 4.3 `list_objects` `SetExpr` push-down (OQ-E, the most-repeated
ask) · 4.2 `CaveatContext` (field/transition ABAC) · 5.7 unified `#sub` grammar + tombstone ladder (X-4) ·
13.1/13.2 `myelin-content` taxonomy + ADF map (X-2) · 13.3 `myelin-query` + `order_key` parity (X-3) · 3.5
firehose resume-cursor protocol (OQ-J) · 9.1/9.2/9.4 `SCHEDULE_AND_RUN_JOB` + per-effect `idem_key` (OQ-F) ·
8.1/8.4 sandbox `requires_approval` defaults + four uniform guarantees (X-6) · 12.6 cross-cell bridge frame
(OQ-I) · 5.1 Issues `<PROJECTKEY>-<seqno>` key + REF-3 reconciliation · 11.4/11.8 per-subject CI-log DEK +
`(job,step,byte-range)` index · 11.6 restriction-flag into OLAP · 2.9 new event tokens · 4.7/4.8/4.9
machine-identity + pseudonym grammar + ReBAC fragments · 7.3 sole templating surface · 7.5 escalation chain.

**CONFIRMED unchanged:** the envelope + outbox + consumer template, the `PersonalDataHolder` spine, the
reserve/settle gate, reindex-from-source, the timer wheel + durable HITL signal, `check`/`delegation`/
`mint_run_token`, the three-surface harness, fail-static, the taxonomy grammar, the projection API.

## 16. Cross-references
- [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) — the rationale for every shape here.
- [`../03-shared-systems-architecture/contract-index.md`](../03-shared-systems-architecture/contract-index.md)
  — the superseded Phase-3 surface.
- Spine: [`../02-holistic-architecture/architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md)
  (ADR-01..ADR-20); [`../02b-doctrine-integration/integration-directives.md`](../02b-doctrine-integration/integration-directives.md).
