# EXT-2 — Dashboard / Analytics Read-Model + Query UX Layer over OLAP

> Extension flagged in [`README.md`](./README.md). A **genuine delta** — the OLAP *store* is covered; the
> UX-facing query/config layer is not.

## The UX goal that requires it

The PM/EM/exec/PgM reporting surfaces are first-class views in the catalogue: delivery analytics
(cycle time, WIP, review latency, CI health), SLA gauges, dashboards with configurable widgets, the
roadmap/portfolio rollup, usage/quota (design-language §7.3 dashboards, §3.7 charting; personas
P7/P8/P11). For these to be *lovable* (trustworthy, fast, drawn from one event stream not bolted-on
integrations — the P7/P11 success criterion), they need a **queryable, permission-aware, near-real-time
read model** and a **saved-dashboard config object** — not just a raw OLAP store.

## What the extension is (summary)

Two pieces on top of the existing OLAP read store: **(1) an analytics read-model + query surface** — the
aggregations (counts, durations, flow metrics, SLA states) exposed as a **permission-aware, ACL-filtered**
query API the views/charting components consume (a chart can never show aggregates over rows the viewer
can't see — the same correctness invariant as search/views, applied to aggregates); **(2) a
saved-dashboard config object** — a first-class, shareable, permissioned dashboard/widget configuration
(like saved views, ADR-06, but for charts), so dashboards are objects, not hard-coded screens.

## Which architecture doc it touches (and what's already covered)

- **`05-refined/00-reconciliation-decisions.md` §8 + §11 / `storage.md`** — the **OLAP read store**
  already accepts the subsystem event stream and honours the **restriction flag** (no analytics for a
  restricted subject — the GDPR `restrict` suppression flows into OLAP). The PII-free portfolio-rollup
  bridge (aggregates *projections* the viewer may see) is already designed. **Delta:** the
  *UX-facing query API* and the *saved-dashboard config object* on top of it.
- **`05-refined/identity-and-access.md`** — provides the ACL/`list-objects` machinery. **Delta:** applying
  it to **aggregate** queries (permission-aware aggregation), not just object lists.
- **Views component (R-10) + §3.7 charting** — consumes this; the read-model is the missing producer.

## Rough size / risk

**Size: M.** **Risk: M** — permission-aware *aggregation* is genuinely harder than permission-aware
*lists* (an aggregate can leak via its value even when no row is shown); the restriction-flag-into-OLAP
design already establishes the suppression discipline, so this builds on a known invariant rather than
inventing one. Near-real-time vs. batch is a tunable (start read-time, materialise-when-measured — the
existing floor-with-named-follow-on pattern).

## Implementation-task framing

"Add a permission-aware analytics query API + a saved-dashboard config object on top of the OLAP read
store, so PM/EM/exec dashboards draw trustworthy, ACL-filtered aggregates from the one event stream and
dashboards are shareable first-class objects — never leaking aggregates the viewer can't see, honouring
the existing restriction flag."
