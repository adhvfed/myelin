# EXT-1 — Client-Facing Context-Projection Prefetch / Bundling

> Extension flagged in [`README.md`](./README.md). A **genuine delta** at the UX/projection seam.

## The UX goal that requires it

Design-language P2 (speed) and §8b.6 ("the system assembles context — and pre-fetches it") demand more
than *linking* the next hop. When a CI check fails, the UI should already have the failing **step** and
the **line of code**; the PR context-pane should arrive with the linked issue + CI run + doc + discussion
*already projected*; a notification should carry "why it fired" **and** the pre-fetched next hop. The
felt result is "the user never assembles context" — a core lovability + wedge promise (R-13, R-22).

## What the extension is (summary)

A **context-projection bundling** capability: a server-side endpoint (or projection-API mode) that, given
an `ArtifactRef` and a viewer, returns the artifact **plus its permission-filtered related projections in
one round-trip** (the chip/unfurl projections for its references, the next-hop in a flow), and a
**client prefetch hint** stream so the client can warm those projections ahead of navigation. This is the
read-side complement to the reference graph: the graph *knows* the edges; this bundles their current
per-viewer projections so the client doesn't make N sequential calls.

## Which architecture doc it touches (and what's already covered)

- **`05-refined/reference-graph.md`** — already provides per-viewer projection + permission check + a
  bounded invalidatable **projection cache** (§3.6). **Delta:** a *bundling/batch* projection mode and a
  prefetch-hint contract; today's resolution is per-`ArtifactRef`, not "this artifact + its N related
  projections in one filtered response."
- **`05-refined/event-bus.md`** — the firehose tier exists (live delivery). **Delta:** using it to push
  prefetch invalidation/warm hints to the client is new.
- **Subsystem projection APIs** — each subsystem already exposes a projection API for its artifacts; the
  delta is a shared bundling convention across them.

## Rough size / risk

**Size: M.** **Risk: M** — must preserve the *permission-aware-per-viewer* invariant in the bundle (no
leak via a pre-fetched projection the viewer can't see), and must not over-fetch (cost/residency). The
existing per-viewer permission check + projection cache make this additive rather than a redesign.

## Implementation-task framing

"Add a permission-aware context-bundling projection mode + client prefetch-hint contract on top of the
reference-graph projection API, so the client renders the PR context-pane / failing-step→line / notified
next-hop without sequential round-trips, preserving the per-viewer no-leak invariant."
