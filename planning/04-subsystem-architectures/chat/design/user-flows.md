# Chat — User Flows

> Phase-4 design sketch (BEFORE architecture; VISION §3/§5.4). Covers the key user flows **including the
> agent/HITL flows** (proposed effects, approval cards, attribution) and the **cross-subsystem flows** Chat
> participates in. Companion: [`information-architecture.md`](./information-architecture.md),
> [`wireframes.md`](./wireframes.md). Each flow names the shared contracts it exercises so the architecture
> stage sees the glue. *No subsystem reads another's DB* (ADR-01) — every cross-subsystem hop is a contract.

---

## Flow 1 — Send a message (the write path, optimistic with honest rollback)

1. Alice types in the **composer** (S3); the body is the `myelin-content` AST — `mention`/`artifact_ref`/
   `embed` are **structured nodes**, not parsed text (ADR-05).
2. On ⏎: the client **optimistically renders** the message (a pending state), with a client **idempotency
   nonce** (Phase-1 §2.3 — dedup retried sends, esp. flaky mobile/agents).
3. Message Service: `Id.check(alice, post, #channel)` (channel-membership-is-ACL, identity §5) → persist
   to the durable per-conversation log (Sketch 02) → **`OutboxTx::emit(chat.message.created, cause)` in
   the SAME transaction** (BUS-2; the no-dual-write guarantee). Each `artifact_ref` node also emits a
   `refs.edge.created` ("discussed in #channel"; Refs §4.1).
4. Fan-out (Sketch 01): NATS-subject-per-channel delivers the message frame to online members' gateways →
   their sockets. Offline members get it on next open (read-fanout) + an unread (Sketch 03).
5. **Honest rollback:** if persist fails, the optimistic message shows a quiet **send-failed + retry**
   (design-language §5.10; idempotency-safe). Optimism for latency, honesty on failure (P2).

*Contracts:* `Id.check`, `OutboxTx::emit`, `refs.edge.created`, the firehose. *Latency budget:* keyboard
< ~100ms perceived (optimistic render); the persist is async (T-8).

---

## Flow 2 — A reference becomes a live, permission-aware unfurl (the wedge, in chat)

The platform-flagship flow (Phase-2 Chat §6.1; design-language §5.3). Alice pastes a PR URL in
`#release-2.4`; the composer's **Search-backed autocomplete** offers an unfurl; she accepts → the message
stores an `artifact_ref(myelin://acme/git/pr/88)` node (**not a copy of the PR** — ADR-05).

