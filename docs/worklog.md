# worklog

A running log of autonomous product work: what changed, why, and what the
evidence was. Newest entries first. Every entry names its proof — if a claim
here has no test or drill behind it, treat it as wrong.

## 2026-08-19 — self-CI catches its first real regression

The dogfood loop closed: myelin's own sandboxed CI **built and
clippy-checked myelin clean**, and its lib-test job failed on a real
regression main was carrying — the ci.pipeline@7 cutover added the
ci_0028 fence-row migration without updating four migration-registry
pin tests, and the filtered test runs used to validate that landing
never re-ran them. Fixed, and re-pushed for self-verification.

Getting there surfaced and fixed two more platform truths:

- **the linux-build-v1 limits were fiction for a real Rust workspace**
  (30-minute timeout, 8 GiB disk). linux-build-v1:2 grants 16 GiB
  memory / 32 GiB disk / 2 hours, and the dev runner now claims the
  build profile (`MYELIN_CI_RUNNER_EXECUTION_PROFILES`).
- **the vendored-cargo boundary admits exactly three recipes** (build /
  test --lib / clippy). myelin's pipeline now uses those; integration
  tests and the myelin-lints architecture gate need the recipe set
  extended (named follow-on).

Open bug from the same session: a workflow whose job-dispatch activity
retries forever (unlaunchable job) never honored its own wait deadline
— five due wf_timers sat unfired for 12+ hours while sibling workflows
completed normally. The stuck run was terminated manually; the
timer-vs-retry starvation needs a real look.

## 2026-08-18 — dogfood: myelin's own CI builds myelin

The self-host tenant `myelin` was bootstrapped on the dev stack, the
`myelin` repo created through the product API, and the source pushed
through myelin's own git wire with a stock git client. The push of a
current-tree snapshot triggered `.myelin/ci.toml` and the pipeline ran
in the gVisor sandbox — the self-CI loop is live.

Two real product findings from trying the FULL history first:

- **onboarding the full history trips a 512 MiB bound in the receive
  op.** pushing the whole 108 MiB-pack history was rejected with
  "upload-pack response exceeded the 536870912-byte wire cap", while a
  snapshot push of the same tree succeeded — so the overflow scales
  with history, not pack size, and the error's labels (upload-pack, on
  a receive; "wire cap", fed from the job's disk_bytes) are misleading.
  needs a repro with instrumentation before fixing; a repo importer
  should chunk ingestion regardless.
- **the pseudonymous-commit gate (contract 10.9) is live and enforced:**
  every historical commit was refused because its author email is a
  real identity, not `<pseudonym>@<tenant>.noreply`. correct behavior —
  and it means real-world onboarding needs an import flow that
  pseudonymizes committer/author identities while preserving a mapping
  (the pseudonym map exists; the importer does not).

## 2026-08-18 — checkpoint: every test surface green on main

Validation state of main (3f060e74 + this entry), all on the linux box:
full workspace tests, clippy `-D warnings`, the contract-coverage gate,
the complete system suite (85/85 with **zero** freshly quarantined
events), and the browser integration suite (5/5). Two mid-session
system-suite flakes were traced to overlapping `fed` invocations racing
service restarts under different project ids — a tooling footgun to
remember, not a product bug (each suite passes untouched).

Next up, in order: a user-facing DSR surface beyond agent-data (gap 2),
an ops entrypoint for post-restore re-erasure, the CT-007 sandbox
branch mining, and the self-CI dogfood push (myelin building myelin in
its own sandbox - the mechanism is already system-tested; the self-host
tenant bootstrap is the missing plumbing).

## 2026-08-18 — the event spine stopped silently losing user actions

The dev instance had 4,386 quarantined outbox events plus 2,656 stuck
behind them (a quarantined head permanently blocks its aggregate). Every
one traced to producers minting envelopes the publisher's own admission
check refuses — meaning real user actions (channel creation, chat
messages, CI runs, page creation, every ReBAC grant) emitted events that
never reached a single consumer, ever:

- identity: `tuple`/`agent` missing from the type-token table — 100% of
  identity.tuple.written died; the S8 reverse index consumed nothing.
- chat: bare-ulid aggregates plus a row/envelope divergence in the pg
  co-commit; channel events died and message events hung behind them.
- ci: path-form `ci/run/...` aggregates (one nested a full ref) and a
  schemeless run subject.
- knowledge: full-ref envelope aggregates vs bare-id row aggregates;
  `database` missing from the token table; `view-` missing from the sub
  grammar; row subjects five segments long.

Fixes: canonical `type:id` partitions everywhere (`channel:<id>`,
`run:<id>`, `log:<run>-<job>`, `page:<id>`, `database:<id>`), canonical
scoped run subjects, %2F-encoded slashes in tuple objects, and the
missing taxonomy/grammar tokens. The changed pipeline code was cut over
to ci.pipeline@7 through the definition fence's own upgrade path.

Regression net: `pgrelay::publisher_admission` exposes the relay's real
admission check and the chat/identity/knowledge producers drive their
actual emit paths through it in unit tests — that net caught two more
live bugs (a subject falling through to the new aggregate form, and the
five-segment row subjects) before they shipped.

Proof: full workspace tests + clippy -D warnings green on linux; full
system suite green; **zero** freshly quarantined events across an entire
85-test system run, down from ~600/day.

