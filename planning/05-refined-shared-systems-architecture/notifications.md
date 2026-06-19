# Phase 5 — Notifications (`myelin-notif`): the ONE "what needs me" inbox + delivery fabric (REFINED)

> Phase: `05-refined-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md)
> (single source of truth). Binding doctrine:
> [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) (EI-02)
> §1/§3/§4/§5/§6/§10, [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md)
> (EI-04) §1/§5.3, and the most-relevant UX doc
> [`external-insights/05-ux-and-design.md`](../../external-insights/05-ux-and-design.md) (EI-05) §4/§6.
> Reconciliation spine: [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) (X-1..X-7,
> OQ-A..OQ-L) + [`contract-index.md`](./contract-index.md) (the frozen build-to surface, **supersedes** the
> Phase-3 index). Carries forward the Phase-3 base
> [`../03-shared-systems-architecture/notifications.md`](../03-shared-systems-architecture/notifications.md).
> Date: 2026-06-19.
>
> **What this doc is.** The REFINED, canonical Notifications shared-system architecture Phase 6/7 build on.
> The Phase-3 design is **correct as written**; Phase 5 ratifies it and **closes the Phase-3 open questions**
> with the now-frozen platform shapes (the `list_objects`/`SetExpr` watcher push-down, the sole templating
> surface, the firehose resume-cursor protocol, per-surface shed budgets, the cross-cell bridge frame, the
> frozen escalation-chain shape, the one erasure posture). No ADR is reversed; no contract Notif exposes
> changes shape. Notif gains **zero NEW contracts** — every Notif change-request in the consolidation is
> CONFIRM (the seam was right); the deltas are platform-side shapes Notif now consumes as frozen rather than
> as "→ P4 open."
>
> **Status convention.** *CONFIRM* = the Phase-3 seam stands, ratified. *SHARPEN* = a Phase-3 open
> encoding is now frozen concrete (Notif consumes it). *FLOOR* = partial answer + named follow-on. Every
> failable property names its **drill** (§6).

---

## Changes vs Phase 3 (every change, listed)

All Notif **contracts** (7.1–7.8 in the refined index) are **CONFIRMED unchanged in shape**. What changed is
that several Phase-3 dependencies that Notif left "[OPEN → P4]" are now **frozen**, and Notif's text is
sharpened to consume the frozen shapes. The complete list:

| # | Change | Nature | Source |
|---|---|---|---|
| C1 | **Read-fanout watcher resolution is now the frozen `list_objects`/`list_subjects` `SetExpr` push-down** (OQ-E). Phase 3 said "`list_subjects(subject, watch)` … additive obligation to P4." The shape is now the per-tenant authz reverse index + `SetExpr` lowered over the consumer's own id column, performant at 50k-member channel density. Notif's read-fanout (§3.5) binds to it concretely. | SHARPEN (consume) | recon §OQ-E; index 4.3/4.4 |
| C2 | **`humanise`/ICU registry is ratified as the ONE platform templating surface** (OQ-L). Phase 3 owned it; Phase 5 freezes that KN living-doc templates, Issues SLA strings, CI status summaries, and every agent-authored message register here — there is no second template engine anywhere. | CONFIRM (scope frozen) | recon §OQ-L; index 7.3 |
| C3 | **The escalation-chain config shape is frozen** (`page → oncall_now → escalate-after-timer` on the `myelin-flow` timer wheel); Issues passes the chain definition; Notif owns policy, the workflow engine owns durability. Phase 3 had this as a CO-DESIGN with Issues. | SHARPEN (co-design → frozen) | recon §5; index 7.5 |
| C4 | **`inbox watch` live transport is now the frozen firehose resume-cursor protocol** (OQ-J): `subscribe(stream, scope, cursor?)` / `resume(stream, scope, last_seq)`, per-`(stream,scope)` monotonic `seq`, `resync_required` → snapshot fallback, bounded scope. Phase 3 §9 OQ-8 left this "[OPEN → P4], co-decided with the Chat connection tier." Now closed. | SHARPEN (consume) | recon §OQ-J; index 3.5 |
| C5 | **Per-surface shed budgets are named v1 floors** (OQ-K): the **agent-mention-storm** lane budget (per-tenant agent-run in-flight cap, humans never queue behind agent runs, `429 + Retry-After`) is the budget the D-N5 30×-agent-surge drill asserts against. Phase 3 named the protected-human-lane behaviour; Phase 5 names the budget floor. | CONFIRM + floor | recon §OQ-K; index 1.11 |
| C6 | **The cross-cell inbox-aggregation floor is now the frozen `CrossCellPointer{subject(opaque), type, correlation_id, home_cell}` frame** with **always-cell-local resolution** (OQ-I). Phase 3 §5.4 described the bridge directionally; the frame + the local-resolution rule are now frozen. Still designed-not-built (single-home-cell is v1). | CONFIRM + frame frozen | recon §OQ-I; index 12.6/5.2 |
| C7 | **The free-text/immutable-content erasure residual is handled by reference to the ONE platform posture** (X-7). Notif's holder behaviour (§3.9) is unchanged — references-not-payloads already does the work — but the residual (an off-cell-sent payload, a name in a delivered redacted summary) is now stated *by reference* to contract 10.9, not restated. | NEW (by reference) | recon §X-7; index 10.9 |
| C8 | **`watcher` relation declaration is a frozen ReBAC-fragment obligation on every watchable subsystem** (index 4.9: "`watcher` relation per watchable type"). Phase 3 handed this to "P4 subsystems"; it is now a frozen fragment item Git/CI/Issues/KN/Chat each carry. | CONFIRM (frozen) | recon §1; index 4.9 |
| C9 | **The preference matcher binds to the frozen `QueryAst`** (OQ-C / contract 3.4). Phase 3 said "the same safe query-AST predicate core (ADR-07/AG-7)"; that core is now the byte-frozen `myelin-query` `QueryAst` (= the `EventMatcher` core). No Notif change beyond pinning the now-frozen grammar. | SHARPEN (consume frozen grammar) | recon §X-3; index 13.3/3.4 |
| C10 | **The `mention(Principal)` write-fanout producer is now the frozen inline structured node** in the `myelin-content` taxonomy (X-2), identical across Chat/Issues/Knowledge, and a producer of `refs.edge.created` (5.4). Phase 3 referenced it (ADR-05); the node is now frozen. No Notif change beyond pinning. | SHARPEN (consume frozen node) | recon §X-2; index 13.1 |

Everything else in the Phase-3 doc — the ONE-inbox C-9 resolution, the data model, the deterministic
explainable ranking, the five-mechanism storm-control, the write-vs-read fanout split, the EU-sovereign
delivery fabric, the durable-workflow escalation, reindex-from-source, the `PersonalDataHolder`
implementation, and all ten drills — is **carried forward unchanged** and is cited rather than restated.

---

## 0. Reading map

- **§1** — purpose, responsibilities, the C-9 resolution (the ONE inbox; scoped views are projections).
  *Unchanged from Phase 3 §1; the C-9 ask is CONFIRMED frozen (C1/C8).*
- **§2** — the data model / schemas. *Unchanged from Phase 3 §2; pins the now-frozen `QueryAst` (C9) and the
  frozen escalation-chain shape (C3).*
- **§3** — algorithms: ranking, storm-control, fanout, routing, humanisation, escalation, reindex-from-source.
  *Carries forward Phase 3 §3; §3.3 pins the sole-templating-surface (C2); §3.5 pins the watcher push-down
  (C1); §3.7 pins the frozen chain (C3).*
- **§4** — contracts exposed & consumed (STABLE; matches the refined contract index §7).
- **§5** — scaling/sharding in the cell topology. *§5.4 pins the frozen cross-cell bridge frame (C6); §5.2
  pins the agent-mention-storm shed budget (C5).*
- **§6** — failure modes + the drills owed (carried forward unchanged; D-N5 now asserts against the named
  shed budget).
- **§7** — the `inbox watch` live transport (NEW sub-section: the firehose resume-cursor protocol, C4).
- **§8** — cited prior art.
- **§9** — required changes to foundational systems (now all frozen/CONFIRMED — closed).
- **§10** — remaining open questions for Phase 6.

**Floors named up front** (VISION §3 / EI-04 §4), all carried from Phase 3 and still floors:

- **Cross-cell inbox aggregation** for a multi-cell tenant is **designed-not-built**; single-home-cell is
  complete. The bridge frame is now frozen (C6, §5.4); the build is a Phase-6+ follow-on.
- **EU-sovereign delivery-provider adapters** ship as a swappable-trait floor — the trait + EU-preferring
  posture are built; the concrete production provider (which EU email/push vendor, with what DPA) is a
  sovereignty/legal call (§3.6, §10). Still a floor.
- **ML-tuned ranking** is a named follow-on; v1 ranking is deterministic + explainable (§3.1). Promotion
  trigger = a measured "important things buried" signal (D-N1), behind the same scoring interface.

---

## 1. Purpose, responsibilities, and the C-9 resolution

**Unchanged from Phase 3 §1** — carried forward in full; cited not restated. The substance:

Notifications owns **the ONE canonical, cross-subsystem, prioritised "what needs *me*" inbox** and the
**delivery fabric** that carries items to email/push/web/mobile/desktop. It is the platform's single answer
to *attention*: one place a human **or an agent** (agents have inboxes too — Phase 3 §1.4) sees everything
across git/CI/issues/knowledge/chat addressed to them or that they watch, ranked so important things are not
buried. It owns: the prioritised per-principal inbox; the router (consumes **Signals**, not raw `evt.*` —
ADR-19); storm-control/DEDUP; routing/preferences/quiet-hours; the delivery fabric; on-call/escalation
routing on the durable-workflow substrate; **backend humanisation** (NOTIF-1); and notification history (a
`PersonalDataHolder`). It is **not** the bus, **not** the chat connection tier, **not** the authority on
visibility, **not** the durable-workflow engine, and does **not** own the reference graph (Phase 3 §1.2).

### 1.3 The C-9 resolution — CONFIRMED frozen

**CONFIRM (recon §5; contract 7.1).** There is exactly **one** cross-subsystem inbox. The Issues
**"My Work"**, Chat **"Activity/Mentions"**, and Git **"Review requests"** surfaces are **scoped, filtered
queries INTO this one inbox — never separate inboxes**. Each is a `filter` over the item's structured
`reason` + `subject` `ArtifactRef`. The Phase-4 ask (ISS named this *blocking*: assigned/blocked/
needs-approval/overdue = reason/subject filters; CHAT Activity/Mentions is a filter not a store) is exactly
this contract — now **frozen**:

| Surface | Is | Implemented as |
|---|---|---|
| **Unified inbox** ("what needs me") | the canonical surface | `list_inbox(principal, filter=∅)` ranked by priority |
| Issues **"My Work"** | a *view* | `list_inbox(principal, filter = subsystem∈{issue} ∧ reason∈{assigned, mentioned, review_requested, sla, watched, blocked, approval_requested})` |
| Chat **"Activity / Mentions"** | a *view* | `list_inbox(principal, filter = subsystem∈{chat} ∧ reason∈{mentioned, replied, thread_watched, approval_requested})` |
| Git **"Review requests"** | a *view* | `list_inbox(principal, filter = subsystem∈{git} ∧ reason∈{review_requested, mentioned})` |

The rule that keeps this true: **a subsystem that wants its own "my X" surface adds a filtered view, never a
second store** — one store → one read-state truth (read it in chat, it's read in the unified inbox), one
priority model, one storm-control budget. *Design-language one-liner (carried to UX): there is one inbox;
everything else is a saved filter on it.* This defeats the exact failure the platform exists to fix: three
inbox-like surfaces fragmenting attention.

### 1.4 Agents have inboxes too

**Unchanged from Phase 3 §1.4.** An agent is a `Principal`; the same inbox + routing model serves an agent's
"things addressed to me." An HITL approval card surfaced to a *human* is a Notif item with
`reason = approval_requested` at high priority (the Agent HITL loop, AG-8). Backend humanisation means an
agent-authored message inherits the human-readable form for free. We do **not** build a parallel
agent-notification system.

---

## 2. The data model / schemas

**Carried forward unchanged from Phase 3 §2** — all tables (`inbox_item`, `notif_pref`, `quiet_hours`,
`delivery`, `oncall_schedule`, `escalation_policy`, `escalation_run`, `humanise_template`, `mute`) are as
specified there; cited not restated. Two pins to the now-frozen platform shapes:

### 2.1 The inbox item — unchanged; the load-bearing invariants restated

The `inbox_item` schema (Phase 3 §2.1) is unchanged. The load-bearing invariants that the reconciliation
depends on:

- **`template_args` holds `ArtifactRef`s, never rendered strings** (NOTIF-1). The human string is produced
  at *read* time by resolving each ref through Refs `resolve(ref, viewer, Display)` — so a renamed PR, a
  retitled issue, or an *erased* author all reflect correctly; a viewer who lost access sees a tombstone, not
  a stale title. This is what makes the **one erasure posture (X-7 / contract 10.9)** apply to Notif "for
  free": most of the inbox needs **no mutation** on erasure because it stores refs, not payloads (§3.9, C7).
- **`origin_event` + `reason`** carry the NOTIF-2 "why it fired" provenance on every item.
- **One read-state store** (the whole point of C-9): the `state` column is the same row across every view.
- **`dedup_key` + `UNIQUE(tenant, recipient, dedup_key)`** make storm-control a *write-time* collapse (an
  `INSERT … ON CONFLICT DO UPDATE`), not a read-time scan (§3.2).

### 2.2 Routing preferences / quiet-hours — the matcher binds the frozen `QueryAst` (C9)

The `notif_pref` / `quiet_hours` schemas are unchanged from Phase 3 §2.2. The one pin: the preference matcher
reuses the **frozen `myelin-query` `QueryAst`** (contract 13.3) — which **is** the `EventMatcher` predicate
core (contract 3.4): a bounded interpreter, no UDFs/loops/recursion, statically cost-bounded,
permission-aware by construction (ADR-07, **not** CEL/JSONLogic). One grammar serves saved views, search
filters, the `EventMatcher`, and notification routing — Notif does not invent a second predicate language
(this is the X-3/OQ-C parity now frozen byte-identical; Phase 3 named it as "the same safe query-AST
predicate core" — that core is now concrete). **Quiet-hours are evaluated in the recipient's tz**, and
**critical/escalated items pierce them by default** (`pierce_classes`) — the one deliberate, explicit
quiet-hours override (you cannot silence an on-call page).

### 2.4 On-call / escalation — the chain config shape is now frozen (C3)

The `oncall_schedule` / `escalation_policy` / `escalation_run` schemas are unchanged from Phase 3 §2.4. The
sharpening (recon §5, contract 7.5): the **escalation-chain config shape is frozen** as the structure Issues
(or any SLA/on-call producer) passes to Notif. A chain is the ordered `escalation_policy.steps` plus the
target resolution + timer, realised on the `myelin-flow` durable timer wheel (contract 9.3) as:

```
page(target, reason)
  → oncall_now(schedule) → principal          // resolve the rotation at fire time
  → notify(principal, channels, class=critical)  // pierces quiet-hours
  → escalate-after-timer(ack_window)           // a myelin-flow durable timer — survives restart
  → if !acked: next step ;  if acked: stop