1. Persist + emit (Flow 1); the `artifact_ref` → a Refs edge (PR#88 ←"discussed in"→ #release-2.4).
2. **Per viewer, lazily on viewport** (Sketch 04): the Unfurl Service calls **`refs.resolve(ref, viewer,
   Display)`** — Refs calls `Id.check(viewer, view, PR#88)` and Git's `project(ref, viewer)` API:
   - **Bob** (can see the PR) → a live card: title, checks, reviewers, an **Approve action**.
   - **Carol** (channel member, *cannot* see the private repo) → a **redacted "no-access" card** — the
     title **never leaks** (Refs §4.2 is the non-leaking chokepoint; Sketch 04).
3. PR#88's checks go green → Git emits `git.pr.checks_completed` → Chat's invalidation consumer **busts the
   shared per-ref projection cache** → every viewer's card updates **live** (Sketch 04; Phase-2 Chat §6.1.4).
4. Bob clicks **Approve** on the card → dispatched through **`EffectApi`** (validated vs his permissions),
   exactly like an agent effect (design-language §5.3 unfurls are an action surface).

*Contracts:* `refs.resolve` (the chokepoint), `Id.check`, Git `project`, the `*.updated` pointer event,
`EffectApi`. *Correctness drills owed:* unfurl-no-leak, unfurl-live-update, unfurl-erasure-safe (Sketch 04).

---

## Flow 3 — The agent-native flagship: CI fails → agent triages → posts → proposes a fix behind a HITL gate

The end-to-end agent + HITL flow (Phase-2 Chat §6.2; design-language §6; Workflow §6.3), **explicit-first**
(CHAT-1 — see the note at the end).

1. `ci.run.failed` Signal → an agent run is dispatched (**explicitly** — an `@oncall` rule or an explicit
   "run triage agent" action, Sketch 07; *not* an implicit casual-mention auto-wake, CHAT-1). The run is a
   **durable workflow** (Workflow §6.1); reserve/settle gates it (no balance → no run, D8).
2. **Plan-then-apply** (ADR-08.3): the agent's brain `step` proposes effects — `issue.create`,
   `refs.create ×2`, `chat.post` — and performs **no side effects**. `EffectApi` validates + applies each
   via the public endpoint (no carve-out).
3. The `chat.post` lands as a message **authored by the agent `Principal`**, with the **agent badge +
   provenance popover** (S12): which agent, on-behalf-of the pusher, triggered by `ci.run.failed`
   (`causation_id`), threaded by `correlation_id`. UI: *"🔴 main red — opened ISSUE-412, triaging"* (the
   string is **humanised at the backend**, NOTIF-1 — the agent does no string work).
4. A `FixAgent` (explicitly run, or policy-dispatched) proposes `git.open_pr` — **sensitive on a protected
   repo** → `Id` gates it → **HITL required**. The workflow hits `ctx.wait_for_signal("approval:<call>",
   timeout=window)` and emits `agent.approval.requested` (payload: tool, args as `ArtifactRef`s, **RISK**,
   **LIVE COST ESTIMATE** — EI-03 §5.1). The workflow is now `state=waiting`, holding **no runtime**, for
   up to `window` (**minutes to days**).
5. **Chat renders the HITL approval card** (S11) in the thread + as a high-priority Notif inbox item
   (C-9; a gate is never missed). The card shows the **proposed effect** (plan-then-apply), the agent's
   scope/delegation, the **cost**, and **Approve / Edit / Reject** (design-language §6.3).
6. A human clicks **Approve** → Chat calls **`Id.check(human, approve, run)`** then **`DurableExecutor::
   signal(run, "approval:<call>", {approved, by:ref}, idem_key = card_id)`** — *the bridge* (Sketch 06;
   idempotent: a double-click is one approval).
7. The waiting workflow flips `running`, replays to the wait, consumes the signal → the gated `git.open_pr`
   runs (AG-8: the step re-runs **with the tool now allowed**) → PR#88 opens → **announced back in the same
   thread** as an agent message. (Deny → tool withheld, ordinary error, agent continues; Edit → human-
   amended effect; timeout → auto-deny + notify.)

*Contracts:* the Signal tier, `EffectApi`, `DurableExecutor::signal`, `Id.check`, `humanise`, `resolve`.
*The bridge is the named-easy-to-forget piece* (EI-03 §5.1) — Chat's seat is steps 5–7 (Sketch 06).

**Explicit-first note (CHAT-1):** this flow is triggered by an **explicit** action / rule, not a casual
`@FixAgent` mention. A mention *notifies* the agent (an item in its inbox) but does **not** auto-spawn a
costed run; implicit auto-dispatch is a separately-decided, intent/cost/DPO-gated feature (C-2/L-3).

---

## Flow 4 — Streaming agent output into a thread (Sketch 07)

1. An explicitly-run agent begins a multi-step run anchored to a thread; the thread shows an agent
   **"working…"** state (the agent treatment; no sparkle/magic iconography — design-language §8b.3).
2. `agent.message.partial` frames ride the **firehose** (ephemeral; if lost, the final is the truth) and
   update the working affordance live.
3. On the agent's `Submit`, the **final durable `chat.message.created`** replaces the partial in the
   timeline (reconciled by the run's id/`correlation_id`); a mid-stream reconnect re-fetches the final/
   in-progress marker from the durable log + live firehose — never a half-message (Sketch 01 resync).
4. Agent presence shows **busy** during the run; **rate-limited** if the agent lane is shedding (ADR-16);
   verbosity stays in the thread (calm-by-default, P8).

*Contracts:* the firehose, `agent.status_changed`, the durable message log + resync.

---

## Flow 5 — Ops: pipe a failing run's logs into an incident channel from the CLI

```bash
tail -n 40 build.log | myelin chat send '#incidents' --thread "$INCIDENT_THREAD" --reply --as ci-bot
```
1. The body is a structured `code` block (`myelin-content`); `--as ci-bot` is authorized because the **CI
   job token is allowed to act as that agent** (`delegation`; identity §7).
2. Same `chat.message.created` event as a human send → an `@oncall` mention can **page via Notifications**
   (the escalation runs on the durable-workflow timer wheel; Notif §3.7).

*Contracts:* CLI `Principal`/token model, `delegation`, `OutboxTx::emit`, Notif escalation.

---

## Flow 6 — GDPR erasure cascade (Chat is the hardest holder; Sketch 05)

1. `identity.human.erased` for person P → Chat's consumer (substrate template) + the DSR orchestrator
   fan-out (GDPR §4) trigger the cascade.
2. **Authored content:** **crypto-shred P's per-subject DEK** → every body P authored becomes unrecoverable
   in hot store + cold tier + **backups** simultaneously (GD-4; Sketch 05) → the records **tombstone**
   ("message deleted") so conversations stay structurally intact.
3. **Mentions of P** in others' messages: the structured `mention(Principal)` node points at P's
   pseudonymous id → Id pseudonym-shred → renders **`[erased user]`** on next render (free; ADR-05 payoff).
4. **Cascade to derived stores:** unfurl cache purge (re-resolves to tombstone — no durable snapshot
   exists, Sketch 04), read-state markers deleted, drafts crypto-shred, Search **purges + reindexes**
   (incl. embeddings), Refs pseudonym-shreds edges, Notif tombstones items — all via the bus/DSR, **never a
   backdoor**.
5. **Named floor (honest):** P's name typed into the *free text* of someone else's un-erased message is
   **not** surgically erasable — covered by retention + access-control + a documented lawful-basis limit
   (Sketch 05; the chat analogue of GD-1's residual).

*Contracts:* `PersonalDataHolder`, KMS crypto-shred, Id pseudonym-shred, the DSR orchestrator, Search/Refs/
Notif erasure. *Drills owed:* erasure-reaches-every-Chat-holder, erased-mention-renders-tombstone (T-5).

---

## Flow 7 — Cross-subsystem flows Chat participates in (summary)

| Flow | Chat's seat | Contracts |
|---|---|---|
| **Issue/PR/CI/Doc state change → unfurl invalidation + channel post** | consume `*.updated`/`*.checks_completed` pointer events → bust unfurl cache + optionally auto-post to a linked channel | bus consume, `resolve` |
| **Mention/reaction → notification** | produce `chat.message.mentioned` (write-fanout producer) → Notif routes (reason + priority) | `mention` node → Signal → Notif |
| **Reference made in chat → reference graph edge** | every `artifact_ref` node → `refs.edge.created` ("discussed in"); densest producer | Refs §4.1 |
| **Search the corpus** | ACL-filtered message + artifact search; composer artifact autocomplete | Search `query` (always `list_objects`-conjoined) |
| **HITL approval → durable workflow signal** | the approval-card surface; `DurableExecutor::signal` | Workflow §6.3 (Flow 3) |
| **Identity change → membership/visibility recompute** | consume `identity.permission.revoked`/`member.added` → recompute membership/unfurl/access | bus consume, ReBAC tuples |

---

## What these flows commit

The write path (optimistic + honest rollback + idempotency + outbox-coherent), the unfurl wedge (live,
per-viewer, non-leaking via Refs), the **full agent HITL bridge** (Chat is the surface; `Id.check(approve)`
→ `DurableExecutor::signal(idem_key=card_id)`), streaming (firehose partials + durable final),
CLI-as-peer, the **erasure cascade** (crypto-shred + tombstone + structured-mention neutralisation, with
the free-text floor named), and the cross-subsystem flows — all expressed as **contract calls, never
cross-DB reads**, all **explicit-first** for agent dispatch (CHAT-1).