## 2026-08-18 — erasure now survives a restore (drilled for real)

The production erase path (`/v1/privacy/me/agent-data/erase` →
`DurableAgentTraceStore::erase_for_subject`) records every erasure in
`post_pit_erasure_ledger` BEFORE destroying the subject DEK — a destroyed
but unrecorded erasure would be silently resurrected by restoring any
pre-erasure backup. The ledger and the `ReErasePass` replay machinery
existed but nothing production ever wrote the ledger.

Proof: `integration_erasure_survives_restore` runs against real
`pg_dump`/`pg_restore` — seed a subject, back up, erase through the
production path, restore into a scratch database, assert the wrapped DEK
IS resurrected there (the drill has teeth), replay the live ledger, and
assert the subject is unreadable again with zero surviving DEK rows.
The privacy-lifecycle system test passes through the new path.

## 2026-08-18 — edge input hardening (pagination) landed

The in-flight working-tree changes found on the dev box, snapshotted as
`wip/edge-page-input-hardening`, rebased onto main and landed:

- pagination queries parse strictly at the edge: canonical integers only,
  bounded query/cursor bytes, duplicate or unknown page parameters are 400s.
- the decorative `view=split` diff parameter is gone; layout is the client's
  job, and the server rejects it loudly instead of ignoring it.
- proof: edge unit tests, plus system tests asserting the 400 contract on
  `/v1/triggers?limt=1`, `limit=01`, `cursor=`, and `diff?view=split`.

## 2026-08-18 — per-tenant load shedding with a protected human lane

The audit found the "protected human lane" existed only in test harnesses:
`ShedLane` was a real algorithm with zero production callers, the tuned
budgets in thresholds.toml claimed "MEASURED" from in-memory simulations,
and the actual edge had one flat 64-slot dispatch semaphore — 64 concurrent
agent requests would starve every human. Now:

- the gateway classes every routed request from its verified principal kind
  (plus optional `x-myelin-run-class` self-demotion) and admits it against
  its tenant's HttpIntake or GitFrontDoor budget. Machine saturation sheds
  429 + Retry-After; humans draw on a reserved fraction.
- pre-auth, machine-classified requests draw from a 48-slot pool ahead of
  the 64-slot general pool, so a storm cannot occupy every dispatch slot.
  Lying about class gets a caller past the pool but not past the per-tenant
  lane, which uses the verified principal.
- budgets retuned from fiction (200/50, 128/32) to fit the real global caps
  (48/12, 4/1), in thresholds.toml and the in-code fallback, kept in sync by
  the existing parity drill.
- proof: gateway unit tests drive authenticate → shed → 429 end to end, and
  `system-tests/tests/overload-shed.system.test.ts` storms the live edge
  with 300 machine-classed requests while a browser-approved human session
  gets strict 200s throughout. Full suite: 84/84.

## 2026-08-18 — fake-safety theater removed (−3,200 lines)

Removed the self-referential layer that asserted prose about the system
instead of exercising it: floor/follow-on registers encoded as rust
constants, "permanent gates" whose pass conditions were sentences nobody
parses, fabricated "MEASURED" RPO/RTO integers printed as dated GREEN
banners, tautological const-true assertions, and an invented p99 leg whose
budget check could never fail. Real behavioral tests in the same files were
kept. Also fixed: `Thresholds::load_canonical` no longer depends on the
build machine's absolute path (honors `MYELIN_THRESHOLDS_PATH`).

Validated on linux: full workspace tests, clippy `-D warnings`, the
contract-coverage gate (99 rows, 0 falsely claimed), and the complete
system suite.

## known gaps (honest list, in priority order)

1. **erasure-restore is closed for the wired path, open for the rest.** the
   agent-data erase now writes the post-PIT ledger and the re-erase pass is
   drilled against a real dump. remaining: the restore RUNBOOK (an ops
   entrypoint that runs the replay after a production restore — today it is
   a library call + drill), and the library-level erase paths (chat, issues,
   git crypto-shred) still do not write the ledger because nothing wires
   them to a user surface yet (gap 2).
2. **DSR is a library, not a product surface.** no service registers a
   PersonalDataHolder; dsr_submit/status/certificate are unwired. the only
   user-facing privacy surface is `/v1/privacy/me/agent-data*`. chat/issues/
   git erasure flows exist as in-memory tested code only.
3. **multi-tenant machine storms can still exhaust the general dispatch
   pool.** the human lane holds per tenant and against well-behaved machine
   traffic; a coordinated cross-tenant storm of requests that lie about
   their class is bounded only by the flat caps. fixing this properly means
   authenticating before dispatch admission (an edge refactor).
4. **the per-service shed gates (search, refs, notif, flow, …) are still
   test-only.** those services are NATS consumers without an admission
   point today; their thresholds.toml rows are targets, not enforcement.
5. **trust-scoped CI cache + agent exec gate are built but unwired**; the CI
   runner does not route cache writes through `CiCacheNamespace`, and
   nothing constructs `AgentExecGate` in production.
6. **stale branch archaeology:** `codex/*`, `claude/*`, `wip/2*`–`wip/35*`
   (CT-007 sandbox slices) sit on a disjoint history root ("founder source
   snapshot") with no common ancestor with main. any useful content must be
   mined as diffs. left in place, treated as archive.