```

Issues passes the chain definition (its SLA policy); **Notif owns the *policy* evaluation, the
durable-workflow engine owns *durability*** (the timers are `myelin-flow` durable timers, not in-process
sleeps). This is the same minute-bucket timer wheel that serves snooze re-surfacing and SLA timers — one
substrate, three uses (Phase 3 §3.7, contract 9.3).

### 2.5 / 2.6 — humanisation template store + mute — unchanged

`humanise_template` (ICU MessageFormat, platform-defaulted + tenant/locale-overridable) and `mute` are
unchanged from Phase 3 §2.5/§2.6. The templating store's **scope** is now frozen as the platform-wide
single surface (§3.3, C2).

---

## 3. The algorithms

**Carried forward from Phase 3 §3** — the algorithms are unchanged; the three sub-sections below pin the
now-frozen platform shapes they depend on. §3.1 (ranking), §3.2 (storm-control), §3.4 (the router), §3.6
(delivery fabric), §3.8 (reindex-from-source), §3.9 (the holder) are **unchanged**; cited not restated.

### 3.1 Priority / ranking — unchanged (deterministic, explainable v1)

**Unchanged from Phase 3 §3.1.** v1 ranking is a deterministic, explainable scoring function
(`priority ∈ 0..100`), grounded in feed-ranking prior art (EdgeRank affinity×weight×decay; Gmail Priority
Inbox), deliberately deterministic-first because an unpredictable inbox ranking erodes trust faster than no
ranking, and "why am I seeing this, ranked here?" must be answerable (NOTIF-2). The `reason → base → class`
table (`approval_requested`/`escalated`/`sla` = 90/critical; `review_requested`/`assigned`/`mentioned` =
70/direct; `replied`/`agent_proposal` = 55/participating; `watched`/`state_changed` = 35/watching;
team/project `fyi` = 15/fyi) is unchanged. `affinity`/`role_weight` derive from **Id `list_objects`/
relations + Refs backlinks** — Notif asks, it does not own who-relates-to-what. ML-tuned ranking is the named
follow-on behind the same scoring interface (strategy pattern).

### 3.2 Storm-control / DEDUP — unchanged

**Unchanged from Phase 3 §3.2.** The five layered, write-time mechanisms: (1) self-suppression
(`actor.principal == recipient` → drop), (2) dedup-key collapse (`ON CONFLICT DO UPDATE SET coalesce_count
= coalesce_count+1` → "+N more"; two-tier with the Bus's Signal-level dedup), (3) thread/subject coalescing
(digest the participating, break out the direct), (4) rate-of-fire damping (per-(recipient, subject_root)
token bucket), (5) mute/DND honoring. Storm-control suppresses *delivery and ranking*, **never the
audit/history** of the underlying event (the events still exist on the bus; Notif is a projection — EI-04
§5.3).

### 3.3 Backend humanisation — the ONE platform templating surface (C2, frozen)

**Carried forward from Phase 3 §3.3; SHARPENED to the frozen platform scope (recon §OQ-L, contract 7.3).**
The `humanise(item | (template_key, args), viewer, locale) → HumanisedString` render pipeline is unchanged:

```
1. look up humanise_template[ (tenant|default), item.template_key, viewer.locale ]
2. for each ArtifactRef arg in item.template_args:
     proj = refs.resolve(ref, viewer, mode=Display)        // per-VIEWER, permission-checked
     if proj is Tombstone: bind the slot to the tombstone display ("a restricted issue" / "[erased user]")
     else: bind to proj.title (+ proj.icon, + a click-route to the ArtifactRef)
