# Phase 3 — Notifications (`myelin-notif`): the ONE "what needs me" inbox + delivery fabric

> Phase: `03-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md). Doctrine
> bound: [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
> (EI-02) §1/§3/§4/§5/§6/§10, [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md)
> (EI-04) §1/§5.3, and the most-relevant doc
> [`external-insights/05-ux-and-design.md`](../../external-insights/05-ux-and-design.md) (EI-05) §4/§6.
> Directives bound: **NOTIF-1, NOTIF-2, NOTIF-3** plus X-1…X-5, BUS-2/BUS-3/BUS-4, STOR-1/2/3/4, ID-1/3.
> Spine bound: **ADR-12** (PersonalDataHolder spine — Notif is the named holder for "notification
> history"), ADR-04/ADR-19 (bus + four primitives; Notif consumes **Signals**), ADR-13 (`ArtifactRef`
> + envelope), ADR-03/ADR-17 (authz + fail-static), ADR-09 (durable-workflow for escalation timers),
> ADR-11 (cells), ADR-16 (backpressure). Resolves the Phase-2 **C-9** inbox overlap.
>
> **This doc closes the one spine gap named in the Phase-3 README** (§4 of
> [`README.md`](./README.md): "Notifications has no dedicated Phase-3 doc"). It is written **on the
> foundational contracts already decided** — it CONSUMES the substrate
> [`00-platform-substrate.md`](./00-platform-substrate.md) (consumer template §5, `serve(AppSpec)`,
> fail-static, telemetry), the Event Bus [`event-bus.md`](./event-bus.md) (Signal store §3.4,
> `define_signal_rule`, the firehose split, reindex-from-source §4.9), Identity
> [`identity-and-access.md`](./identity-and-access.md) (`list_objects`/`check`, the `Principal` model),
> and Refs [`reference-graph.md`](./reference-graph.md) (`resolve(ref, viewer) → Projection|Tombstone`,
> the per-subsystem `project` API). **It does not re-invent any of them.**
>
> **Status convention.** *DECIDED* = committed for P4/P5; *FLOOR* = partial answer + named follow-on;
> *[OPEN → P4/P5]* = handed forward. Every failable property names its **drill** (Phase 5 executes; this
> doc enumerates the obligation).

---

## 0. Reading map

- **§1** — purpose, responsibilities, the C-9 resolution (the ONE inbox; scoped views are projections).
- **§2** — the data model / schemas (the prioritised inbox, routing prefs, channel ledger, dedup,
  on-call/escalation, the humanisation template store).
- **§3** — algorithms (priority/ranking, storm-control/DEDUP, write-fanout vs read-fanout, routing +
  quiet-hours, backend humanisation, escalation on the durable-workflow substrate, reindex-from-source).
- **§4** — contracts exposed & consumed (the glue — STABLE).
- **§5** — scaling/sharding in the cell topology.
- **§6** — failure modes + the drills owed.
- **§7** — cited prior art (notification/feed-fanout literature).
- **§8** — required changes to foundational systems.
- **§9** — open questions for Phase 4.

**Floors named up front** (VISION §3 / EI-04 §4): cross-cell inbox aggregation for a **multi-cell
tenant** is **designed-not-built** (single-cell inbox is complete; §5.4, follow-on → P4 control plane).
The **EU-sovereign delivery-provider adapters** ship as a swappable-trait floor — the trait + the
EU-preferring posture are built, but the concrete production provider selection is a sovereignty/legal
call deferred to P4 (§3.6, §9). The **ML-tuned ranking** is a named follow-on; the v1 ranking is a
deterministic, explainable scoring function (§3.1, promotion trigger = measured "important things
buried" signal).

---

## 1. Purpose, responsibilities, and the C-9 resolution

### 1.1 What `myelin-notif` owns

Notifications owns **the ONE canonical, cross-subsystem, prioritised "what needs *me*" inbox** (UC-X-7)
and the **delivery fabric** that carries items out of the inbox to email/push/web/mobile/desktop. It is
the platform's single answer to *attention*: the one place a human (or an agent — agents have inboxes
too, §1.4) sees everything across git/CI/issues/knowledge/chat that is **addressed to them or that they
asked to watch**, ranked so the important things are not buried.

Concretely it owns:

1. **The prioritised per-principal inbox** — the durable "what needs me" model with per-item **read /
   seen / snoozed / archived / done** state and a **priority score** (§2.1, §3.1).
2. **The router** — consumes **Signals** (not the raw event firehose — BUS-4/ADR-19) and decides, per
   recipient, *whether* to notify, *at what priority*, and *on which channels* (§3.4).
3. **Storm-control / DEDUP** — collapse N near-identical notifications into one, coalesce bursts,
   suppress self-notifications, respect per-thread mute (§3.2).
4. **Per-user routing / preferences / quiet-hours (DND)** — the preference model and its evaluation
   (§2.2, §3.4).
5. **The delivery fabric** — email/push/web/mobile/desktop, each a **region-aware, swappable,
   EU-preferring adapter** behind one trait (§2.3, §3.6); a per-channel delivery ledger with
   at-least-once + idempotent delivery and bounded retry (§3.5).
6. **On-call / escalation routing** — escalation policies + on-call schedules for agent escalations and
   SLA breaches, on the **durable-workflow substrate** (ADR-09) for timed, resumable escalation chains
   (§2.4, §3.7).
7. **Backend humanisation of machine strings** — the templating layer that turns `"merge_request
   merged"` + a routable `ArtifactRef` into a human, render-time-resolved string **at the source**, so
   every consumer and every agent-authored message inherits it (NOTIF-1; §3.3).
