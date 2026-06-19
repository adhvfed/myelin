# Chat — Subsystem Roadmap (Phase 6)

> Phase: `06-roadmaps/subsystems`. The detailed, sequenced build roadmap for the **chat** subsystem.
> Slots into the master sequencing bands M0..M6 ([`../00-master-sequencing.md`](../00-master-sequencing.md)) —
> it refines the work *inside* the bands and must not contradict the band ordering or the gate invariant.
> Frozen architecture (this roadmap sequences, it does not redesign):
> [`../../04-subsystem-architectures/chat/architecture/`](../../04-subsystem-architectures/chat/architecture/)
> (00..07) + [`../../04-subsystem-architectures/chat/design/`](../../04-subsystem-architectures/chat/design/)
> (information-architecture, user-flows, wireframes — PRESERVED from Phase 4).
> Build-to contracts: [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md).
> Drills: [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> (CHAT-D1..CHAT-D19 + the shared families + the E2E wedge). Binding doctrine: EI-01 (order-by-non-negotiability,
> prove-it, the ratchet, name-your-floors) + EI-04 (real-time collab transport, erasure-vs-immutability,
> reindex-from-source). Plain-text identifiers (no backticks-as-emphasis). Markdown only; no commits. Date: 2026-06-19.

---

## 0. Where chat lives in the master sequence

Chat is a **consumer subsystem** (master §2 M4, §3.2) — and the **maximal consumer**: it unfurls everything,
references everything, and is the most visible surface of the agent-native principle (arch 00 §2). Its bulk of
build is **M4**, the last subsystem band, because it consumes the producers' artifacts — Git commits/PRs,
Knowledge docs, Issues, CI runs — and unfurls them per-viewer. It is **not** on the single longest critical-path
spine (that is harness → Identity → agent-fabric/AG-D4 → Git → CI → X-1 seam → dogfood), but it sits one hop off
it: every E2E flagship (E2E-2 CI-fail → triage agent → issue → chat → fix-PR) terminates in chat, so chat must be
green before the M5 whole-system wedge.

Chat **participates earlier** than M4 in two disciplined ways, per master §2 ("within a band, the per-system
roadmaps parallelise the work") and arch 00 §0:

- In **M2** chat freezes/declares its contract-facing surfaces so dependents and the shared layer compile and so
  the firehose resume-cursor transport — chat's correctness backbone — is co-designed once (it is shared with
  Knowledge collab and Issues boards, contract 3.5). Chat declares its ReBAC fragment, its `chat.*` event tokens,
  its `humanise` keys, its `define_notif_rule` set, and its `declare_indexable` spec in M2 so Identity/Notif/Search
  schemas include them.
- Its **world-scale / hard-problem follow-ons** (mega-channel home-node sharding, ScyllaDB hot tier, cross-org
  channels, comment-threading consolidation) are explicitly scheduled into **M5** (or post-M5 / LEGAL), and the
  switch test lands in **M6**.

The milestones below (M4-C1..M4-C9, plus M2 pre-work C0, plus M5/M6 follow-ons) are the chat-internal
decomposition of the M4 consumer work. The band a milestone belongs to is named on every milestone. **The gate
invariant binds**: no chat milestone is "done" over a red earlier-band gate (master §4) — in particular chat
writes no real message over a red STOR-D1 (M1 restore-verify) and runs no agent compute over a red AG-D4 (M2
sandbox-escape GATE).

---

## 1. The non-negotiability order applied to chat (what kills us first, inside chat)

Following EI-01 §2 and master §1, chat's internal work is ordered by what is catastrophic, not by feature size.
Chat is **the most PII-dense holder in Myelin** (arch 05 §5: a chat body *is* the personal data, not a reference
to it) and the **densest unfurl producer**, so its top failure classes are silent message loss, PII leak through
unfurls/search, and erasure that misses a holder.

1. **Silent message loss / dual-write** — a message persisted without its `chat.message.created` event, or an
   event without the message, or a message lost across a gateway reconnect. This is the Tier-1 floor *for chat*:
   (a) the message persist + outbox emit is ONE transaction (BUS-2), proven by **CHAT-D13**; (b) the live tier is
   allowed to drop ONLY because `resume(stream, scope, last_seq)` recovers the gap, proven by **CHAT-D1** — the
   zero-loss-across-reconnect drill is chat's defining correctness gate (EI-04 §2.2: build the durable resume-cursor
   transport FIRST). These come before any UI, any live delivery, any feature.