3. ICU-format → the final string + the routable links
```

**The frozen scope (OQ-L resolution).** This `humanise` / ICU-MessageFormat registry is the **ONE** platform
templating surface. There is **no second template engine anywhere**:

- **Knowledge** living-doc templates, **Issues** SLA strings (at-risk/unblocked/approval-requested),
  **CI** status summaries (the `CheckStatus.summary` is a `HumanisedRef` — a `(template_key, args)` pair, not
  a raw string; X-1), and **every agent-authored message + HITL card** register their templates here.
- This is load-bearing for four properties at once (all unchanged from Phase 3 §3.3): **permission-safe by
  construction** (every arg resolved per-viewer → a confidential issue humanises to "Alice updated a
  restricted issue", the title never leaks); **erasure-safe** (an erased actor humanises to `[erased user]`
  with no stored PII to scrub — references-not-payloads); **always-current** (titles resolved at read);
  **agent-inherited** (an agent message is a template render too, same human form, same routable links, zero
  agent-side string work).
- **Markdown** in humanised strings renders through the **one editor render path** (`myelin-content`, now the
  frozen taxonomy with the WASM compile target, X-2/contract 13.1) — never leaked raw. Email gets a
  sanitised-HTML projection; CLI gets plain-text. **One content model, many channel projections** — never
  per-channel string maps.

### 3.4 The router (Signal → per-recipient inbox items) — unchanged

**Unchanged from Phase 3 §3.4.** Notif is a **consumer of Signals** (ADR-19) on the shared consumer template
(whitelisted `sig.<tenant>.>` subjects, durable-bind-by-name, idempotent on `event_id`, ack-after-enqueue,
bounded prefetch, lag exported). The per-Signal loop is unchanged:

```
0. AUTHORIZE: compute candidate recipients; for each, check(recipient, view, signal.subject)
              → drop candidates who can't see it          (ADR-03; never leak — non-negotiable)