8. **Notification history** — a **`PersonalDataHolder`** (ADR-12.1, the named holder "notification
   history"); rebuildable from source (NOTIF-3; §3.8).

### 1.2 What Notif is NOT

- It is **not the bus**. Subsystems do **not** call a `notify()` API per change; they emit events
  (outbox-only, BUS-2). The Bus's Signal engine curates events into Signals; Notif consumes Signals.
  "Subsystems emit facts; Notif decides who is summoned" (overview §5.4).
- It is **not the chat connection tier**. Real-time *in-app* delivery rides the web push / firehose
  path; chat presence/typing is a different transport (ADR-04.5). A toast is a delivery channel, not a
  chat message.
- It is **not the authority on visibility**. Notif **never** decides who *may* see an artifact; it asks
  Id (`check`/`list_objects`, §3.4 step 0) and Refs (`resolve` returns a tombstone for a viewer who
  lacks access). A notification can **never** leak content the recipient can't see (§3.3, §6 D-N6).
- It is **not the durable-workflow engine**. Escalation timers and resumable chains *invoke* the ADR-09
  substrate; Notif owns the *policy* (who, in what order, with what timeout), not the timer machinery.
- It does **not** own the typed relation tables, the reference graph, or projection content — it
  **calls** Refs `resolve` for every humanised string and unfurl (REF-1; §3.3).

### 1.3 The C-9 resolution — the ONE inbox; "My Work" and "Activity" are scoped *views into* it

**DECIDED (resolves C-9).** There is exactly **one** cross-subsystem inbox, owned by Notif. The Issues
**"My Work"** hub and the Chat **"Activity/Mentions"** inbox are **scoped, filtered queries INTO this
one inbox — not separate inboxes**. They are *projections* of the same `inbox_item` store, differing
only by a server-side filter, never by a separate data model or a separate fan-out path:

| Surface | Is | Implemented as |
|---|---|---|
| **Unified inbox** ("what needs me") | the canonical surface | `list_inbox(principal, filter=∅)` ranked by priority (§3.1) |
| Issues **"My Work"** | a *view* | `list_inbox(principal, filter = subsystem∈{issue} ∧ reason∈{assigned, mentioned, review_requested, sla, watched})` |
| Chat **"Activity / Mentions"** | a *view* | `list_inbox(principal, filter = subsystem∈{chat} ∧ reason∈{mentioned, replied, thread_watched})` |
| Git "Review requests" | a *view* | `list_inbox(principal, filter = subsystem∈{git} ∧ reason∈{review_requested, mentioned})` |

The rule that makes this true and keeps it true: **the inbox item carries a structured `reason` and a
`subject` `ArtifactRef`** (§2.1); every "scoped view" is a `filter` over those two fields, served by the
same `list_inbox` contract (§4.1). A subsystem that wants its own "my X" surface **adds a filtered view,
never a second store** — enforced as a *design rule* the Phase-4 sketches must follow (this is the
consistency-review C-9 instruction made structural). One store → one read-state truth (read it in chat,
it's read in the unified inbox), one priority model, one storm-control budget. This directly defeats the
exact failure the platform exists to fix: *three inbox-like surfaces fragmenting attention* (P8;
consistency-review C-9 "Why it matters").

> Design-language one-liner (carried to UX): *there is one inbox; everything else is a saved filter on it.*

### 1.4 Agents have inboxes too (the agent-native consequence)

An agent is a `Principal` (ADR-08.1; Id §3). The **same inbox + routing model** serves an agent's
"things addressed to me" — an agent's `EventInbox` (ADR-08, agent-fabric) is a **specialised consumer of
the same Signal/dispatch path**, and an HITL approval card surfaced to a *human* is a Notif item with
`reason = approval_requested` and a high priority (the Agent HITL loop, overview §10.4; AG-8). Backend
humanisation (NOTIF-1) means an agent-authored message inherits the human-readable form for free — the
explicit reason §8 of the Phase-3 README flags Notif's humanisation as "depended on by the Agent HITL
card path." We do **not** build a parallel agent-notification system (the EI-02 §2 anti-pattern).

---

## 2. The data model / schemas

All tables obey the substrate non-negotiables (00 §0.1): **`(tenant, region)` first**, RLS-enforced, no
cross-tenant query path (ID-3); per-tenant envelope-encrypted, crypto-shred-capable; every store is a
`PersonalDataHolder` auto-registered by `serve(AppSpec)` (GD-3). Engine = **Postgres-class** (ADR-14;
overview README "Postgres for routing/prefs/history/inbox") + a Redis/Valkey-class cache that is
**never** a source of truth (STOR-3). Thin, visible SQL over an ORM (§(e) prior).

### 2.1 The inbox item (the prioritised "what needs me" model — the heart)

```sql
CREATE TABLE inbox_item (
  tenant        uuid        NOT NULL,
  region        text        NOT NULL,
  item_id       uuid        NOT NULL,                  -- stable inbox id (the snooze/read handle)
  recipient     uuid        NOT NULL,                  -- the Principal this is FOR (human OR agent)

  -- what & why (the C-9 view keys; §1.3)
  subject       text        NOT NULL,                  -- the ArtifactRef this is about (git/pr/88, …)
  subject_root  text        NOT NULL,                  -- the parent aggregate (for thread coalescing; §3.2)
  reason        notif_reason NOT NULL,                 -- WHY me: mentioned | assigned | review_requested
                                                       --  | replied | watched | sla | approval_requested
                                                       --  | escalated | agent_proposal | state_changed
  origin_signal uuid        NOT NULL,                  -- the Signal that produced this (Bus §3.4) — provenance
  origin_event  text        NOT NULL,                  -- the event_id (idempotency anchor; NOTIF-2 "why")

  -- humanisation (NOTIF-1) — stored as a TEMPLATE + bound refs, rendered per-viewer at read (§3.3)
  template_key  text        NOT NULL,                  -- e.g. 'git.pr.merged' → the humanise template
  template_args jsonb       NOT NULL,                  -- {actor: <ArtifactRef>, pr: <ArtifactRef>, ...} — REFS, not strings

  -- priority & ranking (§3.1)
  priority      smallint    NOT NULL,                  -- 0..100 computed score (the rank key)
  priority_class notif_class NOT NULL,                 -- critical|direct|participating|watching|fyi (explainable bucket)

  -- dedup / storm-control (§3.2)
  dedup_key     text        NOT NULL,                  -- collapses near-identical items within a window
  coalesce_count int        NOT NULL DEFAULT 1,        -- N events folded into this item ("+12 more")
  last_event    text        NOT NULL,                  -- most-recent folded event_id

  -- per-recipient state (the read-state TRUTH — one store, §1.3)
  state         item_state  NOT NULL DEFAULT 'unread', -- unread | seen | read | snoozed | archived | done
  snooze_until  timestamptz,                           -- a durable timer re-surfaces it (ADR-09; §3.7)

  -- GDPR routing
  contains_pii  boolean     NOT NULL DEFAULT false,    -- mirror of the source event flag (ADR-04.4)
  data_role     data_role   NOT NULL,                  -- tenant-content | platform-operational (ADR-12.5)

  occurred_at   timestamptz NOT NULL,                  -- the source fact time
  created_at    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant, recipient, item_id),
  UNIQUE (tenant, recipient, dedup_key)                -- the storm-control collapse key (§3.2)
);

-- The ranked-read index (the one query the inbox UI runs constantly):
CREATE INDEX inbox_ranked ON inbox_item (tenant, recipient, state, priority DESC, occurred_at DESC)
  WHERE state IN ('unread','seen','snoozed');
-- The C-9 scoped-view index (Issues "My Work", Chat "Activity"):
CREATE INDEX inbox_by_reason ON inbox_item (tenant, recipient, reason, priority DESC);
```

**Design notes that are load-bearing:**

- **`template_args` holds `ArtifactRef`s, never rendered strings** (NOTIF-1; §3.3). The human string is
  produced at *read* time by resolving each ref through Refs `resolve(ref, viewer)` — so a renamed PR, a
  retitled issue, or an *erased* author all reflect correctly, and a viewer who lost access sees a
  tombstone, not a stale title. Storing the rendered string would (a) leak on access change, (b) go
  stale, (c) defeat erasure. This is the "humanise at the source paired with a routable ref" mandate
  made concrete.
- **`origin_event` + `reason`** are the **NOTIF-2 "why it fired"** provenance, carried on every item.
  The UI's "why am I seeing this?" pulls `reason` + the causal chain (`correlation_id` via the origin
  event) — *the system assembles context, the user never does* (EI-05 §4).
- **One read-state store** is the whole point of C-9: marking read in Chat's "Activity" view is the same
  row as the unified inbox (§1.3).
- `dedup_key` + the `UNIQUE` constraint make storm-control a *write-time* collapse (an `INSERT … ON
  CONFLICT DO UPDATE SET coalesce_count = coalesce_count+1`), not a read-time scan (§3.2).

### 2.2 Routing preferences / quiet-hours (per principal)

```sql
CREATE TABLE notif_pref (
  tenant        uuid NOT NULL,
  region        text NOT NULL,
  principal     uuid NOT NULL,
  -- channel routing as a matrix: per (reason × priority_class) → which channels
  routing       jsonb NOT NULL,    -- e.g. {"mentioned": {"min_class":"direct", "channels":["inbox","push"]},
                                   --       "sla": {"channels":["inbox","push","email"]}, ...}
  -- the matcher uses the SAME safe query-AST predicate core as EventMatcher (ADR-07; AG-7) — no 2nd lang
  digest        jsonb,             -- batched low-priority delivery: {"cadence":"daily","at":"09:00", classes:["watching","fyi"]}
  PRIMARY KEY (tenant, principal)
);

CREATE TABLE quiet_hours (
  tenant        uuid NOT NULL,
  principal     uuid NOT NULL,
  tz            text NOT NULL,            -- IANA tz; quiet windows are evaluated in the PRINCIPAL's tz
  windows       jsonb NOT NULL,           -- [{days:[mon..fri], from:"18:00", to:"08:00"}], + weekends
  dnd_until     timestamptz,              -- one-shot Do-Not-Disturb override
  -- the ESCALATION OVERRIDE: critical/escalated items pierce quiet hours (on-call cannot be silenced)
  pierce_classes notif_class[] NOT NULL DEFAULT '{critical}',
  PRIMARY KEY (tenant, principal)
);
```

- Preferences are **per-principal, tenant-scoped**, and the matcher reuses the **one safe query-AST
  predicate core** (ADR-07/AG-7) — Notif does not invent a second predicate language. This is the X-5
  contract: the same AST that powers saved views, search filters, and `EventMatcher` powers notification
  routing.
- **Quiet-hours are evaluated in the recipient's tz**, and **critical/escalated items pierce them by
  default** (`pierce_classes`) — you cannot silence an on-call page (§3.7). This is the one place we
  deliberately override user preference, and it is explicit and configurable, not hidden.

### 2.3 The channel delivery ledger (at-least-once + idempotent delivery)

```sql
CREATE TABLE delivery (
  tenant        uuid NOT NULL,
  region        text NOT NULL,
  delivery_id   uuid NOT NULL,
  item_id       uuid NOT NULL,            -- → inbox_item
  recipient     uuid NOT NULL,
  channel       channel_kind NOT NULL,    -- inbox | web_push | mobile_push | email | desktop
  adapter       text NOT NULL,            -- the region-aware adapter that handled it (§3.6)
  idem_key      text NOT NULL,            -- = hash(item_id, channel) — the provider-side DEDUP key
  state         delivery_state NOT NULL,  -- pending | sent | delivered | bounced | failed | suppressed
  attempts      int  NOT NULL DEFAULT 0,
  provider_ref  text,                     -- provider message id (for delivery receipts/bounces)
  redacted      boolean NOT NULL DEFAULT false, -- PII kept OUT of the off-cell payload (§3.6) → true
  created_at    timestamptz NOT NULL DEFAULT now(),
  sent_at       timestamptz,
  PRIMARY KEY (tenant, delivery_id),
  UNIQUE (tenant, idem_key)               -- exactly-one delivery per (item, channel) in effect
);
```

The `UNIQUE(idem_key)` makes delivery **idempotent**: a retried send (after a crash between provider-ack
and ledger-write) is collapsed, so we get **at-least-once-attempt + dedup ≈ effectively-once delivery** —
the same effectively-once discipline the bus uses (EI-02 §4; Helland 2012), applied to email/push.

### 2.4 On-call schedules + escalation policies

```sql
CREATE TABLE oncall_schedule (
  tenant        uuid NOT NULL,
  region        text NOT NULL,
  schedule_id   uuid NOT NULL,
  name          text NOT NULL,                 -- 'platform-oncall'
  rotation      jsonb NOT NULL,                -- [{principal, from, to}] layered rotations + overrides
  tz            text NOT NULL,
  PRIMARY KEY (tenant, schedule_id)
);

CREATE TABLE escalation_policy (
  tenant        uuid NOT NULL,
  policy_id     uuid NOT NULL,
  name          text NOT NULL,
  -- ordered steps: notify target, wait, escalate if unacked
  steps         jsonb NOT NULL,  -- [{target:{schedule|principal|team}, channels:[...], wait:"5m"}, ...]
  repeat        int NOT NULL DEFAULT 1,         -- loop the policy N times before giving up
  ack_window    interval NOT NULL,             -- ack within this or escalate to next step
  PRIMARY KEY (tenant, policy_id)
);

CREATE TABLE escalation_run (                   -- a live escalation (a durable workflow instance handle)
  tenant        uuid NOT NULL,
  run_id        uuid NOT NULL,
  policy_id     uuid NOT NULL,
  trigger_event text NOT NULL,                  -- the originating event_id (an SLA breach / agent escalation)
  workflow_ref  text NOT NULL,                  -- the ADR-09 durable-workflow instance id (§3.7)
  current_step  int NOT NULL DEFAULT 0,
  state         escalation_state NOT NULL,      -- active | acked | resolved | exhausted
  acked_by      uuid,
  acked_at      timestamptz,
  PRIMARY KEY (tenant, run_id)
);
```

The escalation **state machine and timers live in the durable-workflow engine** (ADR-09); these tables
are the *policy* + a *handle* to the running workflow. SLA timers and escalation timers share that
substrate (SC-11: "millions of durable timers") — we do not re-implement durable timers in Notif.

### 2.5 The humanisation template store (NOTIF-1)

```sql
CREATE TABLE humanise_template (
  tenant        uuid,                            -- NULL = platform default; tenant row overrides (locale/brand)
  template_key  text NOT NULL,                   -- 'git.pr.merged', 'ci.run.failed', 'issue.assigned', ...
  locale        text NOT NULL DEFAULT 'en',      -- i18n: per-recipient locale at render (§3.3)
  -- ICU MessageFormat string with named arg slots that bind to RESOLVED refs:
  template      text NOT NULL,                   -- '{actor} merged {pr} into {base}'
  PRIMARY KEY (COALESCE(tenant,'00000000-…'), template_key, locale)
);
```

Templates are **platform-defaulted, tenant/locale-overridable**, and use **ICU MessageFormat** (plurals,
gender, locale-correct ordering — the i18n standard). The arg slots bind to **`ArtifactRef`s resolved at
render** (§3.3), so the template is a *machine→human contract*, the one place a machine string is
humanised, inherited by every consumer and every agent message (NOTIF-1; EI-05 §6).

### 2.6 Storm-control / suppression state

```sql
CREATE TABLE mute (                              -- per-principal thread/subject mutes ("mute this thread")
  tenant uuid NOT NULL, principal uuid NOT NULL,
  subject_root text NOT NULL,                    -- mute the whole aggregate (a chat thread, a PR)
  until timestamptz,                             -- NULL = forever
  PRIMARY KEY (tenant, principal, subject_root)
);
```

---

## 3. The algorithms

### 3.1 Priority / ranking (deterministic, explainable v1; ML-tuned is the named follow-on)

**DECIDED: v1 ranking is a deterministic, explainable scoring function** (not an opaque ML model). The
score `priority ∈ 0..100` is computed at routing time and stored on the item; the **`priority_class`** is
the human-legible bucket the UI groups by (status by glyph/label/position, never colour alone — EI-05
§3). This is grounded in feed-ranking prior art (Facebook EdgeRank's *affinity × weight × decay*; the
Google Inbox/Gmail "important" classifier) but **deliberately deterministic-first** because an inbox
whose ranking a user can't predict or explain *erodes trust faster than no ranking* — and "why am I
seeing this, ranked here?" must be answerable (NOTIF-2; EI-05 §4 "the system assembles context").

```
priority = clamp( base(reason)
                + affinity(recipient, subject)        // do you own / author / participate?
                + role_weight(recipient, subject)     // assignee/reviewer > watcher > org-member
                - age_decay(occurred_at)              // monotone time decay
                + escalation_boost                    // SLA/on-call pierces to the top
                , 0, 100)
```

| `reason` | base | maps to `priority_class` |
|---|---|---|
| `approval_requested`, `escalated`, `sla` | 90 | **critical** (pierces quiet-hours) |
| `review_requested`, `assigned`, `mentioned` (direct @you) | 70 | **direct** |
| `replied` (to your thread), `agent_proposal` | 55 | **participating** |
| `watched`, `state_changed` (on a watched subject) | 35 | **watching** |
| team/project-wide `fyi` | 15 | **fyi** (digest-eligible) |

- **`affinity`/`role_weight`** are derived from **Id `list_objects`/relations** and Refs backlinks (am I
  the assignee? author? reviewer?) — *not* re-computed by Notif. Notif asks; it does not own who-relates-
  to-what (ADR-13).
- **Explainability is a hard requirement**: the score decomposes into named contributing terms, surfaced
  in the "why this priority?" affordance. This is the *proof-it* posture applied to ranking — a buried-
  important-thing is a measurable failure (§6 D-N1), and the deterministic model makes the failure
  *diagnosable*.
- **Promotion trigger (named follow-on, R-5):** a measured "important things buried / false-FYI" signal
  (from read-latency telemetry, §6 D-N1) triggers the ML-tuned ranker — slotted **behind the same
  scoring interface** (strategy pattern; ADR-08 generalisation) so it is a swap, not a rewrite.

### 3.2 Storm-control / DEDUP (the attention-protection core)

The platform exists partly to fix **notification overload** (personas; P8). Storm-control is therefore a
*first-class correctness property*, not a nicety. Five layered mechanisms, applied **at routing/write
time** (cheap; the read path stays a simple ranked scan):

1. **Self-suppression (the loop-safety floor).** Drop any item whose `actor.principal == recipient` — you
   are never notified of your own action (the agent self-guard, AG-6, generalised to humans). Structural:
   reads the envelope `actor`, not a convention.
2. **Dedup-key collapse.** `dedup_key = render(rule.dedup_key_tpl, signal)` (e.g.
   `ci-failure:<pipeline>:<branch>` or `pr-review:<pr_id>:<recipient>`). An incoming item with an
   existing `(recipient, dedup_key)` does **`ON CONFLICT DO UPDATE SET coalesce_count = coalesce_count+1,
   last_event = …, priority = greatest(priority, new)`** — N near-identical events become **one item with
   "+N more"**. This is the storm-control primitive the overview (UC-EDGE-4) names, and it reuses the
   Bus's Signal-level dedup (Bus §3.4) — *two-tier dedup*: Signals collapse at the source (one Signal for
   100 identical CI failures), Notif collapses again per-recipient.
