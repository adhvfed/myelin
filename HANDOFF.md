# Myelin — top-level handoff

**Date:** 2026-07-20 · **Branch:** `claude/search-canonical-rebuild` (pushed, **not merged**)

The project-wide handoff (release track, R4 status, Tier-D gaps) is the ledger at
`planning/system-reviews/2026-06-26/14-release-track-ledger.md`. This file is the top-level entry
point for the work currently in flight on this branch.

---

## In flight: legacy→canonical Git blob search identity migration

**Full detail: [`docs/search-canonical-rebuild.md`](docs/search-canonical-rebuild.md).**

Git blob projection identities moved from raw slash-delimited ids to canonical percent-encoded
`ArtifactRef`s. The index is keyed by the id string, so the cutover rewrote nothing — legacy
documents, vectors and metadata survive untouched and unaddressable by the new writer. Every
resulting failure is silent: duplicated hits, deleted code still queryable, restricted content still
queryable, orphan vectors answering semantic queries.

### Two things to know before touching it

1. **`23fdddd` is shippable on its own.** `git.blob.snapshot` carrying payload `op = "delete"` was
   never a Search tombstone — Search dispatches removal on the event type's trailing verb or an owner
   `Gone`, never a payload field — so deletes fell through to the UPSERT path and deleted/restricted
   blobs stayed queryable. Now `git.blob.removed`. Independently verified across four adversarial
   passes. **This one fixes a live bug.**

2. **Everything else has NO production caller.** `RebuildCoordinator`, `for_service`,
   `enumerate_canonical_truth`, `load_canonical_blob_truth`, `from_replay`, `abandon` — every
   reference outside its defining module is a test. Root cause is pre-existing: `search_app_spec`
   still ships `consumers: Vec::new()` (the SRCH-P06 floor), so there is no production indexer to
   attach a gate to. Nothing is regressed; nothing is live either. Wiring recipe is in the detail doc.

### Status

- 25 + 5 adversarial drills green; warnings-denied Clippy clean; **zero architecture-lint violations
  in the 23 files touched**; live-Postgres proof of journal exclusivity under real concurrency.
- Worktree left for review at `/home/adhv/Projects/myelin-worktrees/search-canonical-rebuild`
  (isolation `iso-e13d7f27`, services stopped).

### Known-red elsewhere (not this branch)

CI is red on `main` and has been since 2026-07-18: `crates/myelin-git/src/pg_pr_store.rs` trips
`tenant-predicate` (13 sites) + `residency-pin` (1). Another agent owns that file. The workspace also
has one failing target, `myelin-ci-controlplane --test drills_ci_p17_reserve_settle_parity`, which
fails identically at baseline.
