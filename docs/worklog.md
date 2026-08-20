# worklog

A running log of autonomous product work: what changed, why, and what the
evidence was. Newest entries first. Every entry names its proof — if a claim
here has no test or drill behind it, treat it as wrong.

## 2026-08-20 — cell capacity cannot change sign at rest

The durable cell registry translated signed PostgreSQL integers into unsigned
domain values with unchecked casts. A corrupt `-1` capacity therefore became a
plausible-looking multi-billion or multi-exabyte limit, while a domain storage
capacity above `i64::MAX` wrapped negative on its way into PostgreSQL. Cell
utilisation also admitted values above one hundred despite being used as a
percentage by placement.

Cell persistence now has one explicit codec, separated from the registry's
routing behavior. Encoding rejects values PostgreSQL cannot represent;
decoding names the precise corrupt field and fails closed; and all numeric
conversions are checked. A forward database invariant admits exactly the enum
and numeric ranges the domain can read, protecting direct SQL and older
writers as well as current Rust code. Its durable integration story rejects
negative, oversized, unknown-enum, and impossible-percentage rows, verifies no
partial entry remains, and accepts the exact largest valid boundary.

**Proof:** registry boundary tests 21/21; storage unit suite 513/513;
control-plane suite 195/195 plus all drills; PostgreSQL placement integration
4/4 against the live federation database; Clippy `-D warnings` for all
control-plane and storage targets/features.

## 2026-08-20 — CI workflow identity no longer hashes Rust spelling

The production `ci.pipeline` definition pin hashed the raw bytes of four Rust
source files. Reformatting or editing a comment therefore changed the durable
workflow identity without changing behavior, while a semantic change in any
helper outside that hand-picked list changed behavior without changing the
identity. Harmless refactors could refuse a fleet cutover, and the supposed
semantic hash was incomplete by construction.

The version-7 definition now carries its existing deployed hash as an explicit
immutable pin. Durable workflow behavior advances through the versioned
cutover protocol already enforced by the registry; source layout and prose are
free to evolve independently. Restarting the live CI control plane against its
existing active version-7 row proves this is a compatible identity-preserving
cleanup, and the full CI lifecycle still executes pushed commits and surfaces
both success and failure.

**Proof:** CI runtime-composition tests 3/3; Clippy `-D warnings` for
`myelin-ci-controlplane`; live control-plane restart against the existing
definition registry; black-box CI lifecycle 5/5.

## 2026-08-20 — the issue key is the issue's everyday address

Issues displayed memorable keys such as `DX-2`, but the human HTTP and CLI
workflows for viewing, closing, and managing dependencies accepted only the
storage UUID. A person could discover an issue by key and still have to recover
an opaque implementation identifier before doing ordinary work.

The Issues grammar now has one shared definition of a canonical issue locator:
either the durable UUID or the displayed `PROJECT-123` key. At the HTTP
boundary, the object guard resolves a key once within the caller's tenant,
authorizes the resulting UUID, and rebinds that UUID for the existing handlers.
Storage and authorization therefore retain their unambiguous internal identity
while the public interface speaks the address people actually remember. The
CLI and examples use the key throughout, and malformed lookalikes still fail
locally.

The system journeys carry live issue keys through reads, idempotent closure,
and the complete dependency lifecycle. UUID-addressed paths remain compatible,
so integrations can keep stable machine identifiers while humans use concise
keys.

**Proof:** Issues API-parser tests 7/7; Edge Issues-handler tests 10/10; CLI
Issues-dispatch tests 4/4; TypeScript typecheck; Clippy `-D warnings` for
`myelin-issues`, `myelin-edge`, and `myelin-cli`; focused black-box Issues and
browser-approved CLI journeys 19/19.

## 2026-08-20 — Git commands consume every word

The Git CLI parser found positional operands by skipping anything that looked
like a flag, then searched the remaining argument list for the handful of flags
it understood. Repository creation and viewing ignored extra operands; most PR
commands ignored unknown and duplicate flags; conflicting review verdicts were
accepted in priority order. Most misleadingly, `pr merge --auto` parsed an
auto-merge boolean that the HTTP dispatcher discarded, so the advertised flag
performed an immediate merge.

Every Git verb now has an exact grammar. Repository coordinates are validated
locally, value and boolean flags are single-use, review accepts exactly one
verdict, and no operand is left unconsumed. The counterfeit auto-merge option is
gone until Myelin has a real durable auto-merge lifecycle. The founder system
journey proves an ambiguous repository-create command exits as usage, leaves no
repository behind, and then creates that same repository through the exact CLI
command; it also pins the removed auto-merge promise.

**Proof:** Git API-parser tests 15/15; CLI unit suite 147/147; TypeScript
typecheck; Clippy `-D warnings` for `myelin-git` and `myelin-cli`; complete
black-box browser-approved CLI journey 16/16.

