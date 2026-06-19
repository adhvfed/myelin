# Sketch 08 — Threads UX, the canvas-vs-Knowledge boundary, cross-org/federated channels

> Exploration note. Resolves the remaining Phase-2 Chat open questions: §9.6 (group-DM vs private channel),
> §9.7 (threads UX), §9.11 (canvas-vs-Knowledge boundary), §9.12 (cross-org/federated channels). These are
> more product/model-shaped than the hard scale problems (Sketches 01–05); I lean and flag.

---

## A — The conversation model (group-DM vs private channel; one entity)

**Decided (Phase-2 Chat §1; Phase-1 §2.2):** one `Conversation` entity with a `kind` + a membership
strategy, **not** separate tables — so DMs, group-DMs, channels, and artifact-linked channels share
read/write/fan-out/erasure machinery (avoid duplicating the hardest logic five times).

`kind ∈ { channel_public, channel_private, dm, group_dm, artifact_linked, announcement }`. The
open §9.6 question — **keep group-DM and private channel both, or unify?** Lean: **keep both as `kind`s
of one entity but unify the machinery.** A group-DM is "a private conversation whose name == its member
set, membership-is-the-ACL, no topic"; a private channel is "named, topic-scoped, invite-managed
membership." They differ in *UX affordances and lifecycle*, not in storage or fan-out — so one entity,
two presentations (the design-language §2 "one component, adapt presentation" principle applied to
conversations). This keeps the model from foreclosing either and avoids a second fan-out path.

- **Membership IS the access-control relation** for private kinds (identity §5 Chat clause:
  `channel.read = member + parent_project->read`) — roles compile to ReBAC tuples (ADR-03). A
  private-channel mention must **not leak** to users who can see the referenced artifact but not the
  channel (Phase-1 §2.4 back-reference caveat) — handled because the *backlink* respects `list_objects`
  (Refs §4.4) and the *unfurl* is per-viewer (Sketch 04).
- **Artifact-linked channels** (born from an incident/release/sprint/repo, carrying a back-reference) are
  a Myelin-specific lever (Phase-1 §2.2) — a `kind` with an `ArtifactRef` link that auto-creates a
  `ref.created` edge ("incident #X discussed in channel Y"), feeding Refs.

---

## B — Threads UX (§9.7)

