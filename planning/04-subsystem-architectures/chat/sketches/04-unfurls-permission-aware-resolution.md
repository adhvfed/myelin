# Sketch 04 — Unfurls: live-vs-snapshot + cheap per-viewer permission-aware resolution

> Exploration note. Resolves Phase-2 Chat §9.4 — *"likely the trickiest correctness problem"* — and the
> mega-channel live-delivery escalation deferred from Sketch 01. The unfurl is "the differentiator"
> (Phase-1 §2.4) and the densest use of the `list_objects`/`resolve` hot path the platform owns.

---

## The problem, precisely

A chat message references an artifact via a structured `artifact_ref(ArtifactRef)` node (ADR-05 — *not*
parsed text). The **unfurl card** is a rich projection of that artifact (a PR with checks/reviewers, an
issue with state/assignee, a CI run with the failing step). Two properties collide with scale:

1. **Per-viewer permission-aware.** If viewer A can see the PR and viewer B cannot, **B must get a
   "no-access" card, never the title** (Phase-1 §2.4 — "THE subtlety that separates a real
   implementation from a demo"; ADR-03). A message in a 500-member channel embedding 3 refs is naïvely an
   N-viewers × M-refs authorization+fetch explosion.
2. **Live, not snapshot.** The card should show *current* state (a PR that went green updates), and —
   critically for GDPR — a *snapshot* may freeze a third party's personal data (a PR author's name) that
   is later erased (Phase-1 §7; Phase-2 Chat §7.8). Frozen snapshots are an erasure leak.

The platform has **already built the chokepoint** that solves both (Refs §4.2): `resolve(ref, viewer,
mode) → Projection | Tombstone`. Chat does **not** re-implement permission-aware resolution — it *calls
Refs*, and Refs calls Id `check` per viewer and the owning subsystem's `project(ref, viewer)` API. **Refs
is the chokepoint that makes unfurls non-leaking** (Refs §4.2; identity §5 Chat clause: "the per-viewer
permission-aware unfurl is *not* a chat concern — chat asks Refs"). So Chat's design problem narrows to:
**make the per-viewer call *cheap* at chat density, and decide the live-vs-snapshot policy.**

---

## Decision 1 — Live vs snapshot (decided: live, with an audit snapshot)

| Option | Verdict |
|---|---|
| **Snapshot at post time** | **Rejected.** Goes stale (Slack/GitHub do this and it confuses); and a snapshot freezes a third party's PII → an **erasure leak** (a since-erased PR author's name frozen in a card; Phase-1 §7; Phase-2 Chat §7.8). |
| **Live per-viewer projection, cached short-TTL, bus-invalidated** | **CHOSEN.** The platform default (design-language §5.3 "live, not snapshot by default"; Refs §4.2). Always current; erasure-safe (an erased author resolves to a tombstone *on next render*); permission-current (a viewer who lost access stops seeing the title). |
| **Live + an audit snapshot recorded separately** | **CHOSEN addition.** For audit/"as-of-hover" (Phase-1 §2.4) and lawful-basis records, record *what the message linked to* (the `ArtifactRef` + a timestamp) — **the ref, not the rendered content**. The audit record is references-not-payloads, so it is itself erasure-safe. |

So: **the card renders live per-viewer; the only thing stored is the `artifact_ref` node + a post-time
timestamp** (the audit "as-of"). No rendered title/state/PII is ever stored in the message or a durable
unfurl snapshot — which is *why* erasure is free (Sketch 05).

---

## Decision 2 — Making per-viewer resolution cheap (the hot-path design)

The unfurl service is a **Chat-owned cache + orchestration layer in front of Refs `resolve`**. The
layered cheapening, in order:

1. **Render lazily — only what is on screen.** A virtualised timeline (Sketch / design wireframes) only
   resolves unfurls for messages currently in the viewport. A scroll-back of 10,000 messages resolves a
   handful of cards, not 10,000. (The single biggest cost-killer; the naïve "resolve every ref in the
   channel" is the trap.)

2. **Split the cache by what varies per-viewer vs. what doesn't** (the key insight, mirroring Refs §4.2):
   - **Projection content** (title/state/icon/render-hint) is **viewer-independent** — cache it **once per
     `ArtifactRef`** with a short TTL, busted by the artifact's `*.updated`/`*.erased` bus events
     (Sketch 10 / Phase-2 Chat §7.2). A popular doc embedded in 500 messages resolves its *content* once.
   - **The permission decision** (can *this* viewer see it?) is **per-viewer** — but it is a **`check` /
     `list_objects`**, which is the platform's fast, cached, Leopard-pre-filtered, fail-static-able
     primitive (Id §8; the "single most load-bearing inter-system contract"). The card is assembled as
     *(shared cached projection) gated by (per-viewer cheap check)* — content is only returned after the
     per-viewer check passes, so **one shared cache entry per ref, never one per (ref, viewer)**, with no
     leak (exactly the Refs §4.2 correctness argument).

3. **Membership-as-permission precompute for the common channel case.** For a **public** channel inside a
   project, "can a channel member see this project artifact?" is frequently a single coarse class, not
   500 individual checks: channel membership compiles to a ReBAC tuple (identity §5 Chat clause:
   `channel.read = member + parent_project->read`), and `list_objects(viewer, view, type)` returns a
   **filter** (Id §8.2 `Filter{set_expr, zookie}`) the unfurl service applies once. For a **private**
   channel whose membership ≈ the visibility class, often *all members can see the same artifacts* — one
   class decision, not N. (Phase-1 §5.4 "precompute per-channel visibility classes where membership ≈
   permission" — this is that, expressed via `list_objects`.)

4. **Bus-driven invalidation, not TTL-only.** The unfurl service runs a consumer (substrate template,
   whitelisted subjects) on the artifact `*.updated`/`*.erased`/`*.checks_completed` pointer events; a
   matching event **busts the shared projection cache entry** for that `ArtifactRef`, and the gateway
   pushes a live card update to viewers currently showing it (Phase-2 Chat §6.1.4). TTL is the backstop;
   the bus event is the precise invalidator.

5. **Resilient-client discipline.** Every `project(ref, viewer)` call to an owning subsystem goes through
   the shared resilient client (timeout/breaker/bulkhead; substrate §6) — a slow/down subsystem degrades
   the *card* to a "couldn't load — retry" state (design-language §5.10), never stalls the message render.

### The flow (per visible message, per viewer)

```
for each artifact_ref node in a VISIBLE message, per viewer:
  proj | tombstone = refs.resolve(ref, viewer, mode = Display | Card)   # Refs §4.2 — the chokepoint
     → Refs: Id.check(viewer, view, ref)  → deny ⇒ TOMBSTONE ("a restricted <type>"), never the title
     → Refs: cache hit (shared, per-ref) OR owner.project(ref, viewer) via resilient client
  render: live card  | "no-access" card | "deleted/erased" tombstone | "couldn't load — retry"
```

Chat owns: the **card UX, lifecycle, the shared per-ref projection cache, lazy-on-viewport resolution,
and bus-invalidation** (Phase-2 Chat §1 "Chat owns the *card UX*, lifecycle, and cache"). Refs/Id own:
**the permission decision and the projection content** (no cross-DB; Chat never reads Git's/Issues' DB).

---

## Decision 3 — Mega-channel live delivery (the escalation deferred from Sketch 01)

A 100k-member announcement channel: a single post must reach up to 100k online sockets fast. Two issues —
**fan-out** (Sketch 01) and **unfurl resolution** (here). Resolution is already solved by lazy-on-viewport
(only the few hundred members *currently looking* resolve the card; the rest resolve on open). For
**delivery**, the NATS-subject-per-channel model (Sketch 01) means 100k gateway-side subscribers on one
subject — workable, but the **channel-sharded home-node** escape hatch (Sketch 01 Decision 2) is the
named escalation: a measured-hot mega-channel gets a dedicated fan-out home node that the gateways pull
from, rather than 100k direct subject subscribers. **Promotion trigger:** measured mega-channel
subscriber count exceeding the subject-fan-out budget (R-5 named-trigger discipline). Until measured, the
subject model is the design.

---

## What this sketch hands forward

- **Live per-viewer unfurls, never durable snapshots; store only the `artifact_ref` node + a post-time
  timestamp** (audit "as-of") — which is what makes erasure free (Sketch 05).
- **Chat calls Refs `resolve` (the non-leaking chokepoint); it does not re-implement permission-aware
  resolution.**
- **Cheapness comes from: lazy-on-viewport + a shared per-`ArtifactRef` projection cache (viewer-
  independent) gated by a per-viewer `check`/`list_objects` (the leak-free pre-filter) + membership-as-
  permission class precompute + bus-driven invalidation + resilient-client degradation.**
- **Mega-channel delivery escalates to channel-sharded home-node on measured volume** (Sketch 01 hatch).
- Owed drills: **unfurl-no-leak** (a viewer lacking access gets a tombstone, never the title) and
  **unfurl-erasure-safe** (an erased third party in a card → tombstone on next render). Both inherit
  Refs/Notif drills (Refs §7 D-?, Notif D-N4/D-N6).