2. **PII leak through the unfurl / search surfaces** — a confidential artifact whose title leaks to a viewer
   lacking access (the "subtlety that separates a real implementation from a demo", arch 05 §4), or a search that
   returns messages from channels you are not in. Proven by **CHAT-D5** (unfurl no-leak), **CHAT-D11** (search ACL
   filter + the `search-requires-acl-filter` lint), and **CHAT-D6** (unfurl erasure-safe — a card never freezes a
   third party's PII into a durable snapshot).
3. **Erasure that misses a Chat holder** — chat is the canonical GD-4 crypto-shred case; a missed store leaves
   recoverable PII. Proven by **CHAT-D8** (erase a person → bodies crypto-shred in hot + cold + backups; mentions
   → `[erased user]`; read-state/drafts/unfurl-cache purged; Search incl. embeddings / Refs / Notif cascade → 0
   recoverable PII).
4. **HITL approval correctness** — a gated agent effect that mutates before approval, or runs twice across a
   kill, or a partial approval that is ill-defined. Proven by **CHAT-D9** (exactly-once across a multi-day kill,
   `idem_key=card_id`) and **CHAT-D10** (per-effect idempotency, `idem_key=card_id:<idx>`).
5. **Explicit-first dispatch** — a casual `@agent` mention must NOT auto-spawn a costed run (CHAT-1; the
   cost-abuse + human-oversight floor). Proven by **CHAT-D17**.
6. Then the breadth (composer, threads, reactions, presence, Activity-as-view, the 13 screens), then the
   world-scale hardening (30× agent surge, deploy-herd, mega-channel home-node), then the switch-test polish.

**Sandbox escape (Tier 2) is NOT owned by chat** — it is the shared AG-D4 / CI-T1 gate (master M2). Chat inherits
it by construction (the four uniform sandbox guarantees, contract 8.4 / X-6) and **must not run any agent compute
tool until AG-D4 is green**. Chat mutations route through `EffectApi` (plan-then-apply), never `ToolHands::exec`;
that routing split is itself the safety boundary (arch 03 §8).

---

## 2. Upstream dependencies (what must exist + be green before chat work starts)

Chat is a careful consumer of the reconciled shared layer plus four owned hot parts (arch 00 §2.1). It **reads no
other subsystem's store** (`no-cross-db`, ADR-01) and interacts only through frozen contracts. The table names,
per chat milestone, the contracts that must already be implemented + green. Critical ones are starred.

| Upstream (contract) | Owner / band | Why chat needs it | Blocks chat milestone |
|---|---|---|---|
| serve(AppSpec), three-surface, liveness≠readiness (1.1–1.3) | substrate / M0 | every chat service boots from it; the gateway readiness-gates reconnects | all |
| OutboxTx::emit + outbox table per-aggregate (conversation_id) + EventHandler whitelisted subjects + consumer_dedup (2.2–2.5) | Bus / M0 | the message persist → outbox co-commit; the only emit path (no-raw-publish); idempotent consumers | M4-C1 (★) |
| EventEnvelope frozen (2.1) | Bus / M0 | the `chat.*` event shapes + causation/correlation align to it | M2-C0 (★) |
| The 12 lints incl. no-raw-publish, tenant-predicate, no-cross-db, residency-pin, search-requires-acl-filter, no-untagged-personal-data, no-host-exec (1.6) | substrate / M0 | chat compiles against the ratchet | all |
| ResilientClient + FailStatic + the protected-human-lane shed order (1.9/1.10/1.11) | substrate / M0 | chat→Refs/Id/Notif calls degrade not cascade; the unfurl resilient-degradation; the per-surface shed budgets | M4-C4, M4-C2 |
| Cross-language harness shim frozen (1.7) | substrate / M0 | bounds the TE-21 BEAM hatch (a no-op in the all-Rust default) | M4-C2 |
| **Identity: authenticate (incl. machine-identity for `--as ci-bot` / agent tokens) (4.1)** | Id / M1 | the gateway resolves a Principal; tenant from token never path (ID-3) | M4-C2 (★) |
| **Identity: check + CaveatContext (4.2)** | Id / M1 | per-action send/edit/membership gate; the HITL approve gate (`check(human, approve, run)`) | M4-C1, M4-C6 (★) |
| **Identity: list_objects SetExpr push-down (4.3)** | Id / M1 | leak-free, no-N+1 conversation list + the unfurl membership-class precompute + the Search ACL conjoin | M4-C4, M4-C7 (★) |
| Identity: list_subjects against the authz reverse index (4.4) | Id / M1 | read-fanout watcher resolution at 50k-member density | M4-C5 |
| Identity: write_tuples/zookie (4.6/4.10) | Id / M1 | membership → ReBAC tuple writes in the same tx; the new-enemy zookie guard | M4-C1 |
| Identity: mint_run_token / revoke (4.7) | Id / M1 | per-run token for agent posts; re-mint on HITL resume | M4-C6, M4-C9 |
| **Identity: resolve_pseudonym / erase (4.8)** | Id / M1 | structured `mention(Principal)` pseudonym-map shred → `[erased user]` | M4-C8 (★) |
| Identity: ReBAC engine accepting the Chat fragment (4.9) | Id / M1 | `channel.read = member + parent_project->read` + the `watcher` relation | M2-C0, M4-C1 |
| Tenancy: (tenant, region) partition (12.1); placement/residency_verify (12.4); isolation-tier (12.5) | Tenancy / M1 | the message store + gateway + cold segments are region-pinned; home-cell routing | M4-C1 (★) |
| Tenancy: cross-cell PII-free pointer bridge frame (12.6) | Tenancy / M1 (frame) | the cross-org channel follow-on rides it; non-foreclosure of the Conversation model | M5-C-X1 |
| **Storage: OLTP tier + RLS + encrypted columns + the outbox (11.1)** | Storage / M1 | the message log + read-state durable record + conversation/membership rows | M4-C1 (★) |
| Storage: BlobStore content-addressed, fs-backed floor (11.2) | Storage / M1 | the cold-tier message segments | M4-C1 |
| **Storage: KMS hierarchy + per-subject DEK (11.3/11.4)** | Storage / M1 | crypto-shred for bodies/drafts — chat is the canonical GD-4 case | M4-C1 (★), M4-C8 |
| **Storage: backup/restore + restore-verify, RPO ≤ 5min / RTO ≤ 1h-tenant (11.5, STOR-D1/D2)** | Storage / M1 | the silent-data-loss floor; chat writes no real message over a red STOR-D1 | M4-C1 (★) |
| Storage: reserve/settle cost gate (11.7) | Storage / M1 | fronts every agent post; no balance → no run | M4-C6, M4-C9 |
| GDPR: PersonalDataHolder trait + harness auto-registration (10.1/1.4); classify-derive + the no-untagged-personal-data lint (10.2); erasure ledger (10.8) | GDPR / M1 | chat auto-registers every store as a holder; PII fields tagged | M4-C1, M4-C8 (★) |
| GDPR: the ONE erasure posture (10.9) | GDPR / M1→M5 | chat's free-text third-party residual handled BY REFERENCE (X-7) | M4-C8 |
| **Bus: firehose transport + the resume-cursor subscription protocol (3.5)** | Bus (seam) / M2 | live delivery / presence / typing / streaming; the zero-loss-across-reconnect backbone | M4-C2 (★) |
| Bus: define_signal_rule / register_automation / arm_trigger (3.1–3.3) | Bus / M2 | the explicit-first structured-trigger dispatch path | M4-C6 |
| **Refs: resolve(ref, viewer, mode) → Projection \| Tombstone (5.2); the 4-step tombstone ladder (5.7); project REQUIRED (5.6); refs.edge.created (5.4)** | Refs / M2 | the per-viewer permission-aware unfurl chokepoint; chat is the densest edge producer | M4-C4 (★) |
| Search: query/semantic always conjoining the list_objects Filter (6.1/6.2); declare_indexable (6.3); reindex (6.4) | Search / M2 | ACL-filtered message search; embeddings-as-PII erasure | M4-C7 |
| Notif: list_inbox (7.1); read-state truth (7.2); humanise the ONE templating surface (7.3); define_notif_rule (7.6) | Notif / M2 | Activity-as-view (never a 2nd store, C-9); the card/agent-message strings; the notify-reason rules | M4-C5, M4-C6 |
| **Agent: ToolSurface::register_tool + frozen requires_approval defaults (8.1, X-6); EffectApi::apply plan-then-apply (8.2); AgentRuntime::step --use-mock (8.3); explicit-first dispatch (8.6, CHAT-1)** | Agent / M2 | the chat ToolDef set; agent posts; mock-provable streaming; explicit-first | M4-C6, M4-C9 (★) |
| **Agent: ToolHands::exec unified sandbox + AG-D4 green (8.4, X-6/ADR-20)** | Agent / M2 | any agent compute chat triggers inherits the four uniform guarantees; NO agent compute over a red AG-D4 | M4-C9 (★) |
| **Workflow: DurableExecutor::signal + per-effect idem_key + durable signal multi-day HITL (9.1/9.4, OQ-F)** | Workflow / M2 | the HITL approve→resume bridge; chat owns the card, not the wait/timer/budget | M4-C6 (★) |
| myelin-content taxonomy frozen + WASM render core (13.1, X-2) | Knowledge (leads) / M2 | the composer + message body = the frozen Chat subset; `render(parse(md)) === md` | M4-C3 (★) |
| **Knowledge: docs/pages exist + project(ref, viewer) (M3)** | Knowledge / M3 | unfurl/embed a `knowledge/page`; the pinned canvas | M4-C4 |
| **Git: commits/PRs + project(ref, viewer) (M3)** | Git / M3 | unfurl a git PR/commit | M4-C4 |
| **CI: ci.check.updated emitted (5.9, X-1) (M4, CI lands first in M4)** | CI / M4 | bust a PR/commit unfurl card on a check change (CHAT-D7); chat is a consumer-for-invalidation only | M4-C4 |
| Issues: issues + project(ref, viewer) (M4, parallel in M4) | Issues / M4 | unfurl an issue | M4-C4 |

**The two permanent gates chat inherits** (master §4): **STOR-D1/D2** (restore-verify — re-runs on every change
touching the message store) and **AG-D4 / CI-T1** (sandbox escape — re-runs on every backend/image/kernel change;
gates any agent compute chat dispatches). Neither is chat-owned; both bound chat's "done".

---

## 3. The contracts chat must implement, and by which milestone

From the frozen contract-index. Chat is a consumer of most contracts; the rows below are the ones chat **owns the
implementation of** (its half of the glue, ADR-13) — the "implement by" column is the chat milestone that ships
it. Pure-consumer contracts (it only *calls* them) are covered by the dependency table §2.

| Contract (index #) | What chat implements | Implement by |
|---|---|---|
| The `chat.*` event taxonomy (2.9 grammar; arch 03 §1) — durable-via-outbox vs firehose-only split | the complete dotted-name list; per-aggregate `conversation_id` ordering | M2-C0 (declare) → M4-C1 (durable set) / M4-C2 (firehose set) |
| OutboxTx::emit co-commit (2.2, BUS-2) | every state change commits its row + its event in one tx; the gateway has NO emit path of its own | M4-C1 |
| ReBAC namespace fragment (4.9) | `channel.read = member + parent_project->read`; the `watcher` relation; membership → write_tuples → zookie | M2-C0 (declare) → M4-C1 (writes) |
| ArtifactRef mint + the frozen `#sub` grammar (5.1/5.7) | `myelin://<tenant>/chat/{channel,message,thread}/<id>`; mint `message-<id>` / `thread-<root>` from the frozen vocabulary; immutable stable opaque ULIDs; stability obligation is chat's | M2-C0 (grammar) → M4-C1 (mint) |
| project(ref, viewer) (5.6) | per-viewer pre-permission-checked projection for chat/{channel,message,thread} → Projection \| Tombstone (never the body) | M4-C4 |
| replay(scope, since) reindex-from-source (2.6) | re-emit `chat.*.snapshot` through the outbox via the live consumer; sub-artifact granular; erased subjects → tombstones | M4-C1 (skeleton) → M4-C7 (full, with Search/Refs parity) |
| declare_indexable(IndexSpec) (6.3) + the ACL conjoin (6.1) | the `chat`/`message` index spec; Search always conjoins the frozen list_objects Filter over `message.id` | M4-C7 |
| PersonalDataHolder over every chat store (10.1) + crypto-shred (11.4) + restriction flag (Art.18) | locate/export/rectify/restrict/erase; per-subject DEK shred for bodies/drafts; pseudonym-map shred for mentions; residual per 10.9 BY REFERENCE | M4-C8 |
| ToolDef registrations + frozen requires_approval defaults (8.1, X-6) | the chat tool set (post/reply/react/start_dm = no; create_channel/invite/archive = yes; cross-subsystem inherits target) routed through EffectApi | M4-C6 |
| define_notif_rule + the fanout-class declaration (7.6; arch 03 §4) | mentioned/replied/thread_watched/approval_requested reasons; write-fanout vs read-fanout class per event | M2-C0 (declare) → M4-C5 (wire) |
| humanise template keys (7.3) | card strings, agent-message strings, `chat.message.mentioned` — no chat-private string map | M2-C0 (register) → M4-C5/M4-C6 (use) |
| The HITL bridge DurableExecutor::signal + per-effect idem_key (9.1/9.4, OQ-F) | the withhold→approve→resume card; `idem_key=card_id` single / `card_id:<idx>` multi | M4-C6 |
| Per-surface shed budgets (1.11, OQ-K) | connection-storm + agent-mention-storm caps + the reserved human lane; concrete numbers are chat's P6 call | M4-C2 (floor) → M5 (tuned by CHAT-D3/D4) |
| Cross-language harness shim (1.7) | a no-op in the all-Rust default; the contract the BEAM hatch would satisfy | M4-C2 (no-op) |
| Cross-cell pointer bridge consumption (12.6) | cross-org channels ride it; per-viewer resolution always cell-local | M5-C-X1 (designed-not-built until then) |

---

## 4. The milestones (mapped to bands, with floor-then-full progression)

Each milestone names its **band**, its **work**, its **entry dependency**, its **exit gate** (the quantified
drills that call it done), and — where relevant — its **floor + named follow-on**. The "first runnable / first
useful / production-hardened" line is drawn explicitly in §6.

### M2-C0 — Chat declares its contract surfaces (M2, the parallel pre-work)

**Band:** M2 (the reactive shared layer). This is the small slice of chat that ships *inside* M2 so the shared
layer and dependents compile, and so the firehose transport (chat's correctness backbone) is co-designed once
across Chat/Knowledge/Issues (contract 3.5 is shared, not chat-private).

**Work:**
- Declare the `chat.*` event taxonomy (the durable-vs-firehose split, arch 03 §1) and freeze the `chat`
  subsystem token + `channel`/`message`/`thread` types in the Bus §6 grammar token table.
- Declare the Chat ReBAC namespace fragment (`channel.read = member + parent_project->read`, the `watcher`
  relation) into Identity's engine (4.9) so the cell schema compiles.
- Freeze the `#sub` grammar contribution (`message-<id>`, `thread-<root>`) into the Refs frozen vocabulary (5.7).
- Register the `humanise` template keys (7.3) and the `define_notif_rule` set (7.6: mentioned / replied /
  thread_watched / approval_requested) so Notif's schema includes them.
- Co-design + validate the firehose resume-cursor protocol (3.5) against chat's `scope = channel:<id>` bounding —
  the per-view scope shape, the `resync_required → *.snapshot` fallback contract.
- Pin the connection-tier language call (TE-21): Rust default; the BEAM hatch written-but-closed, bounded by the
  frozen harness shim (1.7).

**Entry dependency:** M1 green (Identity engine, Tenancy, Storage durability) + the M2 shared-layer crates
(Refs/Search/Notif/Agent/Workflow/Bus-firehose) being built in the same band.

**Exit gate:** the contract-coverage scanner passes for every chat-owned contract row (provider + consumer CDC
coverage); the chat ReBAC fragment compiles into the cell schema; the `chat.*` tokens are frozen. No chat runtime
drill yet — this is declarations, not behaviour. (Honest floor: M2-C0 ships *contracts*, not a working chat; the
behaviour lands in M4.)

### M4-C1 — The durable message store + the outbox co-commit (M4, item 0)

**Band:** M4. The silent-data-loss floor *for chat* — built before any live delivery, any UI, any feature
(arch 00 §4 build-order law item 1).

**Work:**
- The Message Service behind the `MessageStore` trait (`append`/`range`/`tombstone`/`resync_from`); Postgres-
  partitioned hot tier by `(tenant, region)` + time sub-partitions; the object-segment cold tier (fs-backed
  BlobStore floor) — identical under either hot engine.
- The message persist + the `chat.message.created` outbox row in ONE transaction (BUS-2; no dual-write).
- k-sortable ULID `message_id` for intrinsic per-conversation order; `aggregate = conversation_id` with the
  outbox `UNIQUE(aggregate, seq)`.
- Idempotent send: `UNIQUE(conv, client_nonce)`.
- The Conversation / Membership Service: one Conversation entity + `kind`s; membership → `write_tuples` →
  zookie stamped on the conversation, in the same tx as the membership row + the `chat.channel.member_*` event.
- Per-subject DEK encryption of `body_inline`/`body_nodes` + drafts (chat bodies ARE PII, contract 11.4).
- The `#sub` minting (`message-<id>`, `thread-<root>`), stable across edits.
- PersonalDataHolder auto-registration over every store; PII fields tagged `#[personal_data(...)]`.
- `replay(scope, since)` skeleton (re-emit `chat.*.snapshot`; full parity proven in M4-C7).

**Entry dependency:** M1 green (Storage OLTP + restore-verify STOR-D1 green — chat writes no real message over a
red STOR-D1; KMS per-subject DEK; Tenancy partition) + M0 outbox/lints + M2-C0 declared.

**Exit gate:**
- **CHAT-D13** (crash between message persist and event emit → both committed or neither; no orphan/phantom) — CI.
- **CHAT-D14** (retry a send with the same `client_nonce` → one message) — CI.
- **CHAT-D2** (burst sends + edits to one hot channel from many gateways → per-conversation total order
  preserved; ULID + `aggregate=conversation_id`; out-of-order client ops reconcile) — SCHED.

**Floor + follow-on:** message hot tier = Postgres-partitioned (the `MessageStore` trait makes it a swap);
**ScyllaDB hot tier is the named M5 follow-on (R-C6/R-5)**, triggered by measured per-cell write/partition volume.
The cold tier + trait are identical either way.

### M4-C2 — The firehose resume-cursor transport + the connection-tier gateway (M4)

**Band:** M4. The zero-loss-across-reconnect correctness backbone (EI-04 §2.2: build the durable resume-cursor
transport FIRST; a relay without resume cursors silently loses the gap on a reconnect).

**Work:**
- The connection-tier gateway (Rust default, TE-21): WS/SSE termination; stateless (live sockets + presence +
  resume cursors only; NO durable store, NO outbox of its own — it calls the Message Service).
- `subscribe(stream, scope=channel:<id>, cursor?)` (bounded, never `*`; paginated for hot channels);
  `resume(stream, scope, last_seq)` recovering the gap `(last_seq, now]`; `resync_required → *.snapshot`
  (= `MessageStore::resync_from`) fallback when `last_seq` exceeds the retention window.
- Live message delivery, presence, typing, fine-grained read-state, streaming partials — firehose-only, never the
  durable bus (the `no-raw-publish` lint + the firehose seam keep them off structurally).
- The protected-human-lane shed order (ADR-16) + the per-surface shed budgets floor (OQ-K): speculative/presence
  shed first, message delivery last; humans never queue behind agent runs.
- The cross-language harness shim (1.7) satisfied as a no-op (Rust); the gateway speaks the Rust EventEnvelope on
  the wire regardless.

**Entry dependency:** M4-C1 (the durable store + outbox the firehose reads from) + M2 firehose transport (3.5)
green + substrate shed order (1.11).

**Exit gate:**
- **CHAT-D1** (sever the gateway↔firehose mid-publish → `resume` recovers the gap, 0 lost / 0 dup; `last_seq`
  past window → `resync_required → *.snapshot`, still 0 lost) — CI.
- **CHAT-D4** (roll the gateway fleet under a connection storm → bounded reconnect rate; `resume` completes for
  all; no message loss; readiness gates new connections, liveness no restart-storm) — SCHED. *(TE-21 build-gate
  drill — see §5.)*

**Floor + follow-on:** connection-tier language = Rust; the **BEAM/Phoenix hatch is written-but-closed**, bounded
by contract 1.7, opened only if D-C3/D-C4 prove Rust presence-at-scale intractable (a gateway-process swap, not a
platform rewrite). Mega-channel live delivery = firehose subject fan-out with per-view scope bounding; the
**channel-sharded home-node is the measured M5 escalation (R-5)**.

### M4-C3 — The composer + message content over the frozen myelin-content subset (M4)

**Band:** M4. Per VISION §3, no frontend code without a design sketch — the wireframes (S3 composer) are
PRESERVED from Phase 4 and are the build-to.

**Work:**
- The composer + `message` body over the frozen Chat subset of `myelin-content` (13.1): `paragraph,
  heading(1..3), bullet_list, ordered_list, task_list, blockquote, code_block, callout, table, divider, image` +
  the three inline nodes (`mention`/`artifact_ref`/`embed`); excludes `db_view, sync_block, toggle`.
- Reuse the WASM-compiled Rust content core (one editor render path; `render(parse(md)) === md`).
- Per-message CAS on edit (`edited_seq`) — NO collaborative-edit engine (chat is single-author-per-message; the
  CRDT is Knowledge's, not chat's).
- `/` slash menu, `@`-mention + `#`-artifact autocomplete (Search-backed), paste-URL→unfurl, draft persistence.

**Entry dependency:** M2 `myelin-content` frozen + WASM core (13.1) + M4-C1 (the message store the composer writes
to).

**Exit gate:**
- The `render(parse(md)) === md` round-trip holds 100% for the chat subset (the chat instance of KN-D2 / the
  content-core round-trip gate) — CI.
- Structured `mention`/`artifact_ref`/`embed` nodes parse to the frozen node shapes and produce
  `refs.edge.created` uniformly (5.4) — CI.

### M4-C4 — The unfurl service: cheap per-viewer permission-aware unfurls (M4, the wedge differentiator)

**Band:** M4. The platform differentiator (Phase-1 §2.4); the densest unfurl surface. Per arch 00 §4 it ships on
top of the Refs `resolve` chokepoint — chat never re-implements permission-aware resolution.

**Work:**
- The Unfurl Service: a shared, per-`ArtifactRef` projection cache (viewer-independent content), gated by a
  per-viewer `list_objects`/`check` (lowering the frozen `SetExpr` to a JOIN over the candidate id column) —
  ONE cache entry per ref, never per `(ref, viewer)`, with no leak.
- Lazy-on-viewport resolution (resolve only what is on screen); calls Refs `resolve(ref, viewer, mode)` over the
  one 4-step tombstone ladder (5.7).
- Membership-as-permission class precompute via the frozen `list_objects` Filter (one class decision, not N).
- Bus-driven invalidation on `*.updated` / `ci.check.updated` / `*.erased` pointer events (precise; TTL the
  backstop); viewers currently showing the card get a live firehose update.
- `project(ref, viewer)` for chat/{channel,message,thread} — the only way other subsystems read about a chat
  artifact (no cross-DB); per-viewer pre-permission-checked → Projection | Tombstone (never the body).
- Chat as the densest `refs.edge.created` producer (artifact-linked channels, embeds, mentions).

**Entry dependency:** M2 Refs (`resolve`, the ladder, `project`, `refs.edge.created`) green; Identity
`list_objects` SetExpr (4.3) green; **M3 Git + Knowledge producing the artifacts to unfurl**; CI emitting
`ci.check.updated` (5.9, lands in M4 — CI is sequenced first within M4 per master §2); Issues producing issues
(parallel in M4).

**Exit gate:**
- **CHAT-D5** (notify/unfurl a confidential artifact to a viewer lacking access → tombstone rendered, title never
  present — the 4-step ladder step 1) — CI.
- **CHAT-D6** (erase a third party rendered in a card → tombstone on next render, 0 recoverable PII; no durable
  snapshot; the cache re-resolves live → `erased`) — CI.
- **CHAT-D7** (an artifact's `ci.check.updated`/`*.updated` → the shared per-ref cache busts; viewers showing the
  card get a live firehose update within budget) — CI.
- **CHAT-D18** (edit a message referenced by another artifact → the `message-<id>` anchor stays stable/live;
  delete it → the embed degrades to a Tombstone carrying the root, never dangles) — CI.

### M4-C5 — The read-state hot path + Activity-as-view (M4)

**Band:** M4.

**Work:**
- The Read-state Service: Valkey hot markers + counters; batched eventually-consistent flush to the PG durable
  record (Valkey NEVER authoritative); unread derived as a bounded range read (`count(id > last_read)`), never
  write-fanned-out; firehose-only `chat.read_state.updated` events; a PersonalDataHolder.
- The fanout-class declaration (arch 03 §4): write-fanout the bounded high-signal set (mentions via the structured
  `mention(Principal)` node, DMs, thread-replies-to-you, HITL-for-you, keyword matches) → Signals → Notif;
  read-fanout the unbounded ambient set (channel/thread activity, unread) via the per-conversation log + lazy
  unread, watchers resolved by `list_subjects(channel, watcher)` (4.4). A 100k-member announcement does ZERO
  per-member inbox writes on a post (the celebrity-fanout mitigation).
- Activity / Mentions = a `list_inbox` filter (`subsystem ∈ {chat} ∧ reason ∈ {mentioned, replied,
  thread_watched, approval_requested}`) — NEVER a second store (C-9); one read-state truth, linked to chat's
  scroll-state at the mention.

**Entry dependency:** M2 Notif (`list_inbox`, read-state truth, `humanise`, `define_notif_rule`) green; Identity
`list_subjects` (4.4); M4-C1 (the conversation log unread derives from).

**Exit gate:**
- **CHAT-D12** (flush + drop Valkey mid-session → the PG record is authoritative; a marker is at-worst slightly
  stale; unread counts recompute correctly) — CI.

### M4-C6 — The HITL approval-card bridge + the agent ToolDef set (M4)

**Band:** M4. Chat's two named platform obligations (the HITL card; the agent-tool surface), on top of
`DurableExecutor::signal` + `EffectApi` — chat owns the *card*, not the wait/timer/budget/sandbox.

**Work:**
- The HITL Card Service: render the approval card (in thread + Notif inbox, C-9); gate the click with
  `Id.check(human, approve, run)` (4.2); post `DurableExecutor::signal(run, name, payload, idem_key)` with the
  frozen per-effect `idem_key` (`card_id` single / `card_id:<effect_idx>` multi); a declined effect is WITHHELD
  (one `EffectApi::apply` per approved effect); timeout auto-denies; resume runs under a freshly-minted attenuated
  token (4.7).
- The chat ToolDef set (8.1, frozen X-6 defaults): `chat.post`/`reply_in_thread`/`react`/`start_dm` =
  `requires_approval false`; `chat.create_channel`/`invite`/`archive_channel` = true; any cross-subsystem effect
  inherits the TARGET subsystem's default. All side-effecting tools route through `EffectApi` (plan-then-apply,
  reserves), NEVER `ToolHands::exec` (the routing split is the safety boundary).
- Reserve/settle on every spend-bearing agent post (11.7); chat surfaces cost (the card's live estimate) but never
  holds the wallet.
- `run --dry-run` (8.7) on chat tools returns ProposedEffects without applying.

**Entry dependency:** M2 Workflow (`DurableExecutor::signal` + per-effect idem_key, 9.1/9.4) green; Agent
(`EffectApi`, ToolSurface, the four guarantees) green; Storage reserve/settle (11.7); Notif `humanise`.

**Exit gate:**
- **CHAT-D9** (request approval, kill Chat + Workflow mid-wait, approve days later → the gated tool runs exactly
  once; double-click is one approval; deny withholds with no mutation; timeout auto-denies; resume under a fresh
  token) — CI.
- **CHAT-D10** (a multi-effect card approved 2-of-3 → the 2 resume approved, the 1 withheld, each independent
  `idem_key=card_id:<idx>`; no effect runs twice; the withheld never mutates) — CI.

### M4-C7 — Search indexing (ACL-filtered) + reindex-from-source parity (M4)

**Band:** M4.

**Work:**
- `declare_indexable(IndexSpec{subsystem:"chat", type:"message", ft_fields:["body"], struct_fields:[...],
  semantic: EmbeddingSpec, acl_object_type:"message"})` (6.3).
- Search ALWAYS conjoins the frozen `list_objects` Filter over the `message.id` column before scoring (the
  `search-requires-acl-filter` lint, 6.1) — the SetExpr lowers to a JOIN against Id's authz reverse index; no
  N+1, no post-filter.
- Embeddings-as-personal-data: on erasure, Search purges + reindexes embeddings (not just FT) — never hides; an
  HYOK tenant whose `can_derive_plaintext_index()=false` structurally skips message indexing (11.3).
- `replay(scope, since)` full parity: Search/Refs/Notif read-models rebuild from `chat.*.snapshot`; steady-state
  and recovery share ONE path (the outbox → consumer template); erased subjects emit tombstones (no PII
  resurrected).

**Entry dependency:** M2 Search (the ACL conjoin, `declare_indexable`, `reindex`) green; Identity `list_objects`
(4.3); M4-C1 (`replay` skeleton + the message source).

**Exit gate:**
- **CHAT-D11** (search as a non-member → 0 results from channels you're not in; the `search-requires-acl-filter`
  lint fails any query path reaching the index without the Filter conjoined) — CI.
- **CHAT-D15** (wipe + `replay(scope, since)` → Search/Refs/Notif read-models rebuild; steady-state and recovery
  share one path; erased subjects → tombstones; reindex-parity hash matches) — SCHED.

### M4-C8 — The erasure cascade across every chat holder (M4)

**Band:** M4. Chat is the most PII-dense holder + the canonical GD-4 crypto-shred case (arch 05 §5).

**Work:**
- The GDPR holder: `locate/export/rectify/restrict/erase` over every chat store.
- Author erasure: crypto-shred P's per-subject DEK → every body P authored unrecoverable in hot + cold segments +
  backups SIMULTANEOUSLY (without rewriting the immutable log); tombstone the record.
- Mentioned erasure: the structured `mention(Principal)` → pseudonym-map shred (4.8) → renders `[erased user]` on
  next render (free, because the node is structured + pseudonymous).
- The cascade reaches Search (incl. embeddings) / Refs / Notif via the bus + DSR (10.4), never a backdoor;
  read-state / drafts / unfurl-cache purged.
- The restriction flag (Art. 18) honoured at every read path: a restricted subject is excluded from indexing /
  agent-use / new notification routing / analytics (a distinct state from erasure).
- The free-text third-party residual handled BY REFERENCE to the ONE platform posture (10.9, X-7) — chat writes
  no fifth chat-specific residual statement; it supplies only the structural floor (per-subject DEK shred +
  pseudonym-map shred + `restrict`).

**Entry dependency:** M1 GDPR spine (holder trait, erasure ledger, the no-untagged-personal-data lint) + KMS
per-subject DEK (11.3/11.4) + Identity `resolve_pseudonym`/`erase` (4.8); M4-C1/C4/C5/C7 (the stores the cascade
reaches).

**Exit gate:**
- **CHAT-D8** (erase a person → bodies crypto-shred in hot + cold + backups; mentions → `[erased user]`;
  read-state/drafts/unfurl-cache purged; Search incl. embeddings / Refs / Notif cascade → 0 recoverable PII;
  holder receipts) — SCHED.

**Floor + follow-on:** the free-text third-party residual → **the ONE platform posture (10.9), `[OPEN — LEGAL]`,
ratified ONCE by counsel/DPO (R-C5)** — the structural floor ships regardless; the residual is one ratified
statement, parallel-tracked (LEGAL), not a chat blocker.

### M4-C9 — Agent presence, streaming + explicit-first dispatch (M4)

**Band:** M4. Built and proven against the mock runtime (`--use-mock`, VISION §3 — no real agents during
development).

**Work:**
- Agent presence classes (available/busy/rate-limited/offline) on the firehose; streaming partials
  (`agent.message.partial`) on the firehose, final replaces partial.
- Explicit-first dispatch (CHAT-1, 8.6): a casual `@agent` mention NOTIFIES the agent's inbox, does NOT spawn a
  costed run; only an explicit action / structured trigger dispatches; reserve/settle gates even the explicit run
  (no balance → no run). No auto-spawn path is wired (L-3, counsel-gated).
- The agent provenance popover (S12): "why did this agent post?" from `causation_id`/`correlation_id`/
  `on_behalf_of`.

**Entry dependency:** M2 Agent fabric (`AgentRuntime::step --use-mock`, explicit-first dispatch, `EffectApi`)
green; **AG-D4 green** (no agent compute over a red sandbox gate); reserve/settle (11.7).

**Exit gate:**
- **CHAT-D16** (drive the streaming UX against the mock runtime → partials stream; final replaces partial; a
  mid-stream reconnect `resume`s the final, never a half-message) — CI.
- **CHAT-D17** (a casual `@agent` mention → notifies the agent's inbox, does NOT spawn a costed run; only an
  explicit action / structured trigger dispatches; reserve/settle gates even the explicit run) — CI.

### M5 follow-ons — world-scale hardening + the named floor promotions

**Band:** M5. The floors named in M4 get their scheduled follow-ons; the 30× surge family runs; chat participates
in the whole-system E2E wedge.

- **M5-C-S1 — 30× agent-surge + deploy-herd (the F6 family).**
  - **CHAT-D3** (30× agent message/connection surge on one tenant → human connection/read latency in budget; the
    agent lane sheds 429 + `Retry-After` honoured; other tenants unaffected) — SCHED. *(TE-21 build-gate.)*
  - **CHAT-D4** re-run at scale (deploy-herd, see M4-C2) — SCHED.
  - Tune the per-surface shed budget numbers (R-C2/OQ-K) from the D-C3/D-C4 results.
- **M5-C-S2 — ScyllaDB hot-tier promotion (the named M4-C1 floor follow-on, R-C6/R-5).** Triggered by measured
  per-cell write/partition volume; a `MessageStore` trait swap (cold tier + trait identical); residency-pinned +
  crypto-shred-capable per cell. Re-run CHAT-D2 + CHAT-D8 across the swap.
- **M5-C-S3 — Mega-channel channel-sharded home-node (the named M4-C2 delivery floor, R-5).** Triggered by
  subscriber count exceeding the subject-fan-out budget; the Phoenix/Discord guild model in Rust +
  consistent-hash. Re-run CHAT-D1.
- **M5-C-X1 — Cross-org / federated channels (designed-not-built → on the frozen cross-cell bridge, R-C9).**
  Rides `CrossCellPointer` (12.6); per-viewer resolution always cell-local; multi-cell DSR iterates
  `member_cells` (10.4); needs an explicit cross-tenant capability + residency policy. **→ P6 control plane +
  LEGAL.** Not built unless the bridge ships in M5.
- **M5-C-X2 — Comment-threading consolidation (OQ-L named floor, R-C8).** When document-anchored comments
  (Knowledge/Issues) need real-time presence, promote them onto the Chat threading primitive + the firehose
  transport — a store/transport swap over the shared `#thread-`/`#comment-` `#sub` + content + refs scheme, NOT a
  rewrite. Tracked in the gap report (E-3).
- **The whole-system E2E wedge (chat's participation, testing-strategy §2):**
  - **E2E-1 PR context pane** — chat's unfurl/live-update analog (CHAT-D7) contributes the chat-discussion pane.
  - **E2E-2 CI-fail → triage agent → issue → chat → fix-PR** (the agent-native FLAGSHIP) — chat is the terminal
    surface: the explicit-first dispatch (CHAT-1), the HITL withhold→approve→apply card, the unfurl of the issue +
    fix-PR, all metered through one wallet. Chat must be green for E2E-2.
  - **E2E-4 DSAR fan-out** — chat's `CHAT-D8` erasure (bodies + drafts + mentions + embeddings) is a named
    holder in the 0-holders-missed certificate.

**Entry dependency:** M4 green (all five subsystems exist; the deterministic correctness drills green; the floors
in place to promote).

**Exit gate:** the F6 surge family green (CHAT-D3/D4); the named floor promotions drilled where triggered; the
E2E scenarios chat participates in (E2E-1/E2E-2/E2E-4) green.

### M6 — Dogfooding: the switch test (M6)

**Band:** M6. The team talks in Myelin's own Chat; the switch test is reached by driving the real UI in a browser
(EI-01 §4, the frontend done-bar).

**Work:**
- Drive the real Chat UI (the 13 screens S1–S13) in a browser for the switch test.
- The responsive cases chat owns (SUB-X): the hover-action case (never hover-only on touch), the width-takeover
  case (rail + nav collapse to drawers at the mobile breakpoint), the flip-popover case (the `@`/`#`/slash
  pickers flip above a bottom-pinned composer with a max-height — tested against the REAL anchor).

**Entry dependency:** M5 green (world-scale-ready; the E2E wedge proven; you do not dogfood real team data over a
red restore-verify or DSAR fan-out).

**Exit gate:**
- **CHAT-D19** (drive the real Chat UI → a team could move to it without hitting a wall the old tool didn't have;
  measured-contrast tokens; latency budgets — optimistic send < ~100ms perceived; flip-popovers against the real
  bottom-pinned composer anchor) — SCHED.

---

## 5. The world-scale / hard-problem work, scheduled explicitly (name-the-floor + name-the-follow-on)

Per VISION §3 + EI-04 §4: every floor named with its band, its follow-on band, and its trigger. This is chat's
slice of the master §5 floor table, with chat-internal milestone IDs.

| Floor (shipped) | Band | The full answer (follow-on) | Band | The trigger |
|---|---|---|---|---|
| **Per-message CAS on edit** (single-author; no merge) | M4 (M4-C3) | none — chat does NOT promote to CRDT (the CRDT is Knowledge's; chat messages are single-author) | n/a | n/a (the OQ-L consolidation is the related follow-on, below) |
| **Postgres-partitioned message hot tier** | M4 (M4-C1) | **ScyllaDB hot tier** (a `MessageStore` trait swap; cold tier identical) | M5 (M5-C-S2) | measured per-cell write/partition volume (R-C6/R-5) |
| **Firehose subject fan-out with per-view scope bounding** (mega-channel live delivery) | M4 (M4-C2) | **Channel-sharded home-node** (Phoenix/Discord guild model in Rust + consistent-hash) | M5 (M5-C-S3) | subscriber count exceeds the subject-fan-out budget (R-5) |
| **Connection-tier language = Rust** (the BEAM/Phoenix hatch written-but-closed, bounded by 1.7) | M4 (M4-C2) | **BEAM/Phoenix gateway** (a gateway-process swap, not a platform rewrite) | only if triggered | CHAT-D3/D4 prove Rust presence-at-scale / tail-latency intractable (the TE-21 build-gate) |
| **Single home-cell for a tenant's chat** (near-edge gateway, writes to home cell) | M4 | **Multi-region edge + cross-cell bridge** | M5 / on-demand | the cross-cell bridge (12.6) ships; cross-org demand |
| **fs-backed BlobStore cold segments** | M4 (M4-C1) | **Object-store BlobStore** (one-line swap, 11.2) | M5 | with the platform object-store promotion |
| **Mock agent runtime** (`--use-mock`, scripted-deterministic) | M4 (M4-C9) | **Real LlmAgentRuntime** (a config/impl swap, not a rewrite) | post-M5 / execution | after AG-D4/D2/D3/D5 green (VISION §3) |
| **Free-text third-party erasure residual** (structural floor: DEK shred + pseudonym-map shred + restrict) | M4 (M4-C8) | **Counsel/DPO ratification of the ONE platform residual posture** (10.9) | parallel (LEGAL) | the structural floor ships regardless; the residual is one ratified statement (R-C5) |
| **Cross-org / federated channels designed-not-built** (the Conversation model does not foreclose them) | M4 (frame) | **Cross-org channels live on the cross-cell bridge** (12.6) | M5 / P6+LEGAL | cross-org demand + the cross-tenant capability + multi-cell DSR (R-C9) |
| **Comment-threading consolidation named-not-built** (Chat owns conversation-threads; KN/Issues own anchored comment-threads, over ONE shared `#sub`/content/refs scheme) | M4 (shared scheme) | **Promote anchored comments onto the Chat threading primitive + firehose transport** (a store/transport swap) | M5 / on-demand | anchored comments need real-time multi-party presence (OQ-L, R-C8, gap report E-3) |
| **Per-surface shed budgets** (connection-storm + agent-mention-storm caps + reserved human lane, named-as-floor numbers) | M4 (M4-C2) | **Tuned shed budget numbers** | M5 (M5-C-S1) | CHAT-D3/D4 measured (R-C2/OQ-K) |
| **Canvas = embedded/pinned Knowledge page** (`ArtifactRef`, not a Chat editor) | M4 (M4-C4) | the joint Chat↔Knowledge review of the pin/embed mechanism (the lean is firm: embed, not editor) | M4/M5 | R-C7, joint product |

**Tunables (measured-not-predicted, carried to execution, R-C1..R-C4):** the firehose retention-window size for
`fan.<tenant>.<channel>` (R-C1); the per-surface shed budget numbers (R-C2); the read-state batched-flush cadence
+ the `Notif.mark(item, read)` trigger (R-C3); the unfurl projection-cache TTL + the membership-class refresh
cadence (R-C4). None block a milestone; each is asserted by its drill and tuned against telemetry.

---

## 6. The honest first-runnable / first-useful / production-hardened progression

- **First runnable (end of M4-C2).** A message persists with its event in one transaction (CHAT-D13) and is
  delivered live over the firehose with zero loss across a reconnect (CHAT-D1). You can send a message in one
  channel and another client receives it; a dropped connection loses nothing. There is no composer richness, no
  unfurls, no search, no agents. This is the silent-data-loss floor proven — the only thing that *must* be right
  before anything is built on it.

- **First useful (end of M4-C6).** A team can actually converse: the composer over the frozen content subset
  (M4-C3), per-viewer permission-aware unfurls of git PRs / issues / docs / CI runs with the no-leak +
  erasure-safe + live-update guarantees (M4-C4, CHAT-D5/D6/D7), read-state + unread + Activity-as-view (M4-C5),
  and the HITL approval-card bridge so an agent can post a proposed fix behind a human gate (M4-C6,
  CHAT-D9/D10). This is the agent-native promise made visible — the place where a CI failure becomes a triage
  thread and an agent posts behind an approval card. It is single-cell, Postgres-hot-tier, Rust-gateway, mock
  agents — all named floors.

- **Production-hardened (end of M5 + M6).** ACL-filtered search + reindex-from-source parity (M4-C7), the full
  erasure cascade across every holder incl. embeddings + backups (M4-C8), explicit-first agent dispatch +
  streaming (M4-C9), then the 30× agent-surge / deploy-herd survival with the human lane held + the agent lane
  shedding (M5-C-S1, CHAT-D3/D4), the floor promotions where triggered (Scylla, home-node), the whole-system E2E
  flagship terminating in chat (E2E-2), and finally the switch test driven in a browser (M6, CHAT-D19). A team
  could move off Slack without hitting a wall the old tool didn't have — and that verdict is reached by driving
  it, not by reading the feature list.

The compounding-payoff signal (EI-01 closing) is the health check: because chat is a projection of capabilities
that already exist (Refs `resolve`, the firehose transport, `EffectApi`, `DurableExecutor::signal`,
`list_inbox`), each chat surface should be *smaller* than the last. If a chat feature starts requiring new
substrate, that is the signal to stop and repair the foundation, not to build more chat.

---

## 7. Digest

**Milestones (band → milestone → the work):**
- **M2-C0 (M2 pre-work):** chat declares its contract surfaces — the `chat.*` taxonomy, the ReBAC fragment, the
  `#sub` grammar contribution, the `humanise` keys, the `define_notif_rule` set, and co-designs the firehose
  resume-cursor protocol. Contracts, not behaviour.
- **M4-C1 (M4):** the durable message store + outbox co-commit (the silent-data-loss floor) — `MessageStore`
  trait, ULID order, idempotent send, membership→tuples, per-subject-DEK bodies. Gate: CHAT-D13/D14/D2.
- **M4-C2 (M4):** the firehose resume-cursor transport + the Rust connection-tier gateway (the
  zero-loss-across-reconnect backbone). Gate: CHAT-D1/D4.
- **M4-C3 (M4):** the composer over the frozen myelin-content Chat subset; per-message CAS; `render(parse(md))===md`.
- **M4-C4 (M4):** cheap per-viewer permission-aware unfurls (the wedge differentiator) over the Refs chokepoint +
  the 4-step ladder + the SetExpr JOIN. Gate: CHAT-D5/D6/D7/D18.
- **M4-C5 (M4):** the read-state hot path (Valkey+PG, cache-never-authoritative) + Activity-as-view. Gate: CHAT-D12.
- **M4-C6 (M4):** the HITL approval-card bridge (per-effect idem_key) + the agent ToolDef set (frozen X-6
  defaults, routed through EffectApi). Gate: CHAT-D9/D10.
- **M4-C7 (M4):** ACL-filtered Search indexing + reindex-from-source parity. Gate: CHAT-D11/D15.
- **M4-C8 (M4):** the erasure cascade across every chat holder (crypto-shred bodies + pseudonym-shred mentions +
  embeddings + backups). Gate: CHAT-D8.
- **M4-C9 (M4):** agent presence/streaming + explicit-first dispatch (mock-provable). Gate: CHAT-D16/D17.
- **M5 follow-ons (M5):** 30× surge (CHAT-D3/D4); ScyllaDB promotion; mega-channel home-node; cross-org channels;
  comment-threading consolidation; the E2E wedge (E2E-1/E2E-2/E2E-4).
- **M6:** the switch test driven in a browser (CHAT-D19).

**Floors + named follow-ons:**
- Postgres-partitioned hot tier → **ScyllaDB** (M5, on measured volume).
- Firehose subject fan-out → **channel-sharded home-node** (M5, on subscriber-count).
- Rust gateway → **BEAM/Phoenix hatch** (only if CHAT-D3/D4 prove Rust intractable; bounded by contract 1.7).
- Single home-cell → **multi-region edge + cross-cell bridge** (M5).
- fs-backed BlobStore → **object-store** (M5).
- Mock agent runtime → **real LlmAgentRuntime** (post-M5, after AG-D4/D2/D3/D5).
- Free-text third-party erasure residual → **the ONE platform posture (10.9), counsel/DPO ratified once** (LEGAL,
  parallel — the structural floor ships regardless).
- Cross-org channels → **designed-not-built on the cross-cell bridge** (M5/P6+LEGAL).
- Comment-threading → **consolidation onto the Chat threading primitive** (M5/on-demand, OQ-L).
- Per-surface shed budgets → **tuned numbers** (M5, from CHAT-D3/D4).

**Critical upstream dependencies (must be green before chat builds):**
- **M0:** the outbox + idempotent-consumer template (CHAT-D13 depends on BUS-2); the 12 lints (no-raw-publish,
  search-requires-acl-filter, tenant-predicate, no-untagged-personal-data, no-host-exec); the EventEnvelope.
- **M1 (★ the hard blockers):** Identity `list_objects` SetExpr (4.3 — the unfurl/search/list backbone), `check`
  (4.2 — the HITL gate), `resolve_pseudonym`/`erase` (4.8 — mention shred), the Chat ReBAC fragment (4.9);
  Storage **restore-verify STOR-D1/D2** (chat writes no real message over a red restore-verify) + KMS per-subject
  DEK (11.4 — the crypto-shred substrate); Tenancy `(tenant, region)` partition + residency-pin; the GDPR holder
  spine.
- **M2 (★ the hard blockers):** the firehose resume-cursor transport (3.5 — chat's correctness backbone); Refs
  `resolve` + the 4-step ladder + `project` (5.2/5.7/5.6 — the unfurl chokepoint); the Agent fabric `EffectApi` +
  explicit-first dispatch + **AG-D4 green** (no agent compute over a red sandbox gate); Workflow
  `DurableExecutor::signal` + per-effect idem_key (the HITL bridge); the frozen myelin-content taxonomy + WASM
  core (the composer); Notif `list_inbox`/`humanise` (Activity-as-view).
- **M3 (the producers chat unfurls):** Git (commits/PRs + `project`); Knowledge (docs/pages + `project`).
- **M4 (within-band, CI first):** CI's `ci.check.updated` (5.9, X-1) for unfurl invalidation (CHAT-D7); Issues
  `project` for issue unfurls.

**The two permanent gates chat inherits (re-run forever, never "done"):** STOR-D1/D2 (restore-verify — every
change touching the message store) and AG-D4 / CI-T1 (sandbox escape — every backend/image/kernel change; gates
any agent compute chat dispatches).
