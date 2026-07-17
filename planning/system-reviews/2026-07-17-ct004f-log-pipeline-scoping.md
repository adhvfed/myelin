# CT-004f (CI-P20) — CI log pipeline live-binding scoping

_2026-07-17 — autonomous scoping pass. The contained peer-review burndown is complete; this de-risks the next CI phase so it can begin with a clear plan (or be re-prioritized by the founder against CT-005)._

## State: BUILT vs the live-binding gap

**Built (do NOT rewrite — the seams the wiring plugs into):**
- `myelin-ci-controlplane::log_pipeline` — the real `LogPipeline`: firehose frame ingest → secret redaction (`SecretRedactor`) → sealed T2 content-addressed segment (`SealThreshold`) → `LogSegmentRow`/`LogAnchorRow` typed rows + the `(job, step, byte-range)` OLTP index + the bind-param SQL. `live_tail.rs` holds the resume-cursor / live-tail viewer over the firehose + the buffered index. `CI_LOG_STREAM` / `CI_LOG_APPENDED` taxonomy is frozen (`ci-sandbox::events`, durable/firehose split is structural).
- `myelin-ci-sandbox::runner` (`run_one`) — CAPTURES the guest's real stdout/stderr (gvisor `run_and_capture`, 256 KiB head bound, redaction) and ships each frame to a **`FirehoseSink`**, then emits the durable `ci.log.available` POINTER (references-not-payloads; log bytes NEVER touch the durable bus — enforced by the `no-raw-publish` lint).
- `S3BlobStore` (default-reachable, per #6) — the CAS backing the sealed segments write to.

**The gap (CI-P20 / CT-004f):** `runner.rs:389` `FirehoseSink` is a **STUB** — it only COUNTS frames. The real `LogPipeline` is never driven, so sealed segments + `log_segment`/`log_anchor` rows are never written in the live path, and `ci.log.available` resolves to nothing.

## The cycle constraint (the crux)

`myelin-ci-controlplane` depends on `myelin-ci-sandbox` (the runner reuses the sandbox's `TrustTier`/`JobSpec`). So **ci-sandbox cannot depend on ci-controlplane** — the runner cannot call `LogPipeline` directly. The binding MUST go through a seam:

1. Define/confirm a `LogSink` trait in `ci-sandbox` (or a lower shared crate) — the runner ships redacted frames `(run, job, step, bytes)` to `&dyn LogSink`. `FirehoseSink` becomes the stub impl (tests) / the trait.
2. `ci-controlplane` provides the REAL impl adapting `LogPipeline` to `LogSink` (it CAN name both, being the higher crate).
3. Inject the real sink at the composition root — the `CiRunnerLoop` bind (`runner_bind.rs`) / the CI runner main — the SAME injection idiom `CiPipelineReporter`/`DurableLeaseAdapter` use.
4. On seal: write the segment bytes to `S3BlobStore` (CAS) + the `log_segment`/`log_anchor` rows (tenant-scoped tx, FORCE-RLS) + publish the coalesced `ci.log.available` pointer.

## Suggested sub-steps (each independently verifiable)

1. **Seam**: confirm/extract the `LogSink` trait in ci-sandbox; `FirehoseSink` stub implements it (unit test unchanged). DB-free.
2. **Adapter**: `ci-controlplane` `LogPipelineSink` implements `LogSink` over `LogPipeline` + `S3BlobStore` + the OLTP pool. Unit-test the frame→seal→row mapping DB-free (the pipeline core already is).
3. **Persist**: the sealed-segment BlobStore write + `log_segment`/`log_anchor` co-commit on a tenant-scoped tx. Live-PG + live-S3 integration test (round-trip: frames → sealed bytes at the CAS address → `(job, step, byte-range)` index resolves the bytes back).
4. **Bind**: inject `LogPipelineSink` into the runner at the composition root; behind `MYELIN_CI_RUNNER=1` like the rest of the live runner.
5. **Prove**: a real `runsc` job's stdout → firehose → sealed segment in S3 + `log_segment` rows + the `ci.log.available` pointer resolves end-to-end (extend the CT-004d.2 live pipeline proof).

## Verification discipline
Load-bearing (money-adjacent? no; but durability + PII-safety: log bytes are references-not-payloads, redaction must hold). Hold for an independent adversarial pass before landing the persist + bind steps: probe the redaction path (a secret in a frame must never reach the CAS unredacted) and the crash/seal-window (a crash mid-seal must not lose or double-write segments). Trace the actual seal path, not the doc claim.

## Priority note
CT-004f (readable CI logs) is a usability prerequisite for CT-005 (CI surfaces) and CT-007 (Actions cutover) — you cannot ship CI without logs. It is more mechanical (wire an existing pipeline) than CT-005 (design-first). Founder call: begin CT-004f now, or sequence it against CT-005.
