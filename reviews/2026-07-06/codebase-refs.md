# Cross-artifact refs (myelin-refs-service, myelin-refs)

_The Cross-artifact refs unit (myelin-refs value type + myelin-refs-service engine) is unusually well-engineered: the parse/URN codec is ambiguity-rejecting and fuzz-tested; the resolve chokepoint enforces the leak invariant structurally (a Tombstone has no field to leak into); backlinks/traverse lower the frozen SetExpr over source_root with tenant-first partitioning and correct branch-pruning; and cross_cell keeps only filtered projections/tombstones crossing the cell boundary. Reads are consistently tenant-partitioned, which is the real cross-tenant defense. The main concern is a GDPR erasure gap: the PersonalDataHolder body hardcodes Region(\"fr-par\"), so erase/locate silently miss data for any tenant residing in another EU region — a real correctness bug masked entirely by fr-par-only tests. Lesser items: no ingest-time tenant validation of edge URNs (contained by read-side partitioning) and an overcounting erase receipt._

**Kept findings:** 3  (🟡 1 medium  ·  🔵 1 low  ·  ⚪ 1 nit)

---

### 1. 🟡 DSR holder hardcodes Region("fr-par"), so erase/locate silently operate on the wrong partition for any tenant/cell outside fr-par

- **Severity:** medium  ·  **Verdict:** 🟨 PLAUSIBLE  ·  **Category:** gdpr
- **Location:** `crates/myelin-refs-service/src/holder.rs:176`

**What:** Both `RefsEdgeHolder::part` (line 176) and `RefsCacheHolder::part` (line 360) construct the `(tenant, region)` partition key as `(tenant, Region("fr-par"))` — a hardcoded literal, not derived from cell/tenant residency. The `PersonalDataHolder` trait methods receive only a `GdprTenantId` (no region). The edge projection is partitioned by `(tenant, ev.region)`, where `ev.region` comes from the event envelope (edge_builder.rs apply_created line 614: `self.projection.upsert(&ev.tenant, &ev.region, row)`) and can be `de-fra` or any EU region. So `count_by_actor`/`edges_by_actor`/`purge_subject` all query the fr-par partition unconditionally.

**Impact:** For a tenant/cell whose edges reside in a region other than fr-par, an Art.17 erasure would purge 0 cache entries while returning a success EraseReceipt, and locate/export would under-report 0 edges. This would defeat the '0 recoverable PII' erasure guarantee for non-fr-par residency.

**Fix:** Thread the residency region through the DSR fan-out or derive it from the cell/tenant residency binding the holder was constructed for, instead of the fr-par literal; add a non-fr-par partition test proving erase actually purges.

> _Verifier note:_ Confirmed the hardcoded literal at holder.rs:176 and :360, and confirmed the projection is partitioned by ev.region (edge_builder.rs:614). The architecture does support non-fr-par cells (cross_cell.rs references cell-de-fra-1; residency.rs::pinned takes an arbitrary Region param). Downgraded from CONFIRMED to PLAUSIBLE because the impact is not reproducible in the current tree: (1) the backed holder (with_backing/with_cache) is NOT wired into any production serve path — grep found it only in tests and restore_reerase.rs; the module doc explicitly describes the 'serve-before-the-store-is-wired' unbacked posture. (2) The only live region today is fr-par (dogfood.rs MYELIN_SELF_REGION="fr-par", switch_test SELF_REGION="fr-par"), so the hardcode is currently coincidentally correct. It is a real latent defect (region should be derived, not literal) that becomes an active GDPR failure the moment Refs is deployed in a de-fra cell with a wired backing. Kept medium given GDPR criticality plus the explicitly-planned multi-region architecture.

### 2. 🔵 edge_builder does not validate that source/target URN tenant matches the envelope tenant

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** tenancy
- **Location:** `crates/myelin-refs-service/src/edge_builder.rs:588`

**What:** `apply_created` (lines 561-616) wraps the payload `source`/`target` strings directly as `ArtifactRef(source.clone())` / `ArtifactRef(target.clone())` with no parse or tenant-segment check, derives roots via `strip_sub`, and stores the row in the `ev.tenant`/`ev.region` partition. It never verifies that the tenant segment embedded in the source/target URNs equals `ev.tenant`. A buggy/compromised producer in tenant A can emit `refs.edge.created` with `target = myelin://B/knowledge/page/secret`, and the edge is accepted into tenant A's partition with a cross-tenant URN as index key.

**Impact:** No cross-tenant leak: all reads (inbound_live/outbound_live/backlinks/resolve) are partitioned by envelope tenant and permission-checked in that tenant's authz context, so a cross-tenant target URN sitting in the wrong partition is inert and can never resolve or be permission-admitted. The gap is data-integrity hygiene — garbage cross-tenant URNs can accumulate, and the 'no cross-tenant reference' invariant relies entirely on read-side partitioning rather than being rejected fail-closed at the write boundary.

**Fix:** In apply_created, parse the source/target URNs (the myelin-refs `parse` codec already exists and is not called here) and reject as NonRetryable poison any edge whose URN tenant segment != ev.tenant, matching the fail-closed discipline already applied to missing source/target/rel.

> _Verifier note:_ Confirmed: apply_created uses `ArtifactRef(source.clone())` (line 588) — a raw string wrap, no call to myelin-refs::parse, and no comparison of the URN tenant against ev.tenant. edge_id and the partition both use ev.tenant (lines 593, 614). The reviewer already correctly self-limited the impact to inert data hygiene (reads are tenant-partitioned). Low/defense-in-depth is the right severity; borderline nit but the fail-closed-at-write-boundary point is legitimate.

### 3. ⚪ purge_subject counts candidate refs, not actual evictions, so the erase receipt overstates entries purged

- **Severity:** nit  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** correctness
- **Location:** `crates/myelin-refs-service/src/holder.rs:384`

**What:** `purge_subject` (holder.rs:367-391) iterates each distinct ref an actor's edges touch (source/source_root/target/target_root), calls `cache.invalidate` for each, and does `purged += 1` per distinct invalidate call regardless of whether a live cache entry existed. `cache.invalidate` (cache.rs:340) returns `()` and swallows the backing delete result, so callers cannot know whether an entry was actually present. The method doc says it 'Returns how many entries were evicted' and the erase receipt (holder.rs:475) renders `purged {N} cached projection entries naming the subject`.

**Impact:** The GDPR erase receipt reports a count that is the number of distinct candidate refs invalidated, not the number of live entries actually evicted. When the cache is cold/partial the reported count exceeds real evictions. This is a misleading audit/observability artifact on a compliance receipt, not a data-loss bug (the invalidations themselves still fire).

**Fix:** Have `cache.invalidate` return whether an entry was present and count only real evictions, or reword the receipt/doc to say 'issued N cache invalidations' rather than 'purged N entries'.

> _Verifier note:_ Confirmed by reading holder.rs:376-390 (purged incremented per distinct ref unconditionally) and cache.rs:340-343 (invalidate returns (), swallows delete result). One minor imprecision in the reviewer's impact wording: it over-reports precisely when the cache is NOT fully warm; for a fully warm cache purged == actual evictions. Core defect (counts calls, not evictions) is accurate. Correctly rated nit.
