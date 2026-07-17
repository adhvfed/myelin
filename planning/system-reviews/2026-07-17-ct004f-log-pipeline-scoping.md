# CT-004f (CI-P20) — CI log pipeline live-binding scoping

> **STATUS: COMPLETE (2026-07-17).** seam (`cf22fee`) → adapter (`b1e165c`) → row persist (`dbaa092`) → pointer persist (`6bab572`) → adversarial-verify + harden (`58a89d0`) → integration-suite repair (`1f28d14`) → boundary-redaction seam (`cc568e6`, codex-reviewed `4a58c44`) → live bind (`97767bf`) → **end-to-end runsc capstone (`bb4f669`)**. A real gVisor guest's stdout now seals to the live S3 CAS, indexes in `log_segment`/`log_anchor` + `ci.log.available` outbox, and is readable back from the CAS — proven on live runsc+PG+S3. Readable CI logs are LIVE. Follow-on (NOT CT-004f): CI-1 secret injection must co-populate the `RedactionPlan` (see sub-step 1); the production streaming masker; the git-wire token ambient-secret hardening.


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

---

## REVISION — 2026-07-17 (pre-build seam audit; the plan above was too shallow)

Before writing sub-step 1 I traced the actual seam + the redaction path (verification discipline: trace, don't trust the doc claim). Four findings materially reshape the plan — the original "just wire the pipeline" framing would have baked in a **PII regression** (unredacted secrets sealed into the CAS). Details:

**F1 — the `FirehoseSink` seam is cycle-safe but UNDER-INFORMED.** `runner.rs:398` already defines `pub trait FirehoseSink { fn ship_frame(&self, run_id, job_id, frame: &[u8]) }` — a clean, DB-free, `&self` seam in ci-sandbox (no cycle to fix; sub-step 1 as "extract the trait" is a no-op — it exists). BUT `LogPipeline::new(tenant, region, blobs, redactor)` needs **`tenant` + `region`** to even construct (the residency write-pin + the per-tenant CAS keyspace). `ship_frame` drops both. The runner HAS them (`job.tenant`, `self.region`) — so the seam must **thread `(tenant, region)` per frame/job**. (The runner is MULTI-TENANT — `claim_for_labels` has no tenant filter — so the shared sink cannot hold a single per-runner pipeline; it needs a per-`(tenant,run,job)` pipeline, lazily opened.)

**F2 — the seam needs a terminal `finish(run, job)` op.** `ship_frame` alone never seals: `LogPipeline` seals on threshold or on `flush_job`. A single-command job (RESHAPE-001: one command per job) emits its whole stdout/stderr then is DONE — so the runner must call a terminal `finish`/`flush` after the two stream ships in `run_one` (seal open segment → `drain_pointers` → emit via outbox → flush `log_segment`/`log_anchor` rows). `CountingFirehose` stub gets a no-op `finish`. This is the real content of the seam evolution (sub-step 1), NOT trait extraction.

**F3 — REDACTION LOCATION is the load-bearing decision, and the current live path has NONE.** The `ship_line` redactor is documented "defence-in-depth, NOT the boundary." The BOUNDARY redaction does not exist: `firecracker.rs`/`gvisor.rs` `capture_stream`/`run_and_capture` only base64-deframe + bound (`SANDBOX_CAPTURE_BOUND` 256 KiB) — **no masking**. The "redacted frames" in the runner comments is aspirational (CI-P20). Meanwhile `JobSpec` carries secrets as **opaque `SecretRef` handles only** ("never the secret material"; resolved by the **in-boundary broker**), so the least-privilege runner process **cannot build a real `SecretRedactor`** — it never holds the plaintext needles. Consequence: wiring the pipeline with an empty redactor (the only redactor the runner can supply) would **seal UNREDACTED secrets into the CAS** — a PII regression, precisely the adversarial-hold failure mode. And feeding the pipeline real needles out-of-boundary would leak secret plaintext into the control plane — a WORSE hole. → **Redaction must be a BOUNDARY responsibility** (mask inside the sandbox, where the broker resolved the plaintext, before the bytes cross back), with the pipeline redactor left as empty defence-in-depth. This is a **sandbox-side + secret-broker change that is a hard precondition for the live log wiring**, not a later polish step.

**F4 — frame→line impedance (adapter concern, sub-step 2).** `ship_frame` ships whole streams as `&[u8]`; `LogPipeline::ship_line` takes `&str` lines. The adapter does `String::from_utf8_lossy(frame).lines()` → `ship_line` each, under a single stable `step_id` ("0" — one step per job today; multi-step is a future evolution the seam's step field would carry). The adapter wraps `LogPipeline` in a `Mutex` (`&self` trait vs `&mut self` pipeline) and is DB-free unit-testable with `FsBlobStore(tempdir)`.

### Revised sub-step order (redaction FIRST — it gates correctness)
0. **DECISION (founder / security):** confirm redaction is a boundary responsibility — mask captured stdout/stderr **inside the sandbox** before return, keyed by the job's broker-resolved secrets. (Alternative to reject: pipeline-side redaction, which requires plaintext needles out-of-boundary = leak.) Nothing else lands until this is settled, because it decides whether the seam carries needles (it must NOT).
1. **Boundary redaction** (ci-sandbox + broker): the capture path masks the job's resolved secrets before the bytes leave the boundary. Adversarial test: a known secret printed by the guest never appears in the returned `SandboxResult.stdout`.
2. **Seam evolution** (ci-sandbox, DB-free): thread `(tenant, region)` through `ship_frame`; add terminal `finish(run, job)`. `CountingFirehose` updated; runner `run_one` calls `finish` after the stream ships. Unit tests stay green.
3. **Adapter** (ci-controlplane, DB-free): `LogPipelineSink` implements `FirehoseSink`, holds a per-`(tenant,run,job)` `Mutex<LogPipeline<FsBlobStore/S3BlobStore>>` with an EMPTY redactor (boundary already redacted). Frame→line mapping unit-tested with `FsBlobStore(tempdir)`.
4. **Persist** (live PG + live S3): `finish` seals to `S3BlobStore` + co-commits `log_segment`/`log_anchor` on a tenant-scoped FORCE-RLS tx + drains pointers to the outbox. Round-trip integration test.
5. **Bind + prove**: inject `LogPipelineSink` at the CI-runner composition root behind `MYELIN_CI_RUNNER=1`; extend the CT-004d.2 `runsc` proof end-to-end (guest stdout → sealed segment in S3 → `log_segment` rows → `ci.log.available` resolves), including the redaction adversarial leg (a secret in guest output is absent from the CAS bytes) and the crash-mid-seal leg.

### Progress ledger (2026-07-17)
- **Seam audit** — `7028af5` (this doc's REVISION).
- **Sub-step 2 (seam)** — DONE, `cf22fee` + `b1e165c`. `FirehoseSink` carries `tenant` + a terminal `finish(run, job, tenant, passed)`; `run_one` calls it after the stream ships. `CountingFirehose` stub + the runner's 9 unit tests green. The seam is validated by its consumer (below), not shipped speculatively.
- **Sub-step 3 (adapter)** — DONE, `b1e165c`. `LogPipelineSink` (ci-controlplane) implements `FirehoseSink` over a per-`(tenant,run,job)` `LogPipeline`, EMPTY redactor (F3), frame→line (F4), `close_step`→`flush_job` on finish, sealed rows + pointers handed to an injected `LogPersist`. DB-free, 4 unit tests green (seal→segment/anchor mapping, idempotent finish, failed-anchor, tenant isolation). Clippy-clean.
- **Sub-step 4a (persist — rows)** — DONE, `dbaa092`. `DurableLogPersist` (ci-controlplane): the live `log_segment`/`log_anchor` writer via the frozen bind-param SQL in ONE `with_tenant_tx` FORCE-RLS tx; sync→async block_on bridge; uuid id parsing. `integration_ci_ct004f_durable_log_persist` (feature=integration, live PG) proves it end-to-end under the RLS-enforced app role (sink→pipeline→store→read-back, anchor `passed`, 0 dangling) + re-delivery idempotency. Closes the "frozen SQL proven but no store" gap (CI-P20 used raw inline sqlx).
- **Sub-step 4b (persist — pointer)** — DONE, `6bab572`. The `ci.log.available` pointers co-emit to the durable OUTBOX via `PgRelay::co_commit_in_tx` on the SAME `with_tenant_tx` connection as the index rows (atomic). Deterministic event_id (`cilog-<fnv>` on `(aggregate, byte_start, byte_end)`) → re-delivery dedups via `ON CONFLICT (event_id)` (double-emit 0). CI service-principal envelope. Integration test extended: the pointer lands co-committed; a re-delivered persist does not duplicate outbox rows. **CT-004f persist layer (rows + pointer) is complete + live-proven.**
- **Sub-step 1 (boundary redaction)** — founder-gated (step 0 below). Must land BEFORE sub-step 5.
- **Sub-step 5 (bind + prove)** — inject `LogPipelineSink` at the CI-runner composition root (swap the `CountingFirehose` in `runner_bind.rs::CiRunnerLoop`) behind `MYELIN_CI_RUNNER=1`; extend the CT-004d.2 `runsc` proof end-to-end + the redaction adversarial leg. Gated on sub-step 1 (else a real secret could reach the CAS).
- **Adversarial verify (persist layer)** — DONE, `58a89d0`. An independent adversarial-agent pass over 4a+4b found NO reachable data-loss / double-write / cross-tenant bug (atomicity, RLS backstop, core idempotency traced sound). Two latent LOW findings hardened: L1 (rows dedup on normalized uuid PK but the pointer keyed on the raw string → canonicalize run/job in `ci_log_event_id`); L2 (pointer key was tenant-agnostic on the global `outbox.UNIQUE(event_id)` → include tenant). One reachable-but-untested path closed: the mid-stream byte-budget coalesce pointer (`segment_ref = None`). 3 integration tests green.

### Sub-step 1 (boundary redaction) — DONE + the "moot today" finding
Founder approved boundary redaction; a co-review (codex/gpt-5.6) sharpened the scope. **Key finding on build: the platform injects NO secrets into any job today** — `SecretBroker::resolve` exists but is not wired into any launch path (the guest gets env-var NAMES only, never resolved material). So there are no secret needles anywhere and nothing to mask. The redaction blocker is therefore **moot for shipping logs now** — but only for CI-MANAGED secrets.

**Built (`crate::redaction::RedactionPlan`):** a fail-closed, currently-empty boundary-redaction seam. Every backend capture→result path (`gvisor::build_result`, `firecracker::build_result_from_console`) takes a REQUIRED `&RedactionPlan` and masks both streams as the last step before `SandboxResult` — no capture can forward un-redacted bytes (structural, not a comment). `RedactionPlan::for_job(spec)` is the ONE seam CI-1 injection must populate (co-wired with guest injection; injecting a secret while leaving the plan `none()` is the forbidden state). The git-wire path passes `none()` explicitly (its stdout is the packfile transport, not logs — masking would corrupt it). 7 unit tests (mask/identity/non-utf8/adjacency + the two `build_result` seam tests). The production MASKER (streaming chunk boundaries, encodings, min-length) is deferred to when injection exists.

**Precise security claim (do not overclaim):** *the durable CI logs contain no CI-managed secret plaintext, because the platform injects none, and the boundary seam forces masking the day it does.* NOT "logs contain nothing sensitive" — a job can still print a credential from its own source/cache/literal, which no exact-value redactor can stop.

**Ambient-secret audit (codex's checklist, against the ACTUAL wired state):**
- Guest env: gVisor sets an EXPLICIT `process.env` (base PATH + `spec.env` + git `GIT_PROTOCOL`); it does NOT inherit the runner's environment. ✓
- Network: gVisor runs `--network=none` → cloud-metadata / workload-identity endpoints are unreachable from the guest. ✓
- Secret injection: not wired → no CI-managed secret plaintext in any guest. ✓ (the basis of "moot today")
- Caches: restore not wired (no cache-injected material). ✓
- **git-wire (the one to watch):** injects bind mounts + a per-job ATTENUATED scoped token; a tokened remote URL / credential helper inside the guest could be printed by a job. Blast radius is a short-lived per-job token, but this is the ambient path to harden when the git-credential story matures (out of CT-004f scope; flagged).
- argv / `set -x`: the job's own command — not platform-injected secrets (user's own content, not maskable by design).

### The one decision that needs a founder steer
Step 0 (redaction location). My strong recommendation: **boundary redaction** — it is the only option that keeps secret plaintext inside the sandbox AND guarantees the CAS never holds an unredacted secret. It is more work than "wire the existing pipeline" implied, but the alternative is a security regression. I will NOT autonomously implement a redactor that pulls plaintext into the control plane. Absent a steer, sub-steps 2–3 (seam + adapter, both DB-free and redaction-location-agnostic since the seam carries NO needles either way) are safe to build first; step 1 (boundary redaction) lands before any live wiring (steps 4–5).