3. **Thread/subject coalescing.** Many events on one `subject_root` (a busy PR, a hot chat thread)
   coalesce into a per-thread item ("12 new comments on PR #88") rather than 12 items — unless a *direct*
   `reason` (a mention) breaks out as its own high-priority item. This is the standard "digest the
   participating, break out the direct" feed pattern.
4. **Rate-of-fire damping.** A per-(recipient, subject_root) token bucket bounds how fast a single source
   can mint items; over-budget events fold into the coalesced item. This caps a runaway producer (a force-
   push loop, an agent storm) from melting one user's inbox — the inbox-side complement to the bus's
   per-tenant in-flight cap (ADR-16; §5.2).
5. **Mute / DND honoring.** A `mute(subject_root)` row suppresses non-piercing items for that aggregate;
   quiet-hours/DND (§2.2) suppress *channel delivery* (the item still lands in the inbox, just not as a
   push) except for `pierce_classes`.

**Crucially, storm-control suppresses *delivery and ranking*, never the audit/history of the underlying
event** — the events still exist on the bus (the source of truth); Notif is a *projection* that chooses
what reaches attention (EI-04 §5.3; Kreps "The Log").

### 3.3 Backend humanisation (NOTIF-1 — the #1 "unfinished" fix, at the source)

Raw machine strings (`"merge_request merged"`, raw ids, unrendered markdown) are the #1 "this feels
unfinished" tell (EI-05 §6). The fix is **humanise at the backend, paired with a routable `ArtifactRef`
— not a frontend string map** — so *every* consumer (web, mobile, email, CLI) and *every agent-authored
message* inherits the human form for free.