The perennial wart (Slack's "also send to channel" toggle). Options weighed (Phase-1 §2.5):

| Option | Verdict |
|---|---|
| **Channel-first** (replies inline; threads are an afterthought) | the main-timeline-drowns-in-agent-verbosity failure (P8); rejected as the default. |
| **Threads-first with explicit broadcast** | **CHOSEN lean** (Phase-1 §2.5 recommendation). A reply goes to its thread by default; "also send to channel" is an *explicit, deliberate* broadcast, not the inverse. This keeps agent verbosity and incident detail **out of the main timeline by default** (P8; design-language §6.5) — the calm-by-default principle, which matters *more* in Myelin than in Slack precisely because agents raise volume (competitive-landscape §5 "Zulip-style topic threading considered specifically because agent participation raises volume"). |

- A thread = messages sharing a `thread_root_id` (Phase-1 §2.5). Per-thread read-state + unread/mention
  counts (Sketch 03). The **thread pane** is where most agent/incident detail and **streaming agent
  output** live (Sketch 07; design wireframes).
- Causal anomalies (an edit/reaction arriving before the message it targets) are handled gracefully on
  the client (Phase-1 §5.7) — the k-sortable id + resync (Sketch 01) give a stable order to reconcile to.

---

## C — The canvas-vs-Knowledge boundary (§9.11) — the flagged overlap

A "canvas" / pinned-structured-summary atop an incident channel (Slack-canvas-like) that ties artifacts
together is a **strong fit for Myelin** but **overlaps the Knowledge platform** (Phase-1 §2.4/§3;
Phase-2 Chat §1 non-goal "the canvas feature overlaps Knowledge and is a flagged boundary, not a v1
commit"). Three options:

| Option | Verdict |
|---|---|
| **Build a canvas inside Chat** | **Rejected.** Re-implements the Knowledge block editor + collab + storage — duplicating the platform's hardest editor surface (EI-04 §2; KN-1/KN-4). The exact "don't build five editors" anti-pattern. |
| **Embed a Knowledge page into a channel (via an `ArtifactRef`)** | **CHOSEN lean.** A channel's "canvas" is a **pinned Knowledge page embedded via an `ArtifactRef`** (design-language §5.9 embeds; §5.6 views component). Chat owns the *pin/placement* (a channel can pin a `knowledge/page/<id>` ref at the top); Knowledge owns the *editor, collab, content, storage, erasure*. One editor render path (KN-4/D10), one content model (ADR-05), no duplication. The boundary is clean: **Chat references; Knowledge authors.** |
| **Skip for v1** | acceptable fallback; the embed option is cheap *because* it reuses Knowledge, so lean to build-via-embed but it's not a v1 hard commit. |

**Decided direction:** *canvas = an embedded/pinned Knowledge page, not a Chat-native editor.* This
honours the platform's "share the content model + editor, not re-build it" rule (ADR-05; design-language
§5.9) and keeps the erasure/collab story in the one place that owns it. The boundary is **flagged for the
joint Chat↔Knowledge review** but the lean is firm. → architecture (the pin/embed mechanism), Knowledge
owns the page.

---

## D — Cross-org / federated channels (§9.12) — deferred, but don't foreclose

A cross-org "Slack Connect"-style shared channel has **deep identity, residency, and erasure
implications** (Phase-1 §2.1/§9.13; Phase-2 Chat §9.12). **Decided: deferred for v1, but the model must
not foreclose it.** The honest constraints if/when built:

- **Residency:** a channel shared between an EU tenant and another tenant crosses cells — it rides the
  **control-plane PII-free pointer bridge** (the same cross-cell mechanism Bus §7.4 / Refs §6.5 / Notif
  §5.4 all defer as a named floor): only `subject`/`type`/`correlation_id` cross, never payload/PII;
  per-viewer resolution is always local to the cell holding the artifact. A cross-org message body's
  residency is genuinely hard (whose region?) — this is the same multi-cell-tenant floor the whole
  platform defers (SC-2/SC-3), scoped to chat.
- **Identity:** a member from org B in org A's channel is a cross-tenant principal — and **there is no
  cross-tenant query path** (ID-3; EI-02 §1). Federation needs an explicit, opt-in, legally-gated
  cross-tenant visibility capability (the same `[OPEN → P4/legal]` Refs §6.4 names for cross-tenant
  inbound refs). The *mechanism* (a narrow `public`/explicit-grant userset, never a cross-tenant join) is
  decided; the *policy* is P4/legal.
- **Erasure:** erasing a person in org B who posted in a shared channel must reach org A's cell — the DSR
  orchestrator's multi-cell `member_cells` iteration (GDPR §4; S-9). Tractable *because* the erasure triad
  is references-not-payloads + crypto-shred (Sketch 05), but it's a named floor.

**The non-foreclosure rule:** the `Conversation` model does **not assume single-org membership forever**
(Phase-1 §2.1) — `membership` is a set of principals that *could* span tenants, gated by the cross-tenant
capability when it ships. We design the data model to permit it; we don't build it in v1.

---

## What this sketch hands forward

- **One `Conversation` entity, many `kind`s** (channel pub/priv, dm, group-dm, artifact-linked,
  announcement); keep group-DM and private channel as distinct `kind`s but unify the machinery;
  membership-is-the-ACL via ReBAC tuples.
- **Threads-first with explicit broadcast** (calm-by-default; matters more because agents raise volume);
  the thread pane hosts agent detail + streaming.
- **Canvas = an embedded/pinned Knowledge page** (`ArtifactRef`), not a Chat-native editor — Chat
  references, Knowledge authors. Flagged for joint review; lean firm.
- **Cross-org/federated channels deferred for v1**; the model does **not foreclose** it (membership may
  span tenants); it rides the platform's deferred cross-cell PII-free bridge + the explicit-opt-in
  cross-tenant capability + multi-cell DSR. Named floor, not built.