## 2026-08-20 — issue dependencies are ordinary CLI work

Issue relations were a complete durable API feature—typed creation, permission-
filtered reads, monotonic backlink removal, and idempotent deletion—but remained
unreachable from the CLI. The CLI transport itself only understood GET and
POST, leaving a developer to drop down to raw HTTP to express that one issue
blocks another.

The typed Issues grammar now offers `relation list`, `relation add`, and
`relation remove`. It validates UUIDs, relation vocabulary, and canonical issue
references locally, maps each operation to its native HTTP verb, and documents
the exact commands. The browser-approved CLI system journey creates a real
dependency, reads the typed edge, removes it, and repeats the removal without a
copied credential or storage-specific knowledge.

**Proof:** Issues CLI-parser tests 7/7; CLI unit suite 147/147; TypeScript
typecheck; Clippy `-D warnings` for `myelin-issues` and `myelin-cli`; complete
black-box browser-approved CLI journey 16/16.

## 2026-08-20 — a model response must contain real work

The hosted-model boundary treated malformed function arguments as JSON `null`
and a missing final message as an empty successful answer. Either defect could
turn an invalid provider response into durable, charged agent progress: a tool
would receive an input the model never supplied, or a person would receive no
work product at all.

The Luna response decoder now accepts tool calls only when their collection is
an array, their identity and name are non-empty, and their arguments are a JSON
object. A final answer must contain readable text. Invalid provider output maps
to the existing permanent invalid-response outcome before reaching a governed
tool boundary. The live collaboration journey still reads a failed CI run,
opens governed work, pauses for human approval, and merges only after approval.

**Proof:** agent-model unit suite 23/23 (the explicit real-provider smoke remains
ignored); agent-host unit suite 33/33; Clippy `-D warnings` for
`myelin-agent-model`; black-box collaboration journeys 3/3.

## 2026-08-20 — malformed mentions fail visibly instead of disappearing

The notification router decoded a signal's `mentions` collection with
`unwrap_or_default`. A malformed collection therefore became “zero mentions,”
the delivery was acknowledged, and the intended direct notification vanished
without an inbox row or a dead letter. This was inconsistent with the same
router's strict signal and notification-reason decoding.

Mention decoding now returns the router's existing non-retryable poison error.
The consumer dead-letters the bad event, commits no partial notification row,
and remains free to deliver the next valid signal. The regression describes
that operational sequence rather than merely testing the decoder in isolation.

**Proof:** notification unit suite 348/348; Clippy `-D warnings` for
`myelin-notif`; black-box notification lifecycle and scale journeys 7/7.

## 2026-08-20 — CI deadlines are runtime work, not test choreography

The dogfood incident was literal: CI's production workflow fan-out discovered
and drove `running`/`waiting` pipelines, but never called the timer store. Only
the generic Flow daemon fired timers. CI integration tests hid the missing seam
by reaching into `PgFlowDriveStore` and firing deadlines themselves, so an
unclaimed job could remain parked indefinitely even while the CI worker kept
processing neighbouring runs.

Timer waking now belongs to the Flow worker's bounded ready-work cycle. Every
cycle attempts one due timer before driving one runnable workflow, making
deadline progress independent of an ordinary-work backlog. Both the generic
daemon and the exact-tenant CI fan-out use that same operation, and batch
telemetry reports timer firings separately from workflow drives. Drive inputs
are validated before the first timer mutation, preserving fail-before-write
behaviour for malformed clocks or workers without definitions.

The production accounting test no longer performs timer-store choreography:
the CI poller itself wakes an unclaimed job's deadline and drives the pipeline
into its late-accounting path. A second live-Postgres test arms five deadlines,
adds five ordinary runs, and proves one bounded batch fires all five timers and
settles all ten workflows. The black-box CI journey now also tells the missing
failure story: a non-zero sandbox command becomes a terminal, cost-settled run
whose failed job and diagnostic log remain inspectable through the public API.

**Proof:** Flow unit suite 254/254; CI control-plane unit suite 632/632;
Clippy `-D warnings` for both touched crates; focused durable Flow fairness
integration 1/1; focused production CI timeout/accounting integration 1/1;
TypeScript typecheck; focused black-box CI lifecycle 5/5; complete black-box
suite 110/110 across 25 files with zero skips (256.97 seconds).

## 2026-08-20 — completed work no longer buries new work

The inbox scale test claimed to protect fresh work, but its 250 old rows and
its new row were all mentions. It proved recency inside one reason band while
missing the incident mechanism it documented: a `done` approval (priority 90)
still ranked above every unread mention (priority 70), forever.