**The render pipeline** (`humanise(item, viewer, locale) → HumanisedString`):

```
1. look up humanise_template[ (tenant|default), item.template_key, viewer.locale ]   (§2.5)
2. for each ArtifactRef arg in item.template_args:
     proj = refs.resolve(ref, viewer, mode=Display)        // Refs §5; per-VIEWER, permission-checked
     if proj is Tombstone: bind the slot to the tombstone display ("a restricted issue" / "[erased user]")
     else: bind the slot to proj.title (+ proj.icon, + a click-route to the ArtifactRef)
3. ICU-format the template with the bound slots → the final string + the routable links
```

This is load-bearing for **four** platform properties at once:

- **Permission-safe by construction** (EI-05 §6 + ADR-03): because every arg is resolved per-viewer via
  Refs, a notification of a confidential issue humanises to *"Alice updated a restricted issue"* for a
  viewer who lacks `issue.view` — the title never leaks (the same tombstone discipline as unfurls; Refs
  §4.2). **The string can never contain content the recipient can't see** (§6 D-N6).
- **Erasure-safe** (ADR-12.4; EI-04 §1): because the actor is an `ArtifactRef` to a pseudonymous
  principal resolved at render, an *erased* user humanises to the tombstone (`[erased user]`) with **no
  stored PII to scrub** — the inbox item survives erasure untouched (references-not-payloads).
- **Always-current**: a renamed PR / retitled issue reflects immediately because the title is resolved at
  read, never frozen at write.
- **Agent-inherited** (the README §8 dependency): an agent-authored message is a template render too, so
  an agent's HITL card and an agent's chat message get the same human form, the same routable links, with
  zero agent-side string work (NOTIF-1 "every agent-authored message inherits it").

**Markdown** in humanised strings is rendered through the **one editor render path** (`myelin-content`,
KN-4/D10) — never leaked raw (the EI-05 §6 "unrendered markdown" tell). Email channels get a
sanitised-HTML projection of the same content model; CLI gets the plain-text projection. **One content
model, many channel projections** — never per-channel string maps.

### 3.4 The router (Signal → per-recipient inbox items)

Notif is a **consumer of Signals** (BUS-4/ADR-19), built on the **shared consumer template** (00 §5;
Bus §4.2) — whitelisted subjects (`sig.<tenant>.>` + the curated reasons), durable-bind-by-name,
idempotent on `event_id`, ack-after-enqueue, bounded prefetch, lag exported (§6). For each Signal:

```
0. AUTHORIZE: compute the candidate recipient set, then for each candidate
     check(recipient, view, signal.subject)  →  drop candidates who can't see it   (ADR-03; never leak)
1. RECIPIENT RESOLUTION (the fan-out decision, §3.5):
     - DIRECT (write-fanout): mention(Principal) nodes, assignee/reviewer relations, escalation targets
       → resolved to an EXPLICIT, BOUNDED recipient list (cheap, high-signal)
     - AMBIENT (read-fanout): "watchers of this subject" / "members of this channel"
       → NOT exploded into per-recipient writes; materialised lazily on read (§3.5)
2. For each direct recipient:
     a. reason ← classify(signal, recipient)              // mentioned / assigned / review_requested / …
     b. priority, priority_class ← score(reason, recipient, subject)   (§3.1)
     c. dedup_key ← render(dedup_tpl, signal); STORM-CONTROL collapse (§3.2)
     d. UPSERT inbox_item                                 // the durable inbox truth
     e. channel set ← route(recipient.prefs, reason, priority_class)   ∩  ¬quiet_hours (unless pierce)  (§2.2)
     f. for each channel: enqueue a delivery (§3.5) with idem_key = hash(item, channel)
3. emit notif.item.created via the OUTBOX (BUS-2)         // for web-push live delivery + audit + reindex
```

