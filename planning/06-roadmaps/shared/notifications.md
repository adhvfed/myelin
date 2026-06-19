# Phase 6 — Roadmap: Notifications (myelin-notif)

> Phase: 06-roadmaps. The detailed, sequenced build roadmap for the **notifications** shared system.
> Slots into the master sequencing bands + critical path: [00-master-sequencing.md](../00-master-sequencing.md).
> Frozen architecture (this roadmap sequences, it does not redesign):
> [notifications.md](../../05-refined-shared-systems-architecture/notifications.md) (the refined Notif doc),
> [contract-index.md](../../05-refined-shared-systems-architecture/contract-index.md) (§7 Notif 7.1–7.8 + the
> consumed contracts), the drill catalogue
> [01-whole-system-e2e-and-drill-catalogue.md](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> (NOTIF-D1..D10 + the E2E rows). Doctrine: VISION §3 (name-your-floors; agent-native; EU-sovereign),
> EI-01 §2/§3/§5 (order-by-non-negotiability, prove-it, the committed ratchet), EI-04 §1/§2/§5 (erasure,
> resume-cursor-transport-first, reindex-from-source). Plain-text identifiers (no backticks-as-emphasis).
> Markdown only; no commits. Date: 2026-06-19.
>
> **What this is.** Notif's milestones mapped onto the master bands, each with its work, its floor-then-full
> progression, its upstream dependencies (what contracts must exist first), and the quantified gates/drills
> that call it done. The hard-problem / world-scale work (fan-out amplification, the cross-cell bridge, the
> 30×-agent-surge shed budget, the firehose resume-cursor live transport, the EU-sovereign delivery fabric)
> is scheduled explicitly with its floor named.

---

## 0. Where Notif sits in the master sequence

Notif is one of the five **reactive shared-layer** systems built in band **M2** (master §2, "The reactive
shared layer (refs, search, notif, agents, workflow)"). It is **not on the critical path** (the spine runs
harness → Identity → agent fabric/workflow/AG-D4 → Git → CI → X-1 seam → dogfood) — but it is a **maximal
consumer**: it depends on nearly every M0/M1/M2 contract and gains its full surface only as the producers
(M3) and consumers (M4) come online and register their notify-reasons. So Notif's own build is front-loaded
into M2, and then it **accretes** through M3/M4 (each subsystem registers its `define_notif_rule` set, its
`watcher` relation, its `mention(Principal)` node) and **hardens** in M5 (the surge family, the cross-cell
bridge follow-on, the E2E wedge rows).

The discipline that makes this honest (master §1, EI-01 §1): Notif gains **zero new contracts** vs Phase 3
(the refined doc, "Changes vs Phase 3"). Every Notif contract was right; the build is consuming frozen
platform shapes, not redesigning. The risk to watch is the inverse-signal (EI-01 closing): if wiring a new
subsystem's notify-reasons gets *harder* each time, the `define_notif_rule` / `watcher`-relation seam is
wrong — stop and repair, don't add surface.

**Honest progression at a glance:**

- **First runnable (M2 mid):** the router consumes a Signal, UPSERTs an `inbox_item`, `list_inbox` returns it
  ranked, `humanise` resolves one ref per-viewer. Mock delivery adapter. No subsystem yet registered real
  reasons — drilled with synthetic Signals.
- **First useful (M2 exit → M3):** the full §3 algorithm set green (storm-control, write/read fanout split,
  escalation on the durable wheel, reindex-from-source, the holder), Notif-leak drill (NOTIF-D4) and the
  outbox/escalation durability drills green, and Git+Knowledge registering real reasons so a human's inbox
  shows real "review-requested"/"mentioned"/"page" items.
- **Production-hardened (M4 accrete → M5):** all five subsystems registered; the 30×-agent-surge shed budget
  proven (NOTIF-D5); the cross-cell bridge follow-on built; the four E2E rows that touch Notif green; restore-
  verify on the system-of-record tables (prefs/on-call/templates) confirmed at cell scale.

---

## 1. Milestones (each mapped to a master band)

### N-M2.0 — Holder + outbox + Signal-consumer skeleton (entry into M2; the data-loss floor inherited)

**Band:** M2 (built on the M0 substrate + the M1 durability/identity floor; this is the very first Notif work,
done before any algorithm).

**Work:**
- Stand up `myelin-notif` as a `serve(AppSpec)` service: three ports (public/internal/metrics-health),
  liveness ≠ readiness, graceful drain, forward-only migrations (contracts 1.1–1.3, 1.5 — inherited from M0).
- The data model (refined §2): `inbox_item`, `notif_pref`, `quiet_hours`, `delivery`, `oncall_schedule`,
  `escalation_policy`, `escalation_run`, `humanise_template`, `mute` — all `(tenant, region)`-partitioned
  first column (residency-pin lint, M1). `inbox_item` stores `template_args` as `ArtifactRef`s never rendered
  strings; `UNIQUE(tenant, recipient, dedup_key)` for write-time collapse; one `state` column = the one
  read-state truth.
- The router as an `EventHandler` consumer of **Signals** (`sig.<tenant>.>` whitelist, never `*`; contracts
  2.4/3.1, ADR-19), idempotent on `origin_event`/`event_id`, ack-after-enqueue, bounded prefetch, lag
  exported (telemetry contract 1.8).
- Emit `notif.item.created` / `notif.escalation.acked` **only** via `OutboxTx::emit` (contract 2.2 — the
  `no-raw-publish` lint forbids any other path).
- Register Notif as a `PersonalDataHolder` (notification history) via the harness auto-registration (contract
  1.4/10.1) — "we forgot notification history" is structurally impossible. References-not-payloads from day 1.

**Floor:** the holder's `erase` is wired structurally (references-not-payloads ⇒ tombstone-for-free) but its
**off-cell-payload residual** is by-reference to the platform posture (X-7 / contract 10.9) — see N-M5.2.

**Upstream dependencies (must be green before this starts):**
- M0: `serve(AppSpec)`, the transactional outbox (2.2/2.3) + idempotent-consumer template (2.4/2.5), the
  twelve lints (esp. `no-raw-publish`, `tenant-predicate`, `residency-pin`, `no-untagged-personal-data`), the
  failure-injection harness, the `EventEnvelope` (2.1). **Gate inherited:** SUB-D1/SUB-D2/BUS-D4 green.
- M1: `(tenant, region)` partition key + `residency_verify` (12.1/12.4); the OLTP store + RLS + the outbox
  table (11.1); the `PersonalDataHolder` trait + auto-registration + KMS per-subject DEK (10.1, 1.4, 11.3/11.4);
  restore-verify CI job wired (11.5). **Gate inherited:** STOR-D1/STOR-D2, ID-D3, CP-D2/CP-D3 green.

**Gate to call this milestone done:**
- **NOTIF-D10** (CI): inject a slow/poison Signal type → the whitelisted-template router does not stall, the
  poison terminates, the lag alarm fires. (No silent head-of-line stall; lag bounded.)
- The Notif rows pass the **contract-coverage scanner** (provider + consumer CDC for 7.1–7.8) — an uncommitted
  contract test is no contract test (EI-01 §5).
- A harness self-test: inject a Signal, assert `inbox_item` UPSERTed and the telemetry assertion reads
  `consumer_lag` and `dedup_collapse_ratio`. (Observability is part of the pass condition, EI-01 §3.)

### N-M2.1 — The read surface + humanisation + ranking (first runnable → first useful)

**Band:** M2.

**Work:**
- `list_inbox(principal, filter?, page?) → [InboxItem]` ranked by priority (contract 7.1) — the **ONE inbox**.
  The C-9 resolution: Issues "My Work", Chat "Activity/Mentions", Git "Review requests" are `filter`s over
  `reason`/`subject`, **never** a second store. Implement the filter grammar so a subsystem adds a saved view,
  never a store.
- `mark/snooze/mark_all_read` (contract 7.2) — one read-state truth across all views (read it in chat → read
  in the unified inbox).
- `humanise(item | (template_key, args), viewer, locale) → HumanisedString` (contract 7.3) — **the ONE
  platform templating surface.** The render pipeline resolves each `ArtifactRef` per-viewer via Refs
  `resolve(ref, viewer, Display)`; a tombstone on deny ("a restricted issue" / "[erased user]"); ICU
  MessageFormat; markdown through the one `myelin-content` WASM render path, never raw. Email →
  sanitised-HTML projection; CLI → plain text (one content model, many channel projections).
- v1 ranking (refined §3.1): the deterministic, explainable scoring function (`priority ∈ 0..100`); the
  `reason → base → class` table (approval/escalated/sla = 90/critical … fyi = 15); `affinity`/`role_weight`
  derived from Id `list_objects`/relations + Refs backlinks behind a strategy interface. Every rank carries
  an explain-trace (NOTIF-2). **Floor:** ML-tuned ranking is the named follow-on behind the same scoring
  interface (promotion trigger = a measured "important-buried" signal, NOTIF-D1).
- `get_prefs/set_prefs` (contract 7.4) — routing + quiet-hours; the matcher binds the **frozen
  `myelin-query` `QueryAst`** core (= the `EventMatcher`, contract 3.4/13.3); quiet-hours in the recipient's
  tz; `critical/escalated` pierces by default (`pierce_classes`).
- `define_notif_rule(reason, dedup_tpl, default_class)` (contract 7.6) — the registration seam each subsystem
  will call in M3/M4. Ship with the **Notif-owned default Signal/reason set** stubbed (the content of the
  default set is the M3/M4 per-subsystem enumeration, refined §10 OQ1).
- CLI: `myelin inbox list|show|read|snooze|prefs`.

**Floor:** the delivery side is a **deterministic mock adapter** (`--use-mock`-as-runtime, refined §3.6) — the
EU-sovereign real provider is N-M5.3. Ranking is deterministic-v1 (ML follow-on named above).

**This is "first runnable":** a Signal in → a ranked, humanised inbox item out, per-viewer-safe.

**Upstream dependencies:**
- Identity `list_objects`/`list_subjects` `SetExpr` push-down + `check` + zookie (4.2/4.3/4.4/4.10) — for
  step-0 authorize, affinity, and the read-fanout (next milestone); built in M1.
- Refs `resolve(ref, viewer, Display)` + `project(ref, viewer)` (5.2/5.6) — the humanisation projection;
  built in M2 (Refs ships before/with Notif in the band — see §4 ordering).
- The frozen `myelin-content` taxonomy + WASM render target (13.1) and the `myelin-query` `QueryAst`
  (13.3) — both frozen in M2.

**Gate to call this done:**
- **NOTIF-D1** (SCHED): replay a mixed week → every `critical`/`direct` ranks above every `fyi`;
  first-important latency within budget; an explain-trace per rank. (`important-buried-rate = 0`.)
- **NOTIF-D4** (CI): notify on a confidential issue / private channel to a viewer lacking access → humanised
  **tombstone**, the title **never** appears; item suppressed if the recipient can't see the subject.
  (`0 title/PII leak` — this is the F1 leak floor; the humanise-per-viewer property proven before any real
  subsystem subject flows.)

### N-M2.2 — Storm-control + the write/read fanout split (the scale-axis floor)

**Band:** M2.

**Work:**
- The five write-time storm-control mechanisms (refined §3.2): (1) self-suppression
  (`actor == recipient` → drop), (2) dedup-key collapse (`ON CONFLICT DO UPDATE SET coalesce_count+1` → "+N
  more"), (3) thread/subject coalescing, (4) per-`(recipient, subject_root)` token-bucket rate damping, (5)
  mute/DND honoring. Storm-control suppresses delivery+ranking, **never** the audit/history (Notif is a
  projection — EI-04 §5.3).
- The hybrid fanout (refined §3.5) — the load-bearing scale answer:
  - **Write-fanout** for the bounded high-signal set (mentioned/assigned/reviewer/escalation targets): the
    `mention(Principal)` node — the **frozen inline structured node** in the `myelin-content` taxonomy (X-2),
    identical across Chat/Issues/Knowledge — materialises one `inbox_item` per recipient. Notif reads the
    structured node; it **does not parse free text** (the agent-loop reference gate, AG-6 — only a structured
    ref re-triggers).
  - **Read-fanout** for the unbounded ambient set (every watcher of a hot PR, every member of a 50k-channel):
    store ONE coalesced marker, materialise per-watcher lazily on inbox open. A 50k-watcher celebrity subject
    costs **zero write amplification** (the Twitter @-mention-vs-timeline split).
  - Watcher resolution via the **frozen `list_subjects(subject_root, watcher, zookie?)` / `list_objects(…
    watch …) → Filter{set_expr, zookie}`** push-down (contracts 4.3/4.4) — lower the `SetExpr` into a SQL JOIN
    against the `authz_visible` reverse index over Notif's own `subject_root`/`subject` column: one query, no
    N+1, no post-filter (the `search-requires-acl-filter` discipline generalised to the inbox read). The
    reverse index is pinned performant at **50k-member channel density** (contract 4.4).
- The hot-subject cap (§3.2.4) bounds even the write-fanout side so a mention-storm can't write-amplify.

**Floor:** the read-fanout depends on every watchable subsystem declaring its `watcher` ReBAC fragment
(contract 4.9, C8) — those fragments land **with their subsystems** in M3/M4. Until then Notif's read-fanout
is drilled against synthetic `watcher` tuples; it goes fully live as each subsystem registers.

**Upstream dependencies:** Identity `list_subjects`/`list_objects` `SetExpr` + the authz reverse index + zookie
(4.3/4.4/4.10, M1); the frozen `mention(Principal)` node (13.1, M2); the Bus Signal-level dedup (2.x, M0).

**Gate to call this done:**
- **NOTIF-D2** (CI): 1000 near-identical CI failures + a 30-comment PR burst → bounded items (`coalesce_count`
  correct); self-notifications suppressed. (`dedup-collapse-ratio`; `0 self`.)

### N-M2.3 — Escalation on the durable wheel + the live transport + delivery idempotency (M2 exit; "first useful")

**Band:** M2 (these are the remaining M2-exit Notif drills).

**Work:**
- On-call / escalation (contract 7.5) on the `myelin-flow` durable substrate: `oncall_now(schedule) →
  principal`, `page(target, reason)` starts an escalation **durable workflow** (contracts 9.1/9.3/9.4). The
  **frozen escalation-chain shape** (C3): `page → oncall_now → notify(class=critical, pierces quiet-hours) →
  escalate-after-timer(ack_window) → if !acked next-step / if acked stop`. Issues passes the chain
  definition; **Notif owns policy evaluation, the workflow engine owns durability** (timers are `myelin-flow`
  durable timers, not in-process sleeps — survive a Notif restart, fire effectively-once). Ack is an event
  (`notif.escalation.acked` via outbox; the workflow's signal-wait resolves on it). On-call cannot be silenced
  (`pierce_classes` default `critical`).
- The `inbox watch` live transport (refined §7) — the **frozen firehose resume-cursor protocol** (OQ-J,
  contract 3.5), the same transport Chat/Knowledge/Issues use (built once, in M2):
  `subscribe(stream=fan.<tenant>.inbox.<principal>, scope=inbox:<principal>, cursor?)` →
  `resume(stream, scope, last_seq)` backfills `(last_seq, now]` then live; per-`(stream, scope)` monotonic
  `seq`; an over-old cursor → `resync_required` → full `list_inbox` (cold rebuild, named not silent);
  per-view scope bounded (never `*`, BUS-3 generalised); per-connection in-flight caps (the connection-tier
  shed budget). **EI-04 §2: the durable resume-cursor transport is built first** — Notif consumes it, it does
  not build a bespoke live path.
- The delivery fabric (contract 7.8): the one `DeliveryAdapter{channel, region, send(RedactedMessage,
  idem_key), receipts}` trait — EU-preferring, region-aware, swappable; `RedactedMessage` = humanised summary
  + deep link (Art. 5(1)(c) data-minimisation); `delivery.redacted = true` for off-cell. In-app channels
  (`inbox`/`web_push`/`desktop`) never leave the cell. At-least-once + idempotent on `UNIQUE(idem_key)`.
- Reindex-from-source (contract 7.7): `events::reindex(scope=notif)` → owners replay `*.snapshot` →
  same router re-ingests idempotently → `inbox_item`/`delivery` reconstructed; cold == live. **The only
  recovery path** (no second read path → cannot drift). Doubles as new-recipient backfill + schema-upcaster.
  Retention floor: ~90-day item window; prefs/on-call/templates permanent (restore-verify gated).

**Floor:** the delivery adapter is still the mock (`--use-mock`); the real EU provider is N-M5.3. Cross-cell
inbox aggregation is **designed-not-built** (single-home-cell complete) — the bridge frame is frozen, the
build is N-M5.1.

**This is "first useful":** durable paging, gap-free live inbox, exactly-once delivery, rebuildable inbox.

**Upstream dependencies:** `myelin-flow` `DurableExecutor` + timer wheel + durable signal (9.1/9.3/9.4, M2);
the firehose `subscribe/resume` transport (3.5, M2); reindex-from-source (2.6, M0/M2); the outbox (2.2, M0).

**Gate to call this done — and to clear the M2 exit for Notif:**
- **NOTIF-D7** (CI): start escalation; kill Notif mid-`ack_window` → the durable workflow resumes, pages the
  next step **exactly once**; an ack stops the chain. (`exactly-once page`; `ack-halt`.)
- **NOTIF-D8** (CI): set DND; fire a `critical` escalation → it **pierces** quiet-hours; a `watching` item is
  suppressed. (`critical pierces`; `non-crit suppressed`.)
- **NOTIF-D9** (CI): crash between provider-ack and ledger-write, retry → `UNIQUE(idem_key)` collapses to
  exactly-one delivery per `(item, channel)`. (`1 effective delivery`.)
- **NOTIF-D3** (SCHED): wipe `inbox_item`, `reindex(notif)` → the rebuilt inbox matches live (items +
  read-state from source events). (`reindex-parity hash`.)
- The `inbox watch` resume leg of the OQ-J family: drop the connection mid-stream, reconnect with `last_seq` →
  backfill `(last_seq, now]` then live, **zero items lost**; over-old cursor → `resync_required`. (The refined
  doc's D-N11; in the catalogue this rolls up under the OQ-J resume-cursor family proven once for the shared
  transport — Notif's leg asserts it for `scope=inbox:<principal>`.)

**M2 exit gate context (master §2):** Notif's M2-exit obligations are **NOTIF-D4 / NOTIF-D7** (named in the
master M2 exit gate). Both must be green to clear the band. Note the hard band-gate is **AG-D4** (sandbox
escape, the agent fabric's) — Notif does not own it but cannot be "done in M2" over a red M2 gate (the gate
invariant, EI-01 §2): the band closes as a whole.

### N-M3 — Producer accretion: Git + Knowledge register their reasons + watchers

**Band:** M3 (Notif gains no new contracts; it accretes real reasons as the producers come online).

**Work:**
- Git registers its `define_notif_rule` set (review_requested/mentioned) + its `watcher` ReBAC fragment
  (contract 4.9) → Git "Review requests" becomes a real `list_inbox` filtered view; read-fanout over real PR
  watchers goes live.
- Knowledge registers its set (mentions/comments/shares/watched) + its `watcher` fragment → KN mentions/
  comments flow; the agent-trace-adjacent reasons land.
- Verify the humanise-per-viewer property holds against **real** confidential subjects (Git private repo, KN
  confidential page) — re-run NOTIF-D4 with real subjects, not synthetic.

**Floor:** Issues/Chat reasons + watchers are M4; cross-cell is still single-home.

**Upstream dependencies:** Git (M3) + Knowledge (M3) shipping their ReBAC fragments + event taxonomy +
`project(ref, viewer)`. Notif itself is unchanged — this is pure registration.

**Gate:** the relevant Git/KN exit-gate rows that touch Notif (GIT/KN confidential-leak drills assert the
unfurl/humanise tombstone, e.g. KN-D5/D13, GIT-D8 — Notif's `resolve(Display)` path is the leak surface they
exercise). No new Notif drill; NOTIF-D4 re-confirmed against real subjects.

### N-M4 — Consumer accretion: Issues SLA/escalation + Chat activity/mentions + explicit-first agents

**Band:** M4.

**Work:**
- Issues registers its reasons (assigned/blocked/needs-approval/overdue/sla/unblocked) + the **escalation
  chain definition** it passes to Notif (contract 7.5, the frozen chain shape) + its `watcher` fragment →
  Issues "My Work" becomes a real filtered view; SLA breaches start real escalation chains on the durable
  wheel.
- Chat registers its reasons (mentioned/replied/thread_watched/approval_requested) + its `watcher` fragment →
  Chat "Activity/Mentions" becomes a real filtered view (a filter, not a store — the C-9 invariant). The
  **explicit-first agent dispatch** boundary: a casual `@agent` mention posts a Notif item to the agent's
  inbox (`reason=mentioned`) but **does not** spawn a costed run (CHAT-D17 — Notif is the notify side of that
  boundary).
- HITL approval cards: an agent HITL approval surfaced to a human is a Notif item with
  `reason=approval_requested` at high priority (refined §1.4); the card humanises via the one templating
  surface (action + risk + cost). Agents have inboxes too — the same model, no parallel system.
- CI registers its status-summary reasons; the `CheckStatus.summary` is a `HumanisedRef` = a
  `(template_key, args)` pair that resolves through `humanise` (X-1) — CI registers its templates on the one
  surface, never a raw string.

**Upstream dependencies:** Issues (M4), Chat (M4), CI (M4) shipping their reason sets + chain definitions +
`watcher` fragments + the `HumanisedRef` `CheckStatus.summary` (X-1, 5.9). `myelin-flow` durable signal for
the multi-day HITL wait (9.4).

**Gate:** the relevant M4 exit rows that touch Notif:
- **CHAT-D5** (CI): notify/unfurl a confidential artifact to a viewer lacking access → tombstone, title never
  present (Notif's `humanise` leak surface — re-confirms NOTIF-D4 at the Chat seam).
- **CHAT-D17** (CI): casual `@agent` → notifies the agent's inbox, does **not** auto-spawn a costed run.
- **ISS-D6** (CI): SLA breach starts the escalation chain (Notif's chain-start integration with Issues).

### N-M5.1 — Cross-cell inbox aggregation (the named multi-cell floor's follow-on)

**Band:** M5 (the floor follow-on; master §5 "Multi-cell, after single-cell").

**Work:** a multi-cell recipient's unified inbox aggregates across every cell they belong to via the **frozen
PII-free pointer bridge** `CrossCellPointer{subject(opaque), type, correlation_id, home_cell}` (contract 12.6)
— the control plane carries only the pointer, **never** name/email/body. **Resolution is always cell-local**
(the frozen OQ-I rule, contract 5.2): to render a pointer to an artifact homed in cell B, cell A's gateway
asks **cell B** to `resolve(ref, viewer, Display)` in B (permission-checked in B against B's tuples),
returning only the already-rendered, already-permission-filtered projection (or a tombstone) — never raw rows,
never PII that should stay in B. The DSR orchestrator iterates `member_cells` (contract 10.4) over the same
bridge.

**Floor → full:** single-home-cell (the v1 floor, complete since M2.0) → multi-cell aggregation (this). The §4
contracts were written cell-agnostic so this extends without a rewrite.

**Upstream dependencies:** the M5 multi-cell control plane + the `CrossCellPointer` bridge going live (12.6);
the FLOOR drills GA-D8/CP-D7/CP-D8 (master §5) now owed.

**Gate:** the cross-cell legs of GA-D8/CP-D7/CP-D8 (SCHED) for the inbox-aggregation path: a cross-cell inbox
view resolves cell-locally with **0 PII crossing cells**; cell→cell migration loses 0 inbox items.

### N-M5.2 — The 30×-agent-surge shed budget + the EU-sovereign delivery follow-on + the erasure residual

**Band:** M5 (world-scale hardening + the floor follow-ons).

**Work:**
- **The 30×-agent-surge shed budget** (the world-scale hard problem for Notif, master §5 surge family). The
  protected-human-lane shed order (speculative → batch/CI → agent → human-last, ADR-16) concretised for the
  **agent-mention-storm** profile: a per-tenant agent-run in-flight cap (reserve/settle refuses over-cap);
  **humans never queue behind agent runs** (a separate lane); the agent-generated notification lane sheds
  first with `429 + Retry-After` (the agent runtime honours it, ADR-16.3); a human's interactive inbox read is
  last-to-shed. Plus: bounded consumer prefetch, bounded handler pool, per-tenant in-flight caps (one tenant's
  storm can't starve another's), a delivery-adapter bulkhead per provider, per-recipient rate damping. **These
  are named floors tuned by the drill (T-5), not claimed-final numbers** — the concrete cap is the budget call
  D-N5 asserts against.
- **EU-sovereign delivery — the floor's follow-on** (refined §3.6/§10): the trait + EU-preferring posture +
  `RedactedMessage` minimisation discipline ship as a floor in M2.3 (mock adapter); the **concrete production
  EU email/push provider** (with its DPA/sub-processor posture) is the sovereignty/legal selection deferred to
  here. `[OPEN — LEGAL]` — the engineering posture (trait + redaction + crypto-shred + provider-erasure-request)
  ships; counsel/DPO ratifies the provider + the residual statement. We are not counsel.
- **The erasure residual, instanced** (X-7 / contract 10.9): the one inline-PII case is a name in an
  already-delivered off-cell **redacted** payload. Structural floor (built since M2.0): per-subject-DEK
  crypto-shred of any inline-PII delivery columns + `restrict` suppression (stop new routing/delivery for a
  restricted subject) + a provider-side erasure-request for the off-cell payload (the named sub-processor
  obligation). Notif does **not** restate the platform posture; the residual is governed by reference.

**Upstream dependencies:** the reserve/settle wallet (11.7, M1) gating agent runs; the agent runtime honouring
`429 + Retry-After` (ADR-16.3, M2); the chosen EU provider + DPA (legal, parallel); per-subject DEK (11.4, M1).

**Gate:**
- **NOTIF-D5** (SCHED): 30× agent-generated notification surge on one tenant → the human inbox-read lane holds
  within budget; the agent lane sheds; cross-tenant unaffected; the delivery-adapter bulkhead bounds provider
  load. (`shed-counts`; `delivery-success` — asserted against the §5.2 named shed budget. Part of the master
  M5 F6 surge family, listed as NOTIF-D5.)
- **NOTIF-D6** (SCHED): erase a user → every inbox item humanises to `[erased user]`; **0 recoverable PII**;
  the off-cell-sent payload crypto-shredded / erasure-requested. (`erase-receipt`; `0 recoverable` — the X-7
  posture instanced for Notif.)

### N-M5.3 — The whole-system E2E wedge (Notif's legs)

**Band:** M5 (the four chained-mutation E2E scenarios; master §2/§5).

**Work:** Notif participates in the whole-system E2E rows (drill catalogue §E2E). Its legs:
- **E2E-1 PR context pane:** `humanise` resolves the pane's notification/status strings per-viewer; **0 leak**
  to the unauthorized viewer; the checks panel live-updates via the firehose (the shared per-ref cache busts).
- **E2E-2 CI-fail → triage agent → issue → chat → fix-PR** (the flagship): the HITL approval card is a Notif
  item (`reason=approval_requested`) showing action + risk + cost; the agent's inbox notification on the
  casual mention does not spawn a run; the escalation/notify legs are exactly-once across a kill.
- **E2E-4 DSAR fan-out:** Notif is one of the H1–H18 holders; `locate`→`erase` over notification history
  contributes its receipt; post-erase `locate = 0` recoverable PII; inbox items show `[erased user]`.

**Upstream dependencies:** all five subsystems live (M4); the cross-cell bridge (N-M5.1) for the multi-cell
DSAR leg; the surge/erasure drills (N-M5.2) green.

**Gate:** the E2E rows green with their named green artifacts (E2E-1 pane-resolution trace + zero-leak = 0;
E2E-2 HITL withhold→approve→apply ledger; E2E-4 H1–H18 coverage receipt set including Notif + post-erase
`locate` = 0). Plus **STOR-D2 at cell scale** re-confirmed for Notif's system-of-record tables
(prefs/on-call/templates — RPO/RTO under world-scale load).

---

## 2. Floors and their scheduled follow-ons (name-your-floors, VISION §3 / EI-04 §4)

| Floor (shipped) | Ship band | Follow-on (the full answer) | Follow-on band | Trigger |
|---|---|---|---|---|
| Deterministic mock `DeliveryAdapter` (`--use-mock`) | N-M2.1/M2.3 | Concrete EU-sovereign provider(s) + DPA (real send) | N-M5.2 | sovereignty/legal selection (`[OPEN — LEGAL]`); trait + redaction ship now |
| Single-home-cell inbox (one cell per tenant) | N-M2.0 | Cross-cell inbox aggregation via the frozen bridge frame | N-M5.1 | multi-cell rollup/cross-org demand (OQ-I); FLOOR drills GA-D8/CP-D7/CP-D8 owed |
| Deterministic + explainable v1 ranking | N-M2.1 | ML-tuned ranking (behind the same scoring interface) | post-M5 (measured) | a measured "important-buried" signal (NOTIF-D1), not predicted |
| Synthetic-`watcher`-tuple read-fanout | N-M2.2 | Real per-subsystem `watcher` ReBAC fragments | N-M3/N-M4 | each watchable subsystem ships its fragment (contract 4.9, C8) |
| Stubbed default Signal/notify-reason set | N-M2.1 | Per-subsystem enumerated reason sets | N-M3/N-M4 | each subsystem registers via `define_notif_rule` (7.6) |
| Erasure residual handled by-reference (structural crypto-shred floor built) | N-M2.0 | Provider-side erasure for off-cell payloads + counsel ratification of the residual statement (10.9) | N-M5.2 (parallel legal) | a body delivered off-cell must be expunged; the structural floor ships regardless |

The honest-floor rule binds all of these (EI-04 §4): each is tracked in the gap report with its claimed/proven
status and its linked follow-on; the gap being *invisible* is the only failure.

---

## 3. Contracts this system implements, by milestone

From [contract-index.md](../../05-refined-shared-systems-architecture/contract-index.md) §7 (Notif gains **zero
new contracts** vs Phase 3 — all CONFIRMED or SHARPENED; the build consumes frozen platform shapes).

| Contract | # | Implemented by milestone | Notes |
|---|---|---|---|
| `list_inbox` (the ONE inbox; views are filters) | 7.1 | N-M2.1 | C-9 resolution |
| `mark/snooze/mark_all_read` (one read-state truth) | 7.2 | N-M2.1 | |
| `humanise` (the ONE templating surface) | 7.3 | N-M2.1 | SHARPENED (sole surface, OQ-L); CI registers `HumanisedRef` summaries in N-M4 |
| `get_prefs/set_prefs` (matcher = frozen `QueryAst`) | 7.4 | N-M2.1 | |
| `oncall_now`/`page` (escalation durable workflow; chain shape frozen) | 7.5 | N-M2.3 | SHARPENED; Issues passes the chain in N-M4 |
| `define_notif_rule` (Signal class → reason/priority) | 7.6 | N-M2.1 (seam) → N-M3/N-M4 (registrations) | per-subsystem reason sets accrete |
| `PersonalDataHolder` + `replay` (references-not-payloads) | 7.7 | N-M2.0 (holder) → N-M2.3 (reindex `replay`) | erasure residual instanced N-M5.2 |
| `DeliveryAdapter` (region-aware, EU-preferring, swappable) | 7.8 | N-M2.3 (trait + mock) → N-M5.2 (real EU provider) | floor → full |
| telemetry survival signals (1.8) | 1.8 | N-M2.0 | inbox_read_latency, important_buried_rate, dedup_collapse_ratio, delivery_success/bounce, escalation_ack_latency, quiet_hours_pierce_count, consumer_lag |

Notif also **consumes** (re-invents none, refined §4.2): Signals + `define_signal_rule` (3.1), the
`EventHandler` template (2.4), `OutboxTx::emit` (2.2), `check`+`CaveatContext` (4.2), `list_objects`/
`list_subjects` `SetExpr` (4.3/4.4), `resolve(Display)`+`project` (5.2/5.6), the `mention(Principal)` node
(13.1), durable timers/signals (9.3/9.4), the `QueryAst` core (13.3/3.4), the firehose `subscribe/resume`
(3.5), `zookie` (4.10), `FailStatic` (1.10), the holder auto-reg/KMS (1.4/11.3), the `CrossCellPointer` bridge
(12.6).

---

## 4. Upstream dependencies (what must exist first) + intra-M2 ordering

**Critical upstream dependencies — Notif cannot start a milestone until these are green:**

- **From M0 (substrate):** `serve(AppSpec)` + three ports; the transactional outbox + idempotent-consumer
  template (the data-loss floor Notif inherits — it never invents an emit path); the twelve lints; the
  failure-injection harness; the `EventEnvelope` + `ArtifactRef` token table; reindex-from-source (2.6). Gate:
  SUB-D1/SUB-D2/BUS-D4 + all twelve lints green.
- **From M1 (the dependency root + durability + partition):** Identity `check`/`list_objects`/`list_subjects`
  `SetExpr` push-down + zookie + `FailStatic` bound — **the highest-fan-in dependency**, every step-0
  authorize and the entire read-fanout rests on it; the `(tenant, region)` partition + residency-pin; the
  OLTP store + the outbox table; the `PersonalDataHolder` trait + auto-reg + KMS per-subject DEK; the
  restore-verify CI job; the reserve/settle wallet (for the N-M5.2 surge budget). Gate: ID-D3, ID-D2, ID-D1,
  CP-D2/CP-D3, STOR-D1/STOR-D2 green (the silent-data-loss floor is below Notif — Notif does not write real
  prefs/on-call rows over a red STOR-D1).
- **Within M2 (the reactive layer — ordering inside the band):** Notif depends on three siblings built **in
  the same band**, so the per-system roadmaps must order them ahead of (or co-built with) Notif's consuming
  milestones:
  1. **Refs** `resolve(ref, viewer, Display)` + `project` (5.2/5.6) — required by `humanise` (N-M2.1). Refs
     ships its M2 leak drills (REF-D1/D2) before Notif's humanise leak drill (NOTIF-D4) can be honest.
  2. **The Bus Signals tier** + `define_signal_rule` + the firehose `subscribe/resume` transport (3.1/3.5) —
     required by the router (N-M2.0) and `inbox watch` (N-M2.3). The resume-cursor transport is built **first**
     (EI-04 §2) and Notif consumes it.
  3. **Durable workflow** `DurableExecutor` + timer wheel + durable signal (9.1/9.3/9.4) — required by
     escalation (N-M2.3). FLOW-D1/D2/D5 green before NOTIF-D7 can rest on durable timers.
  4. **The frozen shared crates** `myelin-content` (13.1, the `mention` node + the WASM render target) and
     `myelin-query` (13.3, the `QueryAst`) — required by fanout (N-M2.2) and prefs (N-M2.1). Frozen in M2.
- **From M3/M4 (accretion):** Git/Knowledge/Issues/Chat/CI shipping their `define_notif_rule` sets, `watcher`
  ReBAC fragments (4.9), `mention(Principal)` producers, and (CI) the `HumanisedRef` `CheckStatus.summary`
  (X-1). Notif gains its real reasons only as these land — hence the N-M3/N-M4 accretion milestones.
- **From M5:** the multi-cell control plane + `CrossCellPointer` bridge (12.6) for cross-cell aggregation; the
  reserve/settle wallet + the agent runtime's `429`-honouring for the surge budget; the chosen EU provider +
  DPA (legal) for real delivery.

**Notif is not on the critical path** (master §3.1) but is a **maximal consumer**: its full surface trails the
producers. The acyclicity rule holds (master §3.2, `no-cross-sync-cycle`): Notif is a pure Signal consumer +
projection — it never synchronously calls a producer; every input is an async Signal/event, every cross-cell
hop is the cell-local-resolution pointer bridge.

---

## 5. Digest

**Milestones (each → master band):**
- **N-M2.0 (M2)** — holder + outbox + Signal-consumer skeleton; the data model; references-not-payloads holder
  auto-registered. Gate: NOTIF-D10 (no poison stall) + contract-coverage scanner + harness self-test.
- **N-M2.1 (M2)** — "first runnable": `list_inbox` (the ONE inbox), `humanise` (the ONE templating surface,
  per-viewer-safe), deterministic ranking, prefs/quiet-hours, the `define_notif_rule` seam. Gate: NOTIF-D1
  (ranking) + **NOTIF-D4 (0 title/PII leak)**.
- **N-M2.2 (M2)** — storm-control + the write/read fanout split (50k-watcher = zero write amplification, via
  the frozen `list_subjects`/`SetExpr` push-down). Gate: NOTIF-D2 (storm-control).
- **N-M2.3 (M2)** — "first useful": escalation on the durable wheel (frozen chain shape), `inbox watch` on the
  frozen firehose resume-cursor transport, idempotent delivery, reindex-from-source. Gate: NOTIF-D7/D8/D9/D3 +
  the inbox-watch resume leg. **M2-exit obligations: NOTIF-D4 + NOTIF-D7.**
- **N-M3 (M3)** — Git + Knowledge register reasons + `watcher` fragments; NOTIF-D4 re-confirmed on real
  confidential subjects.
- **N-M4 (M4)** — Issues (SLA chains) + Chat (activity/mentions, explicit-first agents) + CI (`HumanisedRef`
  summaries) register. Gate touch-points: CHAT-D5, CHAT-D17, ISS-D6.
- **N-M5.1 (M5)** — cross-cell inbox aggregation (the multi-cell floor's follow-on; cell-local resolution).
- **N-M5.2 (M5)** — the 30×-agent-surge shed budget + the EU-sovereign delivery follow-on + the erasure
  residual instanced. Gate: NOTIF-D5 (surge, F6 family) + NOTIF-D6 (erasure).
- **N-M5.3 (M5)** — the E2E wedge legs (E2E-1 pane, E2E-2 HITL flagship, E2E-4 DSAR) + STOR-D2 at cell scale.

**Floors + follow-ons:** mock delivery → EU-sovereign provider (N-M5.2); single-home-cell → cross-cell
aggregation (N-M5.1); deterministic ranking → ML ranking (post-M5, measured); synthetic-watcher read-fanout →
real `watcher` fragments (N-M3/M4); stubbed reason set → per-subsystem enumerations (N-M3/M4); by-reference
erasure residual → provider-erasure + counsel ratification (N-M5.2).

**Critical upstream dependencies:** (1) Identity `list_objects`/`list_subjects` `SetExpr` push-down + zookie +
`FailStatic` (M1) — the highest-fan-in dependency, under every step-0 authorize and the whole read-fanout; (2)
the M0 outbox + idempotent-consumer template (Notif's inherited data-loss floor — never its own emit path);
(3) the M1 silent-data-loss + tenancy floor (STOR-D1/STOR-D2, residency-pin) — below Notif's
system-of-record tables; (4) the three M2 siblings ordered ahead of Notif's consuming milestones — Refs
`resolve(Display)`, the Bus Signals + firehose resume-cursor transport, `myelin-flow` durable timers/signals,
plus the frozen `myelin-content`/`myelin-query` crates; (5) M3/M4 producer registrations (`define_notif_rule`
+ `watcher` fragments + `mention` nodes + the X-1 `HumanisedRef`) for Notif's full surface; (6) the M5
multi-cell bridge + reserve/settle wallet + EU provider for the hardening follow-ons. Notif is **off the
critical path** but a **maximal consumer** — front-loaded in M2, accreting through M3/M4, hardened in M5.