The journey now creates 30 real approval notifications, waits until the
product retires all of them, and only then publishes a new mention. Against
the old ordering it failed deterministically: the completed approvals filled
the first page until the 15-second wait expired. The durable order is now
`attention state -> reason priority -> occurrence -> id`: unread, seen, read,
then parked (`snoozed`/`archived`/`done`). Cursor v3 carries the complete sort
position, and `notif_0012` adds the matching online keyset index. The in-memory
ranker follows the same model rather than remaining a friendlier semantic twin.

The same work exposed the other half of the debris incident. Edge rechecked
one identical repository permission for every inbox row, sequentially. A
request-local authorization memo now reuses only exact permission/object and
pull-request-review decisions inside one API call; nothing survives into the
next request, so revocation is still re-confirmed. Issue key resolution uses
the same request-local discipline. The ordinary notification lifecycle fell
from a 30-second timeout to 6.2 seconds; its four journeys now finish in 16.3
seconds total.

**Proof:** notification unit suite 347/347; Edge notification unit tests 5/5;
Clippy `-D warnings` for both touched crates; TypeScript system-test typecheck;
`notification-scale.system.test.ts` 3/3 (fresh item 557ms, 100-row page
34ms, complete paged lookup 59ms); `notification-lifecycle.system.test.ts`
4/4; complete black-box suite 109/109 with zero skips.

The journey vocabulary was cleaned while making the bug observable: signals
carry an explicit closed reason, all inbox walks use the shared guarded pager,
and seed retirement waits for user-visible `done` state before teardown.

## 2026-08-19 — the system-test suite grows a spine (journeys, splits, scale)

The suite is the product's constitution, and it had two problems: whole
flows were re-derived inline per file (the collaboration file carried 12
unrelated journeys across 1,903 lines; the CLI file was one 2,616-line
test), and whole surfaces had no dedicated coverage at all.

**Elegance:**
- `src/journeys/` is the new flow vocabulary: `projects` (create/find),
  `issues` (propose + authorization walk), `chat` (Conversation
  open/post/messages/events), `refs` (await link/backlink/removal),
  `inbox` (mention envelopes, paged reads, seed/retire at scale), plus
  `src/paging.ts` (`walkPaged`/`findPaged` with repeated-cursor and
  unbounded-walk guards baked in).
- `collaboration-lifecycle` was split along pillar lines into
  `issues-lifecycle`, `chat-lifecycle`, `knowledge-lifecycle`,
  `pull-request-lifecycle`, keeping the three agent/CI governance
  journeys under the original name. All 12 tests pass unchanged in
  their new homes.
- `cli-authentication`'s single test is now 16 staged journeys in one
  `describe.sequential` with hoisted state - same narrative, same
  assertions, but each stage reports and fails on its own. All 16 pass
  live.

**New coverage:**
- `notification-scale`: seeds 250 delivered mentions into the
  reviewer's inbox, then pins the two guarantees the O(debris) incident
  violated - a fresh mention surfaces on the FIRST page within 15s
  (measured: 1.7s), and one 100-row page stays under 5s (measured:
  2.3s). The seed retires itself through the product path (Resolved
  signals -> `done`) so it never becomes the debris it guards against.
- `notification-lifecycle` gained the resolve->done retirement pin and
  was rewired onto the journeys.
- `refs-lifecycle`: a 12-edge backlink fan-out walked page by page with
  no loss or repetition, the outbound-links walk, and the
  target-visibility gate (probing a private artifact's reference
  surface 404s in both directions - indistinguishable from a reference
  never minted).

**What writing the scale test taught (all fail-closed, all now
documented in the journey helpers):**
- Inbox reads authorize EVERY row against its subject and silently drop
  unreadable ones - a mention about an artifact you cannot read does
  not exist for you. Correct privacy; also the per-row ~20ms cost that
  makes reads O(rows). The batch-authz follow-on stands.
- Ranking is by reason only (`approval_requested`=90 > `mentioned`=70 >
  `watched`=35); state never demotes a row. So a stale DONE approval
  card outranks a fresh unread mention forever - that is the concrete
  mechanism behind the incident's "fresh behind stale" symptom. The
  fresh-first fix is: rank state, not just reason.
- The product has no inbox retention: rows live forever and every read
  pays for them. The scale test contains its own growth (dedicated
  recipient, SQL-filtered views elsewhere), but retention is now a
  named product need, not a nice-to-have.

## 2026-08-19 — the quarantine table means something again (intake scope)

Chasing the 15k+ `no_registered_consumer` rows led to a design flaw, not
an outage: the agent service's durable intake subscribes to the ENTIRE
event stream but registers per-tenant trigger consumers only for tenants
the placement directory hosts on this cell. Every event from any other
tenant became a quarantine row - including the whole self-CI event flood,
because the self-host `myelin` tenant was never placed (rows were growing
in real time while I watched, ~2,400 during one self-CI run). Noise at
that volume makes the table useless for its actual job: catching drift.

Two fixes, both explicit rather than silently lenient:

- **substrate `IntakeScope`**: a broker-fed service can now declare which
  subjects it hosts. An out-of-scope delivery (a tenant this cell does
  not host - routine in a multi-cell world) is terminated WITHOUT a
  quarantine record; an in-scope delivery with no accepting consumer
  still quarantines loudly, because inside the declared scope that
  remains real wiring drift. `None` keeps the old strict behavior; the
  agent service derives its scope from the placement directory. proof:
  substrate serve tests (out-of-scope terminates recordless, in-scope
  drift still alarms) and a prefix-discipline test (`acme-evil` cannot
  ride `acme`'s scope).
- **the self-host tenant is placed**: `03-dev-placement.sql` now binds
  the `myelin` dogfood tenant to the dev cell like any other tenant, so
  its events are consumed instead of discarded, and its agent triggers
  can actually fire.

Also corrected en route: an earlier "the intake is wedged" theory was a
timezone misread (box clock vs UTC); the intake was healthy all along -
the worklog keeps the correction because the misread cost real time.

Chasing the suite to green after this landing surfaced three more real
findings (filed in the gap list, not fixed here):

- **the notifications service had died silently.** a stale fed-project
  boot (the known dual-invocation footgun) came up against a cell with
  no tenants, refused intake, panicked, and exited - while fed showed
  "running". restarted under the correct project; the notification
  suite's failures during the outage window were backlog aftermath.
- **the inbox read path is O(debris).** each `/v1/notif/inbox` page
  cost ~2s (per-row issue-authorization checks; 64 of 101 fetched rows
  survived filtering) and fresh unread mentions sorted behind pages of
  stale `done` items. 1,133 accumulated dev-stack test rows were enough
  to push the notification system tests over their 30s budgets even
  though every write landed in under a second. debris cleaned; the
  read-path fix (batch authz, fresh-first paging) is a named follow-on.
- **envelope validation is inconsistent across consumers**: the agent
  trigger consumer dead-lettered 267 test signal envelopes for a
  non-canonical `recorded_at` that the notif router accepts. one
  contract, two verdicts - needs a single answer.

## 2026-08-19 — chat is live: push delivery reaches the browser

The gap: the web chat UI polled every 5 seconds while the edge's "tenant
firehose" only ever emitted `repo.created`/`repo.pushed` — no chat event
had ever reached any stream. The SSE hub, the proxy plumbing, and the
frontend stream helpers all existed; nothing connected chat to them.

Now the edge serves `GET /v1/chat/conversations/{id}/events`. The
subscription is authorized with the SAME visibility gate as message
reads (`public_conversation`), so a viewer who cannot read a
conversation cannot observe its activity either — no metadata leak
through the stream. On each accepted `chat.message.post` the gateway
broadcasts a reference frame (conversation id + message id, never
content) on the conversation-scoped channel; clients revalidate through
the authorized read path. The web client now subscribes over a
same-origin proxy and treats its old poll as a 30-second safety net.

Proof, outside-in:
- `system-tests/tests/chat-live.system.test.ts`: a second collaborator's
  live subscription receives the posted message's reference frame (and
  never its content); subscriptions are refused exactly where reads are
  refused (401 unauthenticated, 400 malformed, 404 for a private-project
  room probed by a peer).
- browser: two real sessions — a message sent in one appears in the
  other within 8 seconds, no reload, far under the 30s fallback poll, so
  only push delivery explains it.
- unit: the gateway's post→frame mapping fires only on a 201 post with a
  bounded conversation id.

Deliberately NOT built yet: tenant-coarse chat pings for sidebar
liveness (a naive version would leak conversation existence to
non-project-members; needs a visibility-scoped design).

## 2026-08-19 — the self-CI loop is closed: fully green self-build

Run 8c952c07 on the self-hosted `myelin` tenant: **build, test, and
clippy all succeeded** inside myelin's own gVisor sandbox. Getting from
"caught its first regression" to green took three more real fixes, two
of them found BY the self-CI itself:

- the ci-sandbox crate's uid-semantics tests (userns leases, foreign
  ownership refusals) cannot be expressed under the sandbox's fake-root
  euid; they now skip loudly there (MYELIN_REQUIRE_USERNS_TESTS=1
  hard-fails on hosts that must prove them). 80 → 8 → 0 across rounds.
- the self-CI clippy job then caught the skip helper compiling as dead
  code under test-support feature unification — a lint only a
  workspace-wide `-D warnings` build sees. now `#[cfg(test)]`.

The dogfood story end to end: myelin hosts its own source, a stock git
push through its own wire triggers `.myelin/ci.toml`, the pipeline runs
in its own sandbox, it caught a real regression main was carrying plus
a lint, and it now verifies every fix. Also observed and noted: the CI
log read endpoint refuses byte ranges starting at 0 while tail ranges
read fine (bug, unfixed).

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
