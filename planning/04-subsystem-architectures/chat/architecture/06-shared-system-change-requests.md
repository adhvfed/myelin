# Chat — 06 · Required Shared-System Changes (for Phase-5 reconciliation)

> See [`00-overview.md`](./00-overview.md) for framing. This is the **explicit, itemized list** of what Chat
> needs from the shared systems that is **not already in the Phase-3 contracts**. Each item names the **owner**,
> the **nature** (new / confirmation / small extension), and **why Chat needs it**. Most are *confirmations of
> already-named seams* (Chat reuses the Phase-3 chokepoints heavily); a few are small additive sharpenings.
> Nothing here reverses a Phase-3 contract. Tagged `CHG-Cn`.

---

## 1. The change requests

| # | Change | Owner | Nature | Why Chat needs it |
|---|---|---|---|---|
| **CHG-C1** | **Cross-language harness parity for the connection-tier gateway (substrate §13 Q1 → P4).** Confirm the minimum shim a *non-Rust* gateway must implement: three-surface topology, liveness≠readiness, **no fire-and-forget emit** (all emit via the Rust Message Service's outbox), `PersonalDataHolder` over ephemeral state, resilient-client + `Retry-After`, the telemetry survival signal set, the protected-human-lane shed order, forward-only migrations. | substrate | Confirmation of a named seam (substrate §13 Q1) | Even though Chat commits **Rust** ([01 §1](./01-tech-and-data-model.md)), the BEAM escape hatch (TE-21) is only admissible if this shim is frozen. If we never diverge, this is a no-op; naming it keeps the hatch real. |
| **CHG-C2** | **`list_objects` `Filter` push-down over a chat-side id column** (S-10 family). Confirm the `Filter{set_expr, zookie}` Chat composes for: (a) the unfurl membership-as-permission class precompute (over channel/artifact ids), and (b) the Search reindex ACL filter (over `message` id). | Identity | Confirmation/usage of Id §8.2 + S-10 | The cheap-per-viewer unfurl path ([02 §4.3](./02-internals-and-algorithms.md)) and the `search-requires-acl-filter` conjunction both depend on a consumer-composable, facet-expressible `Filter`, not opaque-id-only. |
| **CHG-C3** | **Firehose presence/typing/read-state/partial frame conventions.** Confirm the NATS-core subject grammar Chat uses (`fan.<tenant>.<channel>` for delivery; presence/typing/partial subjects), TTL semantics, and that the durable bus carries only the coarse `chat.read_state.updated` summary. | Bus (firehose seam) | Confirmation of Bus §4.3 usage | Chat's live tier + agent streaming ([02 §1/§7.3](./02-internals-and-algorithms.md)) ride NATS core; the firehose `publish`/`tail` API + the durable-vs-firehose split must be the agreed seam, not a Chat-private transport. |
| **CHG-C4** | **`DurableExecutor::signal` idempotency + the per-effect `idem_key` scheme for batch approval.** Confirm `signal(run, name, payload, idem_key)` is idempotent on `idem_key=card_id`, AND that a multi-effect card may post **one signal per effect** with `idem_key=card_id:<effect_idx>` so a *partial* approval is well-defined. | Workflow | Confirmation + a small joint decision (Workflow §6.3 `[OPEN → P4 joint]`) | The HITL bridge ([02 §5](./02-internals-and-algorithms.md)) — a double-click must be one approval; a batch card (open PR + link issue + post) needs per-effect resolution. Resolves the Sketch-06 open question. |
| **CHG-C5** | **`mint_run_token` callable mid-workflow on resume (S-11).** Confirm a multi-day HITL workflow re-mints its short-lived agent token when it resumes after an approval, so the workflow holds no long-lived privileged token. | Identity / Workflow | Confirmation of S-11 | A Chat-surfaced approval may arrive *days* later; the resumed gated tool-exec must run under a freshly-minted attenuated token, not a stale one. |
| **CHG-C6** | **Notif default Signal/notify-reason rule set Chat hands Notif (Notif §9 Q1/Q5).** Confirm the `define_notif_rule` set Chat registers: `mentioned`/`replied`/`thread_watched`/`approval_requested` → reason/priority/class, with the dedup templates. Plus the `humanise` template keys for chat-originated items + agent messages (NOTIF-1). | Notif | Confirmation of a P4 deliverable (Notif §3.1/§9) | The fanout-class declaration ([03 §4](./03-events-contracts-and-glue.md)) is Chat's; the routing/priority/storm-control is Notif's — the rule set is the seam between them. |
| **CHG-C7** | **`watcher` relation read-fanout resolution at chat density.** Confirm `list_subjects(channel, watcher)` is performant for read-fanout over large channels (the ambient unread set), and that per-thread watch derives from the channel `watcher`. | Identity / Notif | Confirmation of Notif §8.3 usage | The read-fanout half of the fanout boundary ([03 §4](./03-events-contracts-and-glue.md)) resolves watchers at read; a slow `list_subjects` on a 50k-member channel would defeat it. |
| **CHG-C8** | **The free-text third-party erasure residual — the documented lawful-basis limit (GD-1 family).** Co-own with LEGAL/DPO the exact residual statement for a person's name typed into the free text of another user's un-erased message. | GDPR/Audit + LEGAL | New policy artifact (extends GD-1 to chat) | Chat names this floor honestly ([05 §5](./05-hard-problems.md)); the *documented lawful-basis limit + retention + access-control* posture is a policy deliverable Chat cannot write alone. |
| **CHG-C9** | **Per-surface shed budgets for the connection-storm + agent-mention-storm profile.** Confirm the concrete protected-human-lane reservation size + per-tenant in-flight caps tuned for connection churn (a deploy reconnect thundering-herd; a 30× agent-mention surge). | substrate + Chat (sets its budgets) | Small extension (substrate §13 Q3) — Chat's P4 deliverable | The gateway is *the* edge where the shed lane applies ([02 §1.4](./02-internals-and-algorithms.md)); the 30×-agent-surge drill asserts against these budgets. |
| **CHG-C10** | **Cross-cell PII-free pointer bridge (multi-cell + cross-org).** Confirm the deferred bridge carries only `subject`/`type`/`correlation_id` and that per-viewer resolution is always cell-local — the seam cross-org channels + a multi-cell tenant's chat will ride. | control plane (Bus §7.4) | Confirmation of a deferred platform floor (Tenancy §10) | Both the single-home-cell floor ([05 §1](./05-hard-problems.md)) and cross-org channels ([05 §7](./05-hard-problems.md)) depend on this bridge when they ship. |
| **CHG-C11** | **Embeddings-as-personal-data erasure for chat message vectors.** Confirm Search purges + reindexes message **embeddings** (not just FT terms) on `identity.human.erased`, and that an HYOK `can_derive_plaintext_index()=false` tenant structurally skips message indexing. | Search / Storage | Confirmation of Search §erasure + Storage §6.2 | Chat declares semantic indexing for RAG/dedup ([03 §7](./03-events-contracts-and-glue.md)); an erased person's message vectors must purge, else an erasure leak via the index. |
| **CHG-C12** | **`MessageStore` Scylla-promotion seam (R-5).** Confirm Storage supports a ScyllaDB hot tier as the measured promotion behind the `MessageStore` trait, residency-pinned + crypto-shred-capable per cell (incl. the smallest self-host cell). | Storage | Confirmation of a measured-trigger follow-on | The named floor ([05 §2](./05-hard-problems.md)) — the trait makes it a swap, but Storage must support the engine self-hostably per cell. |

---

## 2. What Chat does NOT need (explicitly — to bound Phase-5 scope)

To keep the reconciliation honest, Chat confirms it needs **no** new contract for the following — it reuses the
Phase-3 chokepoint as-is:

- **Per-viewer permission-aware unfurl** — `refs.resolve` (Refs §4.2) is sufficient; Chat re-implements nothing.
- **The inbox** — `Notif.list_inbox` + a filter (C-9); Chat builds no inbox store.
- **The HITL durable wait/timer/resume** — `DurableExecutor` (Workflow §6.3); Chat owns only the card + the
  signal post.
- **The cost/wallet** — `EffectApi` reserves (D8/CI-2); Chat surfaces cost, owns no spend path.
- **Agent loop guards** — the platform's structural ones (AG-6; Bus §4.7); Chat honours them via `emit(draft,
  cause)`, builds no bespoke loop logic.
- **The editor / content model** — `myelin-content` (Knowledge-led, ADR-05); Chat consumes the AST, builds no
  editor.

Continue to [`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md).