- **Step 0 is non-negotiable**: authorize *before* materialising a recipient. A notification is a *read*
  of the subject on the recipient's behalf; it obeys `check` exactly like any read (Id §8). This is why
  Notif can never become a permission side-channel.
- **The router is idempotent on `origin_event`** (the consumer template's dedup ledger): a redelivered
  Signal re-produces the *same* `inbox_item` (UPSERT on `dedup_key`) and the *same* `delivery` (UPSERT on
  `idem_key`) — at-least-once + idempotent ≈ effectively-once (EI-02 §4).

### 3.5 Write-fanout for mentions vs read-fanout for bodies (the fan-out architecture)

This is the central scaling decision, and it is the **hybrid fanout-on-write / fanout-on-read** model
from the feed-systems literature (Twitter's "@-mention vs timeline" split; Facebook TAO; the
fan-out-on-write-vs-read trade-off — Krishnan/"Timelines at Scale", Silberstein et al. "Feeding
Frontier" VLDB 2010). The decision matches the doctrine (overview §5.2: "**targeted write-fanout** for
high-signal events… while low-signal bodies stay read-fanout"):

| Class | Strategy | Why | Example |
|---|---|---|---|
| **DIRECT / high-signal** | **fan-out on WRITE** — materialise an `inbox_item` per recipient at routing time | recipient set is **small and bounded** (the mentioned/assigned/reviewer/escalation targets); these MUST be reliable, ranked, and pierce delivery — worth the write | a `mention(Principal)` node, "review requested from you", an SLA page |
| **AMBIENT / low-signal** | **fan-out on READ** — store ONE coalesced "activity" marker per `(subject_root)`; materialise per-watcher *lazily* when a watcher opens their inbox | recipient set is **large and unbounded** (every watcher of a hot PR, every member of a 5k-person channel); exploding 5k writes per event is the "celebrity fan-out" disaster | "channel #general had 200 messages", "PR #88 got 30 new comments" |

- **The mention is the canonical write-fanout producer** (overview §5.4): a `mention(Principal)` node in
  the shared content model (ADR-05) is the platform-uniform "notify this principal" signal across chat,
  issues, and docs — Notif does not parse free text; it reads the structured node (which is *also* the
  agent-loop reference gate, AG-6 — only a structured ref re-triggers, never raw text).
- **Read-fanout watchers** are computed at read time via `list_subjects(subject_root, watch)` (Id §8.3)
  ∩ the recipient's filter — so a "celebrity" subject with 50k watchers costs **zero write amplification**
  and one bounded read per watcher who actually looks. This is the exact celebrity-problem mitigation the
  feed literature prescribes.
- **The hot-subject cap** (§3.2.4) bounds even the write-fanout side: a mention-storm on one subject is
  rate-damped so a malicious/agent mention flood can't write-amplify.

### 3.6 The EU-sovereign delivery fabric (region-aware swappable adapters)

Delivery to email/push/web/mobile/desktop egresses **out of the cell** to third-party providers
(email/SMS providers are **sub-processors** — ADR-12.8; gdpr-eu-sovereignty §3.7). This is a **sovereignty
egress review point** (overview §5.6). The design:

```rust
/// One trait; fs-vs-S3-style swap (STOR-1 philosophy). EU-preferring, region-aware.
pub trait DeliveryAdapter {
    fn channel(&self) -> ChannelKind;
    fn region(&self) -> Region;                          // the adapter is PINNED to a region
    fn send(&self, msg: RedactedMessage, idem_key: &str) -> Result<ProviderRef>; // idempotent
    fn receipts(&self) -> ReceiptStream;                 // delivery/bounce/complaint callbacks
}
```

- **EU-preferring, region-aware, swappable** (ADR-12.8 + VISION §1): the adapter is selected by the
  tenant's `region`; an EU tenant's email/push routes through an **EU-hosted provider** by default. The
  trait makes provider choice a **config swap**, not a rewrite — the same strategy-pattern mandate that
  swaps mock→real agents, generalised to sub-processors (ADR-12.8).
- **PII-minimised payloads** (overview §5.6; gdpr §3.3): the off-cell payload is **`RedactedMessage`** —
  it carries *a humanised summary + a deep link back into the cell*, **never** the full artifact body or
  free-text PII where avoidable (`delivery.redacted = true`). "You have a new review request — open in
  Myelin" rather than the full diff in an email. This keeps the third-party provider's exposure minimal
  (data-minimisation, GDPR Art. 5(1)(c)) and shrinks the breach surface.
- **In-app channels** (`inbox`, `web_push` to an open session, `desktop`) **never leave the cell** — they
  ride the firehose/web-push path inside the cell, so the sovereign default needs no third party at all.
- **FLOOR (named):** the **trait + the EU-preferring posture + the redaction discipline are built**; the
  *concrete production provider* (which EU email/push vendor, with what DPA) is a **sovereignty/legal
  selection deferred to P4** (§9). v1 dev uses a deterministic **mock adapter** (logs the redacted
  payload + asserts idempotency) — the `--use-mock`-as-runtime discipline (D6), so the whole delivery
  path is testable with zero egress.

### 3.7 On-call / escalation on the durable-workflow substrate (ADR-09)

An **escalation** (an SLA breach Signal, or an agent escalation — UC-AG-19/UC-EDGE-25) starts a
**durable workflow** (ADR-09) whose steps walk the `escalation_policy`:

```
arm escalation_run → workflow:
  step k: resolve target (oncall_schedule rotation @ now, or principal/team)
        → notify(target, channels, priority_class=critical)   // PIERCES quiet-hours (§2.2)
        → durable-timer wait(ack_window)                       // ADR-09 durable timer — survives restart
        → if acked (an event resolves it): state=acked, stop
        → else: step k+1 (escalate)
  after all steps × repeat: state=exhausted → alert the owner + a critical fallback
```

- The **timers are ADR-09 durable timers**, not in-process sleeps — an escalation that waits 15 minutes
  survives a Notif restart and fires exactly once (the "wait days for a human signal without holding
  resources" property; agent-native-design §3.2). Notif owns the *policy*; the workflow engine owns
  *durability*.
- **Ack is an event**: acknowledging a page emits `notif.escalation.acked` via the outbox; the workflow's
  signal-wait resolves on it (ADR-09 signals). On-call **cannot be silenced** (`pierce_classes` default
  `critical`) — the one deliberate quiet-hours override.
- The same durable-timer substrate serves **snooze re-surfacing** (`snooze_until`) and **SLA timers**
  (SC-11, millions of timers) — one substrate, three uses.

### 3.8 Reindex-from-source (NOTIF-3 — the inbox read-model is rebuildable)

The inbox is a **derived read-model**; per NOTIF-3 / EI-04 §5.3 it is **rebuildable from source via the
live consumer path** — Notif never has a bespoke recovery backdoor:

```
events::reindex(scope=notif) → owners replay their *.snapshot events through the outbox→bus→Signal path
  → the SAME router (§3.4) re-ingests them idempotently (origin_event dedup)
  → inbox_item / delivery read-models are reconstructed; cold == live (the parity drill, §6 D-N3)
```

- This is the **only** recovery path (SEARCH-1 analogue): there is no "read the inbox from some other
  store" code. Steady-state and recovery use **one code path** → cannot drift (EI-04 §5.3).
- It doubles as the **new-recipient backfill** and the **schema-upcaster** path (a changed item shape is
  a reindex). A wiped inbox is `reindex(notif, since=<retention floor>)`.
- **Retention floor:** the inbox keeps a bounded window (e.g. 90 days of items, prefs/on-call/templates
  are permanent); older items age out (they remain reconstructable from the OLAP/Audit long-term holder
  fed off the bus — Bus §4.8). This bounds the holder, aiding GDPR minimisation.

### 3.9 The PersonalDataHolder implementation (ADR-12.1 — Notif IS the "notification history" holder)

```rust
impl PersonalDataHolder for Notif {
  fn locate(subject)  -> inbox_items where recipient=subject OR template_args reference subject;
  fn export(subject)  -> the subject's inbox history (refs resolved via owners, §3.3) — a DSR receipt;
  fn rectify(subject) -> no stored PII to rectify (references-not-payloads); profile rectification is Id's;
  fn restrict(subject)-> stop new routing/delivery for a restricted subject;
  fn erase(subject)   -> crypto-shred the per-subject key for any inline PII; delete delivery rows
                         (provider_ref may need a provider-side erasure call — §9); items humanise to
                         the tombstone automatically (refs resolve to [erased]) — references-not-payloads
                         means MOST of the inbox needs no mutation on erasure (EI-04 §1).
}
```

Because items store **refs not strings** (§2.1/§3.3), erasing a *person* tombstones their appearance in
everyone's inbox **for free** — the references-not-payloads lever (ADR-12.4) does the work. The only
inline-PII cases (a redacted email subject already sent off-cell) are crypto-shred + a provider-side
erasure request (a named sub-processor obligation, §9). Notif is auto-registered as a holder by
`serve(AppSpec)` (GD-3), so "we forgot notification history" is structurally impossible.

---

## 4. Contracts exposed & consumed

### 4.1 Contracts EXPOSED (the glue — STABLE; field names + units reconciled per X-5)

| Contract | Signature (illustrative) | Consumed by | Semantics |
|---|---|---|---|
| **list_inbox** | `list_inbox(principal, filter?, page?) → [InboxItem]` ranked by `priority DESC` | inbox UI, Issues "My Work", Chat "Activity", CLI `inbox list` | the ONE inbox (C-9); a "scoped view" is a `filter` over `reason`/`subject` (§1.3). |
| **item state** | `mark(item_id, state)`; `snooze(item_id, until)`; `mark_all_read(filter)` | inbox UI, CLI | one read-state truth across all views (§1.3). |
| **humanise** | `humanise(item \| (template_key, args), viewer, locale) → HumanisedString{text, links[], icon}` | every channel renderer, **agent messages** (NOTIF-1) | resolves refs per-viewer; permission-safe; erasure-safe (§3.3). |
| **prefs** | `get_prefs(principal)` / `set_prefs(principal, routing, quiet_hours, digest)` | settings UI, CLI `inbox prefs` / `notify prefs` | per-principal routing + quiet-hours (§2.2). |
| **on-call** | `oncall_now(schedule) → principal`; `page(target, reason)` | CLI `oncall show|page`, SLA engine, Agent Fabric (escalation) | resolves rotation; starts an escalation run (§3.7). |
| **define_notif_rule** | `define_notif_rule(reason, dedup_tpl, default_class)` | admin, subsystem P4 | how a Signal class maps to an inbox reason/priority (§3.1). |
| **PersonalDataHolder** | `locate/export/rectify/restrict/erase(subject)` | DSR orchestrator (ADR-12) | the "notification history" holder (§3.9). |
| **replay** | `replay(scope, since) → emits notif read-model from source` | Bus reindex (NOTIF-3) | reindex-from-source; the only recovery path (§3.8). |
| **telemetry** | `inbox_read_latency`, `important_buried_rate`, `dedup_collapse_ratio`, `delivery_success/bounce`, `escalation_ack_latency`, `quiet_hours_pierce_count`, `consumer_lag` | Phase-5 drills (X-1) | the survival signals the drills read (§6). |

**CLI surface** (consistency-review C-4, adopted): `myelin inbox list|show|read|snooze|watch|prefs` for
the per-user "what needs me" feed; `myelin notify prefs|test`, `myelin oncall show|page` for delivery/on-
call config (C-4's deliberate intent-split). `inbox watch` streams new items live over the firehose/
web-push path (the `--watch` long-poll, CA-8).

### 4.2 Contracts CONSUMED (Notif builds on these; it re-invents none)

| Consumed contract | From | Used for |
|---|---|---|
| **Signal stream** `sig.<tenant>.<severity>.<rule>` + `define_signal_rule` | Bus (§3.4, §3.7 of Bus) | the router consumes curated Signals, never `evt.*` (BUS-4) |
| **`EventHandler` consumer template** | substrate / Bus (00 §5) | the router IS a template consumer (whitelist, idempotent, ack-after, lag) |
| **`OutboxTx::emit`** | `myelin-events` (BUS-2) | emit `notif.item.created` / `notif.escalation.acked` — the ONLY emit path |
| **`check` / `list_objects` / `list_subjects`** | Id (§8) | step-0 authorize; affinity/role; read-fanout watcher resolution (§3.4/§3.5) |
| **`resolve(ref, viewer) → Projection\|Tombstone`** + per-subsystem **`project`** | Refs (§5) | the humanisation render — title/icon/route per-viewer, tombstone on deny (§3.3) |
| **`mention(Principal)` node** | `myelin-content` (ADR-05) | the canonical write-fanout "notify this principal" producer (§3.5) |
| **durable timers / signals** | durable-workflow engine (ADR-09) | escalation chains, snooze re-surfacing, SLA timers (§3.7) |
| **safe query-AST predicate core** | `myelin-query` (ADR-07/AG-7) | the preference matcher — one predicate language, one DoS surface (§2.2) |
| **`FailStatic` / fail-static reads** | substrate (00 §8) / Id | the inbox degrades static on an Id hiccup (§5.3) |
| **`PersonalDataHolder` auto-registration / KMS** | `myelin-gdpr` (00 §3.4, ADR-12) | crypto-shred, holder registration (§3.9) |

---

## 5. Scaling / sharding in the cell topology (ADR-11)

### 5.1 In-cell, tenant-partitioned, bus-driven
Notif is **cell-local** and **tenant-partitioned** (`(tenant, region)` first column everywhere — EI-02
§1). All heavy work is **async off the bus** (router consumes Signals; ADR-11.5) — there is no
synchronous "notify" call in any subsystem's write path; subsystems emit, Notif reacts. The router is a
**stateless, horizontally-replicable** consumer pool (recoverable by reconnecting to the durable log +
reindex-from-source, §3.8).

### 5.2 The fan-out scale axis (the dominant cost) and its bounds
The dominant scale risk is **fan-out amplification** (one event → many recipients). The §3.5 hybrid
(write-fanout for bounded direct sets, read-fanout for unbounded ambient sets) is the structural answer —
a "celebrity" subject with 50k watchers costs **zero write amplification**. On top of it:
- **Bounded everything** (X-3/ADR-16): bounded consumer prefetch, bounded handler pool, **per-tenant
  in-flight caps** (one tenant's mention-storm can't starve another's), bounded delivery-adapter
  concurrency (a bulkhead per provider — §6 D-N5).
- **Per-recipient rate damping** (§3.2.4) caps a single recipient's inbox write-rate from any one source.
- **The protected human lane** (ADR-16): an agent-generated notification storm sheds before a human's
  interactive inbox read; the 30× agent-surge drill (§6 D-N5) asserts the human inbox stays responsive.

### 5.3 Fail-static behaviour (ADR-17 / 00 §8)
On an **Id hiccup**, the inbox **fails static**: `list_inbox` serves the already-materialised items (the
inbox store is the truth; ranking is precomputed) and humanisation falls back to **cached projections**
(Refs' bounded projection cache) — already-authenticated traffic survives. **New routing** that needs a
fresh `check` waits/degrades within the staleness bound (a notification's *delivery* can tolerate seconds
of staleness; its *authorization* never relaxes — a `check` that can't resolve fail-*closes* the routing
decision, never fails open: an unsure item is held, not leaked). This is the EI-02 §10 fail-static-for-
availability / fail-closed-for-authorization split applied to Notif.

### 5.4 Cross-cell inbox aggregation (FLOOR — designed-not-built)
A **multi-cell tenant** (a 10k-org spanning cells, SC-2/SC-3) needs a recipient's inbox to aggregate
items from **every cell they belong to**. **This is a named floor, not built in v1.** Design seam: the
inbox is materialised **per home-cell**; a multi-cell recipient's unified view is an **aggregation across
their cells' inboxes via the control-plane PII-free pointer bridge** (Bus §7.4 / Tenancy §10) — the
bridge carries only `subject`/`type`/`correlation_id`, and **humanisation always resolves locally in the
cell that holds the artifact** (no PII crosses cells — residency-preserving). Follow-on owner: **P4
control plane + multi-cell tenancy (SC-2/SC-3)**. The single-cell path is complete; the §4 contracts are
cell-agnostic so this extends without a rewrite.

### 5.5 Stateful-component register + blast-radius note (X-4)
| Stateful component | Shared-state / sharding plan | Blast radius if it dies |
|---|---|---|
| `inbox_item` store (PG) | tenant-partitioned; the inbox read-model | one cell's inbox reads degrade; **rebuildable** from source (§3.8) — no permanent loss |
| `delivery` ledger (PG) | tenant-partitioned; idempotency via `idem_key` | in-flight deliveries pause; the `UNIQUE(idem_key)` prevents double-send on recovery |
| `notif_pref` / `quiet_hours` / templates / on-call (PG) | tenant-partitioned; system of record (NOT derived) | routing degrades to defaults; gated by restore-verify (ADR-18) |
| `escalation_run` handles | PG handle + ADR-09 workflow holds durability | escalation timers survive (durable workflow) — no missed page on restart |
| Notif cache (Redis/Valkey) | NEVER source of truth (STOR-3); ranked-list + projection cache | cold cache → a slower first inbox read; no loss |
Everything else (the router pool, delivery workers, the humanise renderer) is **stateless and
replaceable**.

---

## 6. Failure modes + the drills owed (PROVE-IT)

Per the honesty rule (EI-01 P3; T-2/T-4), each failable property names the **quantified drill** that
proves it (Phase 5 executes; this enumerates the obligation). Each emits a **green artifact** when it
passes; until then the property is **claimed, not proven**.

| # | Property / failure mode | Drill (quantified gate) | Reads (telemetry) |
|---|---|---|---|
| **D-N1** | **Important things buried** (the core attention failure) | Replay a mixed week of events for a synthetic user; assert every `critical`/`direct` item ranks above every `fyi`, and **inbox-read-latency-to-first-important is within budget**. Gate: **0 critical items below an fyi; explain-trace present for each rank**. | `important_buried_rate`, `inbox_read_latency` |
| **D-N2** | **Notification storm overwhelms a user** (UC-EDGE-4) | Fire 1000 near-identical CI failures + a 30-comment PR burst; assert they collapse to bounded items (`coalesce_count` correct, "+N more"), self-notifications suppressed. Gate: **N identical → 1 item; 0 self-notifications**. | `dedup_collapse_ratio` |
| **D-N3** | **Inbox read-model lost** (NOTIF-3) | Wipe `inbox_item`; `reindex(notif)`; assert the rebuilt inbox **matches live** (same items, same read-state from the source-of-truth events). Gate: **cold == live**. | `reindex_parity` |
| **D-N4** | **Notification leaks content a recipient can't see** (the security/GDPR property) | Notify on a confidential issue / private channel to a viewer who lacks access; assert the humanised string is the **tombstone** ("a restricted issue"), the title never appears, and the item is suppressed if the *recipient* can't see the subject. Gate: **0 title/PII leak; tombstone rendered**. | leak-drill assertions |
| **D-N5** | **30× agent-surge starves the human inbox** (ADR-16) | 30× agent-generated notification surge on one tenant; assert the **human inbox-read lane holds** (latency in budget), the agent-generated lane sheds, **other tenants unaffected**, delivery-adapter bulkhead bounds provider load. Gate: **human latency within budget; cross-tenant unaffected**. | per-tenant in-flight, shed counters, bulkhead rejections |
| **D-N6** | **Erased user still appears in inboxes** (ADR-12 / EI-04 §1) | Erase a user; assert their appearance in every existing inbox item humanises to `[erased user]` (refs resolve to tombstone) with **no stored PII recoverable** and any off-cell-sent payload crypto-shredded/erasure-requested. Gate: **0 recoverable PII; tombstone everywhere**. | holder erase receipts |
| **D-N7** | **Escalation missed / double-paged across a restart** (§3.7) | Start an escalation; kill Notif mid-`ack_window`; assert the durable workflow resumes, pages the **next** step exactly once (no miss, no double), and an ack stops the chain. Gate: **0 missed, 0 duplicate pages; ack stops chain**. | `escalation_ack_latency` |
| **D-N8** | **Quiet-hours over-suppress a page** | Set DND; fire a `critical` escalation; assert it **pierces** quiet-hours and delivers, while a `watching` item is correctly suppressed. Gate: **critical pierces; non-critical suppressed**. | `quiet_hours_pierce_count` |
| **D-N9** | **Double delivery** (channel idempotency) | Crash between provider-ack and ledger-write, then retry; assert the `UNIQUE(idem_key)` collapses it to **one** delivery. Gate: **exactly-one effective delivery per (item, channel)**. | `delivery_success`, dedup |
| **D-N10** | **Consumer head-of-line stall** (BUS-3) | Inject a slow/poison Signal type; assert the whitelisted-template router does not stall, terminates poison, lag-alarm fires. Gate: **lag bounded; no silent stall**. | `consumer_lag` |

---

## 7. Cited prior art (notification / feed-fanout literature)

- **Fan-out on write vs read; the celebrity problem.** A. Silberstein, J. Terrace, B. F. Cooper, R.
  Ramakrishnan, *Feeding Frontier: Pushing Data Centrically to the Edge* / Yahoo PNUTS feed work — and
  the canonical **fan-out-on-write vs fan-out-on-read** trade-off (push vs pull timelines). R. Krishnan
  et al., Twitter engineering, *Timelines at Scale* (the @-mention write-fanout vs home-timeline
  read-fanout split) — the literal basis for §3.5. Facebook **TAO** (Bronson et al., USENIX ATC 2013) —
  the read-optimised social-graph cache behind ambient/read-fanout. Instagram/Pinterest "fanout" writeups
  — hybrid push/pull. These ground the §3.5 hybrid: **write-fanout the bounded high-signal set,
  read-fanout the unbounded ambient set.**
- **Feed ranking.** Facebook **EdgeRank** (affinity × edge-weight × time-decay) and successors — the
  structure of the §3.1 deterministic score. Google **Gmail/Inbox "Importance"** classifier (Aberdeen et
  al., *The Learning Behind Gmail Priority Inbox*, 2010) — the precedent for an importance model **and**
  the reason we ship deterministic-first (the classifier was the named follow-on, not the floor).
- **Effectively-once delivery + idempotency.** P. Helland, *Idempotence Is Not a Medical Condition* (ACM
  Queue 2012) — at-least-once + idempotent (the `idem_key`/`dedup_key` UPSERTs) ≈ effectively-once.
  M. Kleppmann, *DDIA* (2017) ch. 11 — derived read-models from a log, change capture (the inbox as a bus
  projection). J. Kreps, *The Log* (2013) — the inbox is a projection, the bus is the source of truth
  (§3.2, §3.8).
- **Storm-control / rate-limiting.** Token-bucket / leaky-bucket rate limiting (the §3.2.4 damping);
  Google **SRE** ch. 21/22 (overload, graceful degradation) — the backpressure posture (§5.2).
- **Durable escalation / on-call.** PagerDuty's escalation-policy + on-call-rotation model (the §2.4 data
  model). Temporal / durable-execution literature (Cadence/Temporal design) — durable timers + signals
  for resumable, restart-surviving escalation and snooze (§3.7; ADR-09).
- **Humanisation / i18n.** Unicode **ICU MessageFormat** (plurals/gender/locale ordering) — the §2.5
  template format; the "humanise at the source paired with a routable ref" mandate (EI-05 §6).
- **Doctrine.** EI-02 §1 (tenant-first), §4 (outbox/effectively-once), §5 (backpressure), §6 (causality/
  "why it fired"), §10 (fail-static); EI-04 §1 (erasure vs immutability), §5.3 (reindex-from-source);
  **EI-05 §4** (system assembles context / why-it-fired), **§6** (humanise at the backend).

---

## 8. Required changes to foundational systems (if any)

Notif is written to consume the foundational contracts **as already specified** — it requires **no
breaking change** to the substrate, Bus, Id, or Refs. The following are **confirmations / small
additive obligations** the foundational docs already anticipate:

1. **Bus — a curated default Signal set for "what needs me" reasons.** Notif depends on Signals classed
   by the §3.1 `reason`s (`mentioned`/`assigned`/`review_requested`/`sla`/`agent_proposal`/…). The Bus
   already exposes `define_signal_rule` (Bus §5.4) and flags "the default Signal rule set" as a **P4 open
   question** (Bus §10.4). **No change**; Notif is the primary author of that default set — recorded here
   as the dependency. (Additive: the Bus's `mention(Principal)` → Signal mapping must exist so write-
   fanout has a Signal to consume; this is within the Bus's existing taxonomy, §6.3.)
2. **Refs — `resolve` must accept a `Display` mode that returns the humanisation projection.** Refs §5
   already exposes `resolve(ref, viewer, mode) → Projection|Tombstone` returning
   `{title, state, icon, render_hint, sub_anchor?}` — exactly what §3.3 binds. **No change**; confirmed
   sufficient. (Confirmation: the projection's `title` is the human display string; the tombstone is the
   "restricted/erased" form. Both already specified — Refs §4.2.)
3. **Id — `list_subjects(subject, watch)` for read-fanout watcher resolution.** Id §8.3 exposes
   `list_subjects(object, permission) → SubjectTree`. Notif's read-fanout (§3.5) needs a `watch`
   permission/relation per watchable subject — subsystems **declare** the `watcher` relation (Id §5
   already shows `watcher` on `issue`); **additive obligation handed to P4 subsystems**: declare a
   `watcher` relation on every watchable artifact type. **No Id-engine change.**
4. **Durable-workflow engine (ADR-09) — escalation/snooze/SLA timers.** Notif invokes durable timers +
   signals (§3.7). ADR-09 is *directional* (build-vs-adopt is a P3/P4 call); Notif's requirement (durable
   timer + signal-wait + resume-on-restart) is **within ADR-09's committed semantics**. **No change**;
   recorded as a hard consumer of that substrate.

The one **new platform-level obligation Notif introduces** (additive, not breaking): **every subsystem's
event taxonomy (Bus §10.1, a P4 deliverable) must declare which of its events carry a `mention(Principal)`
node or map to a notify-`reason`**, so Notif's router has a complete, validated set of write-fanout
producers. This is a *checklist item on the P4 subsystem taxonomy deliverable*, not a contract change.

---

## 9. Open questions for Phase 4

1. **The default Signal/notify-reason rule set + admin authoring UX** (with Bus §10.4): which events are
   `direct` vs `ambient` vs `fyi` by default, per subsystem; the Zapier-class rule builder over the AST.
   Product/UX-shaped → P4 + design language.
2. **The concrete EU-sovereign delivery providers** (the §3.6 FLOOR's follow-on): which EU-hosted email/
   push vendor(s), the DPA/sub-processor posture, the **provider-side erasure** mechanism for an already-
   sent off-cell payload (D-N6's residual). Sovereignty/legal call → P4 + DPO.
3. **Cross-cell inbox aggregation** for multi-cell tenants (the §5.4 FLOOR) — the control-plane pointer-
   bridge aggregation + local-only humanisation, residency-proven. → P4 control plane (SC-2/SC-3).
4. **ML-tuned ranking promotion** (the §3.1 follow-on): the measured "important-buried" threshold that
   triggers it; the ranker slots behind the same scoring interface (strategy pattern). → P4/P5, named
   promotion trigger (R-5).
5. **Per-subsystem `watcher` relation + notify-reason declaration** (§8.3/§8): each subsystem (Git/CI/
   Issues/Knowledge/Chat) declares its watchable types and which events map to which reasons, validated
   against §3.1. → P4 subsystems.
6. **Digest cadence + batching UX** (§2.2 `digest`): the daily/weekly digest's compose/dedup rules and
   the "snooze to digest" flow — product/UX → P4 + design language.
7. **Push-token lifecycle + multi-device** (web/mobile/desktop): device registration, token rotation,
   per-device routing, and "delivered on one device → seen everywhere" — the C-9 read-state truth extended
   to devices. → P4 (Notif + the apps).
8. **Inbox-`watch` live transport** (CA-8): the `inbox watch` / web-push streaming mechanics over the
   firehose split (long-poll vs SSE vs WebSocket) — co-decided with the Chat connection tier (TE-21). → P4.

---

## 10. Cross-references

- Foundational Phase-3 docs CONSUMED: [`00-platform-substrate.md`](./00-platform-substrate.md) (consumer
  template §5, `serve`, fail-static §8, telemetry §10), [`event-bus.md`](./event-bus.md) (Signals §3.4/
  §4.4, outbox, firehose split §4.3, reindex §4.9), [`identity-and-access.md`](./identity-and-access.md)
  (`check`/`list_objects`/`list_subjects` §8, Principal §3), [`reference-graph.md`](./reference-graph.md)
  (`resolve`→Projection/Tombstone §5, per-subsystem `project`).
- Spine: ADR-12 (PersonalDataHolder — Notif is the "notification history" holder), ADR-04/ADR-19 (bus +
  Signals), ADR-13 (`ArtifactRef`/envelope), ADR-03/ADR-17 (authz/fail-static), ADR-09 (durable-workflow),
  ADR-11 (cells), ADR-16 (backpressure), ADR-05 (`mention` node).
- Directives: **NOTIF-1** (backend humanisation), **NOTIF-2** ("why it fired" provenance), **NOTIF-3**
  (reindex-from-source); X-1…X-5, BUS-2/3/4, STOR-1/3, ID-1/3.
- Doctrine: EI-02 §1/§4/§5/§6/§10; EI-04 §1/§5.3; **EI-05 §4/§6** (the most-relevant doc).
- Resolves: **C-9** (the one inbox; "My Work"/"Activity" are scoped views), closes the Phase-3 README §4
  "Notifications has no dedicated Phase-3 doc" gap. CLI per **C-4** (`inbox` vs `notify` intent-split).
- Consistency note: the prior partial coverage in
  [`shared-systems-overview.md`](../02-holistic-architecture/shared-systems-overview.md) §5 and the
  [`README.md`](./README.md) §4 gap note are now superseded by this detailed design (code/doc-truth: this
  doc is the canonical Notif spec; the overview §5 is the springboard it elaborates).
```