1. RECIPIENT RESOLUTION:
     - DIRECT (write-fanout): mention(Principal) nodes, assignee/reviewer relations, escalation targets
     - AMBIENT (read-fanout): watchers / channel members (NOT exploded into writes; materialised on read)
2. per direct recipient: classify reason → score → dedup/storm-control collapse → UPSERT inbox_item
     → channel set ← route(prefs, reason, class) ∩ ¬quiet_hours (unless pierce) → enqueue deliveries
3. emit notif.item.created via the OUTBOX (for web-push live delivery + audit + reindex)
```

Step 0 is non-negotiable: a notification is a *read* of the subject on the recipient's behalf; it obeys
`check` exactly. The router is idempotent on `origin_event` → at-least-once + idempotent ≈ effectively-once.

### 3.5 Write-fanout for mentions vs read-fanout for bodies — the frozen `list_objects` push-down (C1)

**Carried forward from Phase 3 §3.5; SHARPENED to bind the frozen OQ-E shape (recon §OQ-E, contracts
4.3/4.4).** The hybrid fanout architecture is unchanged: **fan-out on WRITE** for the small bounded
high-signal set (mentioned/assigned/reviewer/escalation targets — materialise an `inbox_item` per
recipient), **fan-out on READ** for the large unbounded ambient set (every watcher of a hot PR, every member
of a 50k-person channel — store ONE coalesced marker, materialise per-watcher lazily on inbox open). This is
the Twitter "@-mention write-fanout vs timeline read-fanout" split; a celebrity subject with 50k watchers
costs **zero write amplification**.

**What is now frozen (C1).** The read-fanout watcher resolution — Phase 3's "additive obligation handed to
P4: declare a `watcher` relation per watchable type" — is now the frozen platform mechanism:

- **The `watcher` relation is a frozen ReBAC-fragment obligation** (contract 4.9: "`watcher` relation per
  watchable type"). Every watchable subsystem (Git/CI/Issues/KN/Chat) declares it in its namespace fragment
  (C8). Notif does not invent it; it reads it.
- **Read-fanout watchers are resolved via the frozen `list_subjects(subject_root, watcher, zookie?) →
  SubjectTree`** (contract 4.4), served by the **same per-tenant authz reverse index** that backs the OQ-E
  `list_objects` `SetExpr` push-down (contract 4.3). The reconciliation pins this index as **performant at
  50k-member channel density** (recon §1, the CHAT blocking ask) — the read-fanout half of the fanout
  boundary no longer risks a slow `list_subjects` defeating it. For a watcher *list* scan (a watcher opening
  a filtered inbox over many subjects), Notif uses `list_objects(recipient, watch, type) → Filter{set_expr,
  zookie}` and lowers the `SetExpr` (the `InRelation{relation: watcher, via_column}` / `TupleSet` forms) into
  a SQL JOIN against the `authz_visible` reverse index over Notif's own `inbox_item.subject_root` /
  `subject` column — **one query, no N+1, no post-filter** (the `search-requires-acl-filter` discipline
  generalised to the inbox read).
- **Consistency.** A security-sensitive read passes the `zookie` so it does not use the fail-static cache; a
  just-revoked `watch` grant (a newer zookie from `write_tuples`) is reflected because the JOIN reads the
  reverse index at-or-after the zookie's revision watermark (contract 4.10). An item is **held, not leaked**,
  if a `check` can't resolve fresh (§5.3).

The **mention is the canonical write-fanout producer**: a `mention(Principal)` node — now the frozen inline
structured node in the `myelin-content` taxonomy (X-2, identical across Chat/Issues/Knowledge, C10) — is the
platform-uniform "notify this principal" signal. Notif reads the structured node; it does **not** parse free
text (which is also the agent-loop reference gate, AG-6: only a structured ref re-triggers, never raw text).
The **hot-subject cap** (§3.2.4) bounds even the write-fanout side so a mention-storm can't write-amplify.

### 3.6 The EU-sovereign delivery fabric — unchanged (FLOOR)

**Unchanged from Phase 3 §3.6.** One trait `DeliveryAdapter{channel, region, send(RedactedMessage,
idem_key), receipts}` — EU-preferring, region-aware, swappable (the same strategy-pattern mandate that swaps
mock→real agents, generalised to sub-processors; ADR-12.8). **PII-minimised off-cell payloads**
(`RedactedMessage` = a humanised summary + a deep link, never the full body where avoidable;
`delivery.redacted = true` — GDPR Art. 5(1)(c) data-minimisation). In-app channels (`inbox`, `web_push`,
`desktop`) never leave the cell. **FLOOR (named):** the trait + EU-preferring posture + redaction discipline
are built; the concrete production EU provider (with its DPA) is a sovereignty/legal selection deferred
(§10). v1 dev uses a deterministic mock adapter (`--use-mock`-as-runtime, D6).

### 3.7 On-call / escalation on the durable-workflow substrate — frozen chain (C3)

**Carried forward from Phase 3 §3.7; the chain config shape is now frozen (§2.4, C3, contract 7.5).** An
escalation (an SLA breach Signal, or an agent escalation) starts a **durable workflow** (`myelin-flow`,
contract 9.3/9.4) walking the `escalation_policy` steps. The frozen chain shape (§2.4) is what Issues passes;
the timers are **`myelin-flow` durable timers** that survive a Notif restart and fire effectively-once (the
"wait days for a human signal without holding resources" property — contract 9.4 holds no runtime while
waiting). **Ack is an event** (`notif.escalation.acked` via outbox; the workflow's signal-wait resolves on
it). **On-call cannot be silenced** (`pierce_classes` default `critical`). The same durable-timer substrate
serves snooze re-surfacing and SLA timers — one substrate, three uses.

### 3.8 Reindex-from-source — unchanged

**Unchanged from Phase 3 §3.8 (NOTIF-3 / contract 7.7).** The inbox is a derived read-model, rebuildable
**only** via the live consumer path: `events::reindex(scope=notif)` → owners replay `*.snapshot` events
through outbox→bus→Signal → the **same router** re-ingests idempotently (`origin_event` dedup) →
`inbox_item`/`delivery` reconstructed; cold == live (the D-N3 parity drill). This is the only recovery path
(no "read the inbox from some other store" code → steady-state and recovery share one code path → cannot
drift). It doubles as new-recipient backfill and the schema-upcaster path. Retention floor: a bounded window
(~90 days of items; prefs/on-call/templates permanent); older items age out, reconstructable from the
OLAP/Audit long-term holder — bounding the holder aids GDPR minimisation.

### 3.9 The `PersonalDataHolder` implementation — unchanged; residual by reference (C7)

**Carried forward from Phase 3 §3.9 (contract 7.7).** Notif IS the "notification history" holder
(`locate/export/rectify/restrict/erase`), auto-registered by `serve(AppSpec)` (so "we forgot notification
history" is structurally impossible). Because items store **refs not strings** (§2.1), **erasing a person
tombstones their appearance in everyone's inbox for free** — references-not-payloads (ADR-12.4) does the
work; most of the inbox needs no mutation on erasure.

**The residual, now stated by reference (C7, X-7 / contract 10.9).** The one inline-PII case is a name in an
already-delivered off-cell **redacted** payload (the only place Notif emits free text outside the cell). This
is handled **per the platform erasure posture** (`00-reconciliation §X-7`, contract 10.9): the structural
floor is per-subject DEK crypto-shred of any inline-PII delivery columns + the `restrict` suppression (stop
new routing/delivery for a restricted subject) + a provider-side erasure request for the off-cell payload
(the named sub-processor obligation). Notif does **not** restate the posture; the residual third-party
free-text case is governed where the content lives (the authoring subsystem), not in the inbox. `restrict`
also suppresses indexing/agent-use/analytics/notification for a restricted subject (contract 10.1).

---

## 4. Contracts exposed & consumed

### 4.1 Contracts EXPOSED — STABLE, matching the refined contract index §7

| Contract | Index # | Signature | Status vs Phase 3 |
|---|---|---|---|
| **`list_inbox`** | 7.1 | `list_inbox(principal, filter?, page?) → [InboxItem]` ranked by priority — the ONE inbox; scoped views are `filter`s over `reason`/`subject`, never a second store. | CONFIRMED |
| **item state** | 7.2 | `mark(item_id, state)`; `snooze(item_id, until)`; `mark_all_read(filter)` — one read-state truth across all views. | CONFIRMED |
| **`humanise`** | 7.3 | `humanise(item \| (template_key, args), viewer, locale) → HumanisedString{text, links[], icon}` — resolves each `ArtifactRef` per-viewer via Refs `resolve(Display)`; permission/erasure-safe; ICU. **The ONE platform templating surface** (KN/Issues/CI/agent all register here). | SHARPENED (sole surface frozen, OQ-L) |
| **prefs** | 7.4 | `get_prefs/set_prefs(principal, routing, quiet_hours, digest)` — matcher reuses the frozen `QueryAst` core. | CONFIRMED |
| **on-call** | 7.5 | `oncall_now(schedule) → principal`; `page(target, reason)` — resolves rotation; starts an escalation durable workflow. **Escalation-chain config shape frozen** (`page → oncall_now → escalate-after-timer`). | SHARPENED (chain shape frozen) |
| **`define_notif_rule`** | 7.6 | `define_notif_rule(reason, dedup_tpl, default_class)` — Signal class → inbox reason/priority; each subsystem registers its set. | CONFIRMED |
| **`PersonalDataHolder` + `replay`** | 7.7 | `locate/export/rectify/restrict/erase(subject)`; `replay(scope, since)` — references-not-payloads; erasing a person tombstones their appearance; inbox rebuilt by reindex-from-source. | CONFIRMED |
| **`DeliveryAdapter`** | 7.8 | `{channel, region, send(RedactedMessage, idem_key), receipts}` — region-aware, EU-preferring, swappable; PII-minimised off-cell; at-least-once + idempotent. | CONFIRMED |
| **telemetry** | (1.8) | `inbox_read_latency`, `important_buried_rate`, `dedup_collapse_ratio`, `delivery_success/bounce`, `escalation_ack_latency`, `quiet_hours_pierce_count`, `consumer_lag` — the drill survival signals. | CONFIRMED |

**CLI surface** (unchanged from Phase 3 §4.1): `myelin inbox list|show|read|snooze|watch|prefs` for the
per-user feed; `myelin notify prefs|test`, `myelin oncall show|page` for delivery/on-call config. `inbox
watch` streams new items live over the firehose resume-cursor path (§7, C4).

### 4.2 Contracts CONSUMED — Notif builds on these; it re-invents none

| Consumed contract | Index # | Used for | Change vs Phase 3 |
|---|---|---|---|
| **Signal stream** + `define_signal_rule` | 3.1 | the router consumes curated Signals, never `evt.*`. | unchanged |
| **`EventHandler` consumer template** | 2.4 | the router IS a template consumer (whitelist, idempotent, ack-after, lag). | unchanged |
| **`OutboxTx::emit`** | 2.2 | emit `notif.item.created` / `notif.escalation.acked` — the ONLY emit path. | unchanged |
| **`check`** (+ `CaveatContext`) | 4.2 | step-0 authorize; field-level redaction off the hot path if ever needed. | unchanged (caveat now frozen) |
| **`list_objects` / `list_subjects`** (`SetExpr`) | 4.3 / 4.4 | step-0 candidate filtering; affinity/role; **read-fanout watcher resolution via the authz reverse index, 50k-member-performant**. | **SHARPENED** (frozen push-down, C1) |
| **`resolve(ref, viewer, Display) → Projection\|Tombstone`** + per-subsystem **`project`** | 5.2 / 5.6 | the humanisation render — title/icon/route per-viewer, tombstone on deny. **Cross-cell: resolution is always cell-local** (OQ-I). | SHARPENED (cell-local pinned, C6) |
| **`mention(Principal)` inline node** | 13.1 | the canonical write-fanout "notify this principal" producer. | SHARPENED (frozen node, C10) |
| **durable timers / signals** | 9.3 / 9.4 | escalation chains, snooze re-surfacing, SLA timers. | unchanged |
| **the frozen `QueryAst` core** | 13.3 / 3.4 | the preference matcher — one predicate language, one DoS surface. | SHARPENED (frozen grammar, C9) |
| **firehose `subscribe/resume`** | 3.5 | `inbox watch` live transport (§7). | **SHARPENED** (frozen protocol, C4) |
| **`zookie` consistency** | 4.10 | read-your-writes on watcher revocation; bypass fail-static on security-sensitive reads. | unchanged |
| **`FailStatic`** | 1.10 | the inbox degrades static on an Id hiccup (§5.3). | unchanged |
| **`PersonalDataHolder` auto-reg / KMS** | 1.4 / 11.3 | crypto-shred, holder registration; per-subject DEK for inline-PII delivery columns. | unchanged (DEK by reference to 11.4) |
| **`CrossCellPointer` bridge** | 12.6 | cross-cell inbox aggregation (floor, §5.4). | SHARPENED (frame frozen, C6) |

---

## 5. Scaling / sharding in the cell topology

### 5.1 In-cell, tenant-partitioned, bus-driven — unchanged

**Unchanged from Phase 3 §5.1.** Notif is cell-local and tenant-partitioned (`(tenant, region)` first column
everywhere). All heavy work is async off the bus (the router consumes Signals; no synchronous "notify" call
in any write path). The router is a stateless, horizontally-replicable consumer pool, recoverable by
reconnecting to the durable log + reindex-from-source.

### 5.2 The fan-out scale axis + the agent-mention-storm shed budget (C5)

**Carried forward from Phase 3 §5.2; the shed budget is now a named floor (recon §OQ-K, contract 1.11).** The
dominant scale risk is fan-out amplification (one event → many recipients); the §3.5 hybrid is the structural
answer. On top of it: bounded consumer prefetch, bounded handler pool, **per-tenant in-flight caps** (one
tenant's mention-storm can't starve another's), bounded delivery-adapter concurrency (a bulkhead per
provider), and per-recipient rate damping.

**The named v1 shed-budget floor (OQ-K, the agent-mention-storm row).** The protected-human-lane shed order
(speculative → batch/CI → agent → human-last, ADR-16) is concretised for Notif's storm profile:

| Surface | Storm profile | In-flight cap (per tenant) | Protected-human-lane reservation | Shed order |
|---|---|---|---|---|
| Notif router / agent-mention | agent-mention-storm | per-tenant agent-run in-flight cap (reserve/settle refuses over-cap) | **humans never queue behind agent runs** (separate lane) | the agent-generated notification lane sheds first with `429 + Retry-After` (the agent runtime honours it, ADR-16.3); a human's interactive inbox read is served last-to-shed |

These are **named floors tuned by the drills** (T-5), not claimed-final numbers; the concrete cap is a
Phase-6 budget call, asserted by D-N5 (the 30×-agent-surge drill). The floor is: every lane is bounded, has a
reserved human lane, and applies the shed order — an unbounded lane is the cascade (EI-02 §5).

### 5.3 Fail-static behaviour — unchanged

**Unchanged from Phase 3 §5.3 (contract 1.10).** On an Id hiccup, the inbox **fails static**: `list_inbox`
serves already-materialised items (the inbox store is the truth; ranking is precomputed); humanisation falls
back to cached projections. **New routing that needs a fresh `check` fail-*closes*** — an unsure item is
**held, not leaked** (the EI-02 §10 split: fail-static for availability, fail-closed for authorization). A
security-sensitive read carries the zookie and bypasses the static cache (§3.5).

### 5.4 Cross-cell inbox aggregation — FLOOR; the bridge frame now frozen (C6)

**Carried forward from Phase 3 §5.4; the bridge frame is now frozen (recon §OQ-I, contract 12.6).** A
multi-cell tenant needs a recipient's inbox to aggregate items from every cell they belong to. **This is a
named floor, not built in v1** (single-home-cell is complete). The design seam, now frozen:

- The inbox is materialised **per home-cell**. A multi-cell recipient's unified view aggregates across their
  cells' inboxes via the **frozen PII-free pointer bridge** `CrossCellPointer{subject(opaque), type,
  correlation_id, home_cell}` (contract 12.6) — the control plane carries only the pointer, **never** a
  name/email/body.
- **Resolution is always cell-local** (the frozen OQ-I rule, contract 5.2). To render a pointer to an
  artifact homed in cell B, cell A's gateway (holding the viewer's identity) asks **cell B** to
  `resolve(ref, viewer, Display)` **in B**, permission-checked in B against B's tuples, returning only the
  **already-rendered, already-permission-filtered projection** (or a tombstone) — never raw rows, never PII
  that should stay in B. **Humanisation always resolves locally in the cell that holds the artifact**
  (residency-preserving; no PII crosses cells — ADR-11). The DSR orchestrator iterates `member_cells`
  (contract 10.4) over the same bridge.
- Follow-on owner: Phase-6+ control plane + multi-cell tenancy. The single-cell path is complete; the §4
  contracts are cell-agnostic so this extends without a rewrite.

### 5.5 Stateful-component register — unchanged

**Unchanged from Phase 3 §5.5.** `inbox_item` (tenant-partitioned, rebuildable from source), `delivery`
(idempotent via `idem_key`), `notif_pref`/`quiet_hours`/templates/on-call (system of record, restore-verify
gated), `escalation_run` handles (durability in the `myelin-flow` workflow — no missed page on restart), the
Redis/Valkey cache (NEVER source of truth — cold cache → slower first read, no loss). Everything else (router
pool, delivery workers, the humanise renderer) is stateless and replaceable.

---

## 6. Failure modes + the drills owed (PROVE-IT)

**Carried forward unchanged from Phase 3 §6** — the ten drills are the obligation; each emits a green
artifact when it passes. The only sharpening: **D-N5 now asserts against the named agent-mention-storm shed
budget** (§5.2, C5), and **D-N6** is the per-subsystem instance of the platform erasure posture (X-7).

| # | Property / failure | Drill (gate) | Status |
|---|---|---|---|
| **D-N1** | Important things buried | Replay a mixed week; **0 critical below an fyi; explain-trace present**. | unchanged |
| **D-N2** | Notification storm overwhelms a user | 1000 near-identical CI failures + a 30-comment burst; **N identical → 1 item; 0 self-notifications**. | unchanged |
| **D-N3** | Inbox read-model lost | Wipe `inbox_item`; `reindex(notif)`; **cold == live**. | unchanged |
| **D-N4** | Notification leaks content a recipient can't see | Notify on a confidential subject to a viewer lacking access; **0 title/PII leak; tombstone rendered**. | unchanged |
| **D-N5** | 30×-agent-surge starves the human inbox | 30× agent surge on one tenant; **human inbox-read latency in budget; agent lane sheds; cross-tenant unaffected; bulkhead bounds provider load** — asserted against the §5.2 shed budget. | SHARPENED (budget named) |
| **D-N6** | Erased user still appears in inboxes | Erase a user; **0 recoverable PII; tombstone everywhere; off-cell payload crypto-shredded/erasure-requested** — the X-7 posture instanced for Notif. | unchanged (by-reference) |
| **D-N7** | Escalation missed / double-paged across a restart | Kill Notif mid-`ack_window`; **0 missed, 0 duplicate pages; ack stops the chain**. | unchanged |
| **D-N8** | Quiet-hours over-suppress a page | DND + a `critical` escalation; **critical pierces; non-critical suppressed**. | unchanged |
| **D-N9** | Double delivery | Crash between provider-ack and ledger-write, retry; **exactly-one effective delivery per (item, channel)**. | unchanged |
| **D-N10** | Consumer head-of-line stall | Inject a slow/poison Signal; **lag bounded; no silent stall**. | unchanged |
| **D-N11** (NEW) | `inbox watch` reconnect loses an item | Drop the `inbox watch` connection mid-stream; reconnect with `last_seq`; **backfill `(last_seq, now]` then live — zero items lost**; an over-old cursor → `resync_required` → snapshot rebuild. | **NEW** (the OQ-J resume-cursor drill applied to `inbox watch`, §7) |

---

## 7. The `inbox watch` live transport — the firehose resume-cursor protocol (NEW, C4)

**Phase 3 §9 OQ-8 left the `inbox watch` live transport "[OPEN → P4], co-decided with the Chat connection
tier." It is now closed: `inbox watch` rides the frozen firehose resume-cursor protocol (recon §OQ-J,
contract 3.5)** — co-designed once for huge boards (Issues), hot docs (Knowledge), hot channels (Chat), and
the live inbox. Notif uses it identically:

```
subscribe(stream = fan.<tenant>.inbox.<principal>, scope = inbox:<principal>, cursor?) → SubStream
SubStream yields Frame { seq: u64, item_id, ... }          // seq is per-(stream, scope) monotonic
resume(stream, scope, last_seq) → backfill (last_seq, now] then live    // a reconnect loses zero items
```

- **Resume cursor.** Every live inbox frame carries a per-`(stream, scope)` monotonic `seq`. On reconnect the
  client sends its `last_seq`; the transport backfills `(last_seq, now]` from the bounded firehose retention
  window, then resumes live — **a reconnect loses zero items** (the D-N11 drill is the pass condition). If
  `last_seq` is older than the retention window, the client gets `resync_required` and falls back to a full
  `list_inbox` read (the cold-rebuild path, named not silent).
- **Per-view scope bounding.** The `scope` is a **bounded selector** (`inbox:<principal>`), never `*` — the
  transport rejects an unbounded scope (the whitelist-not-`*` rule, BUS-3, generalised to the firehose). One
  client cannot subscribe to the whole tenant's firehose; it gets only its own inbox slice's frames.
- **Backpressure.** Per-connection in-flight frame caps; a slow consumer is dropped to `resync_required`
  rather than buffering unboundedly (the connection-tier shed budget, OQ-K). The durable bus still carries
  only the pointer event (`notif.item.created`); the firehose carries the live frame — the in-app delivery
  path stays in-cell (§3.6, never egresses).

This unifies `inbox watch` with the Chat connection tier and the KN collab transport on one protocol — there
is no bespoke Notif live transport. The mechanism (long-poll vs SSE vs WebSocket at the wire) is the
connection tier's; Notif consumes the `subscribe/resume/scope` contract above.

---

## 8. Cited prior art

**Carried forward unchanged from Phase 3 §7.** The references that ground the design:

- **Fan-out on write vs read; the celebrity problem.** Silberstein et al., *Feeding Frontier* (VLDB 2010);
  Twitter *Timelines at Scale* (the @-mention write-fanout vs home-timeline read-fanout split — the literal
  basis for §3.5); Facebook **TAO** (Bronson et al., USENIX ATC 2013, the read-optimised social-graph cache
  behind read-fanout). These ground the §3.5 hybrid.
- **Feed ranking.** Facebook **EdgeRank** (affinity × edge-weight × time-decay) — the §3.1 score structure;
  Google **Gmail Priority Inbox** (Aberdeen et al., 2010) — the precedent for an importance model *and* the
  reason we ship deterministic-first (the classifier is the named follow-on, not the floor).
- **Effectively-once delivery + idempotency.** Helland, *Idempotence Is Not a Medical Condition* (ACM Queue
  2012) — at-least-once + idempotent (`idem_key`/`dedup_key` UPSERTs) ≈ effectively-once. Kleppmann, *DDIA*
  (2017) ch. 11; Kreps, *The Log* (2013) — the inbox is a projection, the bus is the source of truth.
- **Storm-control / rate-limiting.** Token-/leaky-bucket (the §3.2.4 damping); Google **SRE** ch. 21/22
  (overload, graceful degradation) — the backpressure posture (§5.2).
- **Resume-cursor / at-least-once streaming.** The Kafka/log consumer-offset model and the firehose
  resume-cursor (EI-04 §2.2 "build the durable resume-cursor transport first") — the §7 `subscribe/resume`
  protocol.
- **Durable escalation / on-call.** PagerDuty's escalation-policy + on-call-rotation model (§2.4);
  Temporal/Cadence durable-execution (durable timers + signals for restart-surviving escalation — §3.7).
- **Humanisation / i18n.** Unicode **ICU MessageFormat** (plurals/gender/locale ordering) — the §2.5 / §3.3
  template format; the "humanise at the source paired with a routable ref" mandate (EI-05 §6).
- **Doctrine.** EI-02 §1/§4/§5/§6/§10; EI-04 §1/§2.2/§5.3; EI-05 §4/§6.

---

## 9. Required changes to foundational systems — all now frozen/closed

Phase 3 §8 listed four "confirmations / small additive obligations" the foundational docs anticipated. In
Phase 5 these are **all frozen and CONFIRMED** — there are **no open foundational changes Notif needs**:

1. **Bus — the curated default Signal set for "what needs me" reasons.** Notif is the primary author of the
   default Signal rule set; `define_signal_rule` + the `mention(Principal) → Signal` mapping exist within the
   Bus taxonomy. The frozen event tokens are registered (contract 2.9). **CONFIRMED — closed.**
2. **Refs — `resolve(Display)` returns the humanisation projection.** Frozen (contract 5.2): per-viewer
   `{title, state, icon, render_hint, sub_anchor?}` or a tombstone; **cross-cell resolution is cell-local**
   (C6). **CONFIRMED — closed.**
3. **Id — read-fanout watcher resolution.** Frozen as `list_subjects(subject, watcher)` / the `list_objects`
   `SetExpr` push-down over the per-tenant authz reverse index, 50k-member-performant (C1; contracts
   4.3/4.4). The **`watcher` relation is a frozen ReBAC-fragment obligation** on every watchable subsystem
   (contract 4.9, C8). **CONFIRMED — closed** (was "additive obligation handed to P4").
4. **Durable-workflow engine — escalation/snooze/SLA timers.** Frozen (contracts 9.3/9.4); the
   escalation-chain shape is frozen (C3). **CONFIRMED — closed.**

The one platform-level obligation Notif introduced (every subsystem declares which events carry a
`mention(Principal)` node or map to a notify-`reason`) is now a frozen checklist item on each subsystem's
ReBAC fragment + event taxonomy (contracts 4.9 / 2.9) — not a contract change.

---

## 10. Open questions for Phase 6

The Phase-3 cross-system seams are resolved; what remains is **product/UX-shaped or sovereignty/legal**, plus
the named floors' build work:

1. **The default Signal/notify-reason rule set + admin authoring UX** — which events are `direct` vs
   `ambient` vs `fyi` by default per subsystem; the Zapier-class rule builder over the frozen `QueryAst`.
   Product/UX-shaped → Phase 6 + design language. (Notif is the author; the *content* of the default set is
   still to be enumerated per subsystem.)
2. **The concrete EU-sovereign delivery providers** (the §3.6 FLOOR's follow-on) — which EU-hosted email/push
   vendor(s), the DPA/sub-processor posture, and the **provider-side erasure** mechanism for an
   already-sent off-cell payload (D-N6's residual, tied to the X-7 posture). **`[OPEN — LEGAL]`** /
   sovereignty → Phase 6 + DPO. *Engineering posture (defensible, flag for counsel/DPO):* the trait +
   redaction discipline + crypto-shred + provider erasure-request **ship now**; the residual third-party
   free-text PII in a delivered payload is governed by the platform erasure posture (contract 10.9), which
   counsel ratifies. We are not counsel.
3. **Cross-cell inbox aggregation build** for multi-cell tenants (the §5.4 FLOOR) — the frozen bridge frame +
   cell-local humanisation, residency-proven. Designed-not-built → Phase 6+ control plane.
4. **ML-tuned ranking promotion** (the §3.1 follow-on) — the measured "important-buried" threshold that
   triggers it; the ranker slots behind the same scoring interface (strategy pattern). Measured-not-predicted
   → Phase 6 promotion trigger.
5. **Digest cadence + batching UX** (§2.2 `digest`) — the daily/weekly digest compose/dedup rules and the
   "snooze to digest" flow. Product/UX → Phase 6 + design language.
6. **Push-token lifecycle + multi-device** (web/mobile/desktop) — device registration, token rotation,
   per-device routing, "delivered/seen on one device → seen everywhere" (the C-9 read-state truth extended to
   devices). → Phase 6 (Notif + the apps).

The `inbox watch` live transport (Phase 3 OQ-8) and the watcher read-fanout / escalation-chain seams (Phase 3
OQ-5/OQ-7) are **closed** by the reconciliation (§7, §3.5, §3.7) and are no longer open.

---

## 11. Cross-references

- Reconciliation spine: [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) (X-7 erasure
  posture; OQ-E watcher push-down; OQ-I cross-cell bridge; OQ-J firehose resume-cursor; OQ-K shed budgets;
  OQ-L sole templating surface; §5 escalation chain); [`contract-index.md`](./contract-index.md) (§7 Notif
  contracts 7.1–7.8; 3.5 firehose; 4.3/4.4 push-down; 12.6 bridge; 13.1/13.3 shared crates).
- Phase-3 base (carried forward): [`../03-shared-systems-architecture/notifications.md`](../03-shared-systems-architecture/notifications.md);
  foundational docs CONSUMED — [`../03-shared-systems-architecture/event-bus.md`](../03-shared-systems-architecture/event-bus.md),
  [`../03-shared-systems-architecture/identity-and-access.md`](../03-shared-systems-architecture/identity-and-access.md),
  [`../03-shared-systems-architecture/reference-graph.md`](../03-shared-systems-architecture/reference-graph.md),
  [`../03-shared-systems-architecture/00-platform-substrate.md`](../03-shared-systems-architecture/00-platform-substrate.md).
- Change requests folded: [`../04-subsystem-architectures/cross-subsystem-change-requests.md`](../04-subsystem-architectures/cross-subsystem-change-requests.md) §5 (Notifications).
- Spine: ADR-03/05/09/11/12/13/16/17/19; directives NOTIF-1/2/3.
- Doctrine: EI-02 §1/§4/§5/§6/§10; EI-04 §1/§2.2/§5.3; EI-05 §4/§6.
- Resolves: **C-9** (the one inbox; "My Work"/"Activity" are scoped views) — CONFIRMED frozen.
